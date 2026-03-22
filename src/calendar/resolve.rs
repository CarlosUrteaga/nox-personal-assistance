use crate::calendar::domain::{NormalizedEvent, NormalizedTiming, ResolvedBlocker};
use chrono::{DateTime, Days, NaiveDate, Utc};
use std::collections::{BTreeMap, BTreeSet};

pub fn resolve_blockers(
    events: &[NormalizedEvent],
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    target_emails: &[String],
) -> Vec<ResolvedBlocker> {
    let mut all_day_winners: BTreeMap<NaiveDate, &NormalizedEvent> = BTreeMap::new();
    let mut timed_events = Vec::new();

    for event in events {
        match &event.timing {
            NormalizedTiming::AllDay {
                start_date,
                end_date_exclusive,
            } => {
                let mut current = *start_date;
                while current < *end_date_exclusive {
                    all_day_winners
                        .entry(current)
                        .and_modify(|existing| {
                            if event.priority > existing.priority {
                                *existing = event;
                            }
                        })
                        .or_insert(event);
                    current = current
                        .checked_add_days(Days::new(1))
                        .expect("all-day date progression");
                }
            }
            NormalizedTiming::Timed { .. } => timed_events.push(event),
        }
    }

    let mut resolved = collapse_all_day_winners(&all_day_winners, target_emails);
    let timed = resolve_timed_blockers(
        &timed_events,
        &all_day_winners,
        window_start,
        window_end,
        target_emails,
    );
    resolved.extend(timed);
    resolved.sort_by_key(sort_key);
    resolved
}

fn collapse_all_day_winners(
    all_day_winners: &BTreeMap<NaiveDate, &NormalizedEvent>,
    target_emails: &[String],
) -> Vec<ResolvedBlocker> {
    let mut results: Vec<ResolvedBlocker> = Vec::new();

    for (day, winner) in all_day_winners {
        let next = ResolvedBlocker::new(
            winner.source_id.clone(),
            winner.label.clone(),
            winner.category.clone(),
            resolve_attendees(winner.owner_email.as_deref(), target_emails),
            NormalizedTiming::AllDay {
                start_date: *day,
                end_date_exclusive: day
                    .checked_add_days(Days::new(1))
                    .expect("next all-day date"),
            },
        );

        if let Some(last) = results.last_mut() {
            if last.source_id == next.source_id
                && last.label == next.label
                && last.category == next.category
            {
                if let Some(merged) = last.timing.merge_with(&next.timing) {
                    *last = ResolvedBlocker::new(
                        last.source_id.clone(),
                        last.label.clone(),
                        last.category.clone(),
                        last.attendees.clone(),
                        merged,
                    );
                    continue;
                }
            }
        }

        results.push(next);
    }

    results
}

fn resolve_timed_blockers(
    events: &[&NormalizedEvent],
    all_day_winners: &BTreeMap<NaiveDate, &NormalizedEvent>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    target_emails: &[String],
) -> Vec<ResolvedBlocker> {
    let mut boundaries = BTreeSet::new();
    boundaries.insert(window_start);
    boundaries.insert(window_end);

    for event in events {
        if let NormalizedTiming::Timed { start, end } = event.timing {
            if start < window_end
                && end > window_start
                && !is_shadowed_by_all_day(&event.timing, all_day_winners)
            {
                boundaries.insert(start);
                boundaries.insert(end);
            }
        }
    }

    let ordered = boundaries.into_iter().collect::<Vec<_>>();
    let mut resolved: Vec<ResolvedBlocker> = Vec::new();

    for pair in ordered.windows(2) {
        let segment_start = pair[0];
        let segment_end = pair[1];
        if segment_start >= segment_end {
            continue;
        }

        let winner = events
            .iter()
            .copied()
            .filter(|event| match event.timing {
                NormalizedTiming::Timed { start, end } => {
                    start < segment_end
                        && end > segment_start
                        && !is_shadowed_by_all_day(
                            &NormalizedTiming::Timed { start, end },
                            all_day_winners,
                        )
                }
                NormalizedTiming::AllDay { .. } => false,
            })
            .max_by_key(|event| event.priority);

        let Some(winner) = winner else {
            continue;
        };

        let next = ResolvedBlocker::new(
            winner.source_id.clone(),
            winner.label.clone(),
            winner.category.clone(),
            resolve_attendees(winner.owner_email.as_deref(), target_emails),
            NormalizedTiming::Timed {
                start: segment_start,
                end: segment_end,
            },
        );

        if let Some(last) = resolved.last_mut() {
            if last.source_id == next.source_id
                && last.label == next.label
                && last.category == next.category
            {
                if let Some(merged) = last.timing.merge_with(&next.timing) {
                    *last = ResolvedBlocker::new(
                        last.source_id.clone(),
                        last.label.clone(),
                        last.category.clone(),
                        last.attendees.clone(),
                        merged,
                    );
                    continue;
                }
            }
        }

        resolved.push(next);
    }

    resolved
}

