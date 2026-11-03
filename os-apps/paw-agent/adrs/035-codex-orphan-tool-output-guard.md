# ADR-035: Codex Orphan Tool Output Guard

Date: 2026-06-17

## Status

Accepted

## Context

Production Discord DMs hit the OpenAI Codex Responses API with an invalid request:

```text
No tool call found for function call output with call_id ...
```

The Responses API requires every `function_call_output` item to reference a `function_call` item included in the same request context when no stored `previous_response_id` is used. TemperPaw stores turns in an Anthropic-shaped internal transcript, so recovered or compacted sessions can contain a `tool_result` block without the matching assistant `tool_use` block that originally created it.

Before this ADR, `provider_caller` converted every internal `tool_result` into a Responses API `function_call_output` unconditionally. One orphan tool result could therefore poison the entire Codex request and prevent Paw from replying to any DM in that session.

## Decision

`provider_caller` now builds Codex/OpenAI Responses input through a guarded conversion step:

- collect assistant `tool_use.id` values from the prepared transcript
- convert only matching `tool_result.tool_use_id` values into `function_call_output`
- downgrade orphan tool results into ordinary user context text, preserving image content as user image input where available
- log the number of downgraded orphan outputs

Valid tool-call/tool-result pairs continue to use the Responses API tool-output contract. Orphan results become context instead of invalid protocol items.

## Consequences

Codex requests remain valid after session recovery, compaction, or partial-history reads that omit the originating tool call. The model can still see the orphan tool result as text context, but it is no longer forced through a provider protocol field that requires stricter pairing.

This is a provider wire-format guard only. It does not introduce imperative orchestration and does not change the Temper entity state machine.
