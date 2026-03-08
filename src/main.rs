mod agent;
mod tools;
mod channels;
mod config;

use dotenv::dotenv;
use std::env;
use std::sync::Arc;
use tokio::time::{self, Duration};
use crate::agent::NoxAgent;
use crate::channels::telegram::TelegramChannel;

#[tokio::main]
async fn main() {
    dotenv().ok();
    pretty_env_logger::init();

    let arch = env::consts::ARCH;
    log::info!("NOX Heartbeat running on architecture: {}", arch);

    // 1. Initialize Core Agent
    let agent = Arc::new(NoxAgent::new());

    // 2. Initialize Channels (Telegram)
    let telegram_channel = TelegramChannel::new();

    // 3. Spawn Heartbeat Loop
    let agent_clone = agent.clone();
    let channel_clone = telegram_channel.clone();

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30 * 60));
        loop {
            interval.tick().await;
            log::info!("Heartbeat: Processing...");

            let responses = agent_clone.process_heartbeat().await;
            
            for result in responses {
                match result {
                    Ok(tool_response) => {
                        // Send structured response to Telegram
                        if let Err(e) = channel_clone.send_tool_response(tool_response).await {
                            log::error!("Failed to send heartbeat message: {}", e);
                        }
                    },
                    Err(e) => {
                        log::error!("Heartbeat Agent Error: {}", e);
                        // Optionally notify user of error
                        // channel_clone.send_message(&format!("⚠️ Error: {}", e)).await;
                    }
                }
            }
        }
    });

    log::info!("NOX System Started. Listening for commands...");
    
    // 4. Start Channel (Blocking/Dispatching)
    telegram_channel.start(agent).await;
}
