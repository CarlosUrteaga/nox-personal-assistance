use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;

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
    pub calendar_destination_provider: Option<String>,
    pub google_oauth_credentials_path: Option<String>,
    pub google_oauth_token_path: Option<String>,
    pub google_calendar_access_token: Option<String>,
    pub heartbeat_interval_secs: u64,
    pub heartbeat_sync_window_days: i64,
    pub calendar_target_emails: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub enabled: bool,
    pub bind_address: String,
    pub user_store_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleOAuthBootstrapConfig {
    pub credentials_path: String,
    pub token_path: String,
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
        let calendar_destination_provider = env::var("CALENDAR_DESTINATION_PROVIDER")
            .ok()
            .or_else(|| read_dotenv_value("CALENDAR_DESTINATION_PROVIDER"))
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let google_oauth_credentials_path = env::var("GOOGLE_OAUTH_CREDENTIALS_PATH")
            .ok()
            .or_else(|| read_dotenv_value("GOOGLE_OAUTH_CREDENTIALS_PATH"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let google_oauth_token_path = env::var("GOOGLE_OAUTH_TOKEN_PATH")
            .ok()
            .or_else(|| read_dotenv_value("GOOGLE_OAUTH_TOKEN_PATH"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let resolved_google_oauth_token_path = resolve_google_oauth_token_path(
            google_oauth_credentials_path.as_deref(),
            google_oauth_token_path.as_deref(),
        );
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
                return Err(
                    "DESTINATION_CALENDAR_ID must be set when calendar sync is enabled".to_string(),
                );
            }
            if resolved_google_oauth_token_path.is_none()
                && google_oauth_credentials_path.is_none()
                && google_calendar_access_token.is_none()
            {
                return Err(
                    "GOOGLE_OAUTH_TOKEN_PATH, GOOGLE_OAUTH_CREDENTIALS_PATH, or GOOGLE_CALENDAR_ACCESS_TOKEN must be set when calendar sync is enabled"
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
            calendar_destination_provider,
            google_oauth_credentials_path,
            google_oauth_token_path: resolved_google_oauth_token_path,
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

impl WebConfig {
    pub fn from_env() -> Self {
        let enabled = env::var("WEB_ENABLED")
            .ok()
            .or_else(|| read_dotenv_value("WEB_ENABLED"))
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(true);
        let bind_address = env::var("WEB_BIND_ADDRESS")
            .ok()
            .or_else(|| read_dotenv_value("WEB_BIND_ADDRESS"))
            .unwrap_or_else(|| "127.0.0.1:3000".to_string());
        let user_store_path = env::var("USER_STORE_PATH")
            .ok()
            .or_else(|| read_dotenv_value("USER_STORE_PATH"))
            .unwrap_or_else(|| "data/users.json".to_string());

        Self {
            enabled,
            bind_address,
            user_store_path,
        }
    }
}

impl GoogleOAuthBootstrapConfig {
    pub fn from_env() -> Result<Self, String> {
        let credentials_path = env::var("GOOGLE_OAUTH_CREDENTIALS_PATH")
            .ok()
            .or_else(|| read_dotenv_value("GOOGLE_OAUTH_CREDENTIALS_PATH"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "GOOGLE_OAUTH_CREDENTIALS_PATH must be set".to_string())?;
        let token_path = resolve_google_oauth_token_path_from_env(&credentials_path)
            .ok_or_else(|| "GOOGLE_OAUTH_TOKEN_PATH could not be resolved".to_string())?;

        Ok(Self {
            credentials_path,
            token_path,
        })
    }
}

pub fn resolve_google_oauth_token_path(
    credentials_path: Option<&str>,
    token_path: Option<&str>,
) -> Option<String> {
    token_path
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .or_else(|| {
            credentials_path
                .filter(|value| !value.trim().is_empty())
                .map(derive_google_oauth_token_path)
        })
}

fn resolve_google_oauth_token_path_from_env(credentials_path: &str) -> Option<String> {
    let token_path = env::var("GOOGLE_OAUTH_TOKEN_PATH")
        .ok()
        .or_else(|| read_dotenv_value("GOOGLE_OAUTH_TOKEN_PATH"));
    resolve_google_oauth_token_path(Some(credentials_path), token_path.as_deref())
}

fn derive_google_oauth_token_path(credentials_path: &str) -> String {
    Path::new(credentials_path)
        .with_file_name("token.json")
        .to_string_lossy()
        .into_owned()
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

    let mut seen_ids = HashSet::new();
    for source in &sources {
        if source.id.trim().is_empty() {
            return Err("CALENDAR_SOURCES_JSON source id must not be empty".to_string());
        }
        if source.url.trim().is_empty() {
            return Err(format!(
                "Calendar source '{}' url must not be empty",
                source.id
            ));
        }
        if source.label.trim().is_empty() {
            return Err(format!(
                "Calendar source '{}' label must not be empty",
                source.id
            ));
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

    Ok(sources
        .into_iter()
        .filter(|source| source.enabled)
        .collect())
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
    parse_dotenv_map(&contents).remove(key)
}

pub fn read_dotenv_map() -> BTreeMap<String, String> {
    let contents = fs::read_to_string(".env").unwrap_or_default();
    parse_dotenv_map(&contents)
}

pub fn write_dotenv_values(updates: &BTreeMap<String, String>) -> Result<(), String> {
    let existing_contents = fs::read_to_string(".env").unwrap_or_default();
    let mut rendered = Vec::new();
    let mut seen = HashSet::new();

    for line in existing_contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            rendered.push(line.to_string());
            continue;
        }

        let Some((candidate_key, _raw_value)) = trimmed.split_once('=') else {
            rendered.push(line.to_string());
            continue;
        };
        let normalized_key = candidate_key.trim();
        if let Some(new_value) = updates.get(normalized_key) {
            rendered.push(format!(
                "{}={}",
                normalized_key,
                format_dotenv_value(new_value)
            ));
            seen.insert(normalized_key.to_string());
        } else {
            rendered.push(line.to_string());
        }
    }

    for (key, value) in updates {
        if seen.contains(key) {
            continue;
        }
        rendered.push(format!("{}={}", key, format_dotenv_value(value)));
    }

    let mut output = rendered.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    fs::write(".env", output).map_err(|err| format!("failed to write .env: {}", err))
}

fn parse_dotenv_map(contents: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let Some((candidate_key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = candidate_key.trim();
        if key.is_empty() {
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
        values.insert(key.to_string(), unwrapped.to_string());
    }

    values
}

fn format_dotenv_value(value: &str) -> String {
    let trimmed = value.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return String::new();
    }

    let needs_quotes = trimmed.starts_with(' ')
        || trimmed.ends_with(' ')
        || trimmed.contains('#')
        || trimmed.contains('=');

    if !needs_quotes {
        return trimmed.to_string();
    }

    if !trimmed.contains('\'') {
        return format!("'{}'", trimmed);
    }

    if !trimmed.contains('"') {
        return format!("\"{}\"", trimmed);
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, GoogleOAuthBootstrapConfig, WebConfig, read_dotenv_map,
        resolve_google_oauth_token_path, write_dotenv_values,
    };
    use std::collections::BTreeMap;
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
            env::set_var("DESTINATION_CALENDAR_ID", "");
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "");
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
            env::set_var("DESTINATION_CALENDAR_ID", "");
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "");
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
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "google");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
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
            env::remove_var("CALENDAR_SOURCES_JSON");
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "google");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
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
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "google");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "secret");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        let err = AppConfig::from_env().unwrap_err();
        assert!(err.contains("Duplicate calendar source id"));
    }

    #[test]
    fn config_accepts_google_oauth_token_path_without_manual_access_token() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::set_var(
                "CALENDAR_SOURCES_JSON",
                r#"[{"id":"client","type":"ics","url":"https://example.test/client.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.test"}]"#,
            );
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "google");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "/tmp/token.json");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        let config = AppConfig::from_env().expect("config");

        assert_eq!(
            config.google_oauth_token_path.as_deref(),
            Some("/tmp/token.json")
        );
        assert!(config.calendar_sync_enabled());
    }

    #[test]
    fn config_accepts_google_oauth_credentials_path_without_token_file() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("TELOXIDE_TOKEN", "token");
            env::set_var("CHAT_ID", "1");
            env::set_var(
                "CALENDAR_SOURCES_JSON",
                r#"[{"id":"client","type":"ics","url":"https://example.test/client.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.test"}]"#,
            );
            env::set_var("DESTINATION_CALENDAR_ID", "primary");
            env::set_var("CALENDAR_DESTINATION_PROVIDER", "google");
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "/tmp/credentials.json");
            env::set_var("GOOGLE_OAUTH_TOKEN_PATH", "");
            env::set_var("GOOGLE_CALENDAR_ACCESS_TOKEN", "");
            env::set_var(
                "CALENDAR_TARGET_EMAILS",
                r#"["client@example.test","personal@example.test"]"#,
            );
        }

        let config = AppConfig::from_env().expect("config");

        assert_eq!(
            config.google_oauth_credentials_path.as_deref(),
            Some("/tmp/credentials.json")
        );
        assert_eq!(
            config.google_oauth_token_path.as_deref(),
            Some("/tmp/token.json")
        );
        assert!(config.calendar_sync_enabled());
    }

    #[test]
    fn google_oauth_bootstrap_config_derives_token_path_when_missing() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            env::set_var("GOOGLE_OAUTH_CREDENTIALS_PATH", "/tmp/credentials.json");
            env::remove_var("GOOGLE_OAUTH_TOKEN_PATH");
        }

        let config = GoogleOAuthBootstrapConfig::from_env().expect("bootstrap config");

        assert_eq!(
            config,
            GoogleOAuthBootstrapConfig {
                credentials_path: "/tmp/credentials.json".to_string(),
                token_path: "/tmp/token.json".to_string(),
            }
        );
    }

    #[test]
    fn explicit_google_oauth_token_path_overrides_derived_default() {
        let resolved = resolve_google_oauth_token_path(
            Some("/tmp/credentials.json"),
            Some("/custom/path/google.json"),
        );

        assert_eq!(resolved.as_deref(), Some("/custom/path/google.json"));
    }

    #[test]
    fn web_config_loads_defaults() {
        let _guard = env_lock().lock().expect("env lock");
        let original_env = fs::read_to_string(".env").ok();
        unsafe {
            env::remove_var("WEB_ENABLED");
            env::remove_var("WEB_BIND_ADDRESS");
            env::remove_var("USER_STORE_PATH");
        }
        fs::write(".env", "").expect("clear .env");

        let config = WebConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.bind_address, "127.0.0.1:3000");
        assert_eq!(config.user_store_path, "data/users.json");

        match original_env {
            Some(contents) => fs::write(".env", contents).expect("restore .env"),
            None => {
                let _ = fs::remove_file(".env");
            }
        }
    }

    #[test]
    fn dotenv_write_updates_existing_values() {
        let _guard = env_lock().lock().expect("env lock");
        let original_env = fs::read_to_string(".env").ok();
        fs::write(".env", "A=1\n# comment\nB=two words\n").expect("seed .env");

        let mut updates = BTreeMap::new();
        updates.insert("B".to_string(), "changed".to_string());
        updates.insert("C".to_string(), "three".to_string());
        write_dotenv_values(&updates).expect("write dotenv");

        let parsed = read_dotenv_map();
        assert_eq!(parsed.get("A").map(String::as_str), Some("1"));
        assert_eq!(parsed.get("B").map(String::as_str), Some("changed"));
        assert_eq!(parsed.get("C").map(String::as_str), Some("three"));

        match original_env {
            Some(contents) => fs::write(".env", contents).expect("restore .env"),
            None => {
                let _ = fs::remove_file(".env");
            }
        }
    }
}
