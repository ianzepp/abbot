use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::api::proto::{ApiRequest, ApiResponse};
use crate::bus::{Scope, respond};
use crate::runtime::RuntimeBus;

pub struct ApiServer {
    bus: RuntimeBus,
    addr: String,
}

impl ApiServer {
    pub fn new(bus: RuntimeBus, addr: impl Into<String>) -> Self {
        Self {
            bus,
            addr: addr.into(),
        }
    }

    pub fn start(self) {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                tracing::error!(error = %e, "api server exited");
            }
        });
    }

    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        tracing::info!(addr = %self.addr, "api server listening");

        loop {
            let (stream, addr) = listener.accept().await?;
            let bus = self.bus.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_client(stream, bus).await {
                    tracing::debug!(?addr, error = %e, "api client error");
                }
            });
        }
    }
}

async fn handle_client(stream: TcpStream, bus: RuntimeBus) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(());
        }

        let req: Result<ApiRequest, _> = serde_json::from_str(line.trim_end());
        let resp = match req {
            Ok(ApiRequest::Ping) => ApiResponse::Pong,
            Ok(ApiRequest::PublishChat {
                scope,
                sender,
                origin,
                content,
            }) => {
                bus.create_scope(scope.as_str()).await;
                bus.publish(
                    respond::chat(sender, Scope::from(scope.as_str()), content)
                        .with_origin(origin),
                )
                .await;
                ApiResponse::Ok
            }
            Ok(ApiRequest::PublishTaskRequest {
                task_id,
                scope,
                sender,
                origin,
                head_id,
                goal,
                input,
            }) => {
                bus.create_scope(scope.as_str()).await;
                bus.publish(
                    respond::task_request(sender, Scope::from(scope.as_str()), task_id, head_id, goal, input)
                        .with_origin(origin),
                )
                .await;
                ApiResponse::Ok
            }
            Err(e) => ApiResponse::Error {
                message: format!("invalid request: {e}"),
            },
        };

        let out = serde_json::to_string(&resp).unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"serialize\"}".to_string());
        writer.write_all(out.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
}
