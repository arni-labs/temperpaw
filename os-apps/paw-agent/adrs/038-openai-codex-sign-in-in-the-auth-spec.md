# ADR-038: OpenAI Codex Sign-In Lives in the Auth Spec

- Status: Accepted
- Date: 2026-10-24
- Amends: ADR-031 (its handling of human-login states)

## Context

When OpenAI Codex needed a person to sign in, `provider_auth_gate` failed the
Session with guidance to DM the bot `codex auth`. No module handles that
command, so nobody could act on it.

Two auth states also led nowhere. A refresh that could not succeed without the
person (no refresh token, or one OpenAI refuses) ended in Failed. A poll of a
device code that had run out re-entered DeviceCodeReady with the same code and
expiry, so the code could be offered after it stopped working.

The repository guide says a WASM integration fired by a transition never
dispatches transitions itself; sequencing belongs to the state machine.

## Decision

OpenAICodexAuth declares `SignInRequired`, a callback from Refreshing and
Polling to Starting whose trigger requests a new device code.
`openai_codex_auth` answers it when a refresh finds no refresh token, when
OpenAI answers the refresh with 401 or a refused-token code (invalid_grant,
refresh_token_expired, refresh_token_reused, refresh_token_invalidated), and
when a poll that did not sign the person in finds the code has 30 seconds or
less left. So EnsureFresh and PollDeviceLogin each end at Ready, at
DeviceCodeReady with a code a person can still enter, or at Failed.
EnsureFresh also runs from Disconnected.

`provider_auth_gate` reads the status and makes one move for it, as ADR-031
does:

- `Ready`, and configured `Refreshing`, proceed for EnsureFresh.
- `DeviceCodeReady` dispatches PollDeviceLogin, since the person may have
  entered the code.
- `Starting`, `Polling` and `Refreshing` otherwise wait for the step under way.
- Anything else dispatches the configured refresh action.

If the status after the move does not allow a provider call, the Session fails
with the link, the code and the time left, measured when the route answers.

## Consequences

- A Session that needs a sign-in tells the person exactly what to open and
  type, and their next message polls the code and continues.
- The dashboard's refresh actions now show a new code instead of Failed when
  the stored login is dead.
- A code waiting in DeviceCodeReady can still run out while nobody polls. The
  next poll replaces it; it is never offered after it runs out.

## Verification

- `crates/temperpaw/tests/openai_codex_auth_runs.rs` runs the packaged module
  through the declared callbacks against a simulated OpenAI, and checks every
  entry to DeviceCodeReady holds a code with more than 30 seconds left.
- `provider_auth_gate` unit tests cover the move for each status and the
  prompt.
