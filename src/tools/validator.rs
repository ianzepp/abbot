/// Result of tool validation
#[derive(Debug, Clone)]
pub enum ValidationResult {
    /// Tool execution is allowed
    Allow,
    /// Tool execution is denied with error code and message
    Deny { code: String, message: String },
}

impl ValidationResult {
    pub fn deny(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Deny {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Context for validation decisions
pub struct ValidationContext<'a> {
    pub tool: &'a str,
    pub args: &'a str,
    pub sender: &'a str,
    pub channel: &'a str,
}

/// Trait for validating tool execution requests
#[allow(async_fn_in_trait)]
pub trait Validator: Send + Sync {
    /// Validate a tool execution request
    fn validate(&self, ctx: &ValidationContext) -> ValidationResult;
}

/// Validator that allows all requests (default)
pub struct AllowAll;

impl Validator for AllowAll {
    fn validate(&self, _ctx: &ValidationContext) -> ValidationResult {
        ValidationResult::Allow
    }
}

/// Validator that denies specific tools
pub struct DenyTools {
    tools: Vec<String>,
}

impl DenyTools {
    pub fn new(tools: Vec<String>) -> Self {
        Self { tools }
    }
}

impl Validator for DenyTools {
    fn validate(&self, ctx: &ValidationContext) -> ValidationResult {
        if self.tools.iter().any(|t| t == ctx.tool) {
            ValidationResult::deny("EPERM", format!("tool '{}' is not allowed", ctx.tool))
        } else {
            ValidationResult::Allow
        }
    }
}

/// Validator that only allows specific tools
pub struct AllowTools {
    tools: Vec<String>,
}

impl AllowTools {
    pub fn new(tools: Vec<String>) -> Self {
        Self { tools }
    }
}

impl Validator for AllowTools {
    fn validate(&self, ctx: &ValidationContext) -> ValidationResult {
        if self.tools.iter().any(|t| t == ctx.tool) {
            ValidationResult::Allow
        } else {
            ValidationResult::deny("EPERM", format!("tool '{}' is not allowed", ctx.tool))
        }
    }
}

/// Composite validator that runs multiple validators (all must allow)
pub struct ValidatorChain {
    validators: Vec<Box<dyn Validator>>,
}

impl ValidatorChain {
    pub fn new() -> Self {
        Self { validators: Vec::new() }
    }

    pub fn add(mut self, validator: Box<dyn Validator>) -> Self {
        self.validators.push(validator);
        self
    }
}

impl Default for ValidatorChain {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator for ValidatorChain {
    fn validate(&self, ctx: &ValidationContext) -> ValidationResult {
        for validator in &self.validators {
            let result = validator.validate(ctx);
            if !result.is_allowed() {
                return result;
            }
        }
        ValidationResult::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_all() {
        let v = AllowAll;
        let ctx = ValidationContext {
            tool: "bash",
            args: "rm -rf /",
            sender: "evil",
            channel: "#general",
        };
        assert!(v.validate(&ctx).is_allowed());
    }

    #[test]
    fn test_deny_tools() {
        let v = DenyTools::new(vec!["bash".to_string()]);
        let ctx = ValidationContext {
            tool: "bash",
            args: "ls",
            sender: "user",
            channel: "#general",
        };
        assert!(!v.validate(&ctx).is_allowed());

        let ctx2 = ValidationContext {
            tool: "read",
            args: "file.txt",
            sender: "user",
            channel: "#general",
        };
        assert!(v.validate(&ctx2).is_allowed());
    }

    #[test]
    fn test_validator_chain() {
        let chain = ValidatorChain::new()
            .add(Box::new(AllowAll))
            .add(Box::new(DenyTools::new(vec!["bash".to_string()])));

        let ctx = ValidationContext {
            tool: "bash",
            args: "ls",
            sender: "user",
            channel: "#general",
        };
        assert!(!chain.validate(&ctx).is_allowed());
    }
}
