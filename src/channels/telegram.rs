use crate::agent::NoxAgent;
use crate::config::AppConfig;
use crate::runs::{MetadataItem, RunDraft, RunResult, RunTracker, StepDraft, StepKind};
use std::sync::Arc;
use teloxide::dispatching::Dispatcher;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, Message};

#[derive(Clone)]
pub struct TelegramChannel {
    bot: Bot,
    target_chat: ChatId,
    assistant_name: String,
    run_tracker: RunTracker,
    model_name: String,
}

impl TelegramChannel {
    pub fn new(config: &AppConfig, run_tracker: RunTracker) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("Failed to create reqwest client");

        let bot = Bot::with_client(config.teloxide_token.clone(), client);

        Self {
            bot,
            target_chat: ChatId(config.chat_id),
            assistant_name: config.assistant_name.clone(),
            run_tracker,
            model_name: config.ollama_model.clone(),
        }
    }

    pub async fn start(self, agent: Arc<NoxAgent>) {
        let target_chat = self.target_chat;
        let assistant_name = self.assistant_name.clone();
        let run_tracker = self.run_tracker.clone();
        let model_name = self.model_name.clone();

        let handler = Update::filter_message().endpoint(move |bot: Bot, msg: Message| {
            let agent = agent.clone();
            let assistant_name = assistant_name.clone();
            let run_tracker = run_tracker.clone();
            let model_name = model_name.clone();

            async move {
                log::info!(
                    "Received Telegram message: chat_id={}, has_text={}",
                    msg.chat.id.0,
                    msg.text().is_some()
                );

                if msg.chat.id != target_chat {
                    log::warn!(
                        "Ignoring message from unauthorized chat: incoming_chat_id={}, expected_chat_id={}",
                        msg.chat.id.0,
                        target_chat.0
                    );
                    return Ok::<(), teloxide::RequestError>(());
                }

                let Some(text) = msg.text() else {
                    bot.send_message(msg.chat.id, "Send text messages for now.").await?;
                    return Ok::<(), teloxide::RequestError>(());
                };

                let trimmed = text.trim();
                let response = if trimmed.eq_ignore_ascii_case("/start") {
                    let content = format!(
                        "{} is online.\n\nUse /todo <task>, /todos, /done <id>, /calendar-sync, /reset, or just chat normally.",
                        assistant_name
                    );
                    record_telegram_text_run(
                        &run_tracker,
                        &model_name,
                        msg.chat.id.0,
                        "Start command",
                        trimmed,
                        "Command",
                        &content,
                    );
                    content
                } else if trimmed.eq_ignore_ascii_case("/help") {
                    let content = format!(
                        "Commands:\n/start\n/help\n/reset\n/todo <task>\n/todos\n/done <id>\n/calendar-sync\n\nAny other text is sent to {}.",
                        assistant_name
                    );
                    record_telegram_text_run(
                        &run_tracker,
                        &model_name,
                        msg.chat.id.0,
                        "Help command",
                        trimmed,
                        "Command",
                        &content,
                    );
                    content
                } else if trimmed.eq_ignore_ascii_case("/reset") {
                    agent.clear_memory(msg.chat.id.0).await;
                    let content = "Conversation memory cleared.".to_string();
                    record_telegram_text_run(
                        &run_tracker,
                        &model_name,
                        msg.chat.id.0,
                        "Reset conversation",
                        trimmed,
                        "Command",
                        &content,
                    );
                    content
                } else if trimmed.eq_ignore_ascii_case("/calendar-sync") {
                    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;
                    match agent.calendar_sync(msg.chat.id.0).await {
                        Ok(resp) => resp.content,
                        Err(e) => format!("Calendar sync error: {}", e),
                    }
                } else if let Some(todo_text) = trimmed.strip_prefix("/todo ") {
                    match agent.add_todo(msg.chat.id.0, todo_text.trim()).await {
                        Ok(resp) => resp.content,
                        Err(e) => format!("Assistant error: {}", e),
                    }
                } else if trimmed.eq_ignore_ascii_case("/todos") {
                    match agent.list_todos(msg.chat.id.0).await {
                        Ok(resp) => resp.content,
                        Err(e) => format!("Assistant error: {}", e),
                    }
                } else if let Some(id_text) = trimmed.strip_prefix("/done ") {
                    match id_text.trim().parse::<u64>() {
                        Ok(id) => match agent.complete_todo(msg.chat.id.0, id).await {
                            Ok(resp) => resp.content,
                            Err(e) => format!("Assistant error: {}", e),
                        },
                        Err(_) => "Usage: /done <numeric id>".to_string(),
                    }
                } else {
                    log::info!(
                        "Forwarding Telegram text to agent: chat_id={}, text='{}'",
                        msg.chat.id.0,
                        trimmed
                    );

                    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;

                    match agent.maybe_handle_todo_intent(msg.chat.id.0, trimmed).await {
                        Ok(Some(resp)) => resp.content,
                        Ok(None) => match agent.chat(msg.chat.id.0, trimmed).await {
                            Ok(resp) => resp.content,
                            Err(e) => format!("Assistant error: {}", e),
                        },
                        Err(e) => format!("Assistant error: {}", e),
                    }
                };

                log::info!(
                    "Sending Telegram response: chat_id={}, response_len={}",
                    msg.chat.id.0,
                    response.len()
                );
                bot.send_message(msg.chat.id, response).await?;

                Ok::<(), teloxide::RequestError>(())
            }
        });

        Dispatcher::builder(self.bot.clone(), handler)
            .build()
            .dispatch()
            .await;
    }

    pub async fn send_system_message(&self, message: &str) -> Result<(), String> {
        self.bot
            .send_message(self.target_chat, message.to_string())
            .await
            .map(|_| ())
            .map_err(|e| format!("Failed to send Telegram system message: {}", e))
    }
}

fn record_telegram_text_run(
    run_tracker: &RunTracker,
    model_name: &str,
    chat_id: i64,
    title: &str,
    request_text: &str,
    mode: &str,
    content: &str,
) {
    let run_id = run_tracker.start_run(RunDraft {
        request_title: title.to_string(),
        request_text: request_text.to_string(),
        summary: "Command completed.".to_string(),
        conversation_mode: mode.to_string(),
        model: model_name.to_string(),
        channel: "Telegram".to_string(),
        metadata: vec![
            MetadataItem::new("chat_id", &chat_id.to_string()),
            MetadataItem::new("source", "telegram"),
            MetadataItem::new("request_type", "command"),
        ],
    });
    let step_id = run_tracker.start_step(
        &run_id,
        StepDraft {
            kind: StepKind::Output,
            title: "Render command response".to_string(),
            summary: "Generated the final command response.".to_string(),
        },
    );
    run_tracker.finish_step(
        &run_id,
        &step_id,
        "Generated the final command response.",
        "The command was handled directly by the Telegram layer and converted into a final text response.",
        None,
        Vec::new(),
    );
    run_tracker.complete_run(
        &run_id,
        "Handled the Telegram command successfully.",
        RunResult::text(
            "Command response ready",
            "The command completed successfully.",
            content,
        ),
        Vec::new(),
    );
}
