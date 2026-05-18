# ADR-001: Nonblocking Reply Delivery

## Status

Accepted - 2026-05-18

## Context

Parent agent sessions deliver user-visible output by dispatching
`Channel.SendReply`. The `send_reply` WASM integration then posts to the
transport webhook and records `ReplyDelivered` or `ReplyFailed`.

Production showed that `agent_reply` was dispatching `Channel.SendReply` with
`await_integration=true`. That coupled session completion to transport delivery:
when the webhook or host HTTP boundary exceeded its deadline, the session reply
path waited on the delivery integration and surfaced low-level host timeout text
back to Discord. Under Katagami curation load, this amplified a provider/runtime
pressure event into repeated reply failures.

## Decision

Parent `agent_reply` dispatches `Channel.SendReply` without awaiting the
delivery integration. `Channel.SendReply` remains the entity action that owns
reply delivery, and `send_reply` remains the WASM integration that owns the
webhook call and delivery audit state.

The `send_reply` trigger now has an explicit 30 second timeout budget. If
delivery cannot complete inside that budget, the Channel stays `Connected` and
records `ReplyFailed` through the existing failure action.

## Consequences

Agent sessions can finish after enqueuing the reply action instead of blocking
on Discord/webhook delivery. Delivery failures are still visible in Channel
state history, but they no longer wedge or fail the completed agent session.

This keeps the flow Temper-native: delivery remains represented by entity
actions and WASM integrations, not by an imperative retry or transport-side
watcher.

## Verification

Regression coverage is in
`crates/temperpaw/tests/session_turn_architecture.rs`:

- `agent_reply_dispatches_channel_reply_without_waiting_for_delivery`
- `channel_send_reply_trigger_is_bounded_and_reports_delivery_failure`
