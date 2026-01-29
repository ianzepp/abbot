/// LLM backend configuration for multi-provider support.
/// All backends use OpenAI-compatible /chat/completions API.

pub const MODEL_LARGE: &str = "openai/gpt-5.2";
pub const MODEL_MEDIUM: &str = "ollama/gpt-oss:20b-cloud";
pub const MODEL_SMALL: &str = "ollama/gemma3:1b";

pub fn resolve_model(size: &str) -> &'static str {
    match size {
        "large" => MODEL_LARGE,
        "medium" => MODEL_MEDIUM,
        "small" => MODEL_SMALL,
        _ => MODEL_MEDIUM,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Zai,
    OpenAI,
    OpenRouter,
    Ollama,
}

impl Backend {
    /// Parse model string prefix to determine backend.
    /// Returns (Backend, model_name) tuple.
    ///
    /// Examples:
    /// - "zai/glm-4.7" -> (Zai, "glm-4.7")
    /// - "openai/gpt-4" -> (OpenAI, "gpt-4")
    /// - "openrouter/anthropic/claude-sonnet-4.5" -> (OpenRouter, "anthropic/claude-sonnet-4.5")
    /// - "ollama/llama3" -> (Ollama, "llama3")
    pub fn from_model_prefix(model: &str) -> (Self, &str) {
        if let Some(rest) = model.strip_prefix("zai/") {
            (Self::Zai, rest)
        } else if let Some(rest) = model.strip_prefix("openai/") {
            (Self::OpenAI, rest)
        } else if let Some(rest) = model.strip_prefix("openrouter/") {
            (Self::OpenRouter, rest)
        } else if let Some(rest) = model.strip_prefix("ollama/") {
            (Self::Ollama, rest)
        } else {
            // Default to OpenRouter for backwards compatibility
            (Self::OpenRouter, model)
        }
    }

    pub fn base_url(&self) -> &'static str {
        match self {
            Self::Zai => "https://api.z.ai/api/paas/v4",
            Self::OpenAI => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Ollama => "http://localhost:11434/v1",
        }
    }

    pub fn api_key_env(&self) -> &'static str {
        match self {
            Self::Zai => "ZAI_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Ollama => "OLLAMA_API_KEY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_model_prefix() {
        let (backend, model) = Backend::from_model_prefix("zai/glm-4.7");
        assert_eq!(backend, Backend::Zai);
        assert_eq!(model, "glm-4.7");

        let (backend, model) = Backend::from_model_prefix("openai/gpt-4");
        assert_eq!(backend, Backend::OpenAI);
        assert_eq!(model, "gpt-4");

        let (backend, model) = Backend::from_model_prefix("openrouter/anthropic/claude-sonnet-4.5");
        assert_eq!(backend, Backend::OpenRouter);
        assert_eq!(model, "anthropic/claude-sonnet-4.5");

        let (backend, model) = Backend::from_model_prefix("ollama/llama3");
        assert_eq!(backend, Backend::Ollama);
        assert_eq!(model, "llama3");
    }

    #[test]
    fn test_default_backend() {
        let (backend, model) = Backend::from_model_prefix("anthropic/claude-sonnet-4.5");
        assert_eq!(backend, Backend::OpenRouter);
        assert_eq!(model, "anthropic/claude-sonnet-4.5");
    }

    #[test]
    fn test_base_urls() {
        assert_eq!(Backend::Zai.base_url(), "https://api.z.ai/api/paas/v4");
        assert_eq!(Backend::OpenAI.base_url(), "https://api.openai.com/v1");
        assert_eq!(Backend::OpenRouter.base_url(), "https://openrouter.ai/api/v1");
        assert_eq!(Backend::Ollama.base_url(), "http://localhost:11434/v1");
    }

    #[test]
    fn test_api_key_env() {
        assert_eq!(Backend::Zai.api_key_env(), "ZAI_API_KEY");
        assert_eq!(Backend::OpenAI.api_key_env(), "OPENAI_API_KEY");
        assert_eq!(Backend::OpenRouter.api_key_env(), "OPENROUTER_API_KEY");
        assert_eq!(Backend::Ollama.api_key_env(), "OLLAMA_API_KEY");
    }
}
