use serde::{Deserialize, Serialize};

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
