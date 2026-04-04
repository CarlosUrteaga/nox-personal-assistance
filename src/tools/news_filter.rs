use crate::config::AppConfig;
use crate::tools::news::RawNewsItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TopicBucket {
    Agents,
    Llmops,
    Rag,
}

impl TopicBucket {
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agents" => Some(Self::Agents),
            "llmops" => Some(Self::Llmops),
            "rag" => Some(Self::Rag),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Llmops => "llmops",
            Self::Rag => "rag",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilteredNewsItem {
    pub item: RawNewsItem,
    pub matched_topics: Vec<TopicBucket>,
    pub title_hit_count: usize,
    pub snippet_hit_count: usize,
    pub strong_title_hits: usize,
    pub strong_snippet_hits: usize,
    pub contextual_title_hits: usize,
    pub contextual_snippet_hits: usize,
}

#[derive(Debug, Clone)]
pub struct TopicMatchContext {
    pub enabled_topics: Vec<TopicBucket>,
    pub negative_keywords: Vec<String>,
}

impl TopicMatchContext {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            enabled_topics: config.news_brief_enabled_topics.clone(),
            negative_keywords: config.news_brief_negative_keywords.clone(),
        }
    }
}

pub fn filter_relevant_items(
    items: Vec<RawNewsItem>,
    context: &TopicMatchContext,
) -> Vec<FilteredNewsItem> {
    items.into_iter()
        .filter_map(|item| classify_item(item, context))
        .collect()
}

pub fn classify_item(
    item: RawNewsItem,
    context: &TopicMatchContext,
) -> Option<FilteredNewsItem> {
    let title = normalize_text(&item.title);
    let snippet = normalize_text(&item.snippet);
    let combined = format!("{} {}", title, snippet);

    if contains_negative_signal(&combined, &context.negative_keywords) {
        return None;
    }

    let mut matched_topics = Vec::new();
    let mut title_hit_count = 0;
    let mut snippet_hit_count = 0;
    let mut strong_title_hits = 0;
    let mut strong_snippet_hits = 0;
    let mut contextual_title_hits = 0;
    let mut contextual_snippet_hits = 0;

    for topic in &context.enabled_topics {
        let keywords = keywords_for(topic);
        let topic_title_strong = count_matches(&title, keywords.strong);
        let topic_snippet_strong = count_matches(&snippet, keywords.strong);
        let topic_title_contextual = count_matches(&title, keywords.contextual);
        let topic_snippet_contextual = count_matches(&snippet, keywords.contextual);

        if topic_title_strong + topic_snippet_strong > 0
            || topic_title_contextual + topic_snippet_contextual >= 2
        {
            matched_topics.push(topic.clone());
        }

        strong_title_hits += topic_title_strong;
        strong_snippet_hits += topic_snippet_strong;
        contextual_title_hits += topic_title_contextual;
        contextual_snippet_hits += topic_snippet_contextual;
        title_hit_count += topic_title_strong + topic_title_contextual;
        snippet_hit_count += topic_snippet_strong + topic_snippet_contextual;
    }

    let has_strong_signal = strong_title_hits + strong_snippet_hits > 0;
    let has_contextual_signal = contextual_title_hits >= 1
        || contextual_title_hits + contextual_snippet_hits >= 3;

    if matched_topics.is_empty() || (!has_strong_signal && !has_contextual_signal) {
        return None;
    }

    Some(FilteredNewsItem {
        item,
        matched_topics,
        title_hit_count,
        snippet_hit_count,
        strong_title_hits,
        strong_snippet_hits,
        contextual_title_hits,
        contextual_snippet_hits,
    })
}

pub fn topic_score(item: &FilteredNewsItem) -> f64 {
    let strong = (item.strong_title_hits as f64 * 1.0) + (item.strong_snippet_hits as f64 * 0.65);
    let contextual = (item.contextual_title_hits as f64 * 0.55)
        + (item.contextual_snippet_hits as f64 * 0.3);
    let topic_coverage_bonus = (item.matched_topics.len() as f64 * 0.18).min(0.36);
    (strong + contextual + topic_coverage_bonus).min(1.0)
}

