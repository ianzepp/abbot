// LLM client for OpenAI-compatible APIs.
//
// Provides a simple async client for chat completions with proper error
// handling and response parsing. Supports custom base URLs for local
// models (ollama, llama.cpp, etc) as well as OpenAI's API.

mod openai_compat;

pub use openai_compat::{
    ChatMessage, ChatResult, ChatToolResult, Error, OpenAICompatClient, OpenAICompatDecodeError,
    OpenAICompatHttpError, OpenAICompatTransportError, Role, ToolCall, ToolSpec, Usage,
};
