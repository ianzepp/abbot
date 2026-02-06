use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl KernelError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            help: None,
            detail: None,
            retryable: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    pub fn invalid_args(message: impl Into<String>) -> Self {
        Self::new("E_INVALID_ARGS", message)
            .with_help("Check the arguments and try again with valid values.")
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("E_FORBIDDEN", message)
            .with_help("This operation is not allowed. Check permissions or path constraints.")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("E_NOT_FOUND", message)
            .with_help("The requested resource does not exist. Verify the path or identifier.")
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new("E_IO", message).with_retryable(true)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new("E_TIMEOUT", message)
            .with_retryable(true)
            .with_help("The operation timed out. Consider increasing the deadline or simplifying the request.")
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new("E_CANCELLED", message).with_retryable(false)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("E_INTERNAL", message).with_retryable(true)
    }

    pub fn not_implemented(syscall: impl Into<String>) -> Self {
        let name = syscall.into();
        Self::new(
            "E_NOT_IMPLEMENTED",
            format!("syscall '{name}' is not implemented"),
        )
        .with_help("This syscall does not exist. Check available syscalls.")
    }

    pub fn disabled(message: impl Into<String>) -> Self {
        Self::new("E_DISABLED", message)
            .with_help("This feature is not configured. Check your configuration.")
    }

    pub fn readonly(message: impl Into<String>) -> Self {
        Self::new("E_READONLY", message)
            .with_help("This mount is read-only. Use a writable mount for modifications.")
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "code": "E_INTERNAL",
                "message": "failed to serialize error"
            })
        })
    }
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for KernelError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_error_construction() {
        let err = KernelError::invalid_args("missing required field 'path'");
        assert_eq!(err.code, "E_INVALID_ARGS");
        assert!(err.message.contains("path"));
        assert!(err.help.is_some());
    }

    #[test]
    fn test_error_with_detail() {
        let err = KernelError::forbidden("workspace escape attempted").with_detail(
            json!({"attempted_path": "/etc/passwd", "workspace": "/home/user/project"}),
        );

        assert_eq!(err.code, "E_FORBIDDEN");
        assert!(err.detail.is_some());
    }

    #[test]
    fn test_error_serialization() {
        let err = KernelError::timeout("process exceeded 5000ms");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("E_TIMEOUT"));
        assert!(json.contains("retryable"));
    }

    #[test]
    fn test_error_display() {
        let err = KernelError::not_found("file /tmp/missing.txt does not exist");
        let display = format!("{}", err);
        assert!(display.contains("E_NOT_FOUND"));
        assert!(display.contains("missing.txt"));
    }

    #[test]
    fn test_to_value() {
        let err = KernelError::io("disk full");
        let val = err.to_value();
        assert_eq!(val["code"], "E_IO");
        assert_eq!(val["message"], "disk full");
    }
}