fn keywords_for(topic: &TopicBucket) -> TopicKeywords<'static> {
    match topic {
        TopicBucket::Agents => TopicKeywords {
            strong: &[
                "agent",
                "agents",
                "agentic",
                "tool use",
                "orchestration",
                "planner",
                "executor",
                "multi-agent",
                "terminal agent",
                "code agent",
                "assistant runtime",
            ],
            contextual: &[
                "workflow",
                "autonomy",
                "task execution",
                "tool calling",
                "runtime",
                "planning",
                "coordination",
            ],
        },
        TopicBucket::Llmops => TopicKeywords {
            strong: &[
                "llmops",
                "observability",
                "evaluation",
                "tracing",
                "monitoring",
                "reliability",
                "deployment",
                "governance",
                "guardrails",
                "prompt management",
            ],
            contextual: &[
                "production",
                "workflow",
                "reliability",
                "serving",
                "inference",
                "testing",
                "rollout",
            ],
        },
        TopicBucket::Rag => TopicKeywords {
            strong: &[
                "rag",
                "retrieval",
                "retriever",
                "reranker",
                "vector database",
                "vector search",
                "grounding",
                "chunking",
                "indexing",
                "semantic search",
                "knowledge base",
                "hybrid search",
            ],
            contextual: &[
                "vector",
                "search",
                "index",
                "ranker",
                "retrieved",
                "documents",
                "knowledge",
            ],
        },
    }
}

fn count_matches(text: &str, patterns: &[&str]) -> usize {
    patterns.iter().filter(|pattern| text.contains(**pattern)).count()
}

fn contains_negative_signal(text: &str, configured_negative_keywords: &[String]) -> bool {
    let defaults = [
        "iphone",
        "smartwatch",
        "android app",
        "gaming",
        "xbox",
        "playstation",
        "crypto",
        "bitcoin",
        "ether",
        "funding round",
        "series a",
        "series b",
        "consumer gadget",
        "mobile app",
    ];

    defaults
        .iter()
        .any(|keyword| text.contains(keyword))
        || configured_negative_keywords
            .iter()
            .map(|keyword| keyword.trim().to_ascii_lowercase())
            .filter(|keyword| !keyword.is_empty())
            .any(|keyword| text.contains(&keyword))
}

fn normalize_text(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .replace('&', " and ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

struct TopicKeywords<'a> {
    strong: &'a [&'a str],
    contextual: &'a [&'a str],
}

#[cfg(test)]
mod tests {
    use super::{TopicBucket, TopicMatchContext, classify_item, topic_score};
    use crate::tools::news::RawNewsItem;
    use chrono::Utc;

    fn sample_item(title: &str, snippet: &str) -> RawNewsItem {
        RawNewsItem {
            source_id: "test".to_string(),
            source_name: "Test".to_string(),
            source_weight: 0.8,
            source_kind: "reporting".to_string(),
            title: title.to_string(),
            link: "https://example.test/post".to_string(),
            canonical_link: "https://example.test/post".to_string(),
            snippet: snippet.to_string(),
            published_at: Some(Utc::now()),
            normalized_title: title.to_ascii_lowercase(),
        }
    }

    #[test]
    fn passes_strong_agent_signal() {
        let context = TopicMatchContext {
            enabled_topics: vec![TopicBucket::Agents, TopicBucket::Llmops, TopicBucket::Rag],
            negative_keywords: Vec::new(),
        };
        let item = sample_item(
            "New agent orchestration runtime ships for terminal workflows",
            "The release adds planner and executor coordination with tracing.",
        );

        let classified = classify_item(item, &context).expect("classified");
        assert!(classified.matched_topics.contains(&TopicBucket::Agents));
        assert!(topic_score(&classified) > 0.5);
    }

    #[test]
    fn rejects_negative_gadget_signal() {
        let context = TopicMatchContext {
            enabled_topics: vec![TopicBucket::Agents, TopicBucket::Llmops, TopicBucket::Rag],
            negative_keywords: vec!["smartphone".to_string()],
        };
        let item = sample_item(
            "New iPhone app wraps a chatbot assistant",
            "The mobile app launch focuses on consumers and gadgets.",
        );

        assert!(classify_item(item, &context).is_none());
    }
}
