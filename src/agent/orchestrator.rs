use crate::config::{AppConfig, LlmMode};
use crate::llm::ollama::OllamaClient;
use crate::tools::{DataType, EmailDetails, ToolResponse, calendar, gmail};

pub struct AgentOrchestrator {
    config: AppConfig,
    ollama: Option<OllamaClient>,
}

impl AgentOrchestrator {
    pub fn new(config: AppConfig) -> Result<Self, String> {
        let ollama = if matches!(config.llm_mode, LlmMode::Hybrid | LlmMode::OllamaOnly) {
            Some(OllamaClient::new(
                config.ollama_base_url.clone(),
                config.ollama_model.clone(),
                config.ollama_timeout_secs,
            )?)
        } else {
            None
        };

        Ok(Self { config, ollama })
    }

    pub async fn check_new_emails(&self) -> Result<Option<ToolResponse>, String> {
        if matches!(self.config.llm_mode, LlmMode::OllamaOnly) {
            return Err(
                "ollama_only mode cannot execute Google Workspace actions without Gemini CLI"
                    .to_string(),
            );
        }

        let raw = match gmail::check_new_emails_raw().await? {
            Some(value) => value,
            None => return Ok(None),
        };

        let content = self
            .summarize_or_fallback("email_check", &raw)
            .await
            .unwrap_or(raw.clone());

        Ok(Some(ToolResponse {
            content: content.clone(),
            data_type: DataType::EmailSummary(EmailDetails {
                sender: "Unknown".to_string(),
                subject: content,
                snippet: String::new(),
            }),
        }))
    }

    pub async fn sync_invitations(&self) -> Result<Option<ToolResponse>, String> {
        if matches!(self.config.llm_mode, LlmMode::OllamaOnly) {
            return Err(
                "ollama_only mode cannot execute Google Workspace actions without Gemini CLI"
                    .to_string(),
            );
        }

        let raw = match gmail::sync_invitations_raw(&self.config.priority_emails).await? {
            Some(value) => value,
            None => return Ok(None),
        };

        let content = self
            .summarize_or_fallback("invitation_sync", &raw)
            .await
            .unwrap_or(raw.clone());

        Ok(Some(ToolResponse {
            content,
            data_type: DataType::Text,
        }))
    }

    pub async fn fetch_calendar_summary(&self) -> Result<Option<ToolResponse>, String> {
        if matches!(self.config.llm_mode, LlmMode::OllamaOnly) {
            return Err(
                "ollama_only mode cannot execute Google Workspace actions without Gemini CLI"
                    .to_string(),
            );
        }

        let raw = match calendar::fetch_calendar_summary_raw().await? {
            Some(value) => value,
            None => return Ok(None),
        };

        let content = self
            .summarize_or_fallback("calendar_summary", &raw)
            .await
            .unwrap_or(raw.clone());

        Ok(Some(ToolResponse {
            content,
            data_type: DataType::Text,
        }))
    }

    async fn summarize_or_fallback(&self, task_kind: &str, raw: &str) -> Result<String, String> {
        match self.config.llm_mode {
            LlmMode::GeminiOnly => Ok(raw.to_string()),
            LlmMode::Hybrid => {
                let Some(ollama) = &self.ollama else {
                    return Ok(raw.to_string());
                };

                match ollama.summarize_workspace_result(task_kind, raw).await {
                    Ok(msg) => Ok(msg),
                    Err(e) => {
                        log::warn!(
                            "Ollama unavailable for task '{}', falling back to Gemini output: {}",
                            task_kind,
                            e
                        );
                        Ok(raw.to_string())
                    }
                }
            }
            LlmMode::OllamaOnly => {
                Err("ollama_only mode is not supported for workspace tool execution".to_string())
            }
        }
    }
}
