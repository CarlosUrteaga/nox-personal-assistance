use crate::calendar::destination::{CalendarDestination, build_calendar_destination};
use crate::calendar::domain::{DesiredHubEvent, NormalizedEvent, ReconcileStats};
use crate::calendar::ics::IcsFetcher;
use crate::calendar::resolve::resolve_canonical_events;
use crate::config::AppConfig;
use chrono::DateTime;
use chrono::{Datelike, Duration, TimeZone, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarSyncOutcome {
    pub source_events: usize,
    pub blockers: usize,
    pub stats: ReconcileStats,
}

pub struct CalendarSyncService {
    config: AppConfig,
    fetcher: IcsFetcher,
    destination: Box<dyn CalendarDestination>,
}

impl CalendarSyncService {
    pub fn new(config: AppConfig) -> Result<Self, String> {
        validate_calendar_sync_config(&config)?;

        let fetcher = IcsFetcher::new(config.ollama_timeout_secs)?;
        let destination = build_calendar_destination(&config)?;

        Ok(Self {
            config,
            fetcher,
            destination,
        })
    }

    pub async fn sync_once(&self) -> Result<CalendarSyncOutcome, String> {
        let plan = self.plan_sync().await?;
        let stats = self
            .destination
            .reconcile(&plan.resolved, plan.window_start, plan.window_end)
            .await?;

        Ok(CalendarSyncOutcome {
            source_events: plan.source_events,
            blockers: plan.resolved.len(),
            stats,
        })
    }

    pub async fn preview_once(&self) -> Result<CalendarSyncOutcome, String> {
        let plan = self.plan_sync().await?;
        Ok(CalendarSyncOutcome {
            source_events: plan.source_events,
            blockers: plan.resolved.len(),
            stats: ReconcileStats::default(),
        })
    }

    pub fn heartbeat_interval_secs(&self) -> u64 {
        self.config.heartbeat_interval_secs
    }

    async fn plan_sync(&self) -> Result<PreparedCalendarSync, String> {
        let now = Utc::now();
        let window_start = start_of_utc_day(now);
        let window_end = window_start + Duration::days(self.config.heartbeat_sync_window_days);
        let mut events = Vec::<NormalizedEvent>::new();

        for source in &self.config.calendar_sources {
            let fetched = self
                .fetcher
                .fetch_source_events(source, window_start, window_end)
                .await?;
            events.extend(fetched);
        }

        let resolved = resolve_canonical_events(&events, &self.config.calendar_target_emails);
        log::info!(
            "Resolved calendar canonical events: source_events={}, desired_hub_events={}",
            events.len(),
            resolved.len()
        );

        Ok(PreparedCalendarSync {
            window_start,
            window_end,
            source_events: events.len(),
            resolved,
        })
    }
}

pub fn validate_calendar_sync_config(config: &AppConfig) -> Result<(), String> {
    if config.calendar_sources.is_empty() {
        return Err(
            "Calendar sync is not enabled. Check CALENDAR_SOURCES_JSON in your environment."
                .to_string(),
        );
    }
    if config.destination_calendar_id.is_none() {
        return Err("DESTINATION_CALENDAR_ID must be configured".to_string());
    }
    match config
        .calendar_destination_provider
        .as_deref()
        .unwrap_or("google")
    {
        "google" => {
            if config.google_oauth_token_path.is_none()
                && config.google_oauth_credentials_path.is_none()
                && config.google_calendar_access_token.is_none()
            {
                return Err(
                    "GOOGLE_OAUTH_CREDENTIALS_PATH, GOOGLE_OAUTH_TOKEN_PATH, or GOOGLE_CALENDAR_ACCESS_TOKEN must be configured for calendar sync runtime."
                        .to_string(),
                );
            }
        }
        other => {
            return Err(format!(
                "Unsupported calendar destination provider '{}'",
                other
            ));
        }
    }
    if config.calendar_target_emails.is_empty() {
        return Err("CALENDAR_TARGET_EMAILS must be configured".to_string());
    }
    Ok(())
}

struct PreparedCalendarSync {
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    source_events: usize,
    resolved: Vec<DesiredHubEvent>,
}

fn start_of_utc_day(now: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .expect("valid UTC day boundary")
}

#[cfg(test)]
mod tests {
    use super::start_of_utc_day;
    use chrono::{TimeZone, Utc};

    #[test]
    fn uses_start_of_current_utc_day_for_sync_window() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 26, 18, 45, 12)
            .single()
            .expect("valid timestamp");

        assert_eq!(
            start_of_utc_day(now).to_rfc3339(),
            "2026-03-26T00:00:00+00:00"
        );
    }
}
