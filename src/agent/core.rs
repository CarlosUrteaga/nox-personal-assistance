use crate::tools::{ToolResponse, gmail, calendar};

pub struct NoxAgent;

impl NoxAgent {
    pub fn new() -> Self {
        Self
    }

    pub async fn process_heartbeat(&self) -> Vec<Result<ToolResponse, String>> {
        let mut responses = Vec::new();

        // 1. Check New Emails
        match gmail::check_new_emails().await {
            Ok(Some(resp)) => responses.push(Ok(resp)),
            Ok(None) => {}, // No new emails
            Err(e) => responses.push(Err(format!("Email Check Error: {}", e))),
        }

        // 2. Sync Invitations
        match gmail::sync_invitations().await {
            Ok(Some(resp)) => responses.push(Ok(resp)),
            Ok(None) => {}, // No sync needed
            Err(e) => responses.push(Err(format!("Sync Error: {}", e))),
        }

        responses
    }

    pub async fn handle_command(&self, cmd: &str) -> Result<ToolResponse, String> {
        match cmd {
            "calendar" => {
                calendar::fetch_calendar_summary().await?
                    .ok_or_else(|| "No events found.".to_string())
            },
            "email" => {
                // Manual trigger for sync (scan last 10)
                gmail::sync_invitations().await?
                    .ok_or_else(|| "No sync actions taken.".to_string())
            },
            _ => Err("Unknown command".to_string()),
        }
    }
}
