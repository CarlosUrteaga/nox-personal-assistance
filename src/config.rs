use serde::Deserialize;
use std::fs;
use std::env;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CalendarSourceType {
    Ics,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CalendarSourceConfig {
    pub id: String,
    #[serde(rename = "type")]
    pub source_type: CalendarSourceType,
    pub url: String,
    pub label: String,
    pub priority: u32,
    pub category: String,
    pub enabled: bool,
    pub owner_email: Option<String>,
}

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
    pub calendar_sources: Vec<CalendarSourceConfig>,
    pub destination_calendar_id: Option<String>,
    pub google_calendar_access_token: Option<String>,
    pub heartbeat_interval_secs: u64,
    pub heartbeat_sync_window_days: i64,
    pub calendar_target_emails: Vec<String>,
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
        let calendar_sources = parse_calendar_sources()?;
        let destination_calendar_id = env::var("DESTINATION_CALENDAR_ID")
            .ok()
            .or_else(|| read_dotenv_value("DESTINATION_CALENDAR_ID"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let google_calendar_access_token = env::var("GOOGLE_CALENDAR_ACCESS_TOKEN")
            .ok()
            .or_else(|| read_dotenv_value("GOOGLE_CALENDAR_ACCESS_TOKEN"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let heartbeat_interval_secs = env::var("HEARTBEAT_INTERVAL_SECS")
            .ok()
            .or_else(|| read_dotenv_value("HEARTBEAT_INTERVAL_SECS"))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1800);
        let heartbeat_sync_window_days = env::var("HEARTBEAT_SYNC_WINDOW_DAYS")
            .ok()
            .or_else(|| read_dotenv_value("HEARTBEAT_SYNC_WINDOW_DAYS"))
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(14);
        let calendar_target_emails = parse_calendar_target_emails()?;

        if !calendar_sources.is_empty() {
            if destination_calendar_id.is_none() {
                return Err("DESTINATION_CALENDAR_ID must be set when calendar sync is enabled".to_string());
            }
            if google_calendar_access_token.is_none() {
                return Err(
                    "GOOGLE_CALENDAR_ACCESS_TOKEN must be set when calendar sync is enabled"
                        .to_string(),
                );
            }
            if calendar_target_emails.is_empty() {
                return Err(
                    "CALENDAR_TARGET_EMAILS must be set when calendar sync is enabled".to_string(),
                );
            }
        }

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
            calendar_sources,
            destination_calendar_id,
            google_calendar_access_token,
            heartbeat_interval_secs,
            heartbeat_sync_window_days,
            calendar_target_emails,
        })
    }

    pub fn calendar_sync_enabled(&self) -> bool {
        !self.calendar_sources.is_empty()
    }
}

fn parse_calendar_sources() -> Result<Vec<CalendarSourceConfig>, String> {
    let Some(raw) = env::var("CALENDAR_SOURCES_JSON")
        .ok()
        .or_else(|| read_dotenv_value("CALENDAR_SOURCES_JSON"))
    else {
        return Ok(Vec::new());
    };

    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let sources: Vec<CalendarSourceConfig> = serde_json::from_str(&raw)
        .map_err(|e| format!("CALENDAR_SOURCES_JSON must be valid JSON: {}", e))?;

    let mut seen_ids = std::collections::HashSet::new();
    for source in &sources {
        if source.id.trim().is_empty() {
            return Err("CALENDAR_SOURCES_JSON source id must not be empty".to_string());
        }
        if source.url.trim().is_empty() {
            return Err(format!("Calendar source '{}' url must not be empty", source.id));
        }
        if source.label.trim().is_empty() {
            return Err(format!("Calendar source '{}' label must not be empty", source.id));
        }
        if let Some(owner_email) = &source.owner_email {
            if owner_email.trim().is_empty() {
                return Err(format!(
                    "Calendar source '{}' owner_email must not be empty when provided",
                    source.id
                ));
            }
        }
        if !seen_ids.insert(source.id.to_ascii_lowercase()) {
            return Err(format!("Duplicate calendar source id '{}'", source.id));
        }
    }

    Ok(sources.into_iter().filter(|source| source.enabled).collect())
}

