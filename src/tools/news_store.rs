use crate::tools::news::NewsBrief;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NewsWindowStatus {
    Prepared,
    Sent,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FailureStage {
    Generation,
    Delivery,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceHealth {
    pub last_fetch_attempt_at_epoch_secs: Option<u64>,
    pub last_success_at_epoch_secs: Option<u64>,
    pub last_item_count: Option<usize>,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredWindow {
    pub status: NewsWindowStatus,
    pub brief: Option<NewsBrief>,
    pub prepared_artifact_hash: Option<String>,
    pub skipped_reason: Option<String>,
    pub failure_stage: Option<FailureStage>,
    pub failure_reason: Option<String>,
    pub updated_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewsBriefState {
    pub windows: BTreeMap<String, StoredWindow>,
    pub source_health: BTreeMap<String, SourceHealth>,
}

pub struct NewsBriefStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl NewsBriefStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub async fn get_window(&self, window_key: &str) -> Result<Option<StoredWindow>, String> {
        let _guard = self.lock.lock().await;
        Ok(self
            .load_state_unlocked()
            .await?
            .windows
            .get(window_key)
            .cloned())
    }

    pub async fn save_prepared(&self, window_key: &str, brief: NewsBrief) -> Result<(), String> {
        self.update_state(|state| {
            state.windows.insert(
                window_key.to_string(),
                StoredWindow {
                    status: NewsWindowStatus::Prepared,
                    prepared_artifact_hash: Some(brief.prepared_artifact_hash.clone()),
                    brief: Some(brief),
                    skipped_reason: None,
                    failure_stage: None,
                    failure_reason: None,
                    updated_at_epoch_secs: now_epoch_secs(),
                },
            );
        })
        .await
    }

    pub async fn mark_sent(&self, window_key: &str) -> Result<(), String> {
        self.update_state(|state| {
            if let Some(window) = state.windows.get_mut(window_key) {
                window.status = NewsWindowStatus::Sent;
                window.updated_at_epoch_secs = now_epoch_secs();
            }
        })
        .await
    }

    pub async fn mark_skipped(&self, window_key: &str, reason: &str) -> Result<(), String> {
        self.update_state(|state| {
            state.windows.insert(
                window_key.to_string(),
                StoredWindow {
                    status: NewsWindowStatus::Skipped,
                    prepared_artifact_hash: None,
                    brief: None,
                    skipped_reason: Some(reason.to_string()),
                    failure_stage: None,
                    failure_reason: None,
                    updated_at_epoch_secs: now_epoch_secs(),
                },
            );
        })
        .await
    }

    pub async fn mark_failed(
        &self,
        window_key: &str,
        stage: FailureStage,
        reason: &str,
    ) -> Result<(), String> {
        self.update_state(|state| {
            let existing = state.windows.get(window_key).cloned();
            let brief = existing.as_ref().and_then(|window| window.brief.clone());
            let prepared_artifact_hash = existing
                .as_ref()
                .and_then(|window| window.prepared_artifact_hash.clone());
            state.windows.insert(
                window_key.to_string(),
                StoredWindow {
                    status: NewsWindowStatus::Failed,
                    brief,
                    prepared_artifact_hash,
                    skipped_reason: None,
                    failure_stage: Some(stage),
                    failure_reason: Some(reason.to_string()),
                    updated_at_epoch_secs: now_epoch_secs(),
                },
            );
        })
        .await
    }

    pub async fn record_source_fetch_success(
        &self,
        source_id: &str,
        item_count: usize,
    ) -> Result<(), String> {
        self.update_state(|state| {
            let health = state.source_health.entry(source_id.to_string()).or_default();
            health.last_fetch_attempt_at_epoch_secs = Some(now_epoch_secs());
            health.last_success_at_epoch_secs = Some(now_epoch_secs());
            health.last_item_count = Some(item_count);
            health.consecutive_failures = 0;
        })
        .await
    }

    pub async fn record_source_fetch_failure(&self, source_id: &str) -> Result<(), String> {
        self.update_state(|state| {
            let health = state.source_health.entry(source_id.to_string()).or_default();
            health.last_fetch_attempt_at_epoch_secs = Some(now_epoch_secs());
            health.consecutive_failures = health.consecutive_failures.saturating_add(1);
        })
        .await
    }

    async fn update_state(
        &self,
        mutator: impl FnOnce(&mut NewsBriefState),
    ) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let mut state = self.load_state_unlocked().await?;
        mutator(&mut state);
        self.save_state_unlocked(&state).await
    }

    async fn load_state_unlocked(&self) -> Result<NewsBriefState, String> {
        if !Path::new(&self.path).exists() {
            return Ok(NewsBriefState::default());
        }

        let raw = fs::read_to_string(&self.path)
            .await
            .map_err(|err| format!("Failed to read news brief store: {}", err))?;
        serde_json::from_str(&raw).map_err(|err| format!("Failed to parse news brief store: {}", err))
    }

    async fn save_state_unlocked(&self, state: &NewsBriefState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|err| format!("Failed to create news brief directory: {}", err))?;
        }
        let raw = serde_json::to_string_pretty(state)
            .map_err(|err| format!("Failed to serialize news brief store: {}", err))?;
        fs::write(&self.path, raw)
            .await
            .map_err(|err| format!("Failed to write news brief store: {}", err))
    }
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{FailureStage, NewsBriefStore, NewsWindowStatus};
    use crate::tools::news::{NewsBrief, NewsBriefMetrics};

    #[tokio::test]
    async fn preserves_prepared_artifact_on_delivery_failure() {
        let path = format!("/tmp/news-brief-store-{}.json", std::process::id());
        let _ = tokio::fs::remove_file(&path).await;
        let store = NewsBriefStore::new(&path);
        store
            .save_prepared(
                "2026-04-02 15:00",
                NewsBrief {
                    window_key: "2026-04-02 15:00".to_string(),
                    window_epoch_secs: 1,
                    generated_at_epoch_secs: 2,
                    prepared_artifact_hash: "hash".to_string(),
                    metrics: NewsBriefMetrics::default(),
                    items: Vec::new(),
                },
            )
            .await
            .expect("prepared");
        store
            .mark_failed("2026-04-02 15:00", FailureStage::Delivery, "telegram failed")
            .await
            .expect("failed");

        let window = store
            .get_window("2026-04-02 15:00")
            .await
            .expect("window")
            .expect("present");
        assert_eq!(window.status, NewsWindowStatus::Failed);
        assert!(window.brief.is_some());
        assert_eq!(window.failure_stage, Some(FailureStage::Delivery));
        let _ = tokio::fs::remove_file(&path).await;
    }
}
