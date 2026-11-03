# ADR-002: Production Renderer Packaging and Result Guards

## Status

Accepted

## Date

2026-06-17

## Context

`paw-media` is a core Temper app and `MediaGenerationRequest.Generate` is intended to run entirely through entity actions and WASM integrations. Production exposed the `temper.image_generate` tool, but image requests could appear complete without a usable artifact when the runtime saw stale or incomplete media behavior. That made the state machine report success while the DM response had no file id, asset JSON, URL, or image handle.

The Codex image renderer is a WASM integration, so production images and CI must build and retain its module-local `.wasm` artifact before pruning build targets. Result callback actions also need to be reserved for the integrations that own those state transitions.

Codex image responses include inline image payloads that can exceed the fixed non-streaming WASM SDK HTTP response buffer. A renderer that uses `ctx.http_call` can fail after the provider succeeds, leaving the entity in an implementation-dependent partial state. The provider response path therefore needs the same streaming boundary already used for PawFS image writes.

## Decision

`paw-media` WASM builds are part of the required CI and Docker production build set. The production image must build `os-apps/paw-media/wasm/build.sh` before copying `os-apps` into the runtime layer.

User-facing `MediaGenerationRequest` permissions are limited to create, read, list, and `Generate`. `provider_auth_gate` may report auth readiness or auth failure. `openai_codex_image_generate` may record storing, final result, or provider failure.

The DM tool result renderer treats `Complete` without `ResultFileId`, `ResultPath`, or inline image bytes as an error instead of returning a successful empty image payload.

The Codex image renderer uses `temper_wasm_sdk::http_stream::streaming_call` for the provider response and streams PawFS writes through the same host API. The renderer enforces a bounded maximum response size before decoding the provider body.

## Consequences

CI now fails before deployment if the Codex image renderer is not included in the os-app WASM build set. Production images carry the renderer artifact needed for startup reconciliation and runtime execution.

Callback state transitions are auditable from Cedar policy and tied to their owning modules. A future renderer bug cannot silently masquerade as a successful DM image response without producing an artifact.

Provider image responses can be larger than the old SDK buffer without failing the render step. Oversized responses still fail explicitly at the renderer's configured maximum instead of exhausting host memory.
