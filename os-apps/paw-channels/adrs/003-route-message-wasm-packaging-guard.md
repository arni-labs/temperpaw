# ADR 003: Verify Packaged route_message WASM

## Status

Accepted

## Context

Discord DM routing depends on the `route_message` WASM module bundled with the
`paw-channels` OS app. A production deploy can pass source-level tests while
still packaging or reconciling an unexpected compiled artifact. On 2026-06-17,
production verification showed `Channel.ReceiveMessage` reaching `RouteFailed`
until the rebuilt `route_message.wasm` artifact was hot-uploaded.

The failing behavior was the old ordered `SessionEntries` lookup shape:

```text
/tdata/SessionEntries?$filter=SessionId eq '<id>'&$orderby=Sequence desc&$top=1
```

That query shape can exceed production request limits for large sessions and
must not be present in the packaged router.

## Decision

The Docker image build and CI WASM build must run a verifier against the
packaged `os-apps/paw-channels/wasm/route_message/route_message.wasm` artifact.
The verifier prints the artifact SHA-256 and size for deploy evidence and fails
if the artifact contains the forbidden `$orderby` or `Sequence desc` lookup
strings.

## Consequences

The source test remains responsible for route logic. The packaging verifier
covers the artifact boundary: CI and Docker publish cannot silently ship a
compiled router that reintroduces the production-failing lookup. If a future
route implementation legitimately needs ordered OData queries, it must use a
different bounded query shape and update this ADR and verifier deliberately.
