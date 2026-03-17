use crate::agent::NoxAgent;
use crate::config::AppConfig;
use std::sync::Arc;
use teloxide::dispatching::Dispatcher;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, Message};

#[derive(Clone)]
pub struct TelegramChannel {
    bot: Bot,
    target_chat: ChatId,
    assistant_name: String,
}

impl TelegramChannel {
    pub fn new(config: &AppConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("Failed to create reqwest client");

        let bot = Bot::with_client(config.teloxide_token.clone(), client);

        Self {
            bot,
            target_chat: ChatId(config.chat_id),
            assistant_name: config.assistant_name.clone(),
        }
    }

    pub async fn start(self, agent: Arc<NoxAgent>) {
        let target_chat = self.target_chat;
        let assistant_name = self.assistant_name.clone();

        let handler = Update::filter_message().endpoint(move |bot: Bot, msg: Message| {
            let agent = agent.clone();
            let assistant_name = assistant_name.clone();

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
                    format!(
                        "{} is online.\n\nUse /todo <task>, /todos, /done <id>, /reset, or just chat normally.",
                        assistant_name
                    )
                } else if trimmed.eq_ignore_ascii_case("/help") {
                    format!(
                        "Commands:\n/start\n/help\n/reset\n/todo <task>\n/todos\n/done <id>\n\nAny other text is sent to {}.",
                        assistant_name
                    )
                } else if trimmed.eq_ignore_ascii_case("/reset") {
                    agent.clear_memory(msg.chat.id.0).await;
                    "Conversation memory cleared.".to_string()
                } else if let Some(todo_text) = trimmed.strip_prefix("/todo ") {
                    match agent.add_todo(todo_text.trim()).await {
                        Ok(resp) => resp.content,
                        Err(e) => format!("Assistant error: {}", e),
                    }
                } else if trimmed.eq_ignore_ascii_case("/todos") {
                    match agent.list_todos().await {
                        Ok(resp) => resp.content,
                        Err(e) => format!("Assistant error: {}", e),
                    }
                } else if let Some(id_text) = trimmed.strip_prefix("/done ") {
                    match id_text.trim().parse::<u64>() {
                        Ok(id) => match agent.complete_todo(id).await {
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

                    match agent.maybe_handle_todo_intent(trimmed).await {
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
}
