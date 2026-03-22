use crate::config::{WebConfig, read_dotenv_map, write_dotenv_values};
use crate::runs::{MetadataItem, Run, RunStatus, RunTracker, Step, StepKind, ToolTrace};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Form, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use cookie::{Cookie, SameSite};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
    sync::{Arc, RwLock},
};
use tokio::net::TcpListener;

const SESSION_COOKIE_NAME: &str = "nox_session";
const MIN_PASSWORD_LENGTH: usize = 12;
const ESSENTIAL_SETUP_KEYS: &[&str] = &[
    "TELOXIDE_TOKEN",
    "CHAT_ID",
    "OLLAMA_BASE_URL",
    "OLLAMA_MODEL",
];

#[derive(Clone)]
pub struct WebAppState {
    users: Arc<RwLock<UserStore>>,
    sessions: Arc<RwLock<HashMap<String, SessionRecord>>>,
    user_store_path: String,
    run_tracker: RunTracker,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UserStore {
    users: Vec<UserRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserRecord {
    username: String,
    password_hash: String,
    #[serde(default)]
    onboarding_seen: bool,
    #[serde(default)]
    onboarding_completed: bool,
}

#[derive(Debug, Clone)]
struct SessionRecord {
    username: String,
    csrf_token: String,
}

#[derive(Debug, Deserialize)]
struct AuthForm {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct RegisterForm {
    username: String,
    password: String,
    confirm_password: String,
}

#[derive(Debug, Deserialize, Default)]
struct ConsoleQuery {
    run: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailMode {
    Simple,
    Detailed,
}

#[derive(Debug, Clone)]
struct EssentialSetupStatus {
    values: BTreeMap<String, String>,
    missing_keys: Vec<&'static str>,
}

impl EssentialSetupStatus {
    fn from_env() -> Self {
        let values = read_dotenv_map();
        let missing_keys = ESSENTIAL_SETUP_KEYS
            .iter()
            .copied()
            .filter(|key| {
                values
                    .get(*key)
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>();

        Self {
            values,
            missing_keys,
        }
    }

    fn is_ready(&self) -> bool {
        self.missing_keys.is_empty()
    }
}

struct SettingField {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    secret: bool,
    multiline: bool,
}

struct SetupField {
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    help: &'static str,
    instructions: &'static str,
    secret: bool,
}

const SETUP_FIELDS: &[SetupField] = &[
    SetupField {
        key: "TELOXIDE_TOKEN",
        label: "Telegram bot token",
        placeholder: "123456789:AA...",
        help: "Required. NOX uses this token to connect to your Telegram bot.",
        instructions: "Open BotFather in Telegram, create or select your bot, then copy the API token returned by /newbot or /token.",
        secret: true,
    },
    SetupField {
        key: "CHAT_ID",
        label: "Allowed Telegram chat ID",
        placeholder: "123456789",
        help: "Required. Only this Telegram chat will be allowed to talk to NOX.",
        instructions: "Send a message to your bot, then call https://api.telegram.org/bot<TELOXIDE_TOKEN>/getUpdates and copy the numeric chat.id from the response.",
        secret: false,
    },
    SetupField {
        key: "OLLAMA_BASE_URL",
        label: "Ollama base URL",
        placeholder: "http://127.0.0.1:11434",
        help: "Required. Use the local default unless Ollama runs on another host or port.",
        instructions: "If Ollama runs on the same machine, keep the default http://127.0.0.1:11434. Change it only if your Ollama instance is remote or proxied.",
        secret: false,
    },
    SetupField {
        key: "OLLAMA_MODEL",
        label: "Ollama model",
        placeholder: "qwen2.5:7b",
        help: "Required. This model must already exist in your Ollama instance.",
        instructions: "Pick a model that is already available in Ollama. For example, run `ollama list` and copy one of the installed model names.",
        secret: false,
    },
];

const SETTING_FIELDS: &[SettingField] = &[
    SettingField {
        key: "TELOXIDE_TOKEN",
        label: "Telegram Bot Token",
        help: "Required for Telegram mode.",
        secret: true,
        multiline: false,
    },
    SettingField {
        key: "CHAT_ID",
        label: "Allowed Telegram Chat ID",
        help: "Numeric Telegram chat ID allowed to talk to the bot.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "OLLAMA_BASE_URL",
        label: "Ollama Base URL",
        help: "Usually http://127.0.0.1:11434.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "OLLAMA_MODEL",
        label: "Ollama Model",
        help: "Model name used for chat requests.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "OLLAMA_TIMEOUT_SECS",
        label: "Ollama Timeout Seconds",
        help: "Request timeout in seconds.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "OLLAMA_NUM_PREDICT",
        label: "Ollama Num Predict",
        help: "Maximum generated tokens per response.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "ASSISTANT_NAME",
        label: "Assistant Name",
        help: "Friendly display name.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "SYSTEM_PROMPT",
        label: "System Prompt",
        help: "Single-line system prompt used by the assistant.",
        secret: false,
        multiline: true,
    },
    SettingField {
        key: "MAX_HISTORY_MESSAGES",
        label: "Max History Messages",
        help: "Conversation turns retained in memory.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "TODO_STORE_PATH",
        label: "Todo Store Path",
        help: "Local JSON file used for todos.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "CALENDAR_DESTINATION_PROVIDER",
        label: "Calendar Destination Provider",
        help: "Currently google.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "DESTINATION_CALENDAR_ID",
        label: "Destination Calendar ID",
        help: "Google Calendar target when sync is enabled.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "GOOGLE_CALENDAR_ACCESS_TOKEN",
        label: "Google Calendar Access Token",
        help: "Bearer token for Google Calendar API.",
        secret: true,
        multiline: false,
    },
    SettingField {
        key: "HEARTBEAT_INTERVAL_SECS",
        label: "Heartbeat Interval Seconds",
        help: "Calendar sync interval in seconds.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "HEARTBEAT_SYNC_WINDOW_DAYS",
        label: "Heartbeat Sync Window Days",
        help: "How far ahead the sync resolves blockers.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "CALENDAR_TARGET_EMAILS",
        label: "Calendar Target Emails JSON",
        help: "JSON array of attendees for busy blockers.",
        secret: false,
        multiline: true,
    },
    SettingField {
        key: "CALENDAR_SOURCES_JSON",
        label: "Calendar Sources JSON",
        help: "JSON array of enabled ICS sources.",
        secret: false,
        multiline: true,
    },
    SettingField {
        key: "WEB_ENABLED",
        label: "Web Enabled",
        help: "true or false. Applied on restart.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "WEB_BIND_ADDRESS",
        label: "Web Bind Address",
        help: "Bind address for the settings site. Applied on restart.",
        secret: false,
        multiline: false,
    },
    SettingField {
        key: "USER_STORE_PATH",
        label: "User Store Path",
        help: "Local JSON file for hashed user accounts. Applied on restart.",
        secret: false,
        multiline: false,
    },
];

pub async fn serve(config: WebConfig, run_tracker: RunTracker) -> Result<(), String> {
    let state = WebAppState {
        users: Arc::new(RwLock::new(UserStore::load(&config.user_store_path)?)),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        user_store_path: config.user_store_path,
        run_tracker,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/console", get(console_page))
        .route("/login", get(login_page).post(login))
        .route("/register", get(register_page).post(register))
        .route("/setup", get(setup_page).post(save_setup))
        .route("/setup/skip", post(skip_setup))
        .route("/logout", post(logout))
        .route("/settings", get(settings_page).post(save_settings))
        .with_state(state);

    let listener = TcpListener::bind(&config.bind_address)
        .await
        .map_err(|err| {
            format!(
                "failed to bind web server on {}: {}",
                config.bind_address, err
            )
        })?;

    log::info!(
        "Web settings UI listening on http://{}",
        config.bind_address
    );
    axum::serve(listener, app)
        .await
        .map_err(|err| format!("web server error: {}", err))
}

async fn index(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return if state.has_users() {
            Redirect::to("/login").into_response()
        } else {
            Redirect::to("/register").into_response()
        };
    };

    match state.find_user(&session.username) {
        Some(user) if !user.onboarding_seen => Redirect::to("/setup").into_response(),
        Some(_) => Redirect::to("/console").into_response(),
        None => Redirect::to("/login").into_response(),
    }
}

async fn console_page(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Query(query): Query<ConsoleQuery>,
) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };

    if state
        .find_user(&session.username)
        .map(|user| !user.onboarding_seen)
        .unwrap_or(true)
    {
        return Redirect::to("/setup").into_response();
    }

    let mode = DetailMode::from_query(query.mode.as_deref());
    let setup_status = EssentialSetupStatus::from_env();
    let runs = state.run_tracker.list_runs_with_fallback();
    let selected = if let Some(run_id) = query.run.as_deref() {
        runs.iter().find(|run| run.id == run_id).cloned()
    } else {
        runs.first().cloned()
    };

    page_response(
        "NOX Console",
        render_console_page(
            &session.username,
            mode,
            &runs,
            selected.as_ref(),
            &setup_status,
        ),
    )
}

async fn login_page(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    if current_session(&state, &headers).is_some() {
        return Redirect::to("/console").into_response();
    }

    page_response(
        "Login",
        render_auth_page(
            "Login",
            "/login",
            "Sign in to access the NOX console and shared settings.",
            "Create account",
            "/register",
            state.has_users(),
            None,
            false,
        ),
    )
}

async fn register_page(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    if current_session(&state, &headers).is_some() {
        return Redirect::to("/console").into_response();
    }

    page_response(
        "Register",
        render_auth_page(
            "Register",
            "/register",
            "Create a local account. After registration, NOX will guide you through the minimum setup needed to get the assistant online.",
            "Back to login",
            "/login",
            state.has_users(),
            None,
            true,
        ),
    )
}

async fn login(State(state): State<WebAppState>, Form(form): Form<AuthForm>) -> Response {
    let username = form.username.trim();
    let password = form.password;

    let Some(user) = state.find_user(username) else {
        return auth_error(&state, "Invalid username or password.");
    };

    let parsed_hash = match PasswordHash::new(&user.password_hash) {
        Ok(hash) => hash,
        Err(_) => return internal_error("Stored password hash is invalid."),
    };

    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return auth_error(&state, "Invalid username or password.");
    }

    let redirect_to = if user.onboarding_seen {
        "/console"
    } else {
        "/setup"
    };
    create_session_response(&state, &user.username, redirect_to)
}

async fn register(State(state): State<WebAppState>, Form(form): Form<RegisterForm>) -> Response {
    let username = form.username.trim();
    if let Err(message) = validate_username(username) {
        return register_error(&state, message);
    }
    if form.password.len() < MIN_PASSWORD_LENGTH {
        return register_error(&state, "Password must be at least 12 characters.");
    }
    if form.password != form.confirm_password {
        return register_error(&state, "Password confirmation does not match.");
    }
    if state.find_user(username).is_some() {
        return register_error(&state, "That username already exists.");
    }

    let salt = SaltString::generate(&mut OsRng);
    let password_hash = match Argon2::default().hash_password(form.password.as_bytes(), &salt) {
        Ok(hash) => hash.to_string(),
        Err(_) => return internal_error("Failed to hash password."),
    };

    if let Err(err) = state.insert_user(UserRecord {
        username: username.to_string(),
        password_hash,
        onboarding_seen: false,
        onboarding_completed: false,
    }) {
        return internal_error(&err);
    }

    create_session_response(&state, username, "/setup")
}

async fn setup_page(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };
    let Some(user) = state.find_user(&session.username) else {
        return Redirect::to("/login").into_response();
    };

    let status = EssentialSetupStatus::from_env();
    page_response(
        "Setup",
        render_setup_page(
            &session.username,
            &session.csrf_token,
            &status,
            user.onboarding_completed,
            None,
        ),
    )
}

async fn save_setup(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };

    let csrf_token = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if csrf_token != session.csrf_token {
        let status = EssentialSetupStatus::from_env();
        return page_response(
            "Setup",
            render_setup_page(
                &session.username,
                &session.csrf_token,
                &status,
                false,
                Some("Session validation failed. Refresh the page and try again."),
            ),
        );
    }

    let updates = match collect_setup_values(&form) {
        Ok(values) => values,
        Err(err) => {
            let status = EssentialSetupStatus::from_env();
            return page_response(
                "Setup",
                render_setup_page(
                    &session.username,
                    &session.csrf_token,
                    &merge_setup_status(status, &form),
                    false,
                    Some(&err),
                ),
            );
        }
    };

    if let Err(err) = write_dotenv_values(&updates) {
        return internal_error(&err);
    }
    if let Err(err) = state.update_user(&session.username, |user| {
        user.onboarding_seen = true;
        user.onboarding_completed = true;
    }) {
        return internal_error(&err);
    }

    Redirect::to("/console").into_response()
}

async fn skip_setup(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };

