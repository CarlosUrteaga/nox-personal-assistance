use crate::calendar::domain::{
    DesiredHubEvent, NormalizedEvent, NormalizedTiming, fingerprint_parts, timing_fingerprint,
};
use chrono::{DateTime, Utc};
use std::collections::HashSet;

pub fn resolve_canonical_events(
    events: &[NormalizedEvent],
    target_emails: &[String],
) -> Vec<DesiredHubEvent> {
    let segments = merged_segments(events, target_emails);
    let mut desired_events = segments
        .into_iter()
        .map(|segment| {
            DesiredHubEvent::new(
                canonical_key(&segment.timing),
                segment.source_id_winner,
                segment.timing,
                segment.summary,
                segment.category,
                segment.invite_targets,
                segment.covered_targets,
                segment.has_conflict,
            )
        })
        .collect::<Vec<_>>();

    desired_events.sort_by(|a, b| {
        sort_key(&a.timing)
            .cmp(&sort_key(&b.timing))
            .then_with(|| a.canonical_event_key.cmp(&b.canonical_event_key))
    });
    desired_events
}

fn sort_key(timing: &NormalizedTiming) -> i64 {
    match timing {
        NormalizedTiming::Timed { start, .. } => start.timestamp(),
        NormalizedTiming::AllDay { start_date, .. } => start_date
            .and_hms_opt(0, 0, 0)
            .expect("valid")
            .and_utc()
            .timestamp(),
    }
}

fn merged_segments(events: &[NormalizedEvent], target_emails: &[String]) -> Vec<ResolvedSegment> {
    if events.is_empty() {
        return Vec::new();
    }

    let mut boundaries = events
        .iter()
        .flat_map(|event| {
            let (start, end) = timing_bounds(&event.timing);
            [start, end]
        })
        .collect::<Vec<_>>();
    boundaries.sort();
    boundaries.dedup();

    let mut segments = Vec::new();

    for window in boundaries.windows(2) {
        let segment_start = window[0];
        let segment_end = window[1];
        if segment_start >= segment_end {
            continue;
        }

        let covering = events
            .iter()
            .filter(|event| covers_segment(&event.timing, segment_start, segment_end))
            .collect::<Vec<_>>();
        if covering.is_empty() {
            continue;
        }

        let winner = covering
            .iter()
            .max_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| b.source_id.cmp(&a.source_id))
            })
            .expect("covering is never empty");

        let owners = covering
            .iter()
            .filter_map(|e| e.owner_email.as_ref())
            .map(|e| e.trim().to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let unique_sources = covering
            .iter()
            .map(|event| event.source_id.as_str())
            .collect::<HashSet<_>>();
        let has_conflict = unique_sources.len() > 1;
        let summary = if has_conflict {
            format!("[Conflict] {}", winner.label)
        } else {
            winner.label.clone()
        };
        let (invite_targets, covered_targets) = split_targets(target_emails, &owners);

        segments.push(ResolvedSegment {
            timing: segment_timing(segment_start, segment_end, &covering),
            source_id_winner: winner.source_id.clone(),
            summary,
            category: winner.category.clone(),
            invite_targets,
            covered_targets,
            has_conflict,
        });
    }

    let mut merged: Vec<ResolvedSegment> = Vec::new();
    for segment in segments {
        if let Some(previous) = merged.last_mut() {
            if previous.can_merge_with(&segment) {
                previous.extend_to(segment.timing);
                continue;
            }
        }
        merged.push(segment);
    }

    merged
}

fn split_targets(
    target_emails: &[String],
    owners: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut covered_targets = Vec::new();
    let mut invite_targets = Vec::new();

    for target in target_emails {
        let lower_target = target.trim().to_ascii_lowercase();
        if lower_target.is_empty() {
            continue;
        }
        if owners.contains(&lower_target) {
            covered_targets.push(target.clone());
        } else {
            invite_targets.push(target.clone());
        }
    }

    (invite_targets, covered_targets)
}

