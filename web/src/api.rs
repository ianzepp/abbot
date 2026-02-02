// REST API client using gloo-net.
//
// Provides async functions for fetching data from the backend API endpoints.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

use crate::state::{FileEntry, Message};

const API_BASE: &str = "/api";

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "API error {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

async fn fetch_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, ApiError> {
    let url = format!("{}{}", API_BASE, path);
    let response = Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError {
            status: 0,
            message: e.to_string(),
        })?;

    if !response.ok() {
        return Err(ApiError {
            status: response.status(),
            message: response.status_text(),
        });
    }

    response.json().await.map_err(|e| ApiError {
        status: 0,
        message: e.to_string(),
    })
}

async fn post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
    path: &str,
    body: &T,
) -> Result<R, ApiError> {
    let url = format!("{}{}", API_BASE, path);
    let response = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .map_err(|e| ApiError {
            status: 0,
            message: e.to_string(),
        })?
        .send()
        .await
        .map_err(|e| ApiError {
            status: 0,
            message: e.to_string(),
        })?;

    if !response.ok() {
        return Err(ApiError {
            status: response.status(),
            message: response.status_text(),
        });
    }

    response.json().await.map_err(|e| ApiError {
        status: 0,
        message: e.to_string(),
    })
}

pub async fn get_files(path: &str) -> Result<Vec<FileEntry>, ApiError> {
    let encoded = js_sys::encode_uri_component(path);
    fetch_json(&format!("/files?path={}", encoded)).await
}

pub async fn get_file_content(path: &str) -> Result<String, ApiError> {
    let encoded = js_sys::encode_uri_component(path);
    fetch_json(&format!("/file?path={}", encoded)).await
}

pub async fn get_messages(scope: &str, limit: u32) -> Result<Vec<Message>, ApiError> {
    let scope_encoded = js_sys::encode_uri_component(scope);
    fetch_json(&format!("/messages?scope={}&limit={}", scope_encoded, limit)).await
}

#[derive(Debug, Deserialize)]
pub struct MemoryContent {
    pub content: String,
}

pub async fn get_self() -> Result<MemoryContent, ApiError> {
    fetch_json("/self").await
}

pub async fn get_ltm() -> Result<MemoryContent, ApiError> {
    fetch_json("/ltm").await
}

#[derive(Debug, Deserialize)]
pub struct ConclaveDetail {
    pub id: String,
    pub status: String,
    pub transcript: String,
    pub decision: String,
    pub created_at: u64,
}

pub async fn get_conclave(id: &str) -> Result<ConclaveDetail, ApiError> {
    let encoded = js_sys::encode_uri_component(id);
    fetch_json(&format!("/conclave?id={}", encoded)).await
}

#[derive(Serialize)]
struct SendMessageBody {
    content: String,
    scope: String,
}

#[derive(Deserialize)]
struct EmptyResponse {}

pub async fn send_message(content: &str, scope: &str) -> Result<(), ApiError> {
    let body = SendMessageBody {
        content: content.to_string(),
        scope: scope.to_string(),
    };
    let _: EmptyResponse = post_json("/send", &body).await?;
    Ok(())
}
