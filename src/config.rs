use std::env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmMode {
    Hybrid,
    GeminiOnly,
    OllamaOnly,
}

impl LlmMode {
    fn from_env_value(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "hybrid" => Ok(Self::Hybrid),
            "gemini_only" => Ok(Self::GeminiOnly),
            "ollama_only" => Ok(Self::OllamaOnly),
            _ => Err("LLM_MODE must be one of: hybrid, gemini_only, ollama_only".to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub teloxide_token: String,
    pub chat_id: i64,
    pub priority_emails: String,
    pub llm_mode: LlmMode,
    pub ollama_base_url: String,
    pub ollama_model: String,
    pub ollama_timeout_secs: u64,
    pub heartbeat_interval_secs: u64,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let teloxide_token =
            env::var("TELOXIDE_TOKEN").map_err(|_| "TELOXIDE_TOKEN must be set".to_string())?;
        let chat_id_str = env::var("CHAT_ID").map_err(|_| "CHAT_ID must be set".to_string())?;
        let chat_id = chat_id_str
            .parse::<i64>()
            .map_err(|_| "CHAT_ID must be an integer".to_string())?;

        let priority_emails = env::var("PRIORITY_EMAILS").unwrap_or_default();
        let llm_mode = LlmMode::from_env_value(
            &env::var("LLM_MODE").unwrap_or_else(|_| "hybrid".to_string()),
        )?;
        let ollama_base_url =
            env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
        let ollama_model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5:7b".to_string());
        let ollama_timeout_secs = env::var("OLLAMA_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(90);
        let heartbeat_interval_secs = env::var("HEARTBEAT_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1800);

        Ok(Self {
            teloxide_token,
            chat_id,
            priority_emails,
            llm_mode,
            ollama_base_url,
            ollama_model,
            ollama_timeout_secs,
            heartbeat_interval_secs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::LlmMode;

    #[test]
    fn llm_mode_accepts_valid_values() {
        assert_eq!(LlmMode::from_env_value("hybrid"), Ok(LlmMode::Hybrid));
        assert_eq!(
            LlmMode::from_env_value("gemini_only"),
            Ok(LlmMode::GeminiOnly)
        );
        assert_eq!(
            LlmMode::from_env_value("ollama_only"),
            Ok(LlmMode::OllamaOnly)
        );
    }

    #[test]
    fn llm_mode_rejects_invalid_values() {
        let err = LlmMode::from_env_value("bad").unwrap_err();
        assert!(err.contains("LLM_MODE must be one of"));
    }
}
