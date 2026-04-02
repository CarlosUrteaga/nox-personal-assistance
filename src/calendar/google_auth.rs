use crate::config::GoogleOAuthBootstrapConfig;
use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;

const DEFAULT_AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_REDIRECT_HOST: &str = "127.0.0.1";
const DEFAULT_REDIRECT_PORT: u16 = 8080;
const GOOGLE_CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const EXPIRY_REFRESH_SKEW_SECS: i64 = 60;
const OAUTH_CALLBACK_TIMEOUT_SECS: u64 = 300;

pub struct GoogleAccessTokenProvider {
    client: Client,
    source: TokenSource,
}

enum TokenSource {
    OAuthTokenPath {
        credentials_path: Option<PathBuf>,
        path: PathBuf,
        cache: Mutex<Option<AuthorizedUserToken>>,
        bootstrap: BootstrapFn,
        timeout_secs: u64,
    },
    StaticAccessToken(String),
}

type BootstrapFuture = Pin<Box<dyn Future<Output = Result<GoogleOAuthBootstrapResult, String>> + Send>>;
type BootstrapFn =
    Arc<dyn Fn(GoogleOAuthBootstrapConfig, u64) -> BootstrapFuture + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleOAuthBootstrapResult {
    pub token_path: String,
}

#[derive(Debug, Deserialize)]
struct OAuthClientSecretsFile {
    installed: Option<OAuthClientSecrets>,
    web: Option<OAuthClientSecrets>,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthClientSecrets {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    auth_uri: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Debug)]
struct CallbackState {
    expected_state: String,
    sender: Mutex<Option<oneshot::Sender<CallbackOutcome>>>,
}

type CallbackOutcome = Result<String, String>;

