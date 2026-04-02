use crate::calendar::domain::{DesiredHubEvent, ReconcileStats};
use crate::config::AppConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait CalendarDestination: Send + Sync {
    async fn reconcile(
        &self,
        desired: &[DesiredHubEvent],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String>;
}

pub fn build_calendar_destination(
    config: &AppConfig,
) -> Result<Box<dyn CalendarDestination>, String> {
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
                    "Google Calendar runtime auth is not ready. Set GOOGLE_OAUTH_CREDENTIALS_PATH so NOX can create or recover token.json automatically, set GOOGLE_OAUTH_TOKEN_PATH to an existing token.json, or provide GOOGLE_CALENDAR_ACCESS_TOKEN."
                        .to_string(),
                );
            }
            Ok(Box::new(
                crate::calendar::google::GoogleCalendarDestination::new(
                    config.google_oauth_credentials_path.clone(),
                    config.google_oauth_token_path.clone(),
                    config.google_calendar_access_token.clone(),
                    config
                        .destination_calendar_id
                        .clone()
                        .ok_or_else(|| "DESTINATION_CALENDAR_ID must be configured".to_string())?,
                    config.ollama_timeout_secs,
                )?,
            ))
        }
        other => Err(format!(
            "Unsupported calendar destination provider '{}'",
            other
        )),
    }
}
