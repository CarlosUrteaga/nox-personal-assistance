use crate::calendar::format::format_calendar_sync_summary;
use crate::config::AppConfig;
use crate::calendar::service::CalendarSyncService;
use crate::llm::ollama::{ConversationMessage, ConversationRole, OllamaClient};
use crate::tools::todo::TodoStore;
use crate::tools::{DataType, ToolResponse};
use std::collections::HashMap;
use tokio::sync::Mutex;

pub struct AgentOrchestrator {
    config: AppConfig,
    ollama: OllamaClient,
    todos: TodoStore,
    memory: Mutex<HashMap<i64, Vec<ConversationMessage>>>,
}

impl AgentOrchestrator {
    pub fn new(config: AppConfig) -> Result<Self, String> {
        let ollama = OllamaClient::new(
            config.ollama_base_url.clone(),
            config.ollama_model.clone(),
            config.ollama_timeout_secs,
            config.ollama_num_predict,
        )?;

        Ok(Self {
            todos: TodoStore::new(config.todo_store_path.clone()),
            config,
            ollama,
            memory: Mutex::new(HashMap::new()),
        })
    }

    pub async fn chat(&self, chat_id: i64, user_message: &str) -> Result<ToolResponse, String> {
        let history = {
            let memory = self.memory.lock().await;
            memory.get(&chat_id).cloned().unwrap_or_default()
        };

        log::info!(
            "Processing chat message: chat_id={}, input_len={}, history_messages={}",
            chat_id,
            user_message.len(),
            history.len()
        );

        let response = self
            .ollama
            .chat(&self.config.system_prompt, &history, user_message)
            .await?;

        log::info!(
            "Generated assistant response: chat_id={}, output_len={}",
            chat_id,
            response.len()
        );

        self.store_turn(chat_id, user_message, &response).await;

        Ok(ToolResponse {
            content: response,
            data_type: DataType::Text,
        })
    }

    pub async fn clear_memory(&self, chat_id: i64) {
        let mut memory = self.memory.lock().await;
        memory.remove(&chat_id);
        log::info!("Cleared conversation memory: chat_id={}", chat_id);
    }

    pub async fn add_todo(&self, content: &str) -> Result<ToolResponse, String> {
        let item = self.todos.add(content).await?;
        let response = format!("Saved todo #{}: {}", item.id, item.content);
        log::info!("Added todo: id={}, content_len={}", item.id, item.content.len());

        Ok(ToolResponse {
            content: response,
            data_type: DataType::Text,
        })
    }

    pub async fn list_todos(&self) -> Result<ToolResponse, String> {
        let items = self.todos.list_open().await?;
        log::info!("Listing open todos: count={}", items.len());

        let content = if items.is_empty() {
            "No open todos.".to_string()
        } else {
            let mut lines = vec!["Open todos:".to_string()];
            lines.extend(
                items.into_iter()
                    .map(|item| format!("{}. {}", item.id, item.content)),
            );
            lines.join("\n")
        };

        Ok(ToolResponse {
            content,
            data_type: DataType::Text,
        })
    }

    pub async fn complete_todo(&self, id: u64) -> Result<ToolResponse, String> {
        match self.todos.complete(id).await? {
            Some(item) => {
                log::info!("Completed todo: id={}", item.id);
                Ok(ToolResponse {
                    content: format!("Completed todo #{}: {}", item.id, item.content),
                    data_type: DataType::Text,
                })
            }
            None => Err(format!("Todo #{} was not found.", id)),
        }
    }

    pub async fn calendar_sync(&self) -> Result<ToolResponse, String> {
        let service = CalendarSyncService::new(self.config.clone())?;
        let outcome = service.sync_once().await?;
        let content = format_calendar_sync_summary(&outcome);

        Ok(ToolResponse {
            content,
            data_type: DataType::Text,
        })
    }

    pub async fn maybe_handle_todo_intent(
        &self,
        user_message: &str,
    ) -> Result<Option<ToolResponse>, String> {
        let normalized = user_message.trim();
        let lowered = normalized.to_ascii_lowercase();

        if is_list_todo_request(&lowered) {
            return self.list_todos().await.map(Some);
        }

        if let Some(id) = parse_complete_todo_id(normalized, &lowered) {
            return self.complete_todo(id).await.map(Some);
        }

        if let Some(content) = parse_add_todo_content(normalized, &lowered) {
            return self.add_todo(&content).await.map(Some);
        }

        Ok(None)
    }

    async fn store_turn(&self, chat_id: i64, user_message: &str, assistant_message: &str) {
        let mut memory = self.memory.lock().await;
        let entry = memory.entry(chat_id).or_default();
        entry.push(ConversationMessage {
            role: ConversationRole::User,
            content: user_message.to_string(),
        });
        entry.push(ConversationMessage {
            role: ConversationRole::Assistant,
            content: assistant_message.to_string(),
        });

        let max_messages = self.config.max_history_messages.max(2);
        if entry.len() > max_messages {
            let drop_count = entry.len() - max_messages;
            entry.drain(0..drop_count);
        }
    }
}

fn is_list_todo_request(lowered: &str) -> bool {
    if [
        "show my todo list",
        "show my todos",
        "show todos",
        "list my todos",
        "list todos",
        "what are my todos",
        "what's on my todo list",
        "whats on my todo list",
        "give me my todo list",
        "give me my todos",
        "todo list",
        "todos",
    ]
    .iter()
    .any(|pattern| lowered == *pattern)
    {
        return true;
    }

    (lowered.contains("todo list") || lowered.contains("to do list"))
        && ["show", "give", "list", "display", "what", "see"]
            .iter()
            .any(|verb| lowered.contains(verb))
}

fn parse_complete_todo_id(original: &str, lowered: &str) -> Option<u64> {
    let prefixes = [
        "complete todo ",
        "complete task ",
        "complete ",
        "finish todo ",
        "finish task ",
        "finish ",
        "done todo ",
        "done task ",
        "done ",
        "mark todo ",
        "mark task ",
    ];

    for prefix in prefixes {
        if let Some(rest) = lowered.strip_prefix(prefix) {
            let candidate = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok());
            if candidate.is_some() {
                return candidate;
            }
        }
    }

    if let Some(rest) = original.trim().strip_prefix("/done ") {
        return rest.trim().parse::<u64>().ok();
    }

    None
}

fn parse_add_todo_content(original: &str, lowered: &str) -> Option<String> {
    let prefixes = [
        "/todo ",
        "add todo ",
        "add to do ",
        "create todo ",
        "create to do ",
        "new todo ",
        "new to do ",
        "add task ",
        "create task ",
        "new task ",
        "remember to ",
        "todo ",
        "to do ",
        "task ",
    ];

    for prefix in prefixes {
        if lowered.starts_with(prefix) {
            let content = original[prefix.len()..].trim();
            if !content.is_empty() {
                return Some(content.to_string());
            }
        }
    }

    for phrase in [
        "add a todo to ",
        "add a to do to ",
        "create a todo to ",
        "create a to do to ",
        "create a todo ",
        "create a to do ",
        "add a task to ",
        "create a task to ",
        "add to my todo list ",
        "put on my todo list ",
    ] {
        if let Some(start) = lowered.find(phrase) {
            let content_start = start + phrase.len();
            let content = original[content_start..].trim();
            if !content.is_empty() {
                return Some(content.to_string());
            }
        }
    }

    None
}