impl GoogleAccessTokenProvider {
    pub fn new(
        oauth_credentials_path: Option<String>,
        oauth_token_path: Option<String>,
        access_token: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self, String> {
        Self::new_with_bootstrap(
            oauth_credentials_path,
            oauth_token_path,
            access_token,
            timeout_secs,
            Arc::new(|config, timeout_secs| {
                Box::pin(async move { bootstrap_google_oauth(&config, timeout_secs).await })
            }),
        )
    }

    fn new_with_bootstrap(
        oauth_credentials_path: Option<String>,
        oauth_token_path: Option<String>,
        access_token: Option<String>,
        timeout_secs: u64,
        bootstrap: BootstrapFn,
    ) -> Result<Self, String> {
        let client = build_http_client(timeout_secs, "Google OAuth")?;

        let source = if let Some(path) = oauth_token_path {
            TokenSource::OAuthTokenPath {
                credentials_path: oauth_credentials_path.map(PathBuf::from),
                path: PathBuf::from(path),
                cache: Mutex::new(None),
                bootstrap,
                timeout_secs,
            }
        } else if let Some(token) = access_token {
            TokenSource::StaticAccessToken(token)
        } else {
            return Err(
                "GOOGLE_OAUTH_CREDENTIALS_PATH, GOOGLE_OAUTH_TOKEN_PATH, or GOOGLE_CALENDAR_ACCESS_TOKEN must be configured".to_string(),
            );
        };

        Ok(Self { client, source })
    }

    pub async fn access_token(&self) -> Result<String, String> {
        match &self.source {
            TokenSource::StaticAccessToken(token) => Ok(token.clone()),
            TokenSource::OAuthTokenPath {
                credentials_path,
                path,
                cache,
                bootstrap,
                timeout_secs,
            } => {
                let cached = cache.lock().map_err(|_| token_lock_error())?.clone();
                let mut credentials = match cached {
                    Some(cached) => cached,
                    None => {
                        let loaded = self
                            .load_or_bootstrap_token(
                                path,
                                credentials_path.as_deref(),
                                bootstrap,
                                *timeout_secs,
                            )
                            .await?;
                        *cache.lock().map_err(|_| token_lock_error())? = Some(loaded.clone());
                        loaded
                    }
                };

                if credentials.needs_refresh() {
                    credentials = match self.refresh_token(path, credentials.clone()).await {
                        Ok(refreshed) => refreshed,
                        Err(err) => {
                            self.bootstrap_after_runtime_auth_failure(
                                path,
                                credentials_path.as_deref(),
                                bootstrap,
                                *timeout_secs,
                                err,
                            )
                            .await?
                        }
                    };
                    *cache.lock().map_err(|_| token_lock_error())? = Some(credentials.clone());
                }

                credentials.current_access_token(path)
            }
        }
    }

    async fn load_or_bootstrap_token(
        &self,
        path: &Path,
        credentials_path: Option<&Path>,
        bootstrap: &BootstrapFn,
        timeout_secs: u64,
    ) -> Result<AuthorizedUserToken, String> {
        match read_token_file(path) {
            Ok(token) => Ok(token),
            Err(err) => {
                self.bootstrap_after_runtime_auth_failure(
                    path,
                    credentials_path,
                    bootstrap,
                    timeout_secs,
                    err,
                )
                .await
            }
        }
    }

    async fn refresh_token(
        &self,
        path: &Path,
        credentials: AuthorizedUserToken,
    ) -> Result<AuthorizedUserToken, String> {
        let refresh_token = credentials
            .refresh_token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "GOOGLE_OAUTH_TOKEN_PATH '{}' is missing refresh_token",
                    path.display()
                )
            })?;
        let client_id = credentials
            .client_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "GOOGLE_OAUTH_TOKEN_PATH '{}' is missing client_id",
                    path.display()
                )
            })?;
        let client_secret = credentials
            .client_secret
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "GOOGLE_OAUTH_TOKEN_PATH '{}' is missing client_secret",
                    path.display()
                )
            })?;
        let token_uri = credentials.token_uri();

        let response = self
            .client
            .post(&token_uri)
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| {
                format!(
                    "Failed to refresh Google OAuth token from '{}': {}",
                    path.display(),
                    e
                )
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Google OAuth refresh failed for '{}': HTTP {}: {}",
                path.display(),
                status,
                sanitize_google_body(&body)
            ));
        }

        let refreshed: TokenEndpointResponse = response.json().await.map_err(|e| {
            format!(
                "Failed to parse Google OAuth refresh response for '{}': {}",
                path.display(),
                e
            )
        })?;

        let updated = credentials.with_refreshed_access_token(refreshed)?;
        write_token_file(path, &updated)?;
        Ok(updated)
    }

    async fn bootstrap_after_runtime_auth_failure(
        &self,
        path: &Path,
        credentials_path: Option<&Path>,
        bootstrap: &BootstrapFn,
        timeout_secs: u64,
        cause: String,
    ) -> Result<AuthorizedUserToken, String> {
        let credentials_path = credentials_path.ok_or_else(|| {
            format!(
                "Google Calendar runtime auth could not recover '{}': {}. Set GOOGLE_OAUTH_CREDENTIALS_PATH so NOX can re-auth automatically, run `cargo run -- google-auth`, or provide GOOGLE_CALENDAR_ACCESS_TOKEN.",
                path.display(),
                cause
            )
        })?;

        let config = GoogleOAuthBootstrapConfig {
            credentials_path: credentials_path.to_string_lossy().into_owned(),
            token_path: path.to_string_lossy().into_owned(),
        };
        bootstrap(config, timeout_secs).await.map_err(|err| {
            format!(
                "Google Calendar runtime auth recovery failed for '{}': {}",
                path.display(),
                err
            )
        })?;
        read_token_file(path)
    }
}

pub async fn bootstrap_google_oauth(
    config: &GoogleOAuthBootstrapConfig,
    timeout_secs: u64,
) -> Result<GoogleOAuthBootstrapResult, String> {
    let client_secrets = read_client_secrets(Path::new(&config.credentials_path))?;
    let client = build_http_client(timeout_secs, "Google OAuth bootstrap")?;
    let state = random_hex(16);
    let redirect_uri = redirect_uri();
    let auth_url = build_authorization_url(&client_secrets, &redirect_uri, &state);
    let code = wait_for_authorization_code(&redirect_uri, &state, &auth_url).await?;
    let token_response =
        exchange_authorization_code(&client, &client_secrets, &code, &redirect_uri).await?;
    let token = AuthorizedUserToken::from_token_exchange(&client_secrets, token_response)?;
    write_token_file(Path::new(&config.token_path), &token)?;

    Ok(GoogleOAuthBootstrapResult {
        token_path: config.token_path.clone(),
    })
}

