use crate::channels::telegram::TelegramChannel;
use crate::config::{AppConfig, NewsSourceConfig};
use crate::runs::{MetadataItem, RunDraft, RunError, RunResult, RunTracker, StepDraft, StepKind, ToolTrace};
use crate::tools::news_filter::{FilteredNewsItem, TopicBucket, TopicMatchContext, filter_relevant_items};
use crate::tools::news_format::format_news_brief_message;
use crate::tools::news_score::score_items;
use crate::tools::news_sources::fetch_source_items;
use crate::tools::news_store::{FailureStage, NewsBriefStore, NewsWindowStatus};
use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct RawNewsItem {
    pub source_id: String,
    pub source_name: String,
    pub source_weight: f64,
    pub source_kind: String,
    pub title: String,
    pub link: String,
    pub canonical_link: String,
    pub snippet: String,
    pub published_at: Option<DateTime<Utc>>,
    pub normalized_title: String,
}

#[derive(Debug, Clone)]
pub struct ScoredNewsItem {
    pub item: FilteredNewsItem,
    pub recency_score: f64,
    pub topic_score: f64,
    pub source_quality_score: f64,
    pub cross_source_boost: f64,
    pub final_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewsBriefMetrics {
    pub source_count: usize,
    pub fetched_count: usize,
    pub relevant_count: usize,
    pub deduped_count: usize,
    pub selected_count: usize,
    pub thin_summary_count: usize,
    pub sent_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsBriefItem {
    pub source_id: String,
    pub source_name: String,
    pub source_kind: String,
    pub title: String,
    pub summary: String,
    pub link: String,
    pub canonical_link: String,
    pub matched_topics: Vec<String>,
    pub reason_selected: String,
    pub title_hit_count: usize,
    pub snippet_hit_count: usize,
    pub source_weight: f64,
    pub recency_score: f64,
    pub topic_score: f64,
    pub cross_source_boost: f64,
    pub final_score: f64,
    pub used_title_only_fallback: bool,
    pub thin_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsBrief {
    pub window_key: String,
    pub window_epoch_secs: i64,
    pub generated_at_epoch_secs: i64,
    pub prepared_artifact_hash: String,
    pub metrics: NewsBriefMetrics,
    pub items: Vec<NewsBriefItem>,
}

#[derive(Debug, Clone)]
pub enum GenerateOutcome {
    Prepared(NewsBrief),
    Skipped { reason: String, metrics: NewsBriefMetrics },
}

#[derive(Debug, Clone)]
pub struct CachedSourceItems {
    pub fetched_at: DateTime<Utc>,
    pub items: Vec<RawNewsItem>,
}

pub struct NewsBriefService {
    config: AppConfig,
    store: NewsBriefStore,
    topic_context: TopicMatchContext,
    client: reqwest::Client,
    cache: Mutex<HashMap<String, CachedSourceItems>>,
}

impl NewsBriefService {
    pub fn new(config: AppConfig) -> Result<Self, String> {
        validate_news_brief_config(&config)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.ollama_timeout_secs))
            .build()
            .map_err(|err| format!("Failed to build news brief client: {}", err))?;

        Ok(Self {
            store: NewsBriefStore::new(config.news_brief_store_path.clone()),
            topic_context: TopicMatchContext::from_config(&config),
            config,
            client,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn timezone(&self) -> Result<Tz, String> {
        self.config
            .news_brief_timezone
            .parse::<Tz>()
            .map_err(|_| format!("Unsupported NEWS_BRIEF_TIMEZONE '{}'", self.config.news_brief_timezone))
    }

    pub fn schedule_times(&self) -> Result<Vec<NaiveTime>, String> {
        self.config
            .news_brief_schedule
            .iter()
            .map(|value| parse_schedule_time(value))
            .collect()
    }

    pub async fn window_status(&self, window_key: &str) -> Result<Option<NewsWindowStatus>, String> {
        Ok(self.store.get_window(window_key).await?.map(|window| window.status))
    }

    pub async fn generate_news_brief(
        &self,
        scheduled_at: DateTime<Tz>,
    ) -> Result<GenerateOutcome, String> {
        let window_key = scheduled_at.format("%Y-%m-%d %H:%M").to_string();
        if let Some(existing) = self.store.get_window(&window_key).await? {
            return match existing.status {
                NewsWindowStatus::Prepared | NewsWindowStatus::Sent => existing
                    .brief
                    .map(GenerateOutcome::Prepared)
                    .ok_or_else(|| "Stored prepared artifact is missing".to_string()),
                NewsWindowStatus::Skipped => Ok(GenerateOutcome::Skipped {
                    reason: existing
                        .skipped_reason
                        .unwrap_or_else(|| "Delivery eligibility gate rejected the brief.".to_string()),
                    metrics: existing.brief.map(|brief| brief.metrics).unwrap_or_default(),
                }),
                NewsWindowStatus::Failed => Err(existing
                    .failure_reason
                    .unwrap_or_else(|| "News brief generation already failed for this window.".to_string())),
            };
        }

        let fetched = match self.fetch_all_sources().await {
            Ok(items) => items,
            Err(err) => {
                self.store
                    .mark_failed(&window_key, FailureStage::Generation, &err)
                    .await?;
                return Err(err);
            }
        };

        let metrics = NewsBriefMetrics {
            source_count: self.config.news_brief_sources.len(),
            fetched_count: fetched.len(),
            ..NewsBriefMetrics::default()
        };

        let filtered = filter_relevant_items(fetched, &self.topic_context);
        let relevant_count = filtered.len();
        let scored = score_items(filtered, &self.config, Utc::now());
        let deduped_count = scored.len();

        let items = self.select_items(scored);
        let selected_count = items.len();
        let thin_summary_count = items.iter().filter(|item| item.thin_summary).count();
        let avg_score = average_score(&items);

        let metrics = NewsBriefMetrics {
            relevant_count,
            deduped_count,
            selected_count,
            thin_summary_count,
            ..metrics
        };

        if selected_count < self.config.news_brief_min_items {
            let reason = format!(
                "Delivery eligibility gate rejected the brief: only {} items met the threshold.",
                selected_count
            );
            self.store.mark_skipped(&window_key, &reason).await?;
            return Ok(GenerateOutcome::Skipped { reason, metrics });
        }
        if avg_score < self.config.news_brief_min_avg_score {
            let reason = format!(
                "Delivery eligibility gate rejected the brief: average score {:.2} is below {:.2}.",
                avg_score, self.config.news_brief_min_avg_score
            );
            self.store.mark_skipped(&window_key, &reason).await?;
            return Ok(GenerateOutcome::Skipped { reason, metrics });
        }
        if thin_summary_count > selected_count / 2 {
            let reason = "Delivery eligibility gate rejected the brief: too many summaries were too thin.".to_string();
            self.store.mark_skipped(&window_key, &reason).await?;
            return Ok(GenerateOutcome::Skipped { reason, metrics });
        }

        let brief = NewsBrief {
            window_key: window_key.clone(),
            window_epoch_secs: scheduled_at.with_timezone(&Utc).timestamp(),
            generated_at_epoch_secs: Utc::now().timestamp(),
            prepared_artifact_hash: build_artifact_hash(&window_key, &items),
            metrics,
            items,
        };
        self.store.save_prepared(&window_key, brief.clone()).await?;
        Ok(GenerateOutcome::Prepared(brief))
    }

    pub async fn send_news_brief(
        &self,
        telegram: &TelegramChannel,
        brief: &NewsBrief,
    ) -> Result<(), String> {
        let timezone = self.timezone()?;
        let message = format_news_brief_message(brief, timezone);
        if let Err(err) = telegram.send_system_message(&message).await {
            let error = err.to_string();
            self.store
                .mark_failed(&brief.window_key, FailureStage::Delivery, &error)
                .await?;
            return Err(error);
        }
        self.store.mark_sent(&brief.window_key).await?;
        Ok(())
    }

    async fn fetch_all_sources(&self) -> Result<Vec<RawNewsItem>, String> {
        let mut collected = Vec::new();
        for source in &self.config.news_brief_sources {
            let items = match self.fetch_source_with_cache(source).await {
                Ok(items) => {
                    self.store
                        .record_source_fetch_success(&source.id, items.len())
                        .await?;
                    items
                }
                Err(err) => {
                    self.store.record_source_fetch_failure(&source.id).await?;
                    return Err(err);
                }
            };
            collected.extend(items);
        }
        Ok(collected)
    }

    async fn fetch_source_with_cache(
        &self,
        source: &NewsSourceConfig,
    ) -> Result<Vec<RawNewsItem>, String> {
        let now = Utc::now();
        let cooldown = chrono::Duration::minutes(self.config.news_brief_fetch_cooldown_minutes as i64);
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&source.id) {
                if now.signed_duration_since(cached.fetched_at) < cooldown && !cached.items.is_empty() {
                    return Ok(cached.items.clone());
                }
            }
        }

        let fetched = fetch_source_items(&self.client, source).await?;
        let mut cache = self.cache.lock().await;
        cache.insert(
            source.id.clone(),
            CachedSourceItems {
                fetched_at: now,
                items: fetched.clone(),
            },
        );
        Ok(fetched)
    }

    fn select_items(&self, scored: Vec<ScoredNewsItem>) -> Vec<NewsBriefItem> {
        scored
            .into_iter()
            .take(self.config.news_brief_max_items)
            .map(|item| to_brief_item(item, self.config.news_brief_max_summary_chars))
            .collect()
    }
}

pub struct NewsRunRecorder {
    tracker: RunTracker,
}

impl NewsRunRecorder {
    pub fn new(tracker: RunTracker) -> Self {
        Self { tracker }
    }

    pub fn start(&self, window_key: &str) -> String {
        self.tracker.start_run(RunDraft {
            request_title: format!("Scheduled news brief {}", window_key),
            request_text: format!("Generate and deliver the AI Agents / LLMOps / RAG news brief for {}", window_key),
            summary: "Scheduled news brief started.".to_string(),
            conversation_mode: "Scheduler".to_string(),
            model: "heuristic".to_string(),
            channel: "Telegram".to_string(),
            metadata: vec![
                MetadataItem::new("source", "news_scheduler"),
                MetadataItem::new("window_key", window_key),
            ],
        })
    }

    pub fn start_step(&self, run_id: &str, kind: StepKind, title: &str, summary: &str) -> String {
        self.tracker.start_step(
            run_id,
            StepDraft {
                kind,
                title: title.to_string(),
                summary: summary.to_string(),
            },
        )
    }

    pub fn finish_step(
        &self,
        run_id: &str,
        step_id: &str,
        summary: &str,
        detail: &str,
        trace: Option<ToolTrace>,
        metrics: Vec<MetadataItem>,
    ) {
        self.tracker
            .finish_step(run_id, step_id, summary, detail, trace, metrics);
    }

    pub fn fail_step(&self, run_id: &str, step_id: &str, summary: &str, detail: &str) {
        self.tracker
            .fail_step(run_id, step_id, summary, detail, None, Vec::new());
    }

    pub fn complete(&self, run_id: &str, summary: &str, final_result: RunResult, metadata: Vec<MetadataItem>) {
        self.tracker.complete_run(run_id, summary, final_result, metadata);
    }

    pub fn fail(&self, run_id: &str, summary: &str, title: &str, message: &str, suggestion: &str) {
        self.tracker.fail_run(
            run_id,
            summary,
            RunError {
                title: title.to_string(),
                message: message.to_string(),
                suggestion: suggestion.to_string(),
            },
            RunResult::text(title, summary, message),
            Vec::new(),
        );
    }
}

pub fn validate_news_brief_config(config: &AppConfig) -> Result<(), String> {
    if !config.news_brief_enabled {
        return Err("News brief is disabled. Set NEWS_BRIEF_ENABLED=true.".to_string());
    }
    config
        .news_brief_timezone
        .parse::<Tz>()
        .map_err(|_| format!("Unsupported NEWS_BRIEF_TIMEZONE '{}'", config.news_brief_timezone))?;
    if config.news_brief_sources.is_empty() {
        return Err("NEWS_BRIEF_SOURCES_JSON must contain at least one enabled source".to_string());
    }
    if config.news_brief_schedule.is_empty() {
        return Err("NEWS_BRIEF_SCHEDULE_JSON must contain at least one HH:MM time".to_string());
    }
    for value in &config.news_brief_schedule {
        parse_schedule_time(value)?;
    }
    if config.news_brief_min_items == 0 {
        return Err("NEWS_BRIEF_MIN_ITEMS must be greater than zero".to_string());
    }
    if !(0.0..=1.0).contains(&config.news_brief_min_avg_score) {
        return Err("NEWS_BRIEF_MIN_AVG_SCORE must be between 0.0 and 1.0".to_string());
    }
    Ok(())
}

pub fn parse_schedule_time(value: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M")
        .map_err(|_| format!("Invalid schedule time '{}'. Use HH:MM.", value))
}

fn to_brief_item(item: ScoredNewsItem, max_summary_chars: usize) -> NewsBriefItem {
    let (summary, used_title_only_fallback) = summarize_item(
        &item.item.item.title,
        &item.item.item.snippet,
        &item.item
            .matched_topics
            .iter()
            .map(TopicBucket::as_str)
            .collect::<Vec<_>>(),
        max_summary_chars,
    );
    let thin_summary = is_thin_summary(&item.item.item.title, &summary, used_title_only_fallback);
    NewsBriefItem {
        source_id: item.item.item.source_id.clone(),
        source_name: item.item.item.source_name.clone(),
        source_kind: item.item.item.source_kind.clone(),
        title: item.item.item.title.clone(),
        summary,
        link: item.item.item.link.clone(),
        canonical_link: item.item.item.canonical_link.clone(),
        matched_topics: item
            .item
            .matched_topics
            .iter()
            .map(|topic| topic.as_str().to_string())
            .collect(),
        reason_selected: build_reason_selected(&item),
        title_hit_count: item.item.title_hit_count,
        snippet_hit_count: item.item.snippet_hit_count,
        source_weight: item.item.item.source_weight,
        recency_score: item.recency_score,
        topic_score: item.topic_score,
        cross_source_boost: item.cross_source_boost,
        final_score: item.final_score,
        used_title_only_fallback,
        thin_summary,
    }
}

fn summarize_item(
    title: &str,
    snippet: &str,
    matched_topics: &[&str],
    max_summary_chars: usize,
) -> (String, bool) {
    let snippet = snippet.trim();
    if !snippet.is_empty() {
        let first_sentence = snippet
            .split_terminator(['.', '!', '?'])
            .next()
            .unwrap_or(snippet)
            .trim();
        let cleaned = if is_mostly_title_repetition(title, first_sentence) {
            ""
        } else {
            first_sentence
        };
        if !cleaned.is_empty() {
            return (truncate_sentence(cleaned, max_summary_chars), false);
        }
    }

    let topic_phrase = if matched_topics.is_empty() {
        "AI systems"
    } else {
        matched_topics[0]
    };
    (
        truncate_sentence(&format!(
            "The story reports a new update related to {} systems and tooling.",
            topic_phrase
        ), max_summary_chars),
        true,
    )
}

fn truncate_sentence(value: &str, max_summary_chars: usize) -> String {
    let trimmed = value.trim().trim_end_matches(['.', '!', '?']).trim();
    let preview = trimmed.chars().take(max_summary_chars).collect::<String>();
    if trimmed.chars().count() > max_summary_chars {
        format!("{}.", preview.trim_end())
    } else {
        format!("{}.", trimmed)
    }
}

fn build_reason_selected(item: &ScoredNewsItem) -> String {
    if item.cross_source_boost >= 0.5 {
        return "multi-source coverage boost with high recency".to_string();
    }
    if item.item.strong_title_hits > 0 {
        return format!(
            "strong title match: {}",
            item.item
                .matched_topics
                .first()
                .map(TopicBucket::as_str)
                .unwrap_or("relevant topic")
        );
    }
    format!(
        "{} title hits with score {:.2}",
        item.item.title_hit_count, item.final_score
    )
}

fn build_artifact_hash(window_key: &str, items: &[NewsBriefItem]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(window_key.as_bytes());
    for item in items {
        hasher.update(item.source_id.as_bytes());
        hasher.update(item.canonical_link.as_bytes());
        hasher.update(item.title.as_bytes());
        hasher.update(item.summary.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn average_score(items: &[NewsBriefItem]) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    items.iter().map(|item| item.final_score).sum::<f64>() / items.len() as f64
}

fn is_thin_summary(title: &str, summary: &str, used_title_only_fallback: bool) -> bool {
    summary.chars().count() < 60
        || used_title_only_fallback
        || is_mostly_title_repetition(title, summary)
}

fn is_mostly_title_repetition(title: &str, summary: &str) -> bool {
    let normalized_title = normalize_compare_text(title);
    let normalized_summary = normalize_compare_text(summary);
    normalized_summary.starts_with(&normalized_title)
        || normalized_title.starts_with(&normalized_summary)
}

fn normalize_compare_text(value: &str) -> String {
    value.to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{average_score, is_thin_summary, parse_schedule_time, validate_news_brief_config};
    use crate::config::{AppConfig, CalendarSourceConfig, NewsSourceConfig, NewsSourceKind, NewsSourceType};
    use crate::tools::news_filter::TopicBucket;

    fn config() -> AppConfig {
        AppConfig {
            teloxide_token: "token".to_string(),
            chat_id: 1,
            ollama_base_url: "http://127.0.0.1:11434".to_string(),
            ollama_model: "qwen2.5:7b".to_string(),
            ollama_timeout_secs: 90,
            ollama_num_predict: 120,
            assistant_name: "Nox".to_string(),
            system_prompt: "test".to_string(),
            max_history_messages: 12,
            todo_store_path: "data/todos.json".to_string(),
            calendar_sources: Vec::<CalendarSourceConfig>::new(),
            destination_calendar_id: None,
            calendar_destination_provider: None,
            google_oauth_credentials_path: None,
            google_oauth_token_path: None,
            google_calendar_access_token: None,
            heartbeat_interval_secs: 1800,
            heartbeat_sync_window_days: 14,
            calendar_target_emails: Vec::new(),
            news_brief_enabled: true,
            news_brief_timezone: "UTC".to_string(),
            news_brief_schedule: vec!["08:00".to_string(), "15:00".to_string()],
            news_brief_max_items: 3,
            news_brief_min_items: 2,
            news_brief_min_avg_score: 0.62,
            news_brief_lookback_hours: 24,
            news_brief_fetch_cooldown_minutes: 20,
            news_brief_max_summary_chars: 180,
            news_brief_store_path: format!("/tmp/nox-news-brief-{}.json", std::process::id()),
            news_brief_sources: vec![NewsSourceConfig {
                id: "source".to_string(),
                name: "Source".to_string(),
                source_type: NewsSourceType::Rss,
                url: "https://example.test/feed.xml".to_string(),
                enabled: true,
                source_weight: 0.8,
                source_kind: NewsSourceKind::Reporting,
            }],
            news_brief_enabled_topics: vec![TopicBucket::Agents, TopicBucket::Llmops, TopicBucket::Rag],
            news_brief_negative_keywords: Vec::new(),
        }
    }

    #[test]
    fn validates_news_config() {
        validate_news_brief_config(&config()).expect("valid");
        assert!(parse_schedule_time("08:00").is_ok());
        assert!(parse_schedule_time("8am").is_err());
    }

    #[test]
    fn detects_thin_summary() {
        assert!(is_thin_summary(
            "Agent orchestration update",
            "Agent orchestration update.",
            false
        ));
    }

    #[test]
    fn average_score_handles_empty() {
        assert_eq!(average_score(&[]), 0.0);
    }

}
