# ADR-010: Compaction Fallback and Provider Error Sanitization

## Status

Accepted - 2026-05-18

## Context

Production sessions using the `openai_codex` provider repeatedly failed during
context compaction. The compactor called the ChatGPT Codex responses backend and
the host HTTP call hit its 60 second outer deadline. The Session then failed and
`agent_reply` surfaced the raw backend transport text to Discord.

Context compaction is support work for continuing a turn. It should preserve
enough prior context to keep moving, but it should not be allowed to fail the
whole Session when a provider transport is unavailable.

## Decision

`context_compactor` now uses local fallback compaction immediately for
`openai_codex` sessions instead of making a background compaction call to the
Codex backend. The normal provider call path is unchanged.

For non-Codex compaction providers, non-auth compaction errors also fall back to
a local extractive summary. Auth-expired errors still route through
`CompactionAuthExpired` so the existing auth recovery state machine remains in
charge of token refresh.

`agent_reply` sanitizes known provider transport failures before sending a reply
to a channel. Raw backend URLs and host HTTP details remain in Temper state and
Datadog, not in user-facing Discord messages.

`Session` admission now caps the provider-touching actions as well as creation:
`ProviderAuthReady` is limited to three concurrent dispatches and
`CompactionAuthReady` is limited to one. This applies admission at the scarce
resource boundary identified in ADR-0038, rather than only at Session creation.

## Consequences

Compaction quality can be lower when the local fallback is used, but the Session
keeps progressing and the operator sees an actionable, sanitized message instead
of a raw provider transport string.

This preserves Temper-native flow: compaction still completes through
`CompactionComplete`, auth recovery still goes through `CompactionAuthExpired`,
and reply delivery still goes through `Channel.SendReply`.

## Verification

Regression coverage:

- `context_compactor::tests::openai_codex_compaction_uses_local_fallback_strategy`
- `context_compactor::tests::compaction_transport_failure_uses_local_fallback_summary`
- `context_compactor::tests::compaction_auth_expired_still_routes_to_auth_recovery`
- `agent_reply::tests::build_reply_text_sanitizes_codex_transport_failures`
- `session_admission_caps_provider_touching_actions`
