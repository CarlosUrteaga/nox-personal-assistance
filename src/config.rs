use std::env;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub teloxide_token: String,
    pub chat_id: i64,
    pub ollama_base_url: String,
    pub ollama_model: String,
    pub ollama_timeout_secs: u64,
    pub ollama_num_predict: u32,
    pub assistant_name: String,
    pub system_prompt: String,
    pub max_history_messages: usize,
    pub todo_store_path: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        let teloxide_token =
            env::var("TELOXIDE_TOKEN").map_err(|_| "TELOXIDE_TOKEN must be set".to_string())?;
        let chat_id_str = env::var("CHAT_ID").map_err(|_| "CHAT_ID must be set".to_string())?;
        let chat_id = chat_id_str
            .parse::<i64>()
            .map_err(|_| "CHAT_ID must be an integer".to_string())?;

        let ollama_base_url =
            env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
        let ollama_model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen2.5:7b".to_string());
        let ollama_timeout_secs = env::var("OLLAMA_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(90);
        let ollama_num_predict = env::var("OLLAMA_NUM_PREDICT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(120);
        let assistant_name = env::var("ASSISTANT_NAME").unwrap_or_else(|_| "Nox".to_string());
        let system_prompt = env::var("SYSTEM_PROMPT").unwrap_or_else(|_| {
            "You are Nox, a fast and concise personal assistant in Telegram. Default to short answers. Use 1-4 sentences unless the user explicitly asks for more detail. Help with planning, drafting, summaries, problem solving, and general chat. If a request would require external tools or private integrations that are not available, say so plainly and offer the best offline alternative."
                .to_string()
        });
        let max_history_messages = env::var("MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(12);
        let todo_store_path =
            env::var("TODO_STORE_PATH").unwrap_or_else(|_| "data/todos.json".to_string());

        Ok(Self {
            teloxide_token,
            chat_id,
            ollama_base_url,
            ollama_model,
            ollama_timeout_secs,
            ollama_num_predict,
            assistant_name,
            system_prompt,
            max_history_messages,
            todo_store_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use std::env;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn config_loads_defaults() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::remove_var("ASSISTANT_NAME");
            env::remove_var("SYSTEM_PROMPT");
            env::remove_var("MAX_HISTORY_MESSAGES");
        }

        let config = AppConfig::from_env().expect("config");

        assert_eq!(config.assistant_name, "Nox");
        assert!(config.system_prompt.contains("personal assistant"));
        assert_eq!(config.max_history_messages, 12);
        assert_eq!(config.todo_store_path, "data/todos.json");
        assert_eq!(config.ollama_num_predict, 120);
    }

    #[test]
    fn config_rejects_invalid_chat_id() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "bad");
        }

        let err = AppConfig::from_env().unwrap_err();
        assert!(err.contains("CHAT_ID must be an integer"));
    }
}
