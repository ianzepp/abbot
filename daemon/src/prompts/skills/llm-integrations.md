---
name: llm-integrations
description: Guide to integrating LLM provider APIs in application code
category: development
requires: []
---

# LLM Provider Integrations

Reference for building applications that call LLM provider APIs. Covers authentication patterns, endpoint conventions, and provider-specific notes. Always verify current model IDs and pricing against provider documentation — these change frequently.

---

## Common Patterns

All major providers follow a similar pattern:

1. **Authentication** via API key in a request header
2. **Chat completions** as the primary endpoint (messages in, message out)
3. **Streaming** via Server-Sent Events (SSE)
4. **Tool/function calling** via a tools array in the request
5. **JSON output** via a response_format or schema parameter

Request bodies are JSON. Responses are JSON (or SSE streams of JSON chunks).

---

## Anthropic

**API Docs**: https://docs.anthropic.com/en/api
**SDK (Python)**: `pip install anthropic` — https://github.com/anthropics/anthropic-sdk-python
**SDK (TypeScript)**: `npm install @anthropic-ai/sdk` — https://github.com/anthropics/anthropic-sdk-typescript

**Base URL**: `https://api.anthropic.com`
**Auth Header**: `x-api-key: $ANTHROPIC_API_KEY`
**Version Header**: `anthropic-version: 2023-06-01` (required)

**Key Endpoints**:
- `POST /v1/messages` — Chat completions
- `POST /v1/messages/batches` — Batch processing

**Model ID Format**: `claude-{tier}-{version}` (e.g., `claude-sonnet-4-20250514`)

**Notable Features**:
- System prompt is a top-level `system` field, not a message role
- Tool use schema uses `input_schema` (JSON Schema), not `parameters`
- Streaming uses `event: message_start`, `content_block_delta`, `message_stop` event types
- Extended thinking: `thinking` parameter for chain-of-thought reasoning
- Prompt caching: `cache_control` on message blocks to cache prefixes
- Token counting: `input_tokens` and `output_tokens` in response `usage`

**Gotchas**:
- The `anthropic-version` header is required — requests fail without it
- Max output tokens must be specified explicitly (`max_tokens` is required)
- Image input is supported via base64 `image` content blocks
- Rate limits returned in `retry-after` header on 429 responses

---

## OpenAI

**API Docs**: https://platform.openai.com/docs/api-reference
**SDK (Python)**: `pip install openai` — https://github.com/openai/openai-python
**SDK (TypeScript)**: `npm install openai` — https://github.com/openai/openai-node

**Base URL**: `https://api.openai.com`
**Auth Header**: `Authorization: Bearer $OPENAI_API_KEY`

**Key Endpoints**:
- `POST /v1/chat/completions` — Chat completions
- `POST /v1/embeddings` — Text embeddings
- `POST /v1/images/generations` — Image generation
- `POST /v1/audio/transcriptions` — Speech to text
- `GET /v1/models` — List available models

**Model ID Format**: `gpt-{version}` or `o{version}` (e.g., `gpt-4.1`, `o3`)

**Notable Features**:
- System prompt uses `role: "system"` in messages array
- Tool calling uses `tools` array with `function` type and `parameters` (JSON Schema)
- Structured outputs: `response_format: { type: "json_schema", json_schema: { ... } }`
- Streaming uses `data: {"choices": [{"delta": {...}}]}` SSE format
- Function calling supports `parallel_tool_calls` for multiple simultaneous calls
- Embeddings endpoint supports batch input (array of strings)

**Gotchas**:
- `max_tokens` is optional (defaults to model max) — opposite of Anthropic
- Streaming responses end with `data: [DONE]`
- Rate limits vary by tier — check `x-ratelimit-*` response headers
- Organization header `OpenAI-Organization` needed for multi-org accounts

---

## Google Gemini

**API Docs**: https://ai.google.dev/gemini-api/docs
**SDK (Python)**: `pip install google-genai` — https://github.com/googleapis/python-genai
**SDK (TypeScript)**: `npm install @google/genai` — https://github.com/googleapis/js-genai

**Base URL**: `https://generativelanguage.googleapis.com`
**Auth**: API key as `?key=$GEMINI_API_KEY` query parameter, or OAuth2 Bearer token

**Key Endpoints**:
- `POST /v1beta/models/{model}:generateContent` — Chat completions
- `POST /v1beta/models/{model}:streamGenerateContent` — Streaming
- `POST /v1beta/models/{model}:embedContent` — Embeddings
- `GET /v1beta/models` — List models

**Model ID Format**: `gemini-{version}` (e.g., `gemini-2.5-pro`)

**Notable Features**:
- Request structure uses `contents` array with `parts` (not `messages`)
- System instruction is a top-level `systemInstruction` field
- Tool calling uses `functionDeclarations` with `parameters` (OpenAPI Schema)
- Supports `generationConfig` for temperature, topP, maxOutputTokens
- Grounding with Google Search via `tools: [{ googleSearch: {} }]`
- Multi-modal input (images, video, audio) via inline data or file URIs

**Gotchas**:
- API key goes in query string by default, not a header
- Different request/response shape from OpenAI — not a drop-in replacement
- Streaming uses `?alt=sse` query parameter
- `safetySettings` array controls content filtering thresholds

---

## OpenRouter

**API Docs**: https://openrouter.ai/docs
**SDK**: Uses the OpenAI SDK — set `base_url` to OpenRouter

**Base URL**: `https://openrouter.ai/api`
**Auth Header**: `Authorization: Bearer $OPENROUTER_API_KEY`

