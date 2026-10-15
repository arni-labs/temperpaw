# Feature: sandbox exec (wasm-helpers capture path)

The governed exec path in `wasm-helpers` (`os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs`)
runs a command on a computer's sandbox and captures its stdout/stderr/exit code
via `/tmp/.paw-{out,err,rc}-<id>` files. This feature covers the correctness of
that capture — most importantly that concurrent execs on one sandbox never share
capture files and cross output.

## What to verify
- `capture_run_id` builds a filename- and shell-safe id from the calling
  entity, host clock, and per-instance counter.
- Two execs never share capture files — cross-entity (the ARN-401 bug) OR
  same-entity concurrent (a long-running exec can overlap another dispatch for
  the same entity). Uniqueness comes from a per-dispatch random u64.
- The entity-id label is injective, bounded (≤32 chars + hash), and empty-safe.

## How to drive it (rerun)
Pure logic is unit-tested; run the crate tests and the wasm build:

```
cd os-apps/paw-agent/wasm/wasm-helpers
cargo test                                   # capture_id_* + full suite
cargo build --target wasm32-wasip1 --release # module compiles for the host
```

Pass = tests green and the wasm build is clean.

## Notes
- The live end-to-end exec (a real command on a live sandbox via `computer_exec`)
  runs through Genesis-published `paw-compute`; publishing to Genesis is a
  separate, gated step and is not part of a repo-side change. The unit tests
  reproduce the exact concurrent-collision scenarios, so the fix is verified at
  the logic level here.
