use serde::{Deserialize, Serialize};

pub mod calendar;
pub mod gemini;
pub mod gmail;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolResponse {
    pub content: String,
    pub data_type: DataType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DataType {
    Text,
    Markdown,
    CalendarEvent(EventDetails),
    EmailSummary(EmailDetails),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventDetails {
    pub summary: String,
    pub start_time: String,
    pub end_time: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EmailDetails {
    pub sender: String,
    pub subject: String,
    pub snippet: String,
}
