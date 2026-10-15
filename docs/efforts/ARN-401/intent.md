# Intent: unique sandbox exec capture id per entity
Author: Claude (implementer, adopted for ARN-443). Status: accepted.

## Problem
Concurrent governed execs on one computer crossed each other's stdout. The
sandbox exec path wrote output to `/tmp/.paw-{out,err,rc}-<id>`, where `<id>`
came only from a process-local `AtomicU32` starting at 0. Every trigger dispatch
is a fresh WASM instance, so the counter reset to 0 each time and every exec
reused `/tmp/.paw-out-00000000`. Two execs running at once on the same sandbox
then read each other's files and raced the cleanup delete.

Live impact: a governed `ReleaseRun` rollback ran concurrently with a healthy
release's health probe on the same computer; the rollback's `sandbox_exec`
returned the probe's `502 __HTTP_STATUS` body instead of the `git revert`
output, so a successful revert was reported as Failed.

## Proposed outcome
Two execs from different calling entities never share capture files, so their
output cannot cross and cleanup cannot race — even when they run at the same
instant on the same sandbox.

## Affected users and systems
The `wasm-helpers` sandbox exec path (shared by `computer_exec`/paw-compute and
the paw-agent tool-run path). Any governed exec on a shared computer.

## Constraints
- Pure WASM: the only entropy available to a module is the host clock
  (`get_time_millis`) and a per-instance counter; there is no host RNG or
  per-dispatch invocation id.
- The capture filename must stay filename- and shell-safe (it is interpolated
  into a `/tmp/.paw-*` path and a bash redirection).
- No change to the exec contract or provider API.

## Open questions
- Can two execs for the SAME entity overlap? (Answered: YES. The exec is a
  long-running side effect that does not block the entity actor for its whole
  duration, so same-entity execs can overlap. Resolved by making the capture id
  globally unique per dispatch via a random u64 — see spec.)
