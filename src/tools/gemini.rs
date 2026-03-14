use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GeminiSessionOutput {
    pub session_id: String,
    pub response: String,
}

pub fn run_gemini_command(prompt: &str) -> Result<Option<String>, String> {
    let output = Command::new("gemini")
        .args(&["-p", prompt, "-o", "json", "-y"])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Gemini CLI command failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').ok_or("No JSON found in output")?;
    let result: GeminiSessionOutput = serde_json::from_str(&stdout[json_start..]).map_err(|e| e.to_string())?;
    
    let resp_lower = result.response.to_lowercase();
    
    if resp_lower.contains("insufficient authentication scopes") 
        || resp_lower.contains("permission denied")
        || resp_lower.contains("scope") {
        return Err("Insufficient Google Workspace permissions. Please run 'gemini login' manually.".to_string());
    }

    if result.response.is_empty() 
        || resp_lower.contains("no events") 
        || resp_lower.contains("no messages")
        || resp_lower.contains("no new emails")
        || resp_lower.contains("no sync needed") {
        Ok(None)
    } else {
        Ok(Some(result.response))
    }
}
