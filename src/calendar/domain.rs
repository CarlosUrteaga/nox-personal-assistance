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
    pub label: String,
    pub priority: u32,
    pub category: String,
    pub owner_email: Option<String>,
    pub timing: NormalizedTiming,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBlocker {
    pub source_id: String,
    pub label: String,
    pub category: String,
    pub attendees: Vec<String>,
    pub timing: NormalizedTiming,
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
        label: String,
        priority: u32,
        category: String,
        owner_email: Option<String>,
        timing: NormalizedTiming,
    ) -> Self {
        let fingerprint = fingerprint_parts(&[
            "normalized",
            &source_id,
            &label,
            &priority.to_string(),
            &category,
            owner_email.as_deref().unwrap_or(""),
            &timing_fingerprint(&timing),
        ]);

        Self {
            source_id,
            label,
            priority,
            category,
            owner_email,
            timing,
            fingerprint,
        }
    }

}

impl ResolvedBlocker {
    pub fn new(
        source_id: String,
        label: String,
        category: String,
        attendees: Vec<String>,
        timing: NormalizedTiming,
    ) -> Self {
        let attendee_key = attendees.join(",");
        let fingerprint =
            fingerprint_parts(&["blocker", &source_id, &label, &category, &attendee_key, &timing_fingerprint(&timing)]);

        Self {
            source_id,
            label,
            category,
            attendees,
            timing,
            fingerprint,
        }
    }
}

impl NormalizedTiming {
    pub fn intersects_window(&self, window_start: DateTime<Utc>, window_end: DateTime<Utc>) -> bool {
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

    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Timed { start: a_start, end: a_end }, Self::Timed { start: b_start, end: b_end }) => {
                *a_start < *b_end && *a_end > *b_start
            }
            (
                Self::AllDay {
                    start_date: a_start,
                    end_date_exclusive: a_end,
                },
                Self::AllDay {
                    start_date: b_start,
                    end_date_exclusive: b_end,
                },
            ) => *a_start < *b_end && *a_end > *b_start,
            _ => false,
        }
    }

    pub fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Timed { .. }, Self::Timed { .. }) | (Self::AllDay { .. }, Self::AllDay { .. })
        )
    }

    pub fn timed_range(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        match self {
            Self::Timed { start, end } => Some((*start, *end)),
            _ => None,
        }
    }

    pub fn merge_with(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Timed { start: left_start, end: left_end },
                Self::Timed { start: right_start, end: right_end },
            ) if *left_end == *right_start => Some(Self::Timed {
                start: *left_start,
                end: *right_end,
            }),
            (
                Self::AllDay {
                    start_date: left_start,
                    end_date_exclusive: left_end,
                },
                Self::AllDay {
                    start_date: right_start,
                    end_date_exclusive: right_end,
                },
            ) if *left_end == *right_start => Some(Self::AllDay {
                start_date: *left_start,
                end_date_exclusive: *right_end,
            }),
            _ => None,
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
