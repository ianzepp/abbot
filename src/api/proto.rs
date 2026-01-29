use serde::{Deserialize, Serialize};

use crate::bus::Origin;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiRequest {
    PublishChat {
        scope: String,
        sender: String,
        origin: Origin,
        content: String,
    },
    PublishTaskRequest {
        task_id: String,
        scope: String,
        sender: String,
        origin: Origin,
        head_id: String,
        goal: String,
        input: String,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiResponse {
    Ok,
    Error { message: String },
    Pong,
}

