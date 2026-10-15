# ADR-0054: Discord Transport Reconcile Policy

## Status

Accepted. Amended by ADR-0066: Railway health checks use `/readyz`, which no longer fails on Discord; Discord state is in its body.

## Context

TemperPaw starts Discord through the spec-owned `TransportConnection.Start` flow in the `paw-channels` app. Startup schedules the `transport_reconcile` WASM module, which may call the local runtime endpoint that starts the Discord transport process and then records the transport state back through `TransportConnection` and `AgentRoute` actions.

After moving Railway cutover to `/healthz`, production could cut over to the new process correctly, but `/readyz` still showed Discord as degraded because the reconcile module's host HTTP call was denied by Cedar policy.

## Decision

`paw-channels` explicitly permits `transport_reconcile`/`transport-reconcile` host HTTP calls to `HttpEndpoint`.

System principals may create, register, update, enable, and disable `AgentRoute` records during startup reconciliation. Human and supervisor management permissions remain unchanged.

Railway deployment health checks continue to use `/healthz`; `/readyz` remains the post-cutover proof that Discord and the app surfaces are usable.

## Consequences

- Discord transport reconciliation stays inside the spec-owned app flow instead of using a local install or capability-request escape hatch.
- `/readyz` can prove the Discord path after the process is already live.
- This change is non-destructive: it does not reset, wipe, truncate, recreate, replace, restore over, or manually delete any database state.
