# ADR-003: App-Scoped MediaGeneration Request Contract

## Status

Accepted

## Date

2026-06-17

## Context

Production had an older root `TemperPaw.MediaGeneration` entity set named `MediaGenerations` persisted from an earlier media stub. Installing the newer `paw-media` app originally added `TemperPaw.Media.MediaGeneration`, which avoided the namespace collision in CSDL but still shared the simple entity type name used by storage and action dispatch.

OData entity-set names are route handles, and Temper entity state is keyed by the simple automaton name. A new route alone could still hydrate legacy `MediaGeneration` rows and dispatch the stale root workflow.

That made `temper.image_generate(...)` vulnerable to dispatching the old `Requested -> Succeeded` stub instead of the governed `Created -> Authorizing -> Generating -> Storing -> Complete` media workflow.

## Decision

`paw-media` uses the app-scoped entity type `MediaGenerationRequest` and exposes it through the app-scoped OData entity set `MediaGenerationRequests`.

The Monty `temper.image_generate` tool and the `openai_codex_image_generate` provider WASM use `/tdata/MediaGenerationRequests` for create, action dispatch, readback, and internal callback actions.

The legacy `MediaGeneration` entity type and `/tdata/MediaGenerations` route are not used by `paw-media` runtime code.

## Consequences

Production can install `paw-media` beside older persisted specs without route or storage ambiguity. The clean contract for DMs is now: `temper.image_generate` creates a `TemperPaw.Media.MediaGenerationRequest` through `MediaGenerationRequests`, then waits for the provider WASM to produce a PawFS file, path, or inline image bytes before returning success.

Future media entities should choose app-scoped entity type and entity-set names when legacy or cross-app collisions are possible.
