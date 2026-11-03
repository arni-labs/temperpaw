# Plan: unique sandbox exec capture id per entity
Spec: docs/efforts/ARN-401/spec.md

## What we are addressing
Concurrent governed execs on one computer cross each other's stdout because the
capture-file id is not unique across concurrent invocations. Scope the id to the
calling entity (plus host clock and per-instance counter) so cross-entity execs
never share capture files.

## Approach
Split id-building into a pure `capture_run_id(entity_id, now_millis, seq)` and a
thin `unique_run_id(ctx)` that supplies the clock and counter. Encode the entity
id injectively (hex-escape) so the sanitizer cannot collapse two distinct ids to
one token.

## Steps
1. Rewrite `unique_run_id` to take `&Context` and delegate to `capture_run_id`.
2. Add the pure `capture_run_id` with the injective, filename-safe encoding.
3. Thread `ctx` into `tensorlake_exec`'s call.
4. Unit tests: entity scoping, repeat separation, unsafe-input sanitization,
   injectivity (the `a/b` vs `a?b` case).

## Files / surfaces touched
- `os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs`

## Expected end state
- `cargo test` green on `wasm-helpers` (capture-id tests included).
- `cargo build --target wasm32-wasip1 --release` clean.
- Two different entities' execs produce different capture paths even at the same
  millisecond; same-entity execs are serialized by the actor model.