    if let Err(err) = state.update_user(&session.username, |user| {
        user.onboarding_seen = true;
    }) {
        return internal_error(&err);
    }

    Redirect::to("/console").into_response()
}

async fn logout(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token_from_headers(&headers) {
        if let Ok(mut sessions) = state.sessions.write() {
            sessions.remove(&token);
        }
    }

    redirect_with_cookie("/login", expire_session_cookie())
}

async fn settings_page(State(state): State<WebAppState>, headers: HeaderMap) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };

    let values = read_dotenv_map();
    page_response(
        "Settings",
        render_settings_page(&session.username, &session.csrf_token, &values, None, None),
    )
}

async fn save_settings(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return Redirect::to("/login").into_response();
    };

    let csrf_token = form
        .get("csrf_token")
        .map(String::as_str)
        .unwrap_or_default();
    if csrf_token != session.csrf_token {
        let current_values = read_dotenv_map();
        return page_response(
            "Settings",
            render_settings_page(
                &session.username,
                &session.csrf_token,
                &current_values,
                Some("Session validation failed. Refresh the page and try again."),
                None,
            ),
        );
    }

    let settings = match collect_settings(&form) {
        Ok(values) => values,
        Err(err) => {
            let current_values = read_dotenv_map();
            return page_response(
                "Settings",
                render_settings_page(
                    &session.username,
                    &session.csrf_token,
                    &merge_values(current_values, &form),
                    Some(&err),
                    None,
                ),
            );
        }
    };

    if let Err(err) = write_dotenv_values(&settings) {
        return internal_error(&err);
    }

    page_response(
        "Settings",
        render_settings_page(
            &session.username,
            &session.csrf_token,
            &settings,
            None,
            Some("Saved to .env. Restart the process to apply changes already loaded in memory."),
        ),
    )
}

