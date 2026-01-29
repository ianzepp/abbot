use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::proto::{ApiRequest, ApiResponse};

pub struct ApiClient {
    addr: String,
}

impl ApiClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self { addr: addr.into() }
    }

    pub async fn request(&self, req: &ApiRequest) -> Result<ApiResponse, String> {
        let mut stream = TcpStream::connect(&self.addr)
            .await
            .map_err(|e| format!("connect {}: {}", self.addr, e))?;

        let msg = serde_json::to_string(req).map_err(|e| e.to_string())?;
        stream
            .write_all(format!("{}\n", msg).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
        let resp: ApiResponse = serde_json::from_str(line.trim_end()).map_err(|e| e.to_string())?;
        Ok(resp)
    }
}

