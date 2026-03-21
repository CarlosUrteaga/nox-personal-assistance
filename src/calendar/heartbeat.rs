use crate::calendar::domain::{NormalizedEvent, ReconcileStats};
use crate::calendar::google::GoogleCalendarClient;
use crate::calendar::ics::IcsFetcher;
use crate::calendar::resolve::resolve_blockers;
use crate::channels::telegram::TelegramChannel;
use crate::config::AppConfig;
use chrono::{Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarSyncOutcome {
    pub source_events: usize,
    pub blockers: usize,
    pub stats: ReconcileStats,
}

pub struct CalendarHeartbeat {
    config: AppConfig,
    telegram: TelegramChannel,
}

impl CalendarHeartbeat {
    pub fn new(config: AppConfig, telegram: TelegramChannel) -> Result<Self, String> {
        validate_calendar_sync_config(&config)?;

        Ok(Self { config, telegram })
    }

    pub async fn run(self) {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(self.config.heartbeat_interval_secs));

        loop {
            interval.tick().await;
            match run_calendar_sync(&self.config).await {
                Ok(outcome) if outcome.stats.changed() => {
                    let message = format!(
                        "NOX heartbeat updated blockers.\nSource events: {}\nResolved blockers: {}\nCreated: {}\nUpdated: {}\nCancelled: {}",
                        outcome.source_events,
                        outcome.blockers,
                        outcome.stats.created,
                        outcome.stats.updated,
                        outcome.stats.deleted
                    );
                    if let Err(err) = self.telegram.send_system_message(&message).await {
                        log::error!("Failed to send heartbeat success message: {}", err);
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    log::error!("Calendar heartbeat failed: {}", err);
                    let message = format!("NOX heartbeat failed.\n{}", sanitize_error(&err));
                    if let Err(send_err) = self.telegram.send_system_message(&message).await {
                        log::error!("Failed to send heartbeat failure message: {}", send_err);
                    }
                }
            }
        }
    }
}

fn sanitize_error(error: &str) -> String {
    error.chars().take(280).collect()
}

pub async fn run_calendar_sync(config: &AppConfig) -> Result<CalendarSyncOutcome, String> {
    validate_calendar_sync_config(config)?;

    let fetcher = IcsFetcher::new(config.ollama_timeout_secs)?;
    let google = GoogleCalendarClient::new(
        config
            .google_calendar_access_token
            .clone()
            .ok_or_else(|| "GOOGLE_CALENDAR_ACCESS_TOKEN must be configured".to_string())?,
        config
            .destination_calendar_id
            .clone()
            .ok_or_else(|| "DESTINATION_CALENDAR_ID must be configured".to_string())?,
        config.ollama_timeout_secs,
    )?;

    let window_start = Utc::now();
    let window_end = window_start + Duration::days(config.heartbeat_sync_window_days);
    let mut events = Vec::<NormalizedEvent>::new();

    for source in &config.calendar_sources {
        let fetched = fetcher
            .fetch_source_events(source, window_start, window_end)
            .await?;
        events.extend(fetched);
    }

    let resolved = resolve_blockers(
        &events,
        window_start,
        window_end,
        &config.calendar_target_emails,
    );
    log::info!(
        "Resolved calendar blockers: source_events={}, blockers={}",
        events.len(),
        resolved.len()
    );

    let stats = google.reconcile(&resolved, window_start, window_end).await?;

    Ok(CalendarSyncOutcome {
        source_events: events.len(),
        blockers: resolved.len(),
        stats,
    })
}

fn validate_calendar_sync_config(config: &AppConfig) -> Result<(), String> {
    if config.calendar_sources.is_empty() {
        return Err(
            "Calendar sync is not enabled. Check CALENDAR_SOURCES_JSON in your environment."
                .to_string(),
        );
    }
    if config.destination_calendar_id.is_none() {
        return Err("DESTINATION_CALENDAR_ID must be configured".to_string());
    }
    if config.google_calendar_access_token.is_none() {
        return Err("GOOGLE_CALENDAR_ACCESS_TOKEN must be configured".to_string());
    }
    Ok(())
}
