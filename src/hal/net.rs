use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

#[derive(Debug, Clone)]
pub struct HalHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct HalHttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub truncated: bool,
    pub body_size: usize,
}

#[derive(Debug, Clone)]
pub enum HalNetError {
    InvalidArgs(String),
    Connect(String),
    Http(String),
    Timeout { timeout: Duration },
}

impl HalNetError {
    pub fn invalid_args(msg: impl Into<String>) -> Self {
        Self::InvalidArgs(msg.into())
    }

    pub fn http(e: impl std::fmt::Display) -> Self {
        Self::Http(e.to_string())
    }

    pub fn connect(e: impl std::fmt::Display) -> Self {
        Self::Connect(e.to_string())
    }
}

impl std::fmt::Display for HalNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HalNetError::InvalidArgs(msg) => write!(f, "{msg}"),
            HalNetError::Connect(msg) => write!(f, "{msg}"),
            HalNetError::Http(msg) => write!(f, "{msg}"),
            HalNetError::Timeout { timeout } => write!(f, "request timed out after {:?}", timeout),
        }
    }
}

impl std::error::Error for HalNetError {}

#[async_trait]
pub trait HalNet: Send + Sync {
    async fn http_request(&self, req: HalHttpRequest) -> Result<HalHttpResponse, HalNetError>;
}

#[derive(Debug, Default, Clone)]
pub struct HostHalNet;

#[async_trait]
impl HalNet for HostHalNet {
    async fn http_request(&self, req: HalHttpRequest) -> Result<HalHttpResponse, HalNetError> {
        if req.url.trim().is_empty() {
            return Err(HalNetError::invalid_args("url is empty"));
        }
        if req.max_body_bytes == 0 {
            return Err(HalNetError::invalid_args("max_body_bytes must be > 0"));
        }

        let method = req.method.parse::<reqwest::Method>().map_err(|_| {
            HalNetError::invalid_args(format!("unsupported method: {}", req.method))
        })?;

        let mut headers = HeaderMap::new();
        for (k, v) in &req.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| HalNetError::invalid_args(format!("invalid header name: {k}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|_| HalNetError::invalid_args(format!("invalid header value for {k}")))?;
            headers.insert(name, value);
        }

        let client = reqwest::Client::builder()
            .timeout(req.timeout)
            .default_headers(headers)
            .build()
            .map_err(HalNetError::http)?;

        let mut r = client.request(method, &req.url);
        if let Some(body) = req.body {
            r = r.body(body);
        }

        let response = match r.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return Err(HalNetError::Timeout {
                        timeout: req.timeout,
                    });
                }
                if e.is_connect() {
                    return Err(HalNetError::connect(e));
                }
                return Err(HalNetError::http(e));
            }
        };

        let status = response.status().as_u16();
        let resp_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let mut body: Vec<u8> = Vec::new();
        let mut truncated = false;
        let mut body_size: usize = 0;

        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(next) = stream.next().await {
            let chunk = match next {
                Ok(c) => c,
                Err(e) => {
                    if e.is_timeout() {
                        return Err(HalNetError::Timeout {
                            timeout: req.timeout,
                        });
                    }
                    return Err(HalNetError::http(e));
                }
            };

            body_size = body_size.saturating_add(chunk.len());

            if body.len() < req.max_body_bytes {
                let remaining = req.max_body_bytes - body.len();
                if chunk.len() <= remaining {
                    body.extend_from_slice(&chunk);
                } else {
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
            } else {
                truncated = true;
                break;
            }

            if body.len() >= req.max_body_bytes {
                // We consumed exactly the limit; consider this truncated if more data exists.
                // We can't know without reading, so stop here and mark truncated conservatively.
                truncated = true;
                break;
            }
        }

        Ok(HalHttpResponse {
            status,
            headers: resp_headers,
            body,
            truncated,
            body_size,
        })
    }
}
