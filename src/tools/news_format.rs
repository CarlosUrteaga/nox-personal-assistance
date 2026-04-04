use crate::tools::news::NewsBrief;
use chrono::{DateTime, LocalResult, TimeZone};
use chrono_tz::Tz;

pub fn format_news_brief_message(brief: &NewsBrief, timezone: Tz) -> String {
    let header_timestamp = match timezone.timestamp_opt(brief.window_epoch_secs, 0) {
        LocalResult::Single(value) => value.format("%Y-%m-%d %H:%M").to_string(),
        _ => DateTime::from_timestamp(brief.window_epoch_secs, 0)
            .map(|value| value.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| brief.window_key.clone()),
    };

    let mut body = vec![
        format!("🤖 Nox Brief — {}", header_timestamp),
        String::new(),
        "Top AI Agents / LLMOps / RAG updates".to_string(),
        String::new(),
    ];

    for (index, item) in brief.items.iter().enumerate() {
        body.push(format!("{}. {}", index + 1, item.title));
        body.push(format!("Summary: {}", item.summary));
        body.push(format!("Link: {}", item.link));
        if index + 1 < brief.items.len() {
            body.push(String::new());
        }
    }

    body.join("\n")
}

#[cfg(test)]
mod tests {
    use super::format_news_brief_message;
    use crate::tools::news::{NewsBrief, NewsBriefItem, NewsBriefMetrics};
    use chrono_tz::America::Mexico_City;

    #[test]
    fn formats_mobile_readable_message() {
        let brief = NewsBrief {
            window_key: "2026-04-02 15:00".to_string(),
            window_epoch_secs: 1775142000,
            generated_at_epoch_secs: 1775142060,
            prepared_artifact_hash: "abc".to_string(),
            metrics: NewsBriefMetrics::default(),
            items: vec![NewsBriefItem {
                source_id: "source".to_string(),
                source_name: "Source".to_string(),
                source_kind: "reporting".to_string(),
                title: "Agent orchestration update".to_string(),
                summary: "A platform update added agent orchestration controls for production workflows.".to_string(),
                link: "https://example.test/story".to_string(),
                canonical_link: "https://example.test/story".to_string(),
                matched_topics: vec!["agents".to_string()],
                reason_selected: "strong title match: agent orchestration".to_string(),
                title_hit_count: 2,
                snippet_hit_count: 1,
                source_weight: 0.8,
                recency_score: 0.9,
                topic_score: 0.8,
                cross_source_boost: 0.0,
                final_score: 0.85,
                used_title_only_fallback: false,
                thin_summary: false,
            }],
        };

        let rendered = format_news_brief_message(&brief, Mexico_City);
        assert!(rendered.contains("🤖 Nox Brief"));
        assert!(rendered.contains("Summary:"));
        assert!(rendered.contains("Link: https://example.test/story"));
    }
}