fn read_client_secrets(path: &Path) -> Result<OAuthClientSecrets, String> {
    let contents = fs::read_to_string(path).map_err(|e| {
        format!(
            "Failed to read GOOGLE_OAUTH_CREDENTIALS_PATH '{}': {}",
            path.display(),
            e
        )
    })?;
    let parsed: OAuthClientSecretsFile = serde_json::from_str(&contents).map_err(|e| {
        format!(
            "Failed to parse GOOGLE_OAUTH_CREDENTIALS_PATH '{}': {}",
            path.display(),
            e
        )
    })?;

    parsed.installed.or(parsed.web).ok_or_else(|| {
        format!(
            "GOOGLE_OAUTH_CREDENTIALS_PATH '{}' must contain an 'installed' or 'web' OAuth client",
            path.display()
        )
    })
}

fn build_authorization_url(
    client_secrets: &OAuthClientSecrets,
    redirect_uri: &str,
    state: &str,
) -> String {
    let auth_uri = client_secrets
        .auth_uri
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_AUTH_URI);
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        auth_uri,
        urlencoding::encode(&client_secrets.client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(GOOGLE_CALENDAR_SCOPE),
        urlencoding::encode(state)
    )
}

async fn wait_for_authorization_code(
    redirect_uri: &str,
    expected_state: &str,
    auth_url: &str,
) -> Result<String, String> {
    let callback_state = Arc::new(CallbackState {
        expected_state: expected_state.to_string(),
        sender: Mutex::new(None),
    });
    let (code_tx, code_rx) = oneshot::channel::<CallbackOutcome>();
    *callback_state
        .sender
        .lock()
        .map_err(|_| token_lock_error())? = Some(code_tx);

    let listener = TcpListener::bind((DEFAULT_REDIRECT_HOST, DEFAULT_REDIRECT_PORT))
        .await
        .map_err(|e| {
            format!(
                "Failed to bind OAuth callback server on {}: {}. Close anything using this port or change the redirect URI setup.",
                redirect_uri, e
            )
        })?;
    let local_addr = listener.local_addr().map_err(|e| {
        format!(
            "Failed to read OAuth callback server address for {}: {}",
            redirect_uri, e
        )
    })?;

    let app = Router::new()
        .route("/", get(oauth_callback_handler))
        .with_state(callback_state);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    println!("Open this URL in your browser to authorize Google Calendar access:\n");
    println!("{}", auth_url);
    println!(
        "\nWaiting for the OAuth callback on http://{}:{}/",
        local_addr.ip(),
        local_addr.port()
    );

    let received = timeout(
        std::time::Duration::from_secs(OAUTH_CALLBACK_TIMEOUT_SECS),
        code_rx,
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out waiting for Google OAuth callback after {} seconds",
            OAUTH_CALLBACK_TIMEOUT_SECS
        )
    })?
    .map_err(|_| {
        "OAuth callback server stopped before receiving the authorization code".to_string()
    })?;

    let _ = shutdown_tx.send(());
    match server.await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(format!("OAuth callback server failed: {}", err));
        }
        Err(err) => {
            return Err(format!("OAuth callback server task failed: {}", err));
        }
    }

    received
}

async fn oauth_callback_handler(
    State(state): State<Arc<CallbackState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<&'static str> {
    let outcome = match (
        params.get("state").map(String::as_str),
        params.get("code").cloned(),
        params.get("error").cloned(),
    ) {
        (_, _, Some(error)) => Err(format!("Google OAuth returned an error: {}", error)),
        (Some(returned_state), _, _) if returned_state != state.expected_state => {
            Err("Google OAuth callback state mismatch".to_string())
        }
        (_, Some(code), _) if !code.trim().is_empty() => Ok(code),
        _ => Err("Google OAuth callback did not include an authorization code".to_string()),
    };

    if let Ok(mut sender) = state.sender.lock() {
        if let Some(sender) = sender.take() {
            let _ = sender.send(outcome);
        }
    }

    Html(
        "<html><body><h1>Google OAuth complete</h1><p>You can close this window and return to NOX.</p></body></html>",
    )
}

