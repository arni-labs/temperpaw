# ADR-004: OpenRouter as the Second Image Provider

## Status

Proposed

## Date

2026-10-01

## Context

Katagami needs to know whether an art style's prompt carries over to a second image model before a curator accepts it: Rita decided that a submitted art style is drawn again by a second model and shown beside the contributor's pictures. Temper could draw images only through the Codex subscription (ADR 001). fal is locked for top-up. The tenant already holds an `openrouter_api_key` secret, and OpenRouter serves xAI's Grok Imagine, the second model Katagami's earlier bake-offs used.

## Decision

- A second provider module, `openrouter_image_generate`, calls `POST https://openrouter.ai/api/v1/images` and stores the image in PawFS the same way the Codex renderer does, reading the response through the streaming host API.
- It is started by its own action, `GenerateWithOpenRouter` (`Created`/`Failed` to `Generating`). IOA guards cannot compare strings, so the action is the route. The action skips the Codex auth gate: the API key is the credential.
- The request sends only `model`, `prompt`, `n` and, for a non-square size, the nearest documented `aspect_ratio`. OpenRouter rejects a parameter a model does not support, so quality and background are not sent.
- The default model is `x-ai/grok-imagine-image-2.0`; a caller may name any OpenRouter image model.
- The agent tool maps `provider: "openrouter"` (and `grok`, `xai`) to the new action.
- `FailedIsFinal` is removed: both Generate actions retry from `Failed`, and the invariant failed every level of `temper verify`.

## Consequences

- Image calls through OpenRouter are billed to the OpenRouter account (Grok Imagine is about $0.04 an image on 2026-10-01).
- No response id or revised prompt comes back; `provider_response_id` records `openrouter:<model>`.
