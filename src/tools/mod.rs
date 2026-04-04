use serde::{Deserialize, Serialize};

pub mod news;
pub mod news_filter;
pub mod news_format;
pub mod news_scheduler;
pub mod news_score;
pub mod news_sources;
pub mod news_store;
pub mod todo;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolResponse {
    pub content: String,
    pub data_type: DataType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DataType {
    Text,
    Markdown,
}
