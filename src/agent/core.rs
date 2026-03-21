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

    pub async fn chat(&self, chat_id: i64, message: &str) -> Result<ToolResponse, String> {
        self.orchestrator.chat(chat_id, message).await
    }

    pub async fn clear_memory(&self, chat_id: i64) {
        self.orchestrator.clear_memory(chat_id).await;
    }

    pub async fn add_todo(&self, content: &str) -> Result<ToolResponse, String> {
        self.orchestrator.add_todo(content).await
    }

    pub async fn list_todos(&self) -> Result<ToolResponse, String> {
        self.orchestrator.list_todos().await
    }

    pub async fn complete_todo(&self, id: u64) -> Result<ToolResponse, String> {
        self.orchestrator.complete_todo(id).await
    }

    pub async fn calendar_sync(&self) -> Result<ToolResponse, String> {
        self.orchestrator.calendar_sync().await
    }

    pub async fn maybe_handle_todo_intent(&self, message: &str) -> Result<Option<ToolResponse>, String> {
        self.orchestrator.maybe_handle_todo_intent(message).await
    }
}
