use std::env;
use crate::tools::gemini::run_gemini_command;
use super::{ToolResponse, DataType, EmailDetails};

pub async fn check_new_emails() -> Result<Option<ToolResponse>, String> {
    let prompt = "Search for any new unread emails received in the last 30 minutes. \
                  If found, provide a very concise summary (Sender and Subject) for the most recent one. \
                  Return JSON with the summary in the 'response' field or 'No new emails' if none.";
    
    match run_gemini_command(prompt)? {
        Some(response) => {
            // Simple parsing or just returning text for now
            Ok(Some(ToolResponse {
                content: response.clone(),
                data_type: DataType::EmailSummary(EmailDetails {
                    sender: "Unknown".to_string(), 
                    subject: response,
                    snippet: "".to_string(),
                }),
            }))
        },
        None => Ok(None),
    }
}

pub async fn sync_invitations() -> Result<Option<ToolResponse>, String> {
    let priority_emails_raw = env::var("PRIORITY_EMAILS").unwrap_or_default();
    if priority_emails_raw.is_empty() {
        return Err("PRIORITY_EMAILS not set.".to_string());
    }

    let prompt = format!(
        "Search for unread and recently received emails containing calendar invitations (ICS files), meeting invites, or plain-text schedules/meeting details. \
        Priority list: {}. \
        Instructions: \
        1. MANDATORY ACTION: Use 'calendar:create-event' to sync the event. \
        2. Add the target priority emails as 'attendees' to the event to ensure they receive a formal invitation. \
        3. Target logic: If sender is NOT in the priority list -> Add ALL priority emails as attendees. If sender IS in the priority list -> Add ONLY the missing email(s) as attendees. \
        4. Ensure the event summary, time, and description (including meeting links/passcodes) are accurate. \
        5. FINAL CRITICAL STEP: Use 'gmail:modify' to remove the 'UNREAD' label from the processed email(s) to prevent duplication in next heartbeat. \
        Final Output: Return a JSON with a single string in 'response' field summarizing the ACTION taken or 'No sync needed'." ,
        priority_emails_raw
    );

    match run_gemini_command(&prompt)? {
        Some(response) => Ok(Some(ToolResponse {
            content: response,
            data_type: DataType::Text,
        })),
        None => Ok(None),
    }
}
