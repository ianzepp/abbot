// LLM clients for OpenAI-compatible and Anthropic APIs.
//
// Provides simple async clients for chat completions with proper error
// handling and response parsing. Supports custom base URLs for local
// models (ollama, llama.cpp, etc) as well as OpenAI's and Anthropic's APIs.

mod anthropic;
mod client;
mod openai_compat;

// Unified client (preferred for new code)
pub use client::{
    ChatToolResult as UnifiedChatToolResult, LlmClient, Message as UnifiedMessage,
    ToolCall as UnifiedToolCall, ToolSpec as UnifiedToolSpec, Usage as UnifiedUsage,
};

// Anthropic-specific types
pub use anthropic::{
    AnthropicClient, AnthropicDecodeError, AnthropicHttpError, AnthropicTransportError,
    ContentBlock, Message, ToolChoice,
};
pub use anthropic::{
    ChatResult as AnthropicChatResult, ChatToolResult as AnthropicChatToolResult,
    Role as AnthropicRole, ToolCall as AnthropicToolCall, ToolSpec as AnthropicToolSpec,
    Usage as AnthropicUsage,
};

// OpenAI-compatible types
pub use openai_compat::{
    ChatMessage, ChatResult, ChatToolResult, Error, OpenAICompatClient, OpenAICompatDecodeError,
    OpenAICompatHttpError, OpenAICompatTransportError, Role, ToolCall, ToolCallFunction, ToolSpec,
    Usage,
};
