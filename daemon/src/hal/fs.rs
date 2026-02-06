//! Filesystem HAL - Hardware Abstraction Layer for File Operations
//!
//! ARCHITECTURE OVERVIEW
//! =====================
//! This module provides a trait-based abstraction over filesystem operations,
//! enabling dependency injection and testing. The HAL pattern allows the system
//! to swap between real filesystem access (HostHalFs) and mock implementations
//! for testing without changing business logic.
//!
//! DESIGN PHILOSOPHY
//! =================
//! - Trait-based abstraction: Enables testing and platform-specific implementations
//! - Async-first: All operations are async to integrate with tokio runtime
//! - Path-based API: Uses std::path::Path for type-safe path handling
//! - Error conversion: Wraps I/O errors in domain-specific error types
//!
//! TRADE-OFFS
//! ==========
//! - Performance overhead: Trait dispatch adds minor overhead vs direct fs calls
//! - API surface: Limited to common operations; extend as needed for new use cases
//! - Error granularity: Single Io error variant; could be more specific if needed

use std::path::Path;

use async_trait::async_trait;
use tokio::fs;

// =============================================================================
// ERRORS
// =============================================================================
//
// Filesystem errors wrap underlying I/O errors in a domain-specific type.
// This allows callers to handle filesystem failures uniformly without
// coupling to std::io::Error directly.

#[derive(Debug, Clone)]
pub enum HalFsError {
    Io(String),
}

impl HalFsError {
    /// Create an I/O error from any displayable error.
    ///
    /// WHY: Provides a consistent error constructor that handles conversion
    /// from various I/O error types (tokio::io::Error, std::io::Error).
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

// =============================================================================
// TRAIT
// =============================================================================
//
// The HalFs trait defines the filesystem abstraction. Implementations must
// provide async methods for common file operations.
//
// WHY async: All methods are async to integrate with the tokio runtime and
// avoid blocking the executor on I/O operations.
//
// WHY trait: Enables dependency injection, testing with mocks, and potential
// platform-specific implementations without changing business logic.

#[async_trait]
pub trait HalFs: Send + Sync {
    /// Read a file's contents as a UTF-8 string.
    async fn read_to_string(&self, path: &Path) -> Result<String, HalFsError>;

    /// Write bytes to a file, creating or overwriting it.
    async fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), HalFsError>;

    /// Check if a path exists.
    async fn exists(&self, path: &Path) -> Result<bool, HalFsError>;

    /// Create a directory.
    ///
    /// WHY: Fails if parent doesn't exist. Use create_dir_all for recursive creation.
    async fn create_dir(&self, path: &Path) -> Result<(), HalFsError>;

    /// Create a directory and all parent directories as needed.
    async fn create_dir_all(&self, path: &Path) -> Result<(), HalFsError>;
}

// =============================================================================
// HOST IMPLEMENTATION
// =============================================================================
//
// HostHalFs is the production implementation that delegates to tokio::fs.
// This provides real filesystem access for the running system.
//
// WHY separate type: Allows the trait to be implemented for test doubles
// (mocks, fakes) while keeping the production impl simple and obvious.

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
