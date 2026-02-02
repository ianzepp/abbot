use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use crate::kernel::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomKind {
    Conclave,
    Autonomy,
}

impl RoomKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "conclave" => Some(Self::Conclave),
            "autonomy" => Some(Self::Autonomy),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Conclave => "conclave",
            Self::Autonomy => "autonomy",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoomRecord {
    pub id: Uuid,
    pub kind: RoomKind,
    pub scope: String,
    pub created_at: Instant,
}

#[derive(Debug, Default)]
pub struct RoomKernel {
    rooms: Mutex<HashMap<Uuid, RoomRecord>>,
    streams: Mutex<HashMap<Uuid, mpsc::Sender<Frame>>>,
    capacity: usize,
}

impl RoomKernel {
    pub fn new() -> Self {
        Self {
            rooms: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
            capacity: 256,
        }
    }

    pub async fn create(&self, kind: RoomKind, scope: &str) -> Uuid {
        let id = Uuid::new_v4();
        let rec = RoomRecord {
            id,
            kind,
            scope: scope.to_string(),
            created_at: Instant::now(),
        };
        {
            let mut rooms = self.rooms.lock().await;
            rooms.insert(id, rec);
        }
        id
    }

    pub async fn get(&self, room_id: Uuid) -> Option<RoomRecord> {
        let rooms = self.rooms.lock().await;
        rooms.get(&room_id).cloned()
    }

    pub async fn open_stream(&self, room_id: Uuid) -> mpsc::Receiver<Frame> {
        let (tx, rx) = mpsc::channel::<Frame>(self.capacity);
        let mut streams = self.streams.lock().await;
        streams.insert(room_id, tx);
        rx
    }

    pub async fn has_stream(&self, room_id: Uuid) -> bool {
        let streams = self.streams.lock().await;
        streams.contains_key(&room_id)
    }

    pub async fn send(&self, room_id: Uuid, frame: Frame) -> Result<(), ()> {
        let tx = {
            let streams = self.streams.lock().await;
            streams.get(&room_id).cloned()
        };
        let Some(tx) = tx else {
            return Err(());
        };
        tx.send(frame).await.map_err(|_| ())
    }

    pub async fn close_stream(&self, room_id: Uuid) {
        let mut streams = self.streams.lock().await;
        streams.remove(&room_id);
    }
}
