use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u64,
    pub content: String,
    pub completed: bool,
    pub created_at_epoch_secs: u64,
    pub completed_at_epoch_secs: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TodoState {
    next_id: u64,
    items: Vec<TodoItem>,
}

pub struct TodoStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl TodoStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub async fn add(&self, content: &str) -> Result<TodoItem, String> {
        let _guard = self.lock.lock().await;
        let mut state = self.load_state().await?;
        state.next_id += 1;

        let item = TodoItem {
            id: state.next_id,
            content: content.trim().to_string(),
            completed: false,
            created_at_epoch_secs: now_epoch_secs()?,
            completed_at_epoch_secs: None,
        };

        state.items.push(item.clone());
        self.save_state(&state).await?;
        Ok(item)
    }

    pub async fn list_open(&self) -> Result<Vec<TodoItem>, String> {
        let _guard = self.lock.lock().await;
        let state = self.load_state().await?;
        Ok(state
            .items
            .into_iter()
            .filter(|item| !item.completed)
            .collect())
    }

    pub async fn complete(&self, id: u64) -> Result<Option<TodoItem>, String> {
        let _guard = self.lock.lock().await;
        let mut state = self.load_state().await?;

        let mut completed = None;
        for item in &mut state.items {
            if item.id == id {
                item.completed = true;
                item.completed_at_epoch_secs = Some(now_epoch_secs()?);
                completed = Some(item.clone());
                break;
            }
        }

        if completed.is_some() {
            self.save_state(&state).await?;
        }

        Ok(completed)
    }

    async fn load_state(&self) -> Result<TodoState, String> {
        if !Path::new(&self.path).exists() {
            return Ok(TodoState::default());
        }

        let raw = fs::read_to_string(&self.path)
            .await
            .map_err(|e| format!("Failed to read todo store: {}", e))?;
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse todo store: {}", e))
    }

    async fn save_state(&self, state: &TodoState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create todo directory: {}", e))?;
        }

        let raw = serde_json::to_string_pretty(state)
            .map_err(|e| format!("Failed to serialize todo store: {}", e))?;
        fs::write(&self.path, raw)
            .await
            .map_err(|e| format!("Failed to write todo store: {}", e))
    }
}

fn now_epoch_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|e| format!("System time error: {}", e))
}
