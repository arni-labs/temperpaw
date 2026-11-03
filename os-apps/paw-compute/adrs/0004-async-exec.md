# ADR-0004 — Async Exec (start + poll loop)

Status: accepted (ARN-443 D)

## Context

ADR-0002's Exec ran synchronously: `Run` fired `computer_exec`, which started the
process, polled for the result, and reported back — all inside ONE WASM
invocation. A WASM invocation is hard-capped at ~120s (temper-wasm
`WasmResourceLimits`), but a review/panel command runs far longer (up to ~30 min).
So the synchronous Exec cannot carry the panel's runs.

## Decision

Make Exec asynchronous, driven by a `state_timeout` poll loop.

- Exec: `Created → Starting → Running → Succeeded|Failed`.
- `Run` fires `computer_exec_start`, which only LAUNCHES the process (wrapped in a
  long sandbox-side `timeout` — the async path has no 120s cap) via
  `sandbox_exec_start`, and reports `ExecStarted(run_id, started_at_ms)` → `Running`.
- A `state_timeout` on `Running` (10s, `reset_on = ["Poll"]`) fires `Poll` →
  `computer_exec_poll`, which calls `sandbox_exec_poll(run_id)`: finished →
  `RunSucceeded` (a sandbox-timeout exit 124 → `RunFailed`); still running before a
  safety deadline → the module reports success with an EMPTY callback (no
  transition); past the deadline → `RunFailed`. The loop is carried by the timeout
  alone: each fired `Poll` is a `Running` self-loop, and a self-loop re-arms a
  `state_timeout` ONLY when the action is in `reset_on` (kernel
  `state_timeouts.rs`: `is_reset = !state_changed && reset_on.contains(action)`),
  which is why `Poll` lists itself. There is NO `KeepRunning` action: a self-loop
  callback would not re-arm the timer either (only `reset_on` does), so it was pure
  machinery. The kernel accepts the empty-callback report as "no callback"
  (`engine/mod.rs` defaults `callback_action` to `""`; `wasm.rs` skips dispatch
  when empty → `Ok(None)`).
- `Cancel` (Agent) → `Failed`. `Starting` has a start-safety timeout.
- The synchronous `computer_exec` STAYS for LatencyDiag's quick canned command;
  only Exec moves to the async path.
- `wasm-helpers` splits the exec: `sandbox_exec_start` (POST, returns run_id) +
  `sandbox_exec_poll(run_id) → Option<ExecResult>`; the synchronous `sandbox_exec`
  is unchanged for short callers.

## Consequences

- A single command can outlive many WASM invocations; the machine, not a blocked
  invocation, carries the wait — visible in the row's Running/Poll history.
- Result latency is bounded by the poll interval (≤10s after completion).
- `reset_on` is load-bearing, not decorative: without it the poll loop fires once
  and stalls. The same rule makes `Leased`'s `reset_on = ["Heartbeat"]` (Computer)
  the thing that lets a copy's Heartbeat actually renew its lease. Settled by a
  kernel-source read; see ARN-443 decisions D-FINAL / E3.
- The async path carries only bounded stdout/stderr tails (no full-output log-file
  paging — that stays the synchronous path's feature; `stdout_path`/`stdout_bytes`
  are left empty).
- Cancel stops the GOVERNED exec (row → Failed, loop ends); the sandbox process is
  bounded by its own `timeout` wrapper rather than an immediate provider kill (an
  accepted first-pass simplification).
- Real provider proof (a >120s run surviving invocation boundaries, and Cancel)
  lands at C/D's Genesis-publish verification per the effort's C5 condition.
