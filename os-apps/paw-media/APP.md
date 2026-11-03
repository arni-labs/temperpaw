# Paw Media

Paw Media provides governed media generation as Temper-native entity flows.

## Entities

- `MediaGenerationRequest`: request/result state for media generation (`media_type = "image"`, `operation = "generate"`). Two providers, each started by its own action:
  - `Generate`: `provider = "openai_codex"`, the Codex subscription, behind the provider auth gate.
  - `GenerateWithOpenRouter`: `provider = "openrouter"`, OpenRouter's image API with the `openrouter_api_key` tenant secret, no auth gate. `model` is any OpenRouter image model id (`vendor/model`); empty means the default, `x-ai/grok-imagine-image-2.0`, unless the operator configures another. Who may spend the OpenRouter credit is the policy's call, not the module's. The result's `model` says which drew. Katagami's art-style transfer test chooses its own models (ADR 004).

## Agent Tool

Agents call `temper.image_generate(prompt, opts=None)`; `opts={"provider": "openrouter", "model": "..."}` draws through OpenRouter. The tool creates a `MediaGenerationRequest`, dispatches `Generate`, waits for the WASM provider module, then returns PawFS file metadata plus a short-lived inline image marker for immediate multimodal feedback.

Durable bytes are stored through PawFS `File` streams. Inline base64 is only a transient tool-result convenience and uses the spec overflow TTL.

The Codex provider reads generated image responses through the Temper WASM streaming HTTP host API, so provider responses are not constrained by the fixed non-streaming SDK response buffer. The default quality remains `low` to keep DM image generation fast and inexpensive unless callers request a higher quality.