impl WebAppState {
    fn has_users(&self) -> bool {
        self.users
            .read()
            .map(|store| !store.users.is_empty())
            .unwrap_or(false)
    }

    fn find_user(&self, username: &str) -> Option<UserRecord> {
        self.users.read().ok().and_then(|store| {
            store
                .users
                .iter()
                .find(|user| user.username.eq_ignore_ascii_case(username))
                .cloned()
        })
    }

    fn insert_user(&self, user: UserRecord) -> Result<(), String> {
        let mut store = self
            .users
            .write()
            .map_err(|_| "failed to lock user store".to_string())?;
        store.users.push(user);
        store.save(&self.user_store_path)
    }

    fn update_user(
        &self,
        username: &str,
        updater: impl FnOnce(&mut UserRecord),
    ) -> Result<(), String> {
        let mut store = self
            .users
            .write()
            .map_err(|_| "failed to lock user store".to_string())?;
        let Some(user) = store
            .users
            .iter_mut()
            .find(|user| user.username.eq_ignore_ascii_case(username))
        else {
            return Err("user not found".to_string());
        };
        updater(user);
        store.save(&self.user_store_path)
    }
}

impl UserStore {
    fn load(path: &str) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|err| format!("failed to parse {}: {}", path, err)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!("failed to read {}: {}", path, err)),
        }
    }

    fn save(&self, path: &str) -> Result<(), String> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {}", parent.display(), err))?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|err| format!("failed to serialize user store: {}", err))?;
        fs::write(path, contents).map_err(|err| format!("failed to write {}: {}", path, err))
    }
}

impl DetailMode {
    fn from_query(value: Option<&str>) -> Self {
        match value.unwrap_or("detailed").to_ascii_lowercase().as_str() {
            "simple" => Self::Simple,
            _ => Self::Detailed,
        }
    }

    fn as_query(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Detailed => "detailed",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Detailed => "Detailed",
        }
    }
}

fn validate_username(username: &str) -> Result<(), &'static str> {
    if username.len() < 3 || username.len() > 64 {
        return Err("Username must be between 3 and 64 characters.");
    }
    if !username
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("Username may only contain letters, numbers, dots, dashes, and underscores.");
    }
    Ok(())
}

fn collect_setup_values(
    form: &HashMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let teloxide_token = form
        .get("teloxide_token")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let chat_id = form
        .get("chat_id")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let ollama_base_url = form
        .get("ollama_base_url")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let ollama_model = form
        .get("ollama_model")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();

    if teloxide_token.is_empty() {
        return Err("TELOXIDE_TOKEN is required. Copy the token from BotFather.".to_string());
    }
    if chat_id.parse::<i64>().is_err() {
        return Err("CHAT_ID must be a valid integer Telegram chat id.".to_string());
    }
    if ollama_base_url.is_empty() {
        return Err("OLLAMA_BASE_URL is required.".to_string());
    }
    if ollama_model.is_empty() {
        return Err("OLLAMA_MODEL is required.".to_string());
    }

    let mut updates = BTreeMap::new();
    updates.insert("TELOXIDE_TOKEN".to_string(), teloxide_token);
    updates.insert("CHAT_ID".to_string(), chat_id);
    updates.insert("OLLAMA_BASE_URL".to_string(), ollama_base_url);
    updates.insert("OLLAMA_MODEL".to_string(), ollama_model);
    Ok(updates)
}

fn merge_setup_status(
    status: EssentialSetupStatus,
    form: &HashMap<String, String>,
) -> EssentialSetupStatus {
    let mut values = status.values;
    for (key, form_key) in [
        ("TELOXIDE_TOKEN", "teloxide_token"),
        ("CHAT_ID", "chat_id"),
        ("OLLAMA_BASE_URL", "ollama_base_url"),
        ("OLLAMA_MODEL", "ollama_model"),
    ] {
        if let Some(value) = form.get(form_key) {
            values.insert(key.to_string(), value.clone());
        }
    }
    EssentialSetupStatus {
        missing_keys: ESSENTIAL_SETUP_KEYS
            .iter()
            .copied()
            .filter(|key| {
                values
                    .get(*key)
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
            })
            .collect(),
        values,
    }
}

fn collect_settings(form: &HashMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for field in SETTING_FIELDS {
        let raw_value = form
            .get(field.key)
            .cloned()
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let normalized = normalize_setting(field, &raw_value)?;
        values.insert(field.key.to_string(), normalized);
    }
    Ok(values)
}

