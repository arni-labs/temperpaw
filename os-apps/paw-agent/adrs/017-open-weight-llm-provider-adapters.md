# ADR-017: Open-Weight LLM Provider Adapters

Status: Accepted
Date: 2026-06-16

Note: this app already has two historical ADR-017 files. This ADR keeps the requested filename because the decision is scoped and easy to locate by title.

## Context

TemperPaw needs to call hosted and local open-weight models without adding a Rust orchestration loop. Existing provider logic had OpenRouter-specific chat-completions request and stream parsing in `provider_caller`, plus a separate OpenRouter-shaped compaction path in `context_compactor`.

The platform architecture requires LLM runtime work to stay Temper-native: Session state transitions trigger WASM integrations, and the owning WASM remains responsible for HTTP, retry policy, progress events, and Session actions.

## Decision

Add first-class provider names for:

- `openrouter`
- `huggingface`
- `fireworks`
- `sakana_fugu`
- `local_openai`
- `openai_compatible`

Keep existing provider names:

- `anthropic`
- `openai`
- `openai_codex`
- `mock`

Introduce a small shared WASM crate, `openai-chat-wire`, used by `provider_caller` and `context_compactor`. It only owns OpenAI-compatible chat wire concerns:

- Chat Completions request-body construction
- safe `provider_options_json` merge
- reserved-key rejection for `messages`, `tools`, `stream`, and `model`
- SSE delta parsing
- tool-call reconstruction
- usage parsing
- non-stream response text extraction
- custom header JSON parsing

The shared crate does not perform HTTP calls, retries, progress emission, authorization, or entity transitions.

## Provider Configuration

The new providers use Temper Vault/config keys:

- `openrouter_api_key`, `openrouter_api_url`
- `huggingface_api_key`, `hf_token`, `huggingface_api_url`
- `fireworks_api_key`, `fireworks_api_url`
- `sakana_fugu_api_key`, `sakana_fugu_api_url`
- `openai_compatible_api_key`, `openai_compatible_api_url`, `openai_compatible_headers_json`
- `local_openai_api_url`

Default chat-completions endpoints are provided for OpenRouter, Hugging Face, Fireworks, and local Ollama-compatible endpoints. Sakana Fugu and arbitrary custom endpoints require explicit URLs.

## Provider Options

`provider_options_json` is added to Agent and Session state and action params. It is carried through direct Session Configure, provider switching, subagent spawning, channel continuations, managed agents, wiki jobs, and cron-spawned sessions.

This field supports OpenRouter Fusion and routing options such as:

```json
{"plugins":[{"id":"fusion","preset":"general-budget"}]}
```

## Consequences

Open-weight hosted and local providers now share one Chat Completions adapter while preserving Temper-native execution boundaries. Provider-specific failures remain visible through Session state and WASM logs, and users can switch provider/model/options through state transitions instead of a separate runtime controller.
