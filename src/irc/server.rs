use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use crate::bus::Hub;
use super::connection::Connection;

pub struct Server {
    hub: Arc<RwLock<Hub>>,
    port: u16,
}

impl Server {
    pub fn new(hub: Arc<RwLock<Hub>>, port: u16) -> Self {
        Self { hub, port }
    }

    pub async fn run(&self) -> std::io::Result<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port)).await?;
        tracing::info!(port = self.port, "IRC server listening");

        loop {
            let (stream, addr) = listener.accept().await?;
            tracing::info!(?addr, "client connected");

            let hub = self.hub.clone();
            tokio::spawn(async move {
                let conn = Connection::new(stream, hub);
                conn.run().await;
                tracing::info!(?addr, "client disconnected");
            });
        }
    }
}