fn timing_bounds(timing: &NormalizedTiming) -> (DateTime<Utc>, DateTime<Utc>) {
    match timing {
        NormalizedTiming::Timed { start, end } => (*start, *end),
        NormalizedTiming::AllDay {
            start_date,
            end_date_exclusive,
        } => (
            start_date
                .and_hms_opt(0, 0, 0)
                .expect("valid all-day start")
                .and_utc(),
            end_date_exclusive
                .and_hms_opt(0, 0, 0)
                .expect("valid all-day end")
                .and_utc(),
        ),
    }
}

fn covers_segment(
    timing: &NormalizedTiming,
    segment_start: DateTime<Utc>,
    segment_end: DateTime<Utc>,
) -> bool {
    let (start, end) = timing_bounds(timing);
    start < segment_end && end > segment_start
}

fn segment_timing(
    segment_start: DateTime<Utc>,
    segment_end: DateTime<Utc>,
    covering: &[&NormalizedEvent],
) -> NormalizedTiming {
    let all_day_only = covering
        .iter()
        .all(|event| matches!(event.timing, NormalizedTiming::AllDay { .. }));

    if all_day_only && segment_start.time() == chrono::NaiveTime::MIN && segment_end.time() == chrono::NaiveTime::MIN {
        return NormalizedTiming::AllDay {
            start_date: segment_start.date_naive(),
            end_date_exclusive: segment_end.date_naive(),
        };
    }

    NormalizedTiming::Timed {
        start: segment_start,
        end: segment_end,
    }
}

fn canonical_key(timing: &NormalizedTiming) -> String {
    fingerprint_parts(&["resolved_interval", &timing_fingerprint(timing)])
}

#[derive(Debug, Clone)]
struct ResolvedSegment {
    timing: NormalizedTiming,
    source_id_winner: String,
    summary: String,
    category: String,
    invite_targets: Vec<String>,
    covered_targets: Vec<String>,
    has_conflict: bool,
}

impl ResolvedSegment {
    fn can_merge_with(&self, other: &Self) -> bool {
        self.source_id_winner == other.source_id_winner
            && self.summary == other.summary
            && self.category == other.category
            && self.invite_targets == other.invite_targets
            && self.covered_targets == other.covered_targets
            && self.has_conflict == other.has_conflict
            && timings_are_adjacent(&self.timing, &other.timing)
    }

    fn extend_to(&mut self, next_timing: NormalizedTiming) {
        self.timing = merge_timings(&self.timing, &next_timing);
    }
}

fn timings_are_adjacent(left: &NormalizedTiming, right: &NormalizedTiming) -> bool {
    match (left, right) {
        (
            NormalizedTiming::Timed { end: left_end, .. },
            NormalizedTiming::Timed {
                start: right_start, ..
            },
        ) => left_end == right_start,
        (
            NormalizedTiming::AllDay {
                end_date_exclusive: left_end,
                ..
            },
            NormalizedTiming::AllDay {
                start_date: right_start,
                ..
            },
        ) => left_end == right_start,
        _ => false,
    }
}

