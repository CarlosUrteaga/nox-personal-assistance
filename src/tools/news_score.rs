use crate::config::AppConfig;
use crate::tools::news::ScoredNewsItem;
use crate::tools::news_filter::{FilteredNewsItem, topic_score};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;

pub fn score_items(
    items: Vec<FilteredNewsItem>,
    config: &AppConfig,
    now: DateTime<Utc>,
) -> Vec<ScoredNewsItem> {
    let mut scored = items
        .into_iter()
        .map(|item| {
            let source_quality_score = item.item.source_weight.clamp(0.0, 1.0);
            let recency_score = recency_score(item.item.published_at, now, config.news_brief_lookback_hours);
            let topic_score = topic_score(&item);
            ScoredNewsItem {
                item,
                recency_score,
                topic_score,
                source_quality_score,
                cross_source_boost: 0.0,
                final_score: 0.0,
            }
        })
        .collect::<Vec<_>>();

    apply_cross_source_boost(&mut scored);
    for item in &mut scored {
        item.final_score = final_score(item);
    }

    scored.sort_by(|left, right| {
        right
            .final_score
            .partial_cmp(&left.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    suppress_exact_duplicates(scored)
}

pub fn final_score(item: &ScoredNewsItem) -> f64 {
    ((item.recency_score * 0.40)
        + (item.topic_score * 0.35)
        + (item.source_quality_score * 0.15)
        + (item.cross_source_boost * 0.10))
        .clamp(0.0, 1.0)
}

fn recency_score(
    published_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    lookback_hours: u64,
) -> f64 {
    let Some(published_at) = published_at else {
        return 0.2;
    };
    let age = now.signed_duration_since(published_at);
    if age < Duration::zero() {
        return 1.0;
    }
    let lookback_secs = (lookback_hours.max(1) as i64) * 3600;
    let age_secs = age.num_seconds().max(0);
    let ratio = (age_secs as f64 / lookback_secs as f64).clamp(0.0, 1.0);
    (1.0 - ratio.powf(0.65)).clamp(0.0, 1.0)
}

fn apply_cross_source_boost(items: &mut [ScoredNewsItem]) {
    for index in 0..items.len() {
        let mut boost: f64 = 0.0;
        for other_index in 0..items.len() {
            if index == other_index {
                continue;
            }
            let current = &items[index].item.item;
            let other = &items[other_index].item.item;
            if current.source_id == other.source_id {
                continue;
            }

            let similarity = title_similarity(&current.normalized_title, &other.normalized_title);
            if similarity >= 0.72 {
                boost += 0.5;
            }
        }
        items[index].cross_source_boost = boost.min(1.0);
    }
}

fn suppress_exact_duplicates(items: Vec<ScoredNewsItem>) -> Vec<ScoredNewsItem> {
    let mut seen_links = HashSet::new();
    let mut seen_titles = HashSet::new();
    let mut deduped = Vec::new();

    for item in items {
        let link_key = item.item.item.canonical_link.clone();
        let title_key = item.item.item.normalized_title.clone();
        if seen_links.contains(&link_key) || seen_titles.contains(&title_key) {
            continue;
        }
        seen_links.insert(link_key);
        seen_titles.insert(title_key);
        deduped.push(item);
    }

    deduped
}

fn title_similarity(left: &str, right: &str) -> f64 {
    let left_tokens = token_set(left);
    let right_tokens = token_set(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let overlap = left_tokens.intersection(&right_tokens).count() as f64;
    let union = left_tokens.union(&right_tokens).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        overlap / union
    }
}

fn token_set(value: &str) -> HashSet<String> {
    value.split_whitespace()
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::score_items;
    use crate::config::{AppConfig, CalendarSourceConfig, NewsSourceConfig, NewsSourceKind, NewsSourceType};
    use crate::tools::news::RawNewsItem;
    use crate::tools::news_filter::{TopicBucket, TopicMatchContext, classify_item};
    use chrono::{Duration, Utc};

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
            news_brief_schedule: vec!["08:00".to_string()],
            news_brief_max_items: 3,
            news_brief_min_items: 2,
            news_brief_min_avg_score: 0.62,
            news_brief_lookback_hours: 24,
            news_brief_fetch_cooldown_minutes: 20,
            news_brief_max_summary_chars: 180,
            news_brief_store_path: "data/news.json".to_string(),
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

    fn raw_item(title: &str, published_offset_hours: i64) -> RawNewsItem {
        RawNewsItem {
            source_id: "source".to_string(),
            source_name: "Source".to_string(),
            source_weight: 0.8,
            source_kind: "reporting".to_string(),
            title: title.to_string(),
            link: format!("https://example.test/{}", published_offset_hours),
            canonical_link: format!("https://example.test/{}", published_offset_hours),
            snippet: "Agent orchestration adds evaluation and monitoring.".to_string(),
            published_at: Some(Utc::now() - Duration::hours(published_offset_hours)),
            normalized_title: title.to_ascii_lowercase(),
        }
    }

    #[test]
    fn newer_item_outranks_older_item() {
        let context = TopicMatchContext {
            enabled_topics: vec![TopicBucket::Agents, TopicBucket::Llmops, TopicBucket::Rag],
            negative_keywords: Vec::new(),
        };
        let newer = classify_item(raw_item("Agent orchestration runtime", 1), &context).unwrap();
        let older = classify_item(raw_item("Agent orchestration runtime old", 20), &context).unwrap();

        let scored = score_items(vec![older, newer], &config(), Utc::now());
        assert!(scored[0].recency_score >= scored[1].recency_score);
        assert!(scored[0].final_score >= scored[1].final_score);
    }
}
