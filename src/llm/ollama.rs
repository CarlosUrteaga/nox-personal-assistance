use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::time::Instant;

#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
    num_predict: u32,
    client: Client,
}

impl OllamaClient {
    pub fn new(
        base_url: String,
        model: String,
        timeout_secs: u64,
        num_predict: u32,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("Failed to build Ollama HTTP client: {}", e))?;

        Ok(Self {
            base_url,
            model,
            num_predict,
            client,
        })
    }

    pub async fn chat(
        &self,
        system_prompt: &str,
        history: &[ConversationMessage],
        user_message: &str,
    ) -> Result<String, String> {
        log::info!(
            "Sending Ollama chat request: model={}, history_messages={}, user_message_len={}, num_predict={}",
            self.model,
            history.len(),
            user_message.len(),
            self.num_predict
        );
        let started_at = Instant::now();

        let mut messages = Vec::with_capacity(history.len() + 2);
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
        });
        messages.extend(history.iter().map(|message| ChatMessage {
            role: message.role.as_ollama_role().to_string(),
            content: message.content.clone(),
        }));
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: user_message.to_string(),
        });

        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            options: Some(ChatOptions {
                temperature: Some(0.2),
                num_predict: Some(self.num_predict),
            }),
        };

        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Ollama request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unavailable response body>".to_string());
            log::error!("Ollama error response: status={}, body={}", status, body);
            return Err(format!("Ollama returned HTTP {}: {}", status, body));
        }

        let body: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

        let content = body.message.content.trim().to_string();
        if content.is_empty() {
            log::error!("Ollama returned an empty response body");
            return Err("Ollama returned an empty response".to_string());
        }

        log::info!(
            "Received Ollama response: content_len={}, elapsed_ms={}",
            content.len(),
            started_at.elapsed().as_millis()
        );

        Ok(content)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRole {
    User,
    Assistant,
}

impl ConversationRole {
    fn as_ollama_role(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: ConversationRole,
    pub content: String,
}

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: Option<ChatOptions>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: Option<f32>,
    num_predict: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaMessage,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::OllamaChatResponse;

    #[test]
    fn parses_chat_response() {
        let json = r#"{"message":{"content":"hello"}} "#;
        let parsed: OllamaChatResponse = serde_json::from_str(json).expect("valid response");
        assert_eq!(parsed.message.content, "hello");
    }

    #[test]
    fn malformed_response_fails() {
        let json = r#"{"message":{"bad":"x"}}"#;
        let parsed = serde_json::from_str::<OllamaChatResponse>(json);
        assert!(parsed.is_err());
    }
}
