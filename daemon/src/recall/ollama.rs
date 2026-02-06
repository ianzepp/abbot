use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Ollama {
    client: Client,
    base_url: String,
    model: String,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Serialize)]
struct EmbedBatchRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Ollama {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    pub fn local() -> Self {
        Self::new("http://127.0.0.1:11434", "nomic-embed-text")
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, OllamaError> {
        let url = format!("{}/api/embed", self.base_url);
        let req = EmbedRequest {
            model: &self.model,
            input: text,
        };

        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(OllamaError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(OllamaError::Api(format!("{}: {}", status, body)));
        }

        let data: EmbedResponse = resp.json().await.map_err(OllamaError::Request)?;

        data.embeddings
            .into_iter()
            .next()
            .ok_or(OllamaError::Api("empty embeddings response".into()))
    }

    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, OllamaError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let url = format!("{}/api/embed", self.base_url);
        let req = EmbedBatchRequest {
            model: &self.model,
            input: texts.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(OllamaError::Request)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(OllamaError::Api(format!("{}: {}", status, body)));
        }

        let data: EmbedResponse = resp.json().await.map_err(OllamaError::Request)?;
        Ok(data.embeddings)
    }
}

#[derive(Debug)]
pub enum OllamaError {
    Request(reqwest::Error),
    Api(String),
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OllamaError::Request(e) => write!(f, "request error: {}", e),
            OllamaError::Api(msg) => write!(f, "api error: {}", msg),
        }
    }
}

impl std::error::Error for OllamaError {}
