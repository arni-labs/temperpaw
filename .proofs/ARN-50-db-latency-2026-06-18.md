# Proof Report: ARN-50 — Foresight DB Hot-Path Linkage Fix

## Date

2026-06-18

## Branch / Commit

Branch: `codex/arn50-db-latency`

Implementation commit: `ba625e44`

## What Was Done

Re-verified the canonical `foresight` deployment filter in Datadog and implemented the first focused fix for the verified denied raw `PATCH /tdata/Paths` class:

- Added governed `Path.AssignRepairer`, `Path.AssignAdversary`, and `Path.AppendChallengeFlag` actions.
- Permitted those actions for system principals only.
- Updated `spawn_repairers`, `spawn_adversaries`, and `animate_dwellers` to dispatch bound actions instead of raw Path PATCH.
- Added contract and Cedar tests for the new action surface.

## Verification Flow

## Verification Results
| Step | Expected | Actual | Status |
|------|----------|--------|--------|
| Datadog trace decode | Identify canonical Foresight/Supabase filters | `@version:sha-foresight-dd-638ff9b1` and `@peer.service:foresight-supabase` matched; plain `version:` returned 0 buckets | Pass |
| Foresight-only 10m APM sample | Quantify request/query/internal span amplification | 81 HTTP request spans, 1,218 Postgres query spans, 28,305 internal spans | Pass |
| DB span wait/busy split | Confirm DB path is mostly wait, not compute | Hot query averages were ~136-487 ms wall with ~0.11-2.67 ms busy; idle time dominated | Pass |
| Corridor spec/Cedar tests | New actions exist and are system-only | `cargo test -p temperpaw --test corridor_engine_contract --test corridor_cedar_matrix` passed: 25 tests | Pass |
| Edited WASM unit tests | Edited modules still compile and pass unit contracts | `spawn_repairers` 12/12, `spawn_adversaries` 7/7, `animate_dwellers` 6/6 passed | Pass |
| Foresight WASM bundle | All app WASM modules build for `wasm32-unknown-unknown` | `bash os-apps/paw-foresight/wasm/build.sh` completed; all 13 modules built | Pass |

## What Worked
- `@version` and `@peer.service` are the working Datadog filters for this lane.
- The denied Path PATCH class maps directly to app-state updates and can be replaced with entity actions.

## What Didn't Work
- Plain `version:` APM filtering returned zero buckets even though spans carry `version` in custom attributes.
- DBM sample lookup did not return activity rows for the inspected window.

## Limitations

No live deployment was changed in this thread. The fix is verified locally by tests and WASM build, not by a fresh production run.

## What Still Doesn't Work

The larger DB bottleneck is still snapshot/projection/catalog/index write amplification. This fix removes a confirmed denial-recording waste class, but it is not the primary current latency lever.

## Artifacts

- Datadog trace: `2e06e546a5184dcc284c496f78e9ca86`
- Working branch: `codex/arn50-db-latency`

## Architecture Diagram
```text
spawn_repairers ── Path.AssignRepairer ──▶ Path(Solving)
spawn_adversaries ─ Path.AssignAdversary ▶ Path(Repaired)
animate_dwellers ─ Path.AppendChallengeFlag ▶ Path(Scored/Canonical/Tail)

No raw PATCH /tdata/Paths for these hot-linkage updates.
```
