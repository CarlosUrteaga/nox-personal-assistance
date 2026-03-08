use std::env;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use crate::agent::NoxAgent;
use crate::tools::{ToolResponse, DataType};

#[derive(Clone)]
pub struct TelegramChannel {
    bot: Bot,
    target_chat: ChatId,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "NOX Assistant Commands")]
enum CommandList {
    #[command(description = "Display this help message")]
    Help,
    #[command(description = "Start the NOX Assistant")]
    Start,
    #[command(description = "Get today's calendar schedule")]
    Calendar,
    #[command(description = "Scan recent emails for invitations/schedules")]
    Email,
}

impl TelegramChannel {
    pub fn new() -> Self {
        let token = env::var("TELOXIDE_TOKEN").expect("TELOXIDE_TOKEN must be set");
        let chat_id = env::var("CHAT_ID").expect("CHAT_ID must be set")
            .parse::<i64>().expect("CHAT_ID must be an integer");
        
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("Failed to create reqwest client");

        let bot = Bot::with_client(token, client);

        Self {
            bot,
            target_chat: ChatId(chat_id),
        }
    }

    pub async fn send_message(&self, content: &str) -> Result<Message, teloxide::RequestError> {
        self.bot.send_message(self.target_chat, content).await
    }

    pub async fn send_tool_response(&self, response: ToolResponse) -> Result<Message, teloxide::RequestError> {
        // Format based on DataType
        let text = match response.data_type {
            DataType::Text => response.content,
            DataType::Markdown => response.content, // Teloxide default is text, might need parse_mode
            DataType::EmailSummary(details) => format!("📧 New Email from {}: {}", details.sender, details.subject),
            DataType::CalendarEvent(details) => format!("📅 Event Created: {}\nTime: {} - {}", details.summary, details.start_time, details.end_time),
        };
        self.bot.send_message(self.target_chat, text).await
    }

    pub async fn start(self, agent: Arc<NoxAgent>) {
        let handler = Update::filter_message()
            .filter_command::<CommandList>()
            .endpoint(move |bot: Bot, msg: Message, cmd: CommandList| {
                let agent = agent.clone();
                async move {
                    match cmd {
                        CommandList::Help => {
                            bot.send_message(msg.chat.id, CommandList::descriptions().to_string()).await?;
                        }
                        CommandList::Start => {
                            bot.send_message(msg.chat.id, "🌙 NOX System Online. Use /help.").await?;
                        }
                        CommandList::Calendar => {
                            bot.send_message(msg.chat.id, "📅 Fetching schedule...").await?;
                            match agent.handle_command("calendar").await {
                                Ok(resp) => { bot.send_message(msg.chat.id, resp.content).await?; }
                                Err(e) => { bot.send_message(msg.chat.id, format!("⚠️ Error: {}", e)).await?; }
                            }
                        }
                        CommandList::Email => {
                            bot.send_message(msg.chat.id, "📧 Scanning emails...").await?;
                            match agent.handle_command("email").await {
                                Ok(resp) => { bot.send_message(msg.chat.id, resp.content).await?; }
                                Err(e) => { bot.send_message(msg.chat.id, format!("⚠️ Error: {}", e)).await?; }
                            }
                        }
                    }
                    Ok::<(), teloxide::RequestError>(())
                }
            });

        Dispatcher::builder(self.bot.clone(), handler)
            .build()
            .dispatch()
            .await;
    }
}
