use async_trait::async_trait;
use crate::calendar::domain::{ReconcileStats, ResolvedBlocker};
use crate::config::AppConfig;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait CalendarDestination: Send + Sync {
    async fn reconcile(
        &self,
        desired: &[ResolvedBlocker],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String>;
}

pub fn build_calendar_destination(
    config: &AppConfig,
) -> Result<Box<dyn CalendarDestination>, String> {
    match config.calendar_destination_provider.as_deref().unwrap_or("google") {
        "google" => Ok(Box::new(crate::calendar::google::GoogleCalendarDestination::new(
            config
                .google_calendar_access_token
                .clone()
                .ok_or_else(|| "GOOGLE_CALENDAR_ACCESS_TOKEN must be configured".to_string())?,
            config
                .destination_calendar_id
                .clone()
                .ok_or_else(|| "DESTINATION_CALENDAR_ID must be configured".to_string())?,
            config.ollama_timeout_secs,
        )?)),
        other => Err(format!(
            "Unsupported calendar destination provider '{}'",
            other
        )),
    }
}
