use chrono::{DateTime, NaiveDate, Utc};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedTiming {
    Timed {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    AllDay {
        start_date: NaiveDate,
        end_date_exclusive: NaiveDate,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub source_id: String,
    pub source_event_key: String,
    pub label: String,
    pub priority: u32,
    pub category: String,
    pub owner_email: Option<String>,
    pub timing: NormalizedTiming,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredHubEvent {
    pub canonical_event_key: String,
    pub source_id_winner: String,
    pub timing: NormalizedTiming,
    pub summary: String,
    pub category: String,
    pub invite_targets: Vec<String>,
    pub covered_targets: Vec<String>,
    pub has_conflict: bool,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
}

impl ReconcileStats {
    pub fn changed(&self) -> bool {
        self.created > 0 || self.updated > 0 || self.deleted > 0
    }
}

impl NormalizedEvent {
    pub fn new(
        source_id: String,
        source_event_key: String,
        label: String,
        priority: u32,
        category: String,
        owner_email: Option<String>,
        timing: NormalizedTiming,
    ) -> Self {
        let fingerprint = fingerprint_parts(&[
            "normalized",
            &source_id,
            &source_event_key,
            &label,
            &priority.to_string(),
            &category,
            owner_email.as_deref().unwrap_or(""),
            &timing_fingerprint(&timing),
        ]);

        Self {
            source_id,
            source_event_key,
            label,
            priority,
            category,
            owner_email,
            timing,
            fingerprint,
        }
    }
}

impl DesiredHubEvent {
    pub fn new(
        canonical_event_key: String,
        source_id_winner: String,
        timing: NormalizedTiming,
        summary: String,
        category: String,
        mut invite_targets: Vec<String>,
        mut covered_targets: Vec<String>,
        has_conflict: bool,
    ) -> Self {
        invite_targets.sort();
        invite_targets.dedup();
        covered_targets.sort();
        covered_targets.dedup();

        let fingerprint = fingerprint_parts(&[
            "desired_hub_event",
            &canonical_event_key,
            &source_id_winner,
            &summary,
            &category,
            &invite_targets.join(","),
            &covered_targets.join(","),
            if has_conflict { "1" } else { "0" },
            &timing_fingerprint(&timing),
        ]);

        Self {
            canonical_event_key,
            source_id_winner,
            timing,
            summary,
            category,
            invite_targets,
            covered_targets,
            has_conflict,
            fingerprint,
        }
    }
}

impl NormalizedTiming {
    pub fn intersects_window(
        &self,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> bool {
        match self {
            Self::Timed { start, end } => *start < window_end && *end > window_start,
            Self::AllDay {
                start_date,
                end_date_exclusive,
            } => {
                let all_day_start = start_date
                    .and_hms_opt(0, 0, 0)
                    .expect("valid all-day start")
                    .and_utc();
                let all_day_end = end_date_exclusive
                    .and_hms_opt(0, 0, 0)
                    .expect("valid all-day end")
                    .and_utc();
                all_day_start < window_end && all_day_end > window_start
            }
        }
    }
}

pub fn timing_fingerprint(timing: &NormalizedTiming) -> String {
    match timing {
        NormalizedTiming::Timed { start, end } => {
            format!("timed:{}:{}", start.to_rfc3339(), end.to_rfc3339())
        }
        NormalizedTiming::AllDay {
            start_date,
            end_date_exclusive,
        } => format!("all-day:{}:{}", start_date, end_date_exclusive),
    }
}

pub fn fingerprint_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0x1f]);
    }
    hex::encode(hasher.finalize())
}
