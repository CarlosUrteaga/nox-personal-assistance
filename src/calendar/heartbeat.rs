use crate::calendar::format::{
    format_calendar_heartbeat_error, format_calendar_heartbeat_success,
};
use crate::calendar::service::{CalendarSyncService, validate_calendar_sync_config};
use crate::channels::telegram::TelegramChannel;
use crate::config::AppConfig;

pub struct CalendarHeartbeat {
    service: CalendarSyncService,
    telegram: TelegramChannel,
}

impl CalendarHeartbeat {
    pub fn new(config: AppConfig, telegram: TelegramChannel) -> Result<Self, String> {
        validate_calendar_sync_config(&config)?;
        let service = CalendarSyncService::new(config)?;

        Ok(Self { service, telegram })
    }

    pub async fn run(self) {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(self.service.heartbeat_interval_secs()));

        loop {
            interval.tick().await;
            match self.service.sync_once().await {
                Ok(outcome) if outcome.stats.changed() => {
                    let message = format_calendar_heartbeat_success(&outcome);
                    if let Err(err) = self.telegram.send_system_message(&message).await {
                        log::error!("Failed to send heartbeat success message: {}", err);
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    log::error!("Calendar heartbeat failed: {}", err);
                    let message = format_calendar_heartbeat_error(&err);
                    if let Err(send_err) = self.telegram.send_system_message(&message).await {
                        log::error!("Failed to send heartbeat failure message: {}", send_err);
                    }
                }
            }
        }
    }
}
