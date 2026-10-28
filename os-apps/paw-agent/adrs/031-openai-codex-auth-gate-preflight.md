# ADR-031: OpenAI Codex Auth Gate Status Preflight

- Status: Accepted; its handling of human-login states is amended by ADR-038
- Date: 2026-06-08

## Context

`provider_auth_gate` runs before OpenAI Codex provider calls. The previous gate
always dispatched `OpenAICodexAuth.EnsureFresh` for Codex sessions. In
production, a configured auth entity can legitimately be in `Refreshing` while
the stored account tokens still exist. Dispatching `EnsureFresh` into that
state can fail jobs with an infrastructure-looking auth error even though the
provider call is allowed to proceed.

The setup API already exposes `/paw/setup/openai-codex/status`, including the
auth entity state and whether the required Codex subscription secrets are
configured.

## Decision

Before dispatching `EnsureFresh`, `provider_auth_gate` reads the setup status
endpoint.

- `Ready` proceeds without dispatching `EnsureFresh`.
- `Refreshing` proceeds when Codex auth is configured.
- Human-login states such as `DeviceCodeReady`, `Polling`, `Disconnected`, and
  unconfigured `Refreshing` fail before the provider call with Discord device
  login guidance.
- Non-2xx or unrecognized preflight responses fall back to the existing
  `EnsureFresh` action path.

The gate remains a WASM integration on the Session state transition; it does
not add a background watcher or separate orchestration layer.

## Consequences

- Healthy configured refreshes no longer fail CurationJobs before provider
  calls.
- Actual human-auth waits still fail clearly and tell the user to DM
  `codex auth`, complete device login, and reply `codex auth done`.
- Datadog should show `provider_auth_gate: OpenAICodexAuth status is ...;
  skipping EnsureFresh` for ready/configured-refreshing paths instead of
  repeated `dispatching OpenAICodexAuth.EnsureFresh` lines.

## Verification

- Unit tests cover configured `Refreshing`, unconfigured `Refreshing`, and
  device-code guidance.
- Release WASM rebuild is required before deploy so `provider_auth_gate.wasm`
  carries the preflight behavior.
