mod agent;
mod calendar;
mod channels;
mod config;
mod llm;
mod tools;

use crate::agent::NoxAgent;
use crate::calendar::heartbeat::CalendarHeartbeat;
use crate::channels::telegram::TelegramChannel;
use crate::config::AppConfig;
use dotenv::dotenv;
use std::env;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    dotenv().ok();
    pretty_env_logger::init();

    let arch = env::consts::ARCH;
    log::info!("NOX running on architecture: {}", arch);

    let config = AppConfig::from_env().unwrap_or_else(|e| {
        panic!("Failed to load configuration: {}", e);
    });

    log::info!(
        "Loaded config: assistant_name='{}', chat_id={}, ollama_base_url={}, ollama_model={}, ollama_num_predict={}, max_history_messages={}",
        config.assistant_name,
        config.chat_id,
        config.ollama_base_url,
        config.ollama_model,
        config.ollama_num_predict,
        config.max_history_messages
    );

    // 1. Initialize Core Agent
    let agent = Arc::new(NoxAgent::new(config.clone()));

    // 2. Initialize optional calendar heartbeat
    if config.calendar_sync_enabled() {
        log::info!(
            "Calendar sync enabled: sources={}, targets={}, interval_secs={}, window_days={}, destination_calendar={}",
            config.calendar_sources.len(),
            config.calendar_target_emails.len(),
            config.heartbeat_interval_secs,
            config.heartbeat_sync_window_days,
            config.destination_calendar_id.as_deref().unwrap_or("<missing>")
        );
        let heartbeat = CalendarHeartbeat::new(config.clone(), TelegramChannel::new(&config))
            .unwrap_or_else(|e| panic!("Failed to initialize calendar heartbeat: {}", e));
        tokio::spawn(async move {
            heartbeat.run().await;
        });
    } else {
        log::warn!(
            "Calendar sync disabled: no enabled sources found. Check CALENDAR_SOURCES_JSON; multiline .env JSON may not load as expected."
        );
    }

    // 3. Initialize Channel (Telegram)
    let telegram_channel = TelegramChannel::new(&config);

    log::info!("NOX System Started. Listening for chat messages...");

    // 4. Start Channel (Blocking/Dispatching)
    telegram_channel.start(agent).await;
}
