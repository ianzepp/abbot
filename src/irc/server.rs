use tokio::net::TcpListener;
use crate::runtime::RuntimeBus;
use super::connection::Connection;

pub struct Server {
    bus: RuntimeBus,
    port: u16,
}

impl Server {
    pub fn new(bus: RuntimeBus, port: u16) -> Self {
        Self { bus, port }
    }

    pub async fn run(&self) -> std::io::Result<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port)).await?;
        tracing::info!(port = self.port, "IRC server listening");

        loop {
            let (stream, addr) = listener.accept().await?;
            tracing::info!(?addr, "client connected");

            let bus = self.bus.clone();
            tokio::spawn(async move {
                let conn = Connection::new(stream, bus);
                conn.run().await;
                tracing::info!(?addr, "client disconnected");
            });
        }
    }
}
