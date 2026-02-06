use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tick {
    pub seq: u64,
    pub now_ms: i64,
    pub dt_ms: i64,
}

#[derive(Debug)]
pub struct TickKernel {
    tx: watch::Sender<Tick>,
}

impl TickKernel {
    pub fn new() -> (Self, watch::Receiver<Tick>) {
        let now = now_ms();
        let (tx, rx) = watch::channel(Tick {
            seq: 0,
            now_ms: now,
            dt_ms: 0,
        });
        (Self { tx }, rx)
    }

    pub fn receiver(&self) -> watch::Receiver<Tick> {
        self.tx.subscribe()
    }

    pub fn start(&self, interval: Duration) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut seq: u64 = 0;
            let mut last = Instant::now();
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                seq = seq.saturating_add(1);
                let now = Instant::now();
                let dt_ms = now.duration_since(last).as_millis() as i64;
                last = now;
                let _ = tx.send(Tick {
                    seq,
                    now_ms: now_ms(),
                    dt_ms,
                });
            }
        });
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
