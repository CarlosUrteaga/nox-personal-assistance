use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
    client: Client,
}

impl OllamaClient {
    pub fn new(base_url: String, model: String, timeout_secs: u64) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("Failed to build Ollama HTTP client: {}", e))?;

        Ok(Self {
            base_url,
            model,
            client,
        })
    }

    pub async fn summarize_workspace_result(
        &self,
        task_kind: &str,
        raw_payload: &str,
    ) -> Result<String, String> {
        let prompt = format!(
            "You are formatting operational output for a personal assistant heartbeat. \
Task: {}. \
Return one concise end-user message in plain text. \
Do not mention internal tools or JSON parsing. \
If input indicates no action, answer with: No updates. \
Input: {}",
            task_kind, raw_payload
        );

        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt,
            }],
            stream: false,
            options: Some(ChatOptions {
                temperature: Some(0.0),
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
            return Err(format!("Ollama returned HTTP {}", response.status()));
        }

        let body: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

        let content = body.message.content.trim().to_string();
        if content.is_empty() {
            return Err("Ollama returned an empty response".to_string());
        }

        Ok(content)
    }
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