async fn exchange_authorization_code(
    client: &Client,
    client_secrets: &OAuthClientSecrets,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenEndpointResponse, String> {
    let token_uri = client_secrets
        .token_uri
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_TOKEN_URI);

    let response = client
        .post(token_uri)
        .form(&[
            ("client_id", client_secrets.client_id.as_str()),
            ("client_secret", client_secrets.client_secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| format!("Failed to exchange Google OAuth authorization code: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Google OAuth token exchange failed with HTTP {}: {}",
            status,
            sanitize_google_body(&body)
        ));
    }

    response.json().await.map_err(|e| {
        format!(
            "Failed to parse Google OAuth token exchange response: {}",
            e
        )
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AuthorizedUserToken {
    #[serde(default)]
    token: Option<String>,
    #[serde(default, alias = "access_token")]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    expiry: Option<DateTime<Utc>>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl AuthorizedUserToken {
    fn current_access_token(&self, path: &Path) -> Result<String, String> {
        self.access_token_value().ok_or_else(|| {
            format!(
                "GOOGLE_OAUTH_TOKEN_PATH '{}' does not contain an access token",
                path.display()
            )
        })
    }

    fn access_token_value(&self) -> Option<String> {
        self.token
            .as_ref()
            .or(self.access_token.as_ref())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn token_uri(&self) -> String {
        self.token_uri
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string())
    }

    fn needs_refresh(&self) -> bool {
        match (self.access_token_value(), self.expiry) {
            (None, _) => true,
            (Some(_), Some(expiry)) => {
                expiry <= Utc::now() + Duration::seconds(EXPIRY_REFRESH_SKEW_SECS)
            }
            (Some(_), None) => false,
        }
    }

    fn with_refreshed_access_token(
        &self,
        refreshed: TokenEndpointResponse,
    ) -> Result<Self, String> {
        let access_token = refreshed
            .access_token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth refresh response did not include access_token".to_string()
            })?;

        let expires_in = i64::from(refreshed.expires_in.unwrap_or(3600));
        let expiry = Utc::now() + Duration::seconds(expires_in);
        let mut updated = self.clone();
        updated.token = Some(access_token.clone());
        updated.access_token = Some(access_token);
        updated.expiry = Some(expiry);
        if let Some(refresh_token) = refreshed
            .refresh_token
            .clone()
            .filter(|value| !value.trim().is_empty())
        {
            updated.refresh_token = Some(refresh_token);
        }
        if let Some(scope) = refreshed.scope {
            updated.scopes = Some(
                scope
                    .split_whitespace()
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(token_type) = refreshed.token_type {
            updated.extra.insert(
                "token_type".to_string(),
                serde_json::Value::String(token_type),
            );
        }
        Ok(updated)
    }

    fn from_token_exchange(
        client_secrets: &OAuthClientSecrets,
        exchanged: TokenEndpointResponse,
    ) -> Result<Self, String> {
        let access_token = exchanged
            .access_token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth token exchange did not include access_token".to_string()
            })?;
        let refresh_token = exchanged
            .refresh_token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Google OAuth token exchange did not include refresh_token. Re-run consent with prompt=consent.".to_string())?;
        let expires_in = i64::from(exchanged.expires_in.unwrap_or(3600));
        let expiry = Utc::now() + Duration::seconds(expires_in);

        let mut extra = serde_json::Map::new();
        if let Some(token_type) = exchanged.token_type.clone() {
            extra.insert(
                "token_type".to_string(),
                serde_json::Value::String(token_type),
            );
        }

        Ok(Self {
            token: Some(access_token.clone()),
            access_token: Some(access_token),
            refresh_token: Some(refresh_token),
            token_uri: Some(
                client_secrets
                    .token_uri
                    .clone()
                    .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string()),
            ),
            client_id: Some(client_secrets.client_id.clone()),
            client_secret: Some(client_secrets.client_secret.clone()),
            expiry: Some(expiry),
            scopes: Some(
                exchanged
                    .scope
                    .map(|value| {
                        value
                            .split_whitespace()
                            .map(|item| item.to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| vec![GOOGLE_CALENDAR_SCOPE.to_string()]),
            ),
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TokenEndpointResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u32>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
}

fn read_token_file(path: &Path) -> Result<AuthorizedUserToken, String> {
    let contents = fs::read_to_string(path).map_err(|e| {
        format!(
            "Failed to read GOOGLE_OAUTH_TOKEN_PATH '{}': {}",
            path.display(),
            e
        )
    })?;
    serde_json::from_str(&contents).map_err(|e| {
        format!(
            "Failed to parse GOOGLE_OAUTH_TOKEN_PATH '{}': {}",
            path.display(),
            e
        )
    })
}

fn write_token_file(path: &Path, token: &AuthorizedUserToken) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(token).map_err(|e| {
        format!(
            "Failed to serialize refreshed Google OAuth token for '{}': {}",
            path.display(),
            e
        )
    })?;
    fs::write(path, rendered).map_err(|e| {
        format!(
            "Failed to write refreshed Google OAuth token to '{}': {}",
            path.display(),
            e
        )
    })
}

fn build_http_client(timeout_secs: u64, label: &str) -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("Failed to build {} client: {}", label, e))
}

fn redirect_uri() -> String {
    format!(
        "http://{}:{}/",
        DEFAULT_REDIRECT_HOST, DEFAULT_REDIRECT_PORT
    )
}

fn random_hex(bytes_len: usize) -> String {
    let mut bytes = vec![0_u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn sanitize_google_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty response>".to_string();
    }
    trimmed.chars().take(256).collect()
}

fn token_lock_error() -> String {
    "Google OAuth token provider lock was poisoned".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorizedUserToken, BootstrapFn, GOOGLE_CALENDAR_SCOPE, GoogleAccessTokenProvider,
        GoogleOAuthBootstrapResult, OAuthClientSecrets, TokenEndpointResponse,
        bootstrap_google_oauth, build_authorization_url, read_client_secrets, read_token_file,
    };
    use crate::config::GoogleOAuthBootstrapConfig;
    use chrono::{Duration, Utc};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_token_path(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!("nox-{}-{}.json", name, std::process::id()));
        path
    }

    fn test_bootstrap(
        token_body: &'static str,
    ) -> BootstrapFn {
        Arc::new(move |config, _timeout_secs| {
            Box::pin(async move {
                fs::write(&config.token_path, token_body).map_err(|e| e.to_string())?;
                Ok(GoogleOAuthBootstrapResult {
                    token_path: config.token_path,
                })
            })
        })
    }

    #[test]
    fn parses_authorized_user_token_with_google_python_shape() {
        let path = temp_token_path("parse-authorized-user");
        fs::write(
            &path,
            r#"{
  "token": "ya29.token",
  "refresh_token": "refresh",
  "token_uri": "https://oauth2.googleapis.com/token",
  "client_id": "client-id",
  "client_secret": "client-secret",
  "scopes": ["https://www.googleapis.com/auth/calendar"],
  "expiry": "2030-01-01T00:00:00Z"
}"#,
        )
        .expect("write token file");

        let token = read_token_file(&path).expect("parse token file");
        assert_eq!(token.access_token_value().as_deref(), Some("ya29.token"));
        assert!(!token.needs_refresh());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_google_credentials_file() {
        let path = temp_token_path("parse-credentials");
        fs::write(
            &path,
            r#"{
  "installed": {
    "client_id": "client-id",
    "client_secret": "client-secret",
    "auth_uri": "https://accounts.google.com/o/oauth2/v2/auth",
    "token_uri": "https://oauth2.googleapis.com/token"
  }
}"#,
        )
        .expect("write credentials file");

        let client = read_client_secrets(&path).expect("read client secrets");
        assert_eq!(client.client_id, "client-id");
        assert_eq!(client.client_secret, "client-secret");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn authorization_url_contains_expected_google_oauth_params() {
        let client = OAuthClientSecrets {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            auth_uri: Some("https://accounts.google.com/o/oauth2/v2/auth".to_string()),
            token_uri: Some("https://oauth2.googleapis.com/token".to_string()),
        };

        let url = build_authorization_url(&client, "http://127.0.0.1:8080/", "state123");

        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("state=state123"));
        assert!(url.contains(urlencoding::encode(GOOGLE_CALENDAR_SCOPE).as_ref()));
    }

    #[test]
    fn token_exchange_payload_writes_authorized_user_token() {
        let client = super::build_http_client(5, "test").expect("client");
        drop(client);
        let client_secrets = OAuthClientSecrets {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            auth_uri: None,
            token_uri: Some("https://oauth2.googleapis.com/token".to_string()),
        };
        let exchanged = TokenEndpointResponse {
            access_token: Some("ya29.new".to_string()),
            refresh_token: Some("refresh-token".to_string()),
            expires_in: Some(3600),
            scope: Some(GOOGLE_CALENDAR_SCOPE.to_string()),
            token_type: Some("Bearer".to_string()),
        };
        let authorized =
            AuthorizedUserToken::from_token_exchange(&client_secrets, exchanged).expect("token");
        let path = temp_token_path("write-authorized-user");
        super::write_token_file(&path, &authorized).expect("write token file");

        let written = read_token_file(&path).expect("read written token");
        assert_eq!(written.access_token_value().as_deref(), Some("ya29.new"));
        assert_eq!(written.refresh_token.as_deref(), Some("refresh-token"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn provider_reads_static_access_token_without_refresh() {
        let provider = GoogleAccessTokenProvider::new(None, None, Some("secret-token".into()), 5)
            .expect("provider");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let token = runtime
            .block_on(provider.access_token())
            .expect("access token");

        assert_eq!(token, "secret-token");
    }

    #[test]
    fn expired_authorized_user_token_is_marked_for_refresh() {
        let token: AuthorizedUserToken = serde_json::from_str(&format!(
            r#"{{
  "token": "ya29.old",
  "refresh_token": "refresh",
  "client_id": "client-id",
  "client_secret": "client-secret",
  "expiry": "{}"
}}"#,
            (Utc::now() - Duration::minutes(5)).to_rfc3339()
        ))
        .expect("parse token");

        assert!(token.needs_refresh());
    }

    #[test]
    fn missing_access_token_without_refresh_metadata_triggers_recovery() {
        let _guard = env_lock().lock().expect("env lock");
        let path = temp_token_path("missing-access-token");
        fs::write(
            &path,
            r#"{
  "client_id": "client-id"
}"#,
        )
        .expect("write token file");

        let provider = GoogleAccessTokenProvider::new_with_bootstrap(
            Some("/tmp/credentials.json".to_string()),
            Some(path.display().to_string()),
            None,
            5,
            test_bootstrap(
                r#"{
  "token": "ya29.recovered",
  "refresh_token": "refresh",
  "client_id": "client-id",
  "client_secret": "client-secret",
  "expiry": "2030-01-01T00:00:00Z"
}"#,
            ),
        )
        .expect("provider");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let token = runtime
            .block_on(provider.access_token())
            .expect("recovered access token");

        assert_eq!(token, "ya29.recovered");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_token_file_triggers_recovery_when_credentials_exist() {
        let _guard = env_lock().lock().expect("env lock");
        let path = temp_token_path("missing-token-file");
        let _ = fs::remove_file(&path);
        let provider = GoogleAccessTokenProvider::new_with_bootstrap(
            Some("/tmp/credentials.json".to_string()),
            Some(path.display().to_string()),
            None,
            5,
            test_bootstrap(
                r#"{
  "token": "ya29.bootstrapped",
  "refresh_token": "refresh",
  "client_id": "client-id",
  "client_secret": "client-secret",
  "expiry": "2030-01-01T00:00:00Z"
}"#,
            ),
        )
        .expect("provider");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let token = runtime
            .block_on(provider.access_token())
            .expect("access token");

        assert_eq!(token, "ya29.bootstrapped");
        let written = read_token_file(&path).expect("written token");
        assert_eq!(written.access_token_value().as_deref(), Some("ya29.bootstrapped"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_token_file_without_credentials_surfaces_recovery_error() {
        let _guard = env_lock().lock().expect("env lock");
        let path = temp_token_path("missing-token-no-credentials");
        let _ = fs::remove_file(&path);
        let provider = GoogleAccessTokenProvider::new_with_bootstrap(
            None,
            Some(path.display().to_string()),
            None,
            5,
            test_bootstrap("{}"),
        )
        .expect("provider");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let err = runtime
            .block_on(provider.access_token())
            .expect_err("missing credentials should fail");

        assert!(err.contains("Set GOOGLE_OAUTH_CREDENTIALS_PATH so NOX can re-auth automatically"));
    }

    #[test]
    fn bootstrap_config_drives_expected_result_shape() {
        let result = GoogleOAuthBootstrapResult {
            token_path: "/tmp/token.json".to_string(),
        };
        let config = GoogleOAuthBootstrapConfig {
            credentials_path: "/tmp/credentials.json".to_string(),
            token_path: "/tmp/token.json".to_string(),
        };

        assert_eq!(result.token_path, config.token_path);
    }

    #[allow(dead_code)]
    async fn _compile_check_bootstrap_api(
        config: &GoogleOAuthBootstrapConfig,
    ) -> Result<GoogleOAuthBootstrapResult, String> {
        bootstrap_google_oauth(config, 5).await
    }
}
