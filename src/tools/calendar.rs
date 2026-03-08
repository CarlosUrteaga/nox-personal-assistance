use crate::tools::gemini::run_gemini_command;
use crate::tools::{ToolResponse, DataType};

pub async fn fetch_calendar_summary() -> Result<Option<ToolResponse>, String> {
    let prompt = "/calendar:get-schedule today";
    
    match run_gemini_command(prompt)? {
        Some(response) => Ok(Some(ToolResponse {
            content: response.clone(),
            data_type: DataType::Text, // Could be parsed into CalendarEvent if structure was consistent
        })),
        None => Ok(None),
    }
}
