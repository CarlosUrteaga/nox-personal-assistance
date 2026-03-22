use crate::calendar::format::format_calendar_sync_summary;
use crate::calendar::service::CalendarSyncService;
use crate::config::AppConfig;
use crate::llm::ollama::{ConversationMessage, ConversationRole, OllamaClient};
use crate::runs::{
    MetadataItem, RunDraft, RunError, RunResult, RunTracker, StepDraft, StepKind, ToolTrace,
};
use crate::tools::todo::TodoStore;
use crate::tools::{DataType, ToolResponse};
use std::collections::HashMap;
use tokio::sync::Mutex;

pub struct AgentOrchestrator {
    config: AppConfig,
    ollama: OllamaClient,
    todos: TodoStore,
    memory: Mutex<HashMap<i64, Vec<ConversationMessage>>>,
    run_tracker: RunTracker,
}

impl AgentOrchestrator {
    pub fn new(config: AppConfig, run_tracker: RunTracker) -> Result<Self, String> {
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
            run_tracker,
        })
    }

    pub async fn chat(&self, chat_id: i64, user_message: &str) -> Result<ToolResponse, String> {
        let run_id = self.start_run(
            chat_id,
            "Telegram chat request",
            user_message,
            "Chat",
            vec![MetadataItem::new("request_type", "chat")],
        );
        let input_step = self.start_step(
            &run_id,
            StepKind::Input,
            "Request intake",
            "Captured the incoming Telegram message.",
        );
        self.run_tracker.finish_step(
            &run_id,
            &input_step,
            "Captured the incoming Telegram message.",
            "Stored the incoming message as the immutable run input before any planning or generation steps.",
            None,
            vec![MetadataItem::new("input_len", &user_message.len().to_string())],
        );

        let history = {
            let memory = self.memory.lock().await;
            memory.get(&chat_id).cloned().unwrap_or_default()
        };

        let planning_step = self.start_step(
            &run_id,
            StepKind::Planning,
            "Execution planning",
            "Prepared the conversation context and selected the chat path.",
        );
        self.run_tracker.finish_step(
            &run_id,
            &planning_step,
            "Prepared the conversation context and selected the chat path.",
            "Loaded in-memory conversation history and prepared the request for the Ollama chat model.",
            None,
            vec![MetadataItem::new("history_messages", &history.len().to_string())],
        );

        log::info!(
            "Processing chat message: chat_id={}, input_len={}, history_messages={}",
            chat_id,
            user_message.len(),
            history.len()
        );

        let output_step = self.start_step(
            &run_id,
            StepKind::Output,
            "Generate assistant response",
            "Calling Ollama to generate the final reply.",
        );
        let response = match self
            .ollama
            .chat(&self.config.system_prompt, &history, user_message)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &output_step,
                    "Assistant generation failed.",
                    "The Ollama request failed before a final response could be produced.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to generate an assistant response.",
                    RunError {
                        title: "Generation failed".to_string(),
                        message: err.clone(),
                        suggestion: "Check the Ollama server, model availability, and base URL configuration, then retry the request.".to_string(),
                    },
                    RunResult::text(
                        "Assistant response failed",
                        "The runtime could not complete the chat request.",
                        &err,
                    ),
                    Vec::new(),
                );
                return Err(err);
            }
        };

        log::info!(
            "Generated assistant response: chat_id={}, output_len={}",
            chat_id,
            response.len()
        );
        self.run_tracker.finish_step(
            &run_id,
            &output_step,
            "Generated the final assistant response.",
            "The non-streaming Ollama response was received successfully and converted into the final response payload.",
            None,
            vec![MetadataItem::new("output_len", &response.len().to_string())],
        );

        let validation_step = self.start_step(
            &run_id,
            StepKind::Validation,
            "Persist conversation memory",
            "Updating the in-memory conversation history.",
        );
        self.store_turn(chat_id, user_message, &response).await;
        self.run_tracker.finish_step(
            &run_id,
            &validation_step,
            "Conversation memory updated.",
            "Stored the user and assistant turns into the bounded in-memory history for this Telegram chat.",
            None,
            vec![MetadataItem::new("chat_id", &chat_id.to_string())],
        );

        let tool_response = ToolResponse {
            content: response.clone(),
            data_type: DataType::Text,
        };
        self.run_tracker.complete_run(
            &run_id,
            "Generated an assistant response and stored the updated chat context.",
            RunResult::text(
                "Assistant response ready",
                "The chat request completed successfully.",
                &response,
            ),
            vec![
                MetadataItem::new("assistant_name", &self.config.assistant_name),
                MetadataItem::new(
                    "history_limit",
                    &self.config.max_history_messages.to_string(),
                ),
            ],
        );

        Ok(tool_response)
    }

    pub async fn clear_memory(&self, chat_id: i64) {
        let mut memory = self.memory.lock().await;
        memory.remove(&chat_id);
        log::info!("Cleared conversation memory: chat_id={}", chat_id);
    }

    pub async fn add_todo(&self, chat_id: i64, content: &str) -> Result<ToolResponse, String> {
        let run_id = self.start_run(
            chat_id,
            "Add todo",
            content,
            "Command",
            vec![MetadataItem::new("request_type", "todo_add")],
        );
        let input_step = self.start_step(
            &run_id,
            StepKind::Input,
            "Request intake",
            "Captured the add-todo request.",
        );
        self.run_tracker.finish_step(
            &run_id,
            &input_step,
            "Captured the add-todo request.",
            "The incoming command was normalized into a todo creation request.",
            None,
            Vec::new(),
        );
        let tool_step = self.start_step(
            &run_id,
            StepKind::Tool,
            "Write todo",
            "Persisting the new todo item.",
        );
        let item = match self.todos.add(content).await {
            Ok(item) => item,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Failed to store the todo item.",
                    "The todo store write failed before the item could be persisted.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to create the requested todo item.",
                    RunError {
                        title: "Todo creation failed".to_string(),
                        message: err.clone(),
                        suggestion: "Check the todo store path and local filesystem permissions, then retry the command.".to_string(),
                    },
                    RunResult::text("Todo creation failed", "The todo item could not be saved.", &err),
                    Vec::new(),
                );
                return Err(err);
            }
        };
        let response = format!("Saved todo #{}: {}", item.id, item.content);
        log::info!(
            "Added todo: id={}, content_len={}",
            item.id,
            item.content.len()
        );
        self.run_tracker.finish_step(
            &run_id,
            &tool_step,
            "Todo stored successfully.",
            "The todo item was appended to the configured JSON store.",
            None,
            vec![
                MetadataItem::new("todo_id", &item.id.to_string()),
                MetadataItem::new("content_len", &item.content.len().to_string()),
            ],
        );
        let output_step = self.start_step(
            &run_id,
            StepKind::Output,
            "Render final response",
            "Building the final command response.",
        );
        self.run_tracker.finish_step(
            &run_id,
            &output_step,
            "Prepared the add-todo response.",
            "Formatted the persisted todo into a user-facing confirmation message.",
            None,
            Vec::new(),
        );
        self.run_tracker.complete_run(
            &run_id,
            "Created a todo item successfully.",
            RunResult::text("Todo created", "The todo item was saved.", &response),
            vec![MetadataItem::new(
                "todo_store_path",
                &self.config.todo_store_path,
            )],
        );

        Ok(ToolResponse {
            content: response,
            data_type: DataType::Text,
        })
    }

    pub async fn list_todos(&self, chat_id: i64) -> Result<ToolResponse, String> {
        let run_id = self.start_run(
            chat_id,
            "List todos",
            "List open todos",
            "Command",
            vec![MetadataItem::new("request_type", "todo_list")],
        );
        let tool_step = self.start_step(
            &run_id,
            StepKind::Tool,
            "Read todo store",
            "Loading open todos from storage.",
        );
        let items = match self.todos.list_open().await {
            Ok(items) => items,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Failed to read open todos.",
                    "The todo store could not be read for the list operation.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to list open todo items.",
                    RunError {
                        title: "Todo list failed".to_string(),
                        message: err.clone(),
                        suggestion: "Check the todo store path and local filesystem permissions, then retry the command.".to_string(),
                    },
                    RunResult::text("Todo list failed", "The todo list could not be loaded.", &err),
                    Vec::new(),
                );
                return Err(err);
            }
        };
        log::info!("Listing open todos: count={}", items.len());
        self.run_tracker.finish_step(
            &run_id,
            &tool_step,
            "Loaded open todos.",
            "Read the configured JSON store and filtered only open todo items.",
            None,
            vec![MetadataItem::new("open_todos", &items.len().to_string())],
        );

        let content = if items.is_empty() {
            "No open todos.".to_string()
        } else {
            let mut lines = vec!["Open todos:".to_string()];
            lines.extend(
                items
                    .into_iter()
                    .map(|item| format!("{}. {}", item.id, item.content)),
            );
            lines.join("\n")
        };
        let output_step = self.start_step(
            &run_id,
            StepKind::Output,
            "Format todo list",
            "Rendering the todo list response.",
        );
        self.run_tracker.finish_step(
            &run_id,
            &output_step,
            "Prepared the todo list response.",
            "Converted the filtered todo items into a compact response for Telegram.",
            None,
            Vec::new(),
        );
        self.run_tracker.complete_run(
            &run_id,
            "Listed open todo items.",
            RunResult::text(
                "Todo list ready",
                "The todo query completed successfully.",
                &content,
            ),
            Vec::new(),
        );

        Ok(ToolResponse {
            content,
            data_type: DataType::Text,
        })
    }

    pub async fn complete_todo(&self, chat_id: i64, id: u64) -> Result<ToolResponse, String> {
        let run_id = self.start_run(
            chat_id,
            "Complete todo",
            &format!("Complete todo {}", id),
            "Command",
            vec![MetadataItem::new("request_type", "todo_complete")],
        );
        let tool_step = self.start_step(
            &run_id,
            StepKind::Tool,
            "Complete todo item",
            "Marking the target todo as completed.",
        );
        let completion = match self.todos.complete(id).await {
            Ok(result) => result,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Failed to complete the todo item.",
                    "The todo store update failed before completion could be persisted.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to complete the requested todo item.",
                    RunError {
                        title: "Todo completion failed".to_string(),
                        message: err.clone(),
                        suggestion: "Check the todo store path and local filesystem permissions, then retry the command.".to_string(),
                    },
                    RunResult::text("Todo completion failed", "The todo item could not be updated.", &err),
                    Vec::new(),
                );
                return Err(err);
            }
        };
        match completion {
            Some(item) => {
                log::info!("Completed todo: id={}", item.id);
                self.run_tracker.finish_step(
                    &run_id,
                    &tool_step,
                    "Todo completed successfully.",
                    "The target todo item was found and marked as completed in the todo store.",
                    None,
                    vec![MetadataItem::new("todo_id", &item.id.to_string())],
                );
                let content = format!("Completed todo #{}: {}", item.id, item.content);
                self.run_tracker.complete_run(
                    &run_id,
                    "Marked the todo item as completed.",
                    RunResult::text("Todo completed", "The todo item was updated.", &content),
                    Vec::new(),
                );
                Ok(ToolResponse {
                    content,
                    data_type: DataType::Text,
                })
            }
            None => {
                let message = format!("Todo #{} was not found.", id);
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Todo item was not found.",
                    "The command targeted a todo id that does not exist in the current todo store.",
                    None,
                    vec![MetadataItem::new("todo_id", &id.to_string())],
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to complete the requested todo item.",
                    RunError {
                        title: "Todo not found".to_string(),
                        message: message.clone(),
                        suggestion: "Use /todos to inspect the current open ids, then retry with a valid numeric id.".to_string(),
                    },
                    RunResult::text("Todo completion failed", "The requested todo item was not found.", &message),
                    Vec::new(),
                );
                Err(message)
            }
        }
    }

    pub async fn calendar_sync(&self, chat_id: i64) -> Result<ToolResponse, String> {
        let run_id = self.start_run(
            chat_id,
            "Calendar sync",
            "Execute calendar sync",
            "Command",
            vec![MetadataItem::new("request_type", "calendar_sync")],
        );
        let tool_step = self.start_step(
            &run_id,
            StepKind::Tool,
            "Execute calendar sync",
            "Running the calendar synchronization service.",
        );
        let service = match CalendarSyncService::new(self.config.clone()) {
            Ok(service) => service,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Calendar sync setup failed.",
                    "The synchronization service could not be initialized from the current configuration.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Failed to initialize calendar sync.",
                    RunError {
                        title: "Calendar sync failed".to_string(),
                        message: err.clone(),
                        suggestion: "Review the calendar-related env values and retry once the configuration is complete.".to_string(),
                    },
                    RunResult::text("Calendar sync failed", "The sync service could not be initialized.", &err),
                    Vec::new(),
                );
                return Err(err);
            }
        };
        let outcome = match service.sync_once().await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.run_tracker.fail_step(
                    &run_id,
                    &tool_step,
                    "Calendar sync execution failed.",
                    "The synchronization service returned an error before producing the final summary.",
                    None,
                    Vec::new(),
                );
                self.run_tracker.fail_run(
                    &run_id,
                    "Calendar sync failed during execution.",
                    RunError {
                        title: "Calendar sync failed".to_string(),
                        message: err.clone(),
                        suggestion: "Inspect the configured sources, destination calendar settings, and network connectivity, then retry.".to_string(),
                    },
                    RunResult::text("Calendar sync failed", "The synchronization run ended with an error.", &err),
                    Vec::new(),
                );
                return Err(err);
            }
        };
        let content = format_calendar_sync_summary(&outcome);
        self.run_tracker.finish_step(
            &run_id,
            &tool_step,
            "Calendar sync completed.",
            "Fetched configured calendar sources and produced the summarized blocker reconciliation result.",
            Some(ToolTrace {
                label: "calendar.sync.summary".to_string(),
                payload: content.clone(),
            }),
            vec![
                MetadataItem::new("source_events", &outcome.source_events.to_string()),
                MetadataItem::new("blockers", &outcome.blockers.to_string()),
            ],
        );
        self.run_tracker.complete_run(
            &run_id,
            "Calendar sync completed successfully.",
            RunResult::text(
                "Calendar sync ready",
                "The synchronization run finished.",
                &content,
            ),
            Vec::new(),
        );

        Ok(ToolResponse {
            content,
            data_type: DataType::Text,
        })
    }

    pub async fn maybe_handle_todo_intent(
        &self,
        chat_id: i64,
        user_message: &str,
    ) -> Result<Option<ToolResponse>, String> {
        let normalized = user_message.trim();
        let lowered = normalized.to_ascii_lowercase();

        if is_list_todo_request(&lowered) {
            return self.list_todos(chat_id).await.map(Some);
        }

        if let Some(id) = parse_complete_todo_id(normalized, &lowered) {
            return self.complete_todo(chat_id, id).await.map(Some);
        }

        if let Some(content) = parse_add_todo_content(normalized, &lowered) {
            return self.add_todo(chat_id, &content).await.map(Some);
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

    fn start_run(
        &self,
        chat_id: i64,
        title: &str,
        request_text: &str,
        conversation_mode: &str,
        mut metadata: Vec<MetadataItem>,
    ) -> String {
        metadata.push(MetadataItem::new("chat_id", &chat_id.to_string()));
        metadata.push(MetadataItem::new("source", "telegram"));
        self.run_tracker.start_run(RunDraft {
            request_title: title.to_string(),
            request_text: request_text.to_string(),
            summary: "Run started.".to_string(),
            conversation_mode: conversation_mode.to_string(),
            model: self.config.ollama_model.clone(),
            channel: "Telegram".to_string(),
            metadata,
        })
    }

    fn start_step(&self, run_id: &str, kind: StepKind, title: &str, summary: &str) -> String {
        self.run_tracker.start_step(
            run_id,
            StepDraft {
                kind,
                title: title.to_string(),
                summary: summary.to_string(),
            },
        )
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