fn normalize_setting(field: &SettingField, raw_value: &str) -> Result<String, String> {
    let trimmed = raw_value.trim();
    match field.key {
        "CHAT_ID" if !trimmed.is_empty() => {
            trimmed
                .parse::<i64>()
                .map_err(|_| "CHAT_ID must be an integer.".to_string())?;
        }
        "OLLAMA_TIMEOUT_SECS" | "HEARTBEAT_INTERVAL_SECS" if !trimmed.is_empty() => {
            trimmed
                .parse::<u64>()
                .map_err(|_| format!("{} must be an unsigned integer.", field.key))?;
        }
        "OLLAMA_NUM_PREDICT" if !trimmed.is_empty() => {
            trimmed
                .parse::<u32>()
                .map_err(|_| "OLLAMA_NUM_PREDICT must be an unsigned integer.".to_string())?;
        }
        "MAX_HISTORY_MESSAGES" if !trimmed.is_empty() => {
            trimmed
                .parse::<usize>()
                .map_err(|_| "MAX_HISTORY_MESSAGES must be an unsigned integer.".to_string())?;
        }
        "HEARTBEAT_SYNC_WINDOW_DAYS" if !trimmed.is_empty() => {
            trimmed
                .parse::<i64>()
                .map_err(|_| "HEARTBEAT_SYNC_WINDOW_DAYS must be an integer.".to_string())?;
        }
        "CALENDAR_TARGET_EMAILS" | "CALENDAR_SOURCES_JSON" if !trimmed.is_empty() => {
            let parsed: serde_json::Value = serde_json::from_str(trimmed)
                .map_err(|err| format!("{} must be valid JSON: {}", field.key, err))?;
            return Ok(parsed.to_string());
        }
        "WEB_ENABLED" if !trimmed.is_empty() => {
            let normalized = match trimmed.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => "true",
                "0" | "false" | "no" | "off" => "false",
                _ => return Err("WEB_ENABLED must be true or false.".to_string()),
            };
            return Ok(normalized.to_string());
        }
        _ => {}
    }
    if !field.multiline && raw_value.contains('\n') {
        return Err(format!("{} must stay on a single line.", field.key));
    }
    Ok(trimmed.to_string())
}

fn merge_values(
    current_values: BTreeMap<String, String>,
    form: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = current_values;
    for field in SETTING_FIELDS {
        if let Some(value) = form.get(field.key) {
            merged.insert(field.key.to_string(), value.clone());
        }
    }
    merged
}

fn current_session(state: &WebAppState, headers: &HeaderMap) -> Option<SessionRecord> {
    let token = session_token_from_headers(headers)?;
    state.sessions.read().ok()?.get(&token).cloned()
}

fn session_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for cookie in Cookie::split_parse(raw).flatten() {
        if cookie.name() == SESSION_COOKIE_NAME {
            return Some(cookie.value().to_string());
        }
    }
    None
}

fn create_session_response(state: &WebAppState, username: &str, redirect_to: &str) -> Response {
    let mut session_id_bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut session_id_bytes);
    let session_id = hex::encode(session_id_bytes);

    let mut csrf_bytes = [0_u8; 24];
    rand::thread_rng().fill_bytes(&mut csrf_bytes);
    let csrf_token = hex::encode(csrf_bytes);

    if let Ok(mut sessions) = state.sessions.write() {
        sessions.insert(
            session_id.clone(),
            SessionRecord {
                username: username.to_string(),
                csrf_token,
            },
        );
    }

    redirect_with_cookie(redirect_to, build_session_cookie(session_id))
}

fn build_session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

fn expire_session_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE_NAME, String::new()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(0))
        .build()
}

fn redirect_with_cookie(location: &str, cookie: Cookie<'static>) -> Response {
    let mut response = Redirect::to(location).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn page_response(title: &str, content: String) -> Response {
    Html(render_layout(title, &content)).into_response()
}

fn auth_error(state: &WebAppState, message: &str) -> Response {
    page_response(
        "Login",
        render_auth_page(
            "Login",
            "/login",
            "Sign in to access the NOX console and shared settings.",
            "Create account",
            "/register",
            state.has_users(),
            Some(message),
            false,
        ),
    )
}

fn register_error(state: &WebAppState, message: &str) -> Response {
    page_response(
        "Register",
        render_auth_page(
            "Register",
            "/register",
            "Create a local account. After registration, NOX will guide you through the minimum setup needed to get the assistant online.",
            "Back to login",
            "/login",
            state.has_users(),
            Some(message),
            true,
        ),
    )
}

fn internal_error(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(render_layout(
            "Error",
            &format!(
                "<main class=\"shell\"><section class=\"panel\"><h1>Internal Error</h1><p class=\"alert danger\">{}</p></section></main>",
                escape_html(message)
            ),
        )),
    )
        .into_response()
}

fn render_layout(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>{}</style></head><body>{}</body></html>",
        escape_html(title),
        STYLES,
        body
    )
}

fn render_auth_page(
    title: &str,
    action: &str,
    intro: &str,
    alternate_label: &str,
    alternate_href: &str,
    has_users: bool,
    error: Option<&str>,
    include_confirm: bool,
) -> String {
    let error_html = error
        .map(|message| format!("<p class=\"alert danger\">{}</p>", escape_html(message)))
        .unwrap_or_default();
    let confirm_field = if include_confirm {
        "<label><span>Confirm password</span><input name=\"confirm_password\" type=\"password\" autocomplete=\"new-password\" required></label>"
    } else {
        ""
    };
    let availability = if has_users {
        "Existing accounts can sign in or create another user."
    } else {
        "No users exist yet. Create the first account to unlock the console."
    };

    format!(
        "<main class=\"auth-shell\"><section class=\"auth-hero\"><p class=\"eyebrow\">NOX Console</p><h1>{}</h1><p>{}</p><p class=\"muted\">{}</p></section><section class=\"auth-card\"><div class=\"panel glass\"><h2>{}</h2>{}<form method=\"post\" action=\"{}\"><label><span>Username</span><input name=\"username\" autocomplete=\"username\" required></label><label><span>Password</span><input name=\"password\" type=\"password\" autocomplete=\"current-password\" required></label>{}<button type=\"submit\">{}</button></form><p class=\"muted auth-link\"><a href=\"{}\">{}</a></p></div></section></main>",
        escape_html(title),
        escape_html(intro),
        escape_html(availability),
        escape_html(title),
        error_html,
        action,
        confirm_field,
        escape_html(title),
        alternate_href,
        escape_html(alternate_label)
    )
}

