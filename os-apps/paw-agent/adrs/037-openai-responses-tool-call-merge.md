# ADR-037: OpenAI Responses Tool-Call Merge

Status: Accepted
Date: 2026-06-18

## Context

OpenAI/Codex Responses streams can deliver structured tool calls through
`response.output_item.done` before the final `response.completed` event. The
final completed payload may also include assistant text, including reasoning
summary text that looks like `Tool call call_...: execute(...)`.

The `provider_caller` accumulator previously replaced all streamed output items
with the non-empty `response.completed.response.output` array. If that final
array was text-only, an already streamed `function_call` disappeared and the
Session was recorded with `finish_reason=end_turn`. ARN-64 has live LLMObs
evidence for both shapes in the same trace: a broken text-only span
`2777268388898525301`, and a working structured `execute` tool-call span
`18204161239414911689`.

## Decision

`provider_caller` must treat streamed `function_call` items as durable semantic
output. When `response.completed` supplies a non-empty output array, the
accumulator will merge completed output with prior streamed items instead of
blindly replacing them.

Merge policy:

- Preserve every streamed `function_call` unless the completed output contains
  the same `call_id`.
- Allow completed text/message items to supplement streamed tool calls.
- Continue to derive `stop_reason=tool_use` from the merged content whenever any
  structured tool call survives.

The parser will not synthesize a tool call from text that merely resembles one.
That remains a separate safety decision: text-as-tool-call should never erase a
real structured call, but prose alone is not enough authority to execute code.

## Consequences

Positive:

- Codex reasoning-summary text can no longer clobber real streamed tool calls.
- Sessions with legitimate `execute` calls continue through `ProcessToolCalls`
  instead of completing falsely.
- The fix is provider-parser local and does not switch providers or force
  tool-choice behavior.

Risks:

- Responses that intentionally include both text and tool calls will now preserve
  both. This matches Anthropic-style mixed content and the existing Session
  content model.

## Verification

- Add a regression where `response.output_item.done` carries an `execute`
  `function_call`, then `response.completed` carries only text shaped like a
  tool call. The parser must return a `tool_use` block and `stop_reason=tool_use`.
- Run focused `provider-caller` tests.