fn is_shadowed_by_all_day(
    timing: &NormalizedTiming,
    all_day_winners: &BTreeMap<NaiveDate, &NormalizedEvent>,
) -> bool {
    let Some((start, end)) = timing.timed_range() else {
        return false;
    };
    let mut day = start.date_naive();
    let end_day = end.date_naive();
    loop {
        if all_day_winners.contains_key(&day) {
            return true;
        }
        if day >= end_day {
            break;
        }
        day = day
            .checked_add_days(Days::new(1))
            .expect("date progression");
    }
    false
}

fn sort_key(blocker: &ResolvedBlocker) -> (i64, String, String) {
    match blocker.timing {
        NormalizedTiming::Timed { start, .. } => (
            start.timestamp(),
            blocker.source_id.clone(),
            blocker.label.clone(),
        ),
        NormalizedTiming::AllDay { start_date, .. } => (
            start_date
                .and_hms_opt(0, 0, 0)
                .expect("valid all-day start")
                .and_utc()
                .timestamp(),
            blocker.source_id.clone(),
            blocker.label.clone(),
        ),
    }
}

fn resolve_attendees(owner_email: Option<&str>, target_emails: &[String]) -> Vec<String> {
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    let mut attendees = target_emails
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| {
            owner
                .as_deref()
                .map(|owner| value.to_ascii_lowercase() != owner)
                .unwrap_or(true)
        })
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    attendees.sort();
    attendees.dedup();
    attendees
}

#[cfg(test)]
mod tests {
    use super::resolve_blockers;
    use crate::calendar::domain::{NormalizedEvent, NormalizedTiming};
    use chrono::{NaiveDate, TimeZone, Utc};

    #[test]
    fn resolves_highest_priority_timed_overlap() {
        let events = vec![
            NormalizedEvent::new(
                "globant".into(),
                "Busy - Globant".into(),
                80,
                "business".into(),
                Some("globant@example.test".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
                },
            ),
            NormalizedEvent::new(
                "client".into(),
                "Busy - Client".into(),
                100,
                "business".into(),
                Some("client@example.test".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 30, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 30, 0).unwrap(),
                },
            ),
        ];

        let blockers = resolve_blockers(
            &events,
            Utc.with_ymd_and_hms(2026, 3, 20, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 3, 21, 0, 0, 0).unwrap(),
            &["client@example.test".into(), "personal@example.test".into()],
        );

        assert_eq!(blockers.len(), 2);
        assert_eq!(blockers[1].label, "Busy - Client");
        assert_eq!(
            blockers[1].attendees,
            vec!["personal@example.test".to_string()]
        );
    }

    #[test]
    fn all_day_winner_shadows_timed_events() {
        let events = vec![
            NormalizedEvent::new(
                "personal".into(),
                "Busy - Personal".into(),
                60,
                "personal".into(),
                Some("personal@example.test".into()),
                NormalizedTiming::AllDay {
                    start_date: NaiveDate::from_ymd_opt(2026, 3, 20).unwrap(),
                    end_date_exclusive: NaiveDate::from_ymd_opt(2026, 3, 21).unwrap(),
                },
            ),
            NormalizedEvent::new(
                "client".into(),
                "Busy - Client".into(),
                100,
                "business".into(),
                Some("client@example.test".into()),
                NormalizedTiming::Timed {
                    start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
                },
            ),
        ];

        let blockers = resolve_blockers(
            &events,
            Utc.with_ymd_and_hms(2026, 3, 20, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 3, 21, 0, 0, 0).unwrap(),
            &["client@example.test".into(), "personal@example.test".into()],
        );

        assert_eq!(blockers.len(), 1);
        assert!(matches!(
            blockers[0].timing,
            NormalizedTiming::AllDay { .. }
        ));
        assert_eq!(
            blockers[0].attendees,
            vec!["client@example.test".to_string()]
        );
    }
}