fn render_setup_page(
    username: &str,
    csrf_token: &str,
    status: &EssentialSetupStatus,
    completed_before: bool,
    error: Option<&str>,
) -> String {
    let error_html = error
        .map(|message| format!("<p class=\"alert danger\">{}</p>", escape_html(message)))
        .unwrap_or_default();
    let setup_state = if status.is_ready() {
        "Required NOX settings are already present. Review them below or continue to the console."
    } else {
        "Complete the minimum setup so a new user can bring NOX online without guessing hidden env variables."
    };
    let save_label = if completed_before || status.is_ready() {
        "Save and continue"
    } else {
        "Finish setup"
    };

    let fields = SETUP_FIELDS
        .iter()
        .map(|field| {
            let value = status.values.get(field.key).cloned().unwrap_or_default();
            let input_name = setup_input_name(field.key);
            let input_type = if field.secret { "password" } else { "text" };
            format!(
                "<article class=\"setup-field\"><div><p class=\"field-label\">{}</p><p class=\"muted\">{}</p></div><label><span>Value</span><input name=\"{}\" type=\"{}\" placeholder=\"{}\" value=\"{}\"></label><p class=\"field-help\">{}</p></article>",
                escape_html(field.label),
                escape_html(field.instructions),
                input_name,
                input_type,
                escape_html(field.placeholder),
                escape_html(&value),
                escape_html(field.help)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let missing = if status.missing_keys.is_empty() {
        "<span class=\"status-pill success\">Setup ready</span>".to_string()
    } else {
        format!(
            "<span class=\"status-pill danger\">Missing: {}</span>",
            escape_html(&status.missing_keys.join(", "))
        )
    };

    format!(
        "<main class=\"shell\"><header class=\"console-topbar compact\"><div><p class=\"eyebrow\">First-time setup</p><h1>Welcome, {}</h1><p class=\"muted\">{}</p></div><div class=\"topbar-actions\">{}<a class=\"nav-link\" href=\"/settings\">Advanced settings</a></div></header><section class=\"panel wizard-panel\">{}<form method=\"post\" action=\"/setup\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><div class=\"setup-grid\">{}</div><div class=\"wizard-actions\"><button type=\"submit\">{}</button></div></form><form method=\"post\" action=\"/setup/skip\" class=\"skip-form\"><button class=\"secondary\" type=\"submit\">Skip for now</button><p class=\"muted\">You can finish the setup later from the banner in the console or from Settings.</p></form></section></main>",
        escape_html(username),
        escape_html(setup_state),
        missing,
        error_html,
        escape_html(csrf_token),
        fields,
        save_label
    )
}

fn render_console_page(
    username: &str,
    mode: DetailMode,
    runs: &[Run],
    selected_run: Option<&Run>,
    setup_status: &EssentialSetupStatus,
) -> String {
    let total_runs = runs.iter().filter(|run| !run.is_demo).count();
    let running_runs = runs
        .iter()
        .filter(|run| run.status == RunStatus::Running && !run.is_demo)
        .count();
    let failed_runs = runs
        .iter()
        .filter(|run| run.status == RunStatus::Failed && !run.is_demo)
        .count();

    let setup_banner = if setup_status.is_ready() {
        String::new()
    } else {
        format!(
            "<section class=\"alert warning banner\"><strong>Setup incomplete.</strong> Missing required keys: {}. <a href=\"/setup\">Finish setup</a> before expecting Telegram traffic to work.</section>",
            escape_html(&setup_status.missing_keys.join(", "))
        )
    };

    let sidebar_cards = runs
        .iter()
        .map(|run| {
            render_run_card(
                run,
                selected_run.map(|selected| selected.id.as_str()) == Some(run.id.as_str()),
                mode,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let detail_html = selected_run
        .map(|run| render_run_detail(run, mode))
        .unwrap_or_else(|| "<section class=\"panel empty-state\"><h2>No runs yet</h2><p>The console is ready, but there are no observable executions to render.</p></section>".to_string());

    format!(
        "<main class=\"shell\">{}<header class=\"console-topbar\"><div><p class=\"eyebrow\">Assistant Console</p><h1>NOX observable runs</h1><p class=\"muted\">Each Telegram request appears as an execution with status, steps, final output and runtime metadata.</p></div><div class=\"topbar-actions\"><div class=\"toggle-group\">{} {}</div><a class=\"nav-link\" href=\"/setup\">Setup</a><a class=\"nav-link\" href=\"/settings\">Settings</a><form method=\"post\" action=\"/logout\"><button class=\"secondary\" type=\"submit\">Logout</button></form></div></header><section class=\"summary-strip\"><article class=\"summary-chip\"><span>Real runs</span><strong>{}</strong></article><article class=\"summary-chip\"><span>Running</span><strong>{}</strong></article><article class=\"summary-chip\"><span>Failed</span><strong>{}</strong></article><article class=\"summary-chip\"><span>Viewer</span><strong>{}</strong></article><article class=\"summary-chip\"><span>Operator</span><strong>{}</strong></article></section><section class=\"console-grid\"><aside class=\"history-column\"><div class=\"history-header\"><h2>Request history</h2><p class=\"muted\">Scan by run, not by message.</p></div>{}</aside><section class=\"detail-column\">{}</section></section></main>",
        setup_banner,
        render_mode_link(DetailMode::Simple, mode, selected_run),
        render_mode_link(DetailMode::Detailed, mode, selected_run),
        total_runs,
        running_runs,
        failed_runs,
        mode.label(),
        escape_html(username),
        sidebar_cards,
        detail_html
    )
}

fn render_mode_link(
    target_mode: DetailMode,
    current_mode: DetailMode,
    run: Option<&Run>,
) -> String {
    let href = run
        .map(|run| {
            format!(
                "/console?run={}&mode={}",
                escape_html(&run.id),
                target_mode.as_query()
            )
        })
        .unwrap_or_else(|| format!("/console?mode={}", target_mode.as_query()));
    let class = if target_mode == current_mode {
        "toggle active"
    } else {
        "toggle"
    };
    format!(
        "<a class=\"{}\" href=\"{}\">{}</a>",
        class,
        href,
        target_mode.label()
    )
}

fn render_run_card(run: &Run, is_active: bool, mode: DetailMode) -> String {
    let href = format!(
        "/console?run={}&mode={}",
        escape_html(&run.id),
        mode.as_query()
    );
    let active_class = if is_active {
        "run-card active"
    } else {
        "run-card"
    };
    let active_step = run
        .active_step_label
        .as_ref()
        .map(|value| format!("<p class=\"microcopy\">Active: {}</p>", escape_html(value)))
        .unwrap_or_default();
    let demo_badge = if run.is_demo {
        "<span class=\"status-pill neutral\">Demo</span>"
    } else {
        ""
    };

    format!(
        "<a class=\"{}\" href=\"{}\"><div class=\"run-card-top\"><div class=\"status-row\"><span class=\"status-pill {}\">{}</span>{}</div><span class=\"microcopy\">{}</span></div><h3>{}</h3><p>{}</p><div class=\"run-card-meta\"><span>{} steps</span><span>{}</span><span>{}</span></div>{}</a>",
        active_class,
        href,
        run.status.tone_class(),
        run.status.label(),
        demo_badge,
        escape_html(&run.started_at),
        escape_html(&run.request_title),
        escape_html(&run.summary),
        run.step_count,
        escape_html(&run.conversation_mode),
        format_latency(run.latency_ms),
        active_step
    )
}

fn render_run_detail(run: &Run, mode: DetailMode) -> String {
    let visible_steps = if mode == DetailMode::Simple {
        run.steps
            .iter()
            .filter(|step| {
                matches!(
                    step.kind,
                    StepKind::Tool | StepKind::Output | StepKind::Validation
                )
            })
            .count()
            .max(1)
    } else {
        run.steps.len()
    };

    format!(
        "{}<section class=\"detail-grid\"><div class=\"detail-main\">{}{}{} </div><aside class=\"detail-side\">{}</aside></section>",
        render_run_header(run),
        render_final_result_panel(run),
        render_error_panel(run),
        render_step_timeline(run, mode, visible_steps),
        render_metadata_panel(run)
    )
}

fn render_run_header(run: &Run) -> String {
    let finished = run.finished_at.as_deref().unwrap_or("Still running");
    let demo_badge = if run.is_demo {
        "<span class=\"header-chip\">Demo fallback</span>"
    } else {
        ""
    };

    format!(
        "<section class=\"panel run-header\"><div class=\"run-header-top\"><div><p class=\"eyebrow\">Observable execution</p><h2>{}</h2><p>{}</p></div><div class=\"run-status-block\"><span class=\"status-pill {}\">{}</span><span class=\"run-id\">{}</span>{}</div></div><div class=\"request-block\"><h3>User request</h3><p>{}</p></div><div class=\"header-meta\"><span class=\"header-chip\">Started: {}</span><span class=\"header-chip\">Finished: {}</span><span class=\"header-chip\">Latency: {}</span><span class=\"header-chip\">Model: {}</span><span class=\"header-chip\">Channel: {}</span></div></section>",
        escape_html(&run.request_title),
        escape_html(&run.summary),
        run.status.tone_class(),
        run.status.label(),
        escape_html(&run.id),
        demo_badge,
        escape_html(&run.request_text),
        escape_html(&run.started_at),
        escape_html(finished),
        format_latency(run.latency_ms),
        escape_html(&run.model),
        escape_html(&run.channel)
    )
}

fn render_final_result_panel(run: &Run) -> String {
    let result = &run.final_result;
    let highlights = result
        .highlights
        .iter()
        .map(|item| format!("<li>{}</li>", escape_html(item)))
        .collect::<Vec<_>>()
        .join("");
    let body = result
        .body
        .iter()
        .map(|paragraph| format!("<p>{}</p>", escape_html(paragraph)))
        .collect::<Vec<_>>()
        .join("");
    let artifacts = if result.artifacts.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"section-subpanel\"><h4>Artifacts</h4><div class=\"inline-metadata\">{}</div></div>",
            result
                .artifacts
                .iter()
                .map(render_metadata_chip)
                .collect::<Vec<_>>()
                .join("")
        )
    };

    format!(
        "<section class=\"panel result-panel\"><div class=\"section-head\"><div><p class=\"eyebrow\">Final result</p><h3>{}</h3></div></div><p class=\"result-summary\">{}</p><ul class=\"highlight-list\">{}</ul>{}{}</section>",
        escape_html(&result.title),
        escape_html(&result.summary),
        highlights,
        body,
        artifacts
    )
}

fn render_error_panel(run: &Run) -> String {
    let Some(error) = run.error.as_ref() else {
        return String::new();
    };
    format!(
        "<section class=\"panel error-panel\"><div class=\"section-head\"><div><p class=\"eyebrow\">Failure context</p><h3>{}</h3></div></div><p class=\"alert danger\">{}</p><p class=\"muted\">{}</p></section>",
        escape_html(&error.title),
        escape_html(&error.message),
        escape_html(&error.suggestion)
    )
}

fn render_step_timeline(run: &Run, mode: DetailMode, visible_count: usize) -> String {
    let items = run
        .steps
        .iter()
        .filter(|step| {
            mode == DetailMode::Detailed
                || matches!(
                    step.kind,
                    StepKind::Tool | StepKind::Output | StepKind::Validation
                )
        })
        .map(|step| render_step_item(step, mode))
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<section class=\"panel\"><div class=\"section-head\"><div><p class=\"eyebrow\">Execution timeline</p><h3>Visible steps</h3></div><span class=\"section-count\">{}</span></div><ol class=\"timeline\">{}</ol></section>",
        visible_count, items
    )
}

fn render_step_item(step: &Step, mode: DetailMode) -> String {
    let detail_html = if mode == DetailMode::Detailed {
        format!("<p class=\"step-detail\">{}</p>", escape_html(&step.detail))
    } else {
        String::new()
    };
    let metrics_html = if step.metrics.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"inline-metadata\">{}</div>",
            step.metrics
                .iter()
                .map(render_metadata_chip)
                .collect::<Vec<_>>()
                .join("")
        )
    };
    let trace_html = if mode == DetailMode::Detailed {
        step.trace
            .as_ref()
            .map(render_trace_panel)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let finished = step.finished_at.as_deref().unwrap_or("in progress");

    format!(
        "<li class=\"timeline-item\"><div class=\"timeline-marker {}\"></div><article class=\"step-card\"><div class=\"step-head\"><div><div class=\"step-label-row\"><span class=\"step-kind\">{}</span><span class=\"status-pill {}\">{}</span></div><h4>{}</h4></div><div class=\"microcopy\"><span>{}</span><span>{}</span><span>{}</span></div></div><p class=\"step-summary\">{}</p>{}{}{}</article></li>",
        step.status.tone_class(),
        step.kind.label(),
        step.status.tone_class(),
        step.status.label(),
        escape_html(&step.title),
        escape_html(&step.id),
        escape_html(&step.started_at),
        escape_html(finished),
        escape_html(&step.summary),
        detail_html,
        metrics_html,
        trace_html
    )
}

fn render_metadata_panel(run: &Run) -> String {
    let schema_items = [
        MetadataItem::new(
            "Streaming contract",
            "Run status + active step + result payload",
        ),
        MetadataItem::new("Trace contract", "Per-step structured trace payload"),
        MetadataItem::new("Extensibility", "Ready for SSE / websockets"),
    ];
    let primary = run
        .metadata
        .iter()
        .map(render_metadata_row)
        .collect::<Vec<_>>()
        .join("");
    let schema = schema_items
        .iter()
        .map(render_metadata_row)
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<section class=\"panel metadata-panel\"><div class=\"section-head\"><div><p class=\"eyebrow\">Metadata</p><h3>Run context</h3></div></div><dl class=\"metadata-list\">{}</dl><div class=\"section-subpanel\"><h4>Future-ready contracts</h4><dl class=\"metadata-list\">{}</dl></div></section>",
        primary, schema
    )
}

fn render_trace_panel(trace: &ToolTrace) -> String {
    format!(
        "<div class=\"trace-panel\"><div class=\"trace-head\"><span>Trace</span><strong>{}</strong></div><pre>{}</pre></div>",
        escape_html(&trace.label),
        escape_html(&trace.payload)
    )
}

fn render_metadata_row(item: &MetadataItem) -> String {
    format!(
        "<div class=\"metadata-row\"><dt>{}</dt><dd>{}</dd></div>",
        escape_html(&item.label),
        escape_html(&item.value)
    )
}

fn render_metadata_chip(item: &MetadataItem) -> String {
    format!(
        "<span class=\"meta-chip\"><strong>{}</strong><span>{}</span></span>",
        escape_html(&item.label),
        escape_html(&item.value)
    )
}

fn render_settings_page(
    username: &str,
    csrf_token: &str,
    values: &BTreeMap<String, String>,
    error: Option<&str>,
    success: Option<&str>,
) -> String {
    let error_html = error
        .map(|message| format!("<p class=\"alert danger\">{}</p>", escape_html(message)))
        .unwrap_or_default();
    let success_html = success
        .map(|message| format!("<p class=\"alert success\">{}</p>", escape_html(message)))
        .unwrap_or_default();

    let fields = SETTING_FIELDS
        .iter()
        .map(|field| {
            let value = values.get(field.key).cloned().unwrap_or_default();
            let input = if field.multiline {
                format!(
                    "<textarea name=\"{}\" rows=\"4\">{}</textarea>",
                    field.key,
                    escape_html(&value)
                )
            } else {
                let input_type = if field.secret { "password" } else { "text" };
                format!(
                    "<input name=\"{}\" type=\"{}\" value=\"{}\">",
                    field.key,
                    input_type,
                    escape_html(&value)
                )
            };
            format!(
                "<label class=\"setting\"><span>{}</span>{}<small>{}</small></label>",
                escape_html(field.label),
                input,
                escape_html(field.help)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<main class=\"shell\"><header class=\"console-topbar compact\"><div><p class=\"eyebrow\">Configuration</p><h1>NOX settings</h1><p class=\"muted\">Signed in as <strong>{}</strong>. This page writes directly to <code>.env</code>.</p></div><div class=\"topbar-actions\"><a class=\"nav-link\" href=\"/setup\">Setup</a><a class=\"nav-link\" href=\"/console\">Console</a><form method=\"post\" action=\"/logout\"><button class=\"secondary\" type=\"submit\">Logout</button></form></div></header><section class=\"panel settings-panel\"><p class=\"muted\">Passwords are stored separately in <code>USER_STORE_PATH</code> using Argon2. Secrets on this page remain in <code>.env</code>, so protect file access on the host.</p>{}{}<form method=\"post\" action=\"/settings\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><div class=\"settings-grid\">{}</div><div class=\"actions\"><button type=\"submit\">Save .env</button></div></form></section></main>",
        escape_html(username),
        error_html,
        success_html,
        escape_html(csrf_token),
        fields
    )
}

fn setup_input_name(key: &str) -> &'static str {
    match key {
        "TELOXIDE_TOKEN" => "teloxide_token",
        "CHAT_ID" => "chat_id",
        "OLLAMA_BASE_URL" => "ollama_base_url",
        "OLLAMA_MODEL" => "ollama_model",
        _ => "unknown",
    }
}

fn format_latency(latency_ms: u64) -> String {
    if latency_ms >= 1000 {
        format!("{:.1}s", latency_ms as f64 / 1000.0)
    } else {
        format!("{}ms", latency_ms)
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLES: &str = r#"
:root {
  color-scheme: light;
  --bg: #f6f2eb;
  --surface: rgba(255, 253, 248, 0.92);
  --surface-strong: #fffaf2;
  --surface-tint: rgba(250, 241, 229, 0.86);
  --line: rgba(95, 66, 43, 0.16);
  --line-strong: rgba(95, 66, 43, 0.28);
  --text: #1f1711;
  --muted: #716455;
  --accent: #9e4f1d;
  --accent-strong: #7f3e15;
  --info: #1e6e83;
  --success: #256d4c;
  --danger: #a13434;
  --warning: #9e6a15;
  --shadow: 0 24px 60px rgba(38, 26, 18, 0.08);
}
* { box-sizing: border-box; }
body {
  margin: 0;
  min-height: 100vh;
  color: var(--text);
  font-family: "IBM Plex Sans", "Segoe UI", sans-serif;
  background:
    radial-gradient(circle at top left, rgba(158, 79, 29, 0.16), transparent 26%),
    radial-gradient(circle at top right, rgba(30, 110, 131, 0.12), transparent 22%),
    linear-gradient(180deg, #f9f4ed 0%, var(--bg) 100%);
}
a { color: inherit; }
h1, h2, h3, h4, strong { font-family: "IBM Plex Serif", Georgia, serif; }
p { line-height: 1.55; }
code, pre { font-family: "SFMono-Regular", Menlo, monospace; }
.shell { max-width: 1480px; margin: 0 auto; padding: 28px 22px 52px; }
.auth-shell {
  max-width: 1180px; margin: 0 auto; padding: 48px 22px; min-height: 100vh;
  display: grid; grid-template-columns: 1.15fr 0.85fr; gap: 26px; align-items: center;
}
.auth-hero h1 { font-size: clamp(2.8rem, 6vw, 5rem); margin: 10px 0 16px; }
.eyebrow { margin: 0; text-transform: uppercase; letter-spacing: 0.14em; font-size: 0.78rem; color: var(--accent-strong); font-weight: 700; }
.panel, .glass {
  background: var(--surface); border: 1px solid var(--line); border-radius: 24px;
  box-shadow: var(--shadow); backdrop-filter: blur(12px);
}
.panel { padding: 22px; }
.console-topbar {
  display: flex; justify-content: space-between; gap: 18px; align-items: flex-start; margin-bottom: 18px;
}
.console-topbar.compact { margin-bottom: 20px; }
.topbar-actions { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; }
.summary-strip { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 12px; margin-bottom: 18px; }
.summary-chip { background: var(--surface-tint); border: 1px solid var(--line); border-radius: 18px; padding: 14px 16px; }
.summary-chip span { display: block; font-size: 0.82rem; color: var(--muted); }
.summary-chip strong { display: block; margin-top: 6px; font-size: 1.15rem; }
.console-grid { display: grid; grid-template-columns: 360px minmax(0, 1fr); gap: 18px; }
.history-column, .detail-column, .detail-grid, .detail-main, .detail-side { display: grid; gap: 16px; align-content: start; }
.detail-grid { grid-template-columns: minmax(0, 1.7fr) 320px; }
.history-header { padding: 6px 6px 2px; }
.run-card {
  display: block; background: var(--surface); border: 1px solid var(--line); border-radius: 20px;
  padding: 16px; box-shadow: var(--shadow); transition: transform 120ms ease, border-color 120ms ease;
  text-decoration: none;
}
.run-card:hover { transform: translateY(-1px); border-color: var(--line-strong); }
.run-card.active { border-color: rgba(158, 79, 29, 0.45); background: linear-gradient(180deg, rgba(255, 250, 242, 0.98), rgba(249, 241, 230, 0.95)); }
.run-card-top, .run-card-meta, .step-head, .run-header-top, .section-head, .trace-head { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
.status-row { display: flex; gap: 8px; flex-wrap: wrap; }
.status-pill, .header-chip, .meta-chip, .toggle, .nav-link {
  display: inline-flex; align-items: center; gap: 8px; border-radius: 999px; padding: 7px 12px;
  border: 1px solid var(--line); background: rgba(255, 255, 255, 0.75); font-size: 0.82rem; text-decoration: none;
}
.toggle.active { border-color: rgba(158, 79, 29, 0.45); background: rgba(158, 79, 29, 0.12); color: var(--accent-strong); }
.status-pill.success { color: var(--success); }
.status-pill.info { color: var(--info); }
.status-pill.danger { color: var(--danger); }
.status-pill.neutral, .status-pill.muted { color: var(--muted); }
.muted, .microcopy, small, .field-help { color: var(--muted); }
.request-block, .setup-field {
  background: rgba(250, 245, 236, 0.78); border: 1px solid var(--line); border-radius: 18px; padding: 16px;
}
.header-meta, .inline-metadata { display: flex; flex-wrap: wrap; gap: 10px; }
.timeline { list-style: none; padding: 0; margin: 0; display: grid; gap: 12px; }
.timeline-item { display: grid; grid-template-columns: 18px minmax(0, 1fr); gap: 14px; }
.timeline-marker { width: 12px; height: 12px; border-radius: 50%; margin-top: 18px; border: 2px solid currentColor; color: var(--muted); }
.timeline-marker.success { color: var(--success); }
.timeline-marker.info { color: var(--info); }
.timeline-marker.danger { color: var(--danger); }
.timeline-marker.neutral, .timeline-marker.muted { color: rgba(113, 100, 85, 0.55); }
.step-card { background: var(--surface-strong); border: 1px solid var(--line); border-radius: 18px; padding: 16px; }
.step-label-row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.step-kind, .field-label { font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.08em; color: var(--accent-strong); font-weight: 700; }
.trace-panel, .section-subpanel {
  margin-top: 12px; border-radius: 16px; border: 1px solid var(--line); background: rgba(248, 242, 233, 0.85); padding: 12px;
}
.trace-panel pre { margin: 10px 0 0; white-space: pre-wrap; word-break: break-word; font-size: 0.82rem; }
.metadata-list { display: grid; gap: 10px; margin: 0; }
.metadata-row { display: grid; gap: 4px; padding: 12px 0; border-top: 1px solid var(--line); }
.metadata-row:first-child { border-top: 0; padding-top: 0; }
.metadata-row dt { color: var(--muted); font-size: 0.82rem; }
.metadata-row dd { margin: 0; }
.highlight-list { margin: 12px 0 0; padding-left: 18px; display: grid; gap: 8px; }
.toggle-group { display: inline-flex; gap: 8px; padding: 4px; border-radius: 999px; background: rgba(255, 255, 255, 0.62); border: 1px solid var(--line); }
.alert { border-radius: 16px; padding: 12px 14px; margin: 0 0 12px; }
.alert.danger { background: rgba(161, 52, 52, 0.10); color: var(--danger); }
.alert.success { background: rgba(37, 109, 76, 0.10); color: var(--success); }
.alert.warning { background: rgba(158, 106, 21, 0.12); color: var(--warning); }
.banner { margin-bottom: 16px; }
label, form { display: grid; gap: 12px; }
input, textarea, button { font: inherit; }
input, textarea {
  width: 100%; padding: 12px 14px; border-radius: 14px; border: 1px solid var(--line);
  background: rgba(255, 255, 255, 0.84); color: var(--text);
}
textarea { min-height: 120px; resize: vertical; }
button { border: 0; border-radius: 999px; padding: 12px 18px; background: var(--accent); color: #fff9f3; cursor: pointer; }
button.secondary { background: #dfc8b1; color: var(--text); }
.settings-grid, .setup-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 18px; }
.wizard-actions, .actions { margin-top: 8px; }
.skip-form { margin-top: 16px; }
.empty-state { min-height: 320px; display: grid; place-items: center; text-align: center; }
@media (max-width: 1180px) {
  .summary-strip { grid-template-columns: repeat(3, minmax(0, 1fr)); }
  .console-grid, .detail-grid, .auth-shell { grid-template-columns: 1fr; }
}
@media (max-width: 760px) {
  .shell, .auth-shell { padding: 18px 14px 36px; }
  .summary-strip { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .console-topbar, .run-header-top, .section-head, .step-head { flex-direction: column; }
}
"#;

#[cfg(test)]
mod tests {
    use super::{
        DetailMode, EssentialSetupStatus, MIN_PASSWORD_LENGTH, collect_setup_values,
        validate_username,
    };
    use std::collections::HashMap;

    #[test]
    fn username_validation_rejects_symbols() {
        assert!(validate_username("bad user").is_err());
        assert!(validate_username("ok_user").is_ok());
    }

    #[test]
    fn password_policy_is_reasonable() {
        assert!(MIN_PASSWORD_LENGTH >= 12);
    }

    #[test]
    fn detail_mode_defaults_to_detailed() {
        assert_eq!(DetailMode::from_query(None), DetailMode::Detailed);
        assert_eq!(DetailMode::from_query(Some("simple")), DetailMode::Simple);
    }

    #[test]
    fn setup_collects_required_values() {
        let mut form = HashMap::new();
        form.insert("teloxide_token".to_string(), "token".to_string());
        form.insert("chat_id".to_string(), "1".to_string());
        form.insert(
            "ollama_base_url".to_string(),
            "http://127.0.0.1:11434".to_string(),
        );
        form.insert("ollama_model".to_string(), "qwen2.5:7b".to_string());
        let values = collect_setup_values(&form).expect("setup values");
        assert_eq!(values.get("CHAT_ID").map(String::as_str), Some("1"));
    }

    #[test]
    fn essential_setup_status_detects_missing_keys() {
        let status = EssentialSetupStatus {
            values: Default::default(),
            missing_keys: vec!["TELOXIDE_TOKEN"],
        };
        assert!(!status.is_ready());
    }
}
