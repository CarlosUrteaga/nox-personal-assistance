use crate::agent::orchestrator::AgentOrchestrator;
use crate::config::AppConfig;
use crate::tools::ToolResponse;

pub struct NoxAgent {
    orchestrator: AgentOrchestrator,
}

impl NoxAgent {
    pub fn new(config: AppConfig) -> Self {
        let orchestrator = AgentOrchestrator::new(config)
            .unwrap_or_else(|e| panic!("Failed to initialize orchestrator: {}", e));
        Self { orchestrator }
    }

    pub async fn process_heartbeat(&self) -> Vec<Result<ToolResponse, String>> {
        let mut responses = Vec::new();

        // 1. Check New Emails
        match self.orchestrator.check_new_emails().await {
            Ok(Some(resp)) => responses.push(Ok(resp)),
            Ok(None) => {} // No new emails
            Err(e) => responses.push(Err(format!("Email Check Error: {}", e))),
        }

        // 2. Sync Invitations
        match self.orchestrator.sync_invitations().await {
            Ok(Some(resp)) => responses.push(Ok(resp)),
            Ok(None) => {} // No sync needed
            Err(e) => responses.push(Err(format!("Sync Error: {}", e))),
        }

        responses
    }

    pub async fn handle_command(&self, cmd: &str) -> Result<ToolResponse, String> {
        match cmd {
            "calendar" => self
                .orchestrator
                .fetch_calendar_summary()
                .await?
                .ok_or_else(|| "No events found.".to_string()),
            "email" => {
                // Manual trigger for sync (scan last 10)
                self.orchestrator
                    .sync_invitations()
                    .await?
                    .ok_or_else(|| "No sync actions taken.".to_string())
            }
            _ => Err("Unknown command".to_string()),
        }
    }
}
