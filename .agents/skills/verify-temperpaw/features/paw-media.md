# Media generation

## Sub-features
MediaGenerationRequest entity (set MediaGenerationRequests; NOT the legacy 'MediaGeneration' - ADR-003): governed image generation. v1 is image + openai_codex only, generic by design for future types.

## How to get to it (user POV)
An agent requests media; the platform runs the provider call as a WASM integration and stores the result.

## Driving it
Create a PawFS Workspace first (Generate requires workspace_id - a raw create with none dies in Generating). Create a MediaGenerationRequest, dispatch Generate with workspace_id and explicit quality. Read back: ResultFileId + ResultPath, then GET /tdata/Files('<ResultFileId>')/$value for PNG bytes - Complete with an empty ResultFileId is a FAILURE (ADR-002), not proof.

## Gotchas
Auth is Temper secret store via dashboard device-code login, not .env keys (env OPENAI_CODEX_TOKEN is a legacy fallback). Without secrets, expect Status=Failed with ProviderAuthError stamped through Authorizing - valid evidence of the governed machine, capture it rather than treating it as broken.
