use crate::calendar::destination::{CalendarDestination, build_calendar_destination};
use crate::calendar::domain::{NormalizedEvent, ReconcileStats};
use crate::calendar::ics::IcsFetcher;
use crate::calendar::resolve::resolve_blockers;
use crate::config::AppConfig;
use chrono::DateTime;
use chrono::{Duration, Utc};

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
        let window_start = Utc::now();
        let window_end = window_start + Duration::days(self.config.heartbeat_sync_window_days);
        let mut events = Vec::<NormalizedEvent>::new();

        for source in &self.config.calendar_sources {
            let fetched = self
                .fetcher
                .fetch_source_events(source, window_start, window_end)
                .await?;
            events.extend(fetched);
        }

        let resolved = resolve_blockers(
            &events,
            window_start,
            window_end,
            &self.config.calendar_target_emails,
        );
        log::info!(
            "Resolved calendar blockers: source_events={}, blockers={}",
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
            if config.google_calendar_access_token.is_none() {
                return Err("GOOGLE_CALENDAR_ACCESS_TOKEN must be configured".to_string());
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
    resolved: Vec<crate::calendar::domain::ResolvedBlocker>,
}
