# ADR-009: Governed Path Hot-Linkage Actions

## Status

Accepted

## Context

Datadog traces for the canonical `foresight` deployment on Supabase showed a repeated denied raw `PATCH /tdata/Paths('{id}')` class on the corridor hot path. The request was cheap in CPU time but expensive in wall time because each denial still recorded governance decisions and denial patterns while the Supabase pool was already contended.

The affected updates were not business-state shortcuts. They linked newly spawned repairer/adversary agents to a `Path` and appended dweller contradiction flags after costing. Those changes are part of the app state machine and should be visible as governed transitions.

## Decision

Add narrow `Path` actions for these hot-linkage updates:

- `AssignRepairer(repairer_agent_id)` while `Path` is `Solving`.
- `AssignAdversary(adversary_agent_id)` while `Path` is `Repaired`.
- `AppendChallengeFlag(challenge_flags)` while `Path` is `Scored`, `Canonical`, or `Tail`.

Only system principals may dispatch these actions. Foresight WASM modules use bound `TemperPaw.*` action dispatches instead of raw OData `PATCH` for these fields.

## Consequences

The denied-PATCH class is removed from this corridor path, and assignment failures now fail before spawning sessions instead of silently loosening the assigned-agent self-report guard.

This does not solve the larger current write-amplification bottleneck from snapshot, catalog, projection, and index writes. That remains the higher-leverage DB-path work before any inference concurrency widening.
