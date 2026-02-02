use std::path::Path;

use async_trait::async_trait;
use tokio::fs;

#[derive(Debug, Clone)]
pub enum HalFsError {
    Io(String),
}

impl HalFsError {
    pub fn io(e: impl std::fmt::Display) -> Self {
        Self::Io(e.to_string())
    }
}

impl std::fmt::Display for HalFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HalFsError::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for HalFsError {}

#[async_trait]
pub trait HalFs: Send + Sync {
    async fn read_to_string(&self, path: &Path) -> Result<String, HalFsError>;
    async fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HalFsError>;
    async fn exists(&self, path: &Path) -> Result<bool, HalFsError>;
    async fn create_dir(&self, path: &Path) -> Result<(), HalFsError>;
    async fn create_dir_all(&self, path: &Path) -> Result<(), HalFsError>;
}

#[derive(Debug, Default, Clone)]
pub struct HostHalFs;

#[async_trait]
impl HalFs for HostHalFs {
    async fn read_to_string(&self, path: &Path) -> Result<String, HalFsError> {
        fs::read_to_string(path).await.map_err(HalFsError::io)
    }

    async fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HalFsError> {
        fs::write(path, bytes).await.map_err(HalFsError::io)
    }

    async fn exists(&self, path: &Path) -> Result<bool, HalFsError> {
        fs::try_exists(path).await.map_err(HalFsError::io)
    }

    async fn create_dir(&self, path: &Path) -> Result<(), HalFsError> {
        fs::create_dir(path).await.map_err(HalFsError::io)
    }

    async fn create_dir_all(&self, path: &Path) -> Result<(), HalFsError> {
        fs::create_dir_all(path).await.map_err(HalFsError::io)
    }
}
