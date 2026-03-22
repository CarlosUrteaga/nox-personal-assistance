use crate::agent::orchestrator::AgentOrchestrator;
use crate::config::AppConfig;
use crate::runs::RunTracker;
use crate::tools::ToolResponse;

pub struct NoxAgent {
    orchestrator: AgentOrchestrator,
}

impl NoxAgent {
    pub fn new(config: AppConfig, run_tracker: RunTracker) -> Self {
        let orchestrator = AgentOrchestrator::new(config, run_tracker)
            .unwrap_or_else(|e| panic!("Failed to initialize orchestrator: {}", e));
        Self { orchestrator }
    }

    pub async fn chat(&self, chat_id: i64, message: &str) -> Result<ToolResponse, String> {
        self.orchestrator.chat(chat_id, message).await
    }

    pub async fn clear_memory(&self, chat_id: i64) {
        self.orchestrator.clear_memory(chat_id).await;
    }

    pub async fn add_todo(&self, chat_id: i64, content: &str) -> Result<ToolResponse, String> {
        self.orchestrator.add_todo(chat_id, content).await
    }

    pub async fn list_todos(&self, chat_id: i64) -> Result<ToolResponse, String> {
        self.orchestrator.list_todos(chat_id).await
    }

    pub async fn complete_todo(&self, chat_id: i64, id: u64) -> Result<ToolResponse, String> {
        self.orchestrator.complete_todo(chat_id, id).await
    }

    pub async fn calendar_sync(&self, chat_id: i64) -> Result<ToolResponse, String> {
        self.orchestrator.calendar_sync(chat_id).await
    }

    pub async fn maybe_handle_todo_intent(
        &self,
        chat_id: i64,
        message: &str,
    ) -> Result<Option<ToolResponse>, String> {
        self.orchestrator
            .maybe_handle_todo_intent(chat_id, message)
            .await
    }
}