fn parse_calendar_target_emails() -> Result<Vec<String>, String> {
    let Some(raw) = env::var("CALENDAR_TARGET_EMAILS")
        .ok()
        .or_else(|| read_dotenv_value("CALENDAR_TARGET_EMAILS"))
    else {
        return Ok(Vec::new());
    };

    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let emails: Vec<String> = serde_json::from_str(&raw)
        .map_err(|e| format!("CALENDAR_TARGET_EMAILS must be valid JSON: {}", e))?;

    let normalized = emails
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    Ok(normalized)
}

fn read_dotenv_value(key: &str) -> Option<String> {
    let contents = fs::read_to_string(".env").ok()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let (candidate_key, raw_value) = trimmed.split_once('=')?;
        if candidate_key.trim() != key {
            continue;
        }

        let value = raw_value.trim();
        let unwrapped = if (value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"'))
        {
            &value[1..value.len().saturating_sub(1)]
        } else {
            value
        };

        if !unwrapped.is_empty() {
            return Some(unwrapped.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use std::env;
    use std::fs;
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
            env::set_var("CALENDAR_SOURCES_JSON", "");
            env::remove_var("DESTINATION_CALENDAR_ID");
            env::remove_var("GOOGLE_CALENDAR_ACCESS_TOKEN");
            env::set_var("CALENDAR_TARGET_EMAILS", "");
        }

        let config = AppConfig::from_env().expect("config");

        assert_eq!(config.assistant_name, "Nox");
        assert!(config.system_prompt.contains("personal assistant"));
        assert_eq!(config.max_history_messages, 12);
        assert_eq!(config.todo_store_path, "data/todos.json");
        assert_eq!(config.ollama_num_predict, 120);
        assert!(config.calendar_sources.is_empty());
        assert!(!config.calendar_sync_enabled());
    }

    #[test]
    fn config_rejects_invalid_chat_id() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "bad");
            env::set_var("CALENDAR_SOURCES_JSON", "");
            env::remove_var("DESTINATION_CALENDAR_ID");
            env::remove_var("GOOGLE_CALENDAR_ACCESS_TOKEN");
            env::set_var("CALENDAR_TARGET_EMAILS", "");
        }

        let err = AppConfig::from_env().unwrap_err();
        assert!(err.contains("CHAT_ID must be an integer"));
    }

    #[test]
    fn config_parses_calendar_sources() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::set_var(
                "CALENDAR_SOURCES_JSON",
                r#"[{"id":"client","type":"ics","url":"https://example.test/client.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.test"}]"#,
            );
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "secret");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        let config = AppConfig::from_env().expect("config");

        assert_eq!(config.calendar_sources.len(), 1);
        assert_eq!(config.calendar_sources[0].id, "client");
        assert!(config.calendar_sync_enabled());
    }

    #[test]
    fn config_reads_calendar_sources_from_dotenv_file_fallback() {
        let _guard = env_lock().lock().expect("env lock");
        let original_env = fs::read_to_string(".env").ok();
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::set_var("CALENDAR_SOURCES_JSON", "");
            env::remove_var("CALENDAR_SOURCES_JSON");
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "secret");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        fs::write(
            ".env",
            "CALENDAR_SOURCES_JSON='[{\"id\":\"client\",\"type\":\"ics\",\"url\":\"https://example.test/client.ics\",\"label\":\"Busy - Client\",\"priority\":100,\"category\":\"business\",\"enabled\":true,\"owner_email\":\"client@example.test\"}]'\nCALENDAR_TARGET_EMAILS='[\"client@example.test\",\"personal@example.test\"]'\n",
        )
        .expect("write .env fallback");

        let config = AppConfig::from_env().expect("config");
        assert_eq!(config.calendar_sources.len(), 1);
        assert_eq!(config.calendar_sources[0].id, "client");

        match original_env {
            Some(contents) => fs::write(".env", contents).expect("restore .env"),
            None => {
                let _ = fs::remove_file(".env");
            }
        }
    }

    #[test]
    fn config_rejects_duplicate_calendar_source_ids() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::set_var(
                "CALENDAR_SOURCES_JSON",
                r#"[{"id":"client","type":"ics","url":"https://example.test/a.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.test"},{"id":"CLIENT","type":"ics","url":"https://example.test/b.ics","label":"Busy - Client 2","priority":80,"category":"business","enabled":true,"owner_email":"personal@example.test"}]"#,
            );
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "secret");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        let err = AppConfig::from_env().unwrap_err();
        assert!(err.contains("Duplicate calendar source id"));
    }
}
