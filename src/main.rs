mod agent;
mod calendar;
mod channels;
mod config;
mod llm;
mod runs;
mod tools;
mod web;

use crate::agent::NoxAgent;
use crate::calendar::format::{format_calendar_sync_cli, format_calendar_sync_dry_run_cli};
use crate::calendar::google_auth::bootstrap_google_oauth;
use crate::calendar::heartbeat::CalendarHeartbeat;
use crate::calendar::service::CalendarSyncService;
use crate::channels::telegram::TelegramChannel;
use crate::config::{AppConfig, GoogleOAuthBootstrapConfig, WebConfig};
use crate::runs::RunTracker;
use dotenv::dotenv;
use std::env;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    dotenv().ok();
    pretty_env_logger::init();

    let cli_args = env::args().skip(1).collect::<Vec<_>>();
    let arch = env::consts::ARCH;
    log::info!("NOX running on architecture: {}", arch);

    if matches!(cli_args.as_slice(), [command] if command == "google-auth") {
        let config = GoogleOAuthBootstrapConfig::from_env().unwrap_or_else(|err| {
            panic!(
                "Failed to load Google OAuth bootstrap configuration: {}",
                err
            );
        });
        match bootstrap_google_oauth(&config, 90).await {
            Ok(result) => {
                println!(
                    "google-auth complete\nsaved_token_path={}",
                    result.token_path
                );
                return;
            }
            Err(err) => {
                eprintln!("google-auth failed: {}", err);
                std::process::exit(1);
            }
        }
    }

    let web_config = WebConfig::from_env();
    let runtime_config = AppConfig::from_env();
    let run_tracker = RunTracker::new();

    if !cli_args.is_empty() {
        let config = runtime_config.as_ref().unwrap_or_else(|err| {
            panic!("Failed to load configuration for CLI command: {}", err);
        });
        if handle_cli_command(config, &cli_args).await {
            return;
        }
    }

    let web_handle = if web_config.enabled {
        let bind_address = web_config.bind_address.clone();
        let run_tracker = run_tracker.clone();
        Some(tokio::spawn(async move {
            if let Err(err) = web::serve(web_config, run_tracker).await {
                panic!("Web server failed on {}: {}", bind_address, err);
            }
        }))
    } else {
        log::info!("Web settings UI disabled via WEB_ENABLED=false");
        None
    };

    let config = match runtime_config {
        Ok(config) => config,
        Err(err) => {
            if web_handle.is_some() {
                log::warn!(
                    "Core assistant configuration is incomplete: {}. Web UI remains available for setup.",
                    err
                );
                if let Some(handle) = web_handle {
                    let _ = handle.await;
                }
                return;
            }
            panic!("Failed to load configuration: {}", err);
        }
    };

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
    let agent = Arc::new(NoxAgent::new(config.clone(), run_tracker.clone()));

    // 2. Initialize optional calendar heartbeat
    if config.calendar_sync_enabled() {
        log::info!(
            "Calendar sync enabled: sources={}, targets={}, interval_secs={}, window_days={}, destination_calendar={}",
            config.calendar_sources.len(),
            config.calendar_target_emails.len(),
            config.heartbeat_interval_secs,
            config.heartbeat_sync_window_days,
            config
                .destination_calendar_id
                .as_deref()
                .unwrap_or("<missing>")
        );
        let heartbeat = CalendarHeartbeat::new(
            config.clone(),
            TelegramChannel::new(&config, run_tracker.clone()),
        )
        .map_err(|e| {
            log::warn!("Calendar heartbeat disabled: {}", e);
            e
        })
        .ok();
        if let Some(heartbeat) = heartbeat {
            tokio::spawn(async move {
                heartbeat.run().await;
            });
        }
    } else {
        log::warn!(
            "Calendar sync disabled: no enabled sources found. Check CALENDAR_SOURCES_JSON; multiline .env JSON may not load as expected."
        );
    }

    // 3. Initialize Channel (Telegram)
    let telegram_channel = TelegramChannel::new(&config, run_tracker);

    log::info!("NOX System Started. Listening for chat messages...");

    // 4. Start Channel (Blocking/Dispatching)
    telegram_channel.start(agent).await;
}

async fn handle_cli_command(config: &AppConfig, args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }

    match args {
        [command] if command == "calendar-sync" => {
            let service = match CalendarSyncService::new(config.clone()) {
                Ok(service) => service,
                Err(err) => {
                    eprintln!("calendar-sync failed: {}", err);
                    std::process::exit(1);
                }
            };
            match service.sync_once().await {
                Ok(outcome) => {
                    println!("{}", format_calendar_sync_cli(&outcome));
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("calendar-sync failed: {}", err);
                    std::process::exit(1);
                }
            }
        }
        [command, flag] if command == "calendar-sync" && flag == "--dry-run" => {
            let service = match CalendarSyncService::new(config.clone()) {
                Ok(service) => service,
                Err(err) => {
                    eprintln!("calendar-sync --dry-run failed: {}", err);
                    std::process::exit(1);
                }
            };
            match service.preview_once().await {
                Ok(outcome) => {
                    println!("{}", format_calendar_sync_dry_run_cli(&outcome));
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("calendar-sync --dry-run failed: {}", err);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!(
                "unknown command. supported: `calendar-sync`, `calendar-sync --dry-run`, `google-auth`"
            );
            std::process::exit(2);
        }
    }
}