fn merge_timings(left: &NormalizedTiming, right: &NormalizedTiming) -> NormalizedTiming {
    match (left, right) {
        (
            NormalizedTiming::Timed { start, .. },
            NormalizedTiming::Timed { end, .. },
        ) => NormalizedTiming::Timed {
            start: *start,
            end: *end,
        },
        (
            NormalizedTiming::AllDay { start_date, .. },
            NormalizedTiming::AllDay {
                end_date_exclusive, ..
            },
        ) => NormalizedTiming::AllDay {
            start_date: *start_date,
            end_date_exclusive: *end_date_exclusive,
        },
        _ => panic!("merge_timings requires matching timing variants"),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_canonical_events;
    use crate::calendar::domain::{NormalizedEvent, NormalizedTiming};
    use chrono::{TimeZone, Utc};

    #[test]
    fn single_event_covers_owner() {
        let events = vec![NormalizedEvent::new(
            "globant".into(),
            "event-1".into(),
            "Busy - Globant".into(),
            80,
            "business".into(),
            Some("carlos@globant.com".into()),
            NormalizedTiming::Timed {
                start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
            },
        )];

        let targets = vec![
            "carlos@globant.com".to_string(),
            "carlos@personal.com".to_string(),
        ];

        let resolved = resolve_canonical_events(&events, &targets);
        assert_eq!(resolved.len(), 1);
        assert!(!resolved[0].has_conflict);
        assert_eq!(resolved[0].summary, "Busy - Globant");
        assert_eq!(resolved[0].covered_targets, vec!["carlos@globant.com"]);
        assert_eq!(resolved[0].invite_targets, vec!["carlos@personal.com"]);
    }

    #[test]
    fn same_time_with_different_uids_creates_single_consolidated_event() {
        let events = vec![
            NormalizedEvent::new(
                "globant".into(),
                "globant-1".into(),
                "Busy - Globant".into(),
                80,
                "business".into(),
                Some("carlos@globant.com".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
                },
            ),
            NormalizedEvent::new(
                "client".into(),
                "client-1".into(),
                "Busy - Client".into(),
                100,
                "business".into(),
                Some("carlos@client.com".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
                },
            ),
        ];

        let targets = vec![
            "carlos@globant.com".to_string(),
            "carlos@client.com".to_string(),
            "carlos@personal.com".to_string(),
        ];

        let resolved = resolve_canonical_events(&events, &targets);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].has_conflict);
        assert_eq!(resolved[0].summary, "[Conflict] Busy - Client");
        assert_eq!(resolved[0].source_id_winner, "client");
        let mut covered = resolved[0].covered_targets.clone();
        covered.sort();
        assert_eq!(covered, vec!["carlos@client.com", "carlos@globant.com"]);
        assert_eq!(resolved[0].invite_targets, vec!["carlos@personal.com"]);
    }

    #[test]
    fn overlapping_events_split_by_priority_and_fill_gaps() {
        let events = vec![
            NormalizedEvent::new(
                "globant".into(),
                "globant-1".into(),
                "Busy - Globant".into(),
                80,
                "business".into(),
                Some("carlos@globant.com".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
                },
            ),
            NormalizedEvent::new(
                "outlook".into(),
                "outlook-1".into(),
                "Busy - Outlook".into(),
                100,
                "personal".into(),
                Some("carlos@outlook.com".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 30, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 30, 0).unwrap(),
                },
            ),
        ];

        let targets = vec![
            "carlos@globant.com".to_string(),
            "carlos@outlook.com".to_string(),
            "carlos@personal.com".to_string(),
        ];

        let resolved = resolve_canonical_events(&events, &targets);
        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].summary, "Busy - Globant");
        assert_eq!(resolved[0].source_id_winner, "globant");
        assert!(!resolved[0].has_conflict);
        assert_eq!(resolved[0].covered_targets, vec!["carlos@globant.com"]);
        assert_eq!(
            resolved[0].invite_targets,
            vec!["carlos@outlook.com", "carlos@personal.com"]
        );

        assert_eq!(resolved[1].summary, "[Conflict] Busy - Outlook");
        assert_eq!(resolved[1].source_id_winner, "outlook");
        assert!(resolved[1].has_conflict);
        let mut covered = resolved[1].covered_targets.clone();
        covered.sort();
        assert_eq!(covered, vec!["carlos@globant.com", "carlos@outlook.com"]);
        assert_eq!(resolved[1].invite_targets, vec!["carlos@personal.com"]);

        assert_eq!(resolved[2].summary, "Busy - Outlook");
        assert_eq!(resolved[2].source_id_winner, "outlook");
        assert!(!resolved[2].has_conflict);
        assert_eq!(resolved[2].covered_targets, vec!["carlos@outlook.com"]);
        assert_eq!(
            resolved[2].invite_targets,
            vec!["carlos@globant.com", "carlos@personal.com"]
        );
    }
}