**Key Endpoints**:
- `POST /v1/chat/completions` — Chat completions (OpenAI-compatible)
- `GET /v1/models` — List available models with pricing
- `GET /v1/auth/key` — Check key validity and credits

**Model ID Format**: `provider/model-name` (e.g., `anthropic/claude-sonnet-4`, `openai/gpt-4.1`)

**Notable Features**:
- OpenAI-compatible API — use the OpenAI SDK with a custom base URL
- Aggregates models from many providers (Anthropic, OpenAI, Google, Meta, Mistral, etc.)
- Automatic fallback routing between providers
- `HTTP-Referer` and `X-Title` headers for app attribution (optional, helps ranking)
- `transforms` parameter for automatic prompt formatting

**Gotchas**:
- Model IDs include the provider prefix (`anthropic/claude-...`, not just `claude-...`)
- Pricing varies per model — check `/v1/models` for per-token costs
- Some provider-specific features (caching, extended thinking) may not be exposed
- Rate limits depend on the underlying provider being routed to

---

## Ollama (Local Models)

**API Docs**: https://github.com/ollama/ollama/blob/main/docs/api.md
**SDK**: Uses the OpenAI SDK — set `base_url` to local Ollama server

**Base URL**: `http://localhost:11434` (default)
**Auth**: None (local server)

**Key Endpoints**:
- `POST /api/chat` — Chat completions (Ollama-native format)
- `POST /api/generate` — Text generation
- `POST /api/embeddings` — Embeddings
- `POST /v1/chat/completions` — OpenAI-compatible chat endpoint
- `GET /api/tags` — List local models
- `POST /api/pull` — Download a model
- `POST /api/show` — Show model details

**Model ID Format**: `model:tag` (e.g., `llama3:8b`, `codellama:13b-instruct`)

**Notable Features**:
- Runs entirely locally — no API key, no network, no data sharing
- OpenAI-compatible endpoint at `/v1/` — use the OpenAI SDK with `base_url`
- Native endpoint at `/api/` has additional features (model management, raw mode)
- `keep_alive` parameter controls how long models stay loaded in memory
- Supports tool calling on compatible models via the `/api/chat` endpoint

**Gotchas**:
- Models must be pulled before use (`ollama pull model:tag`)
- Performance depends on local hardware (GPU memory, CPU)
- No streaming in the OpenAI-compatible endpoint by default — set `stream: true`
- Large models may take significant time to load on first request
- Context window is model-dependent and often smaller than cloud providers

---

## X.ai (Grok)

**API Docs**: https://docs.x.ai/docs
**SDK**: Uses the OpenAI SDK — set `base_url` to X.ai

**Base URL**: `https://api.x.ai`
**Auth Header**: `Authorization: Bearer $XAI_API_KEY`

**Key Endpoints**:
- `POST /v1/chat/completions` — Chat completions (OpenAI-compatible)
- `GET /v1/models` — List available models

**Model ID Format**: `grok-{version}` (e.g., `grok-3`, `grok-3-mini`)

**Notable Features**:
- OpenAI-compatible API — use the OpenAI SDK with a custom base URL
- Supports tool/function calling
- Supports streaming

**Gotchas**:
- Smaller model selection than other providers
- Check docs for current rate limits and model availability

---

## OpenAI-Compatible Providers

Many providers implement the OpenAI API spec. When working with these, the general pattern is:

```python
from openai import OpenAI

client = OpenAI(
    api_key="...",
    base_url="https://provider-url.com/v1",
)

response = client.chat.completions.create(
    model="model-id",
    messages=[{"role": "user", "content": "Hello"}],
)
```

```typescript
import OpenAI from "openai";

const client = new OpenAI({
    apiKey: "...",
    baseURL: "https://provider-url.com/v1",
});

const response = await client.chat.completions.create({
    model: "model-id",
    messages: [{ role: "user", content: "Hello" }],
});
```

Providers using this pattern: **OpenRouter**, **Ollama**, **X.ai**, **Together.ai**, **Groq**, **Fireworks**, **Mistral**, and many others. The OpenAI SDK with a custom `base_url` is the de facto standard client.

---

## Choosing a Provider

| Consideration | Recommendation |
|---------------|----------------|
| Highest capability | Anthropic, OpenAI, Google — check current benchmarks |
| Lowest latency | Groq, Fireworks (inference-optimized), or Ollama (local) |
| Cost-sensitive | OpenRouter (compare pricing across providers), Ollama (free, local) |
| Privacy/compliance | Ollama (fully local), or providers with data processing agreements |
| Multi-provider flexibility | OpenRouter (aggregator), or OpenAI-compatible SDK with swappable base URL |
| Embeddings | OpenAI, Google, or local models via Ollama |
| Image generation | OpenAI (DALL-E), Google (Imagen) |

---

## General Integration Advice

- **Use official SDKs** when available — they handle auth, retries, streaming, and type safety.
- **Set timeouts** — LLM calls can take 30-120 seconds for long outputs. Default HTTP timeouts are often too short.
- **Handle rate limits** — check for 429 status and `retry-after` headers. Implement exponential backoff.
- **Stream for UX** — streaming responses feel faster to users. All major providers support SSE streaming.
- **Token counting** — response `usage` fields report token counts. Use these for cost tracking, not string length.
- **Error handling** — distinguish between retryable (429, 500, 503) and permanent (400, 401, 403) errors.
- **Environment variables** — store API keys in env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.), never in source code.
- **Model fallbacks** — consider routing to a backup provider/model on failures for production systems.
