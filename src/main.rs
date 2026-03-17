mod agent;
mod channels;
mod config;
mod llm;
mod tools;

use crate::agent::NoxAgent;
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

    // 2. Initialize Channel (Telegram)
    let telegram_channel = TelegramChannel::new(&config);

    log::info!("NOX System Started. Listening for chat messages...");

    // 3. Start Channel (Blocking/Dispatching)
    telegram_channel.start(agent).await;
}
