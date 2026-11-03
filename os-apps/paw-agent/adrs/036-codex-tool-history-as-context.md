# ADR-036: Send Codex Tool History As Context

Date: 2026-06-17

## Status

Accepted

## Context

ADR-035 guarded only orphaned `tool_result` blocks when converting TemperPaw's
Anthropic-shaped transcript into OpenAI/Codex Responses API input. Production
Discord DMs still failed with:

```text
No tool call found for function call output with call_id ...
```

The remaining escape was matched historical tool history. A recovered session
can include an assistant `tool_use` and a later `tool_result`, but Codex still
does not necessarily accept that replayed history as a fresh Responses API
`function_call` / `function_call_output` pair. One rejected historical tool
output prevents Paw from answering the user's next DM at all.

## Decision

For fresh Codex Responses requests, `provider_caller` now converts all prior
TemperPaw tool history into ordinary conversation context:

- assistant `tool_use` blocks become assistant text that records the tool name,
  call id, and arguments
- user `tool_result` blocks become user context text, preserving embedded image
  content as user image input
- no historical `function_call` or `function_call_output` items are emitted in
  request input

New tool calls produced by the current Codex response are still parsed from the
provider output and executed through the existing Temper-native action flow.

## Consequences

Recovered or compacted DM sessions can continue after any historical tool call
shape because provider input no longer depends on Codex accepting replayed
Responses API protocol objects.

The model sees previous tool activity as text context instead of protocol-level
tool state. This is intentionally more conservative: a normal answer is better
than failing the entire DM turn with a provider wire-format error. Image tool
results still retain their attachment metadata through the existing
`reply_attachments_json` path.
