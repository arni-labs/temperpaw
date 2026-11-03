# Spec: unique sandbox exec capture id per entity
Status: accepted. Intent: docs/efforts/ARN-401/intent.md

## Requirements
- The capture id used to build `/tmp/.paw-{out,err,rc}-<id>` must be distinct for
  any two execs that can run concurrently on one sandbox.
- The id must be filename- and shell-safe; an absent calling entity falls back to
  a safe token.
- Pure and unit-testable (no host calls inside the id-building function).

## Design
`unique_run_id(ctx)` = `capture_run_id(ctx.entity_id, random_u64())`.

- **Uniqueness comes from `random_u64()`** — a random u64 drawn once per
  dispatch. On `wasm32-wasip1` this is WASI `random_get` (the temper host wires
  `wasi_snapshot_preview1`); on native (tests) it is the OS RNG. A random draw is
  globally unique per dispatch, so two execs never share capture files even for
  the SAME entity in the same instant. No wall-clock is read, keeping the module
  free of ambient time (DST discipline).
- **The entity id is a readable label only** — `capture_run_id(entity_id, rand)`
  (pure, testable) prefixes the id with a bounded, filename-safe encoding of the
  entity so temp files are human-attributable:
  - injective encoding: alphanumerics and `-` pass through; every other byte —
    `_` included — becomes `_` + two hex digits (so `_` only ever marks an
    escape, and `a/b` never aliases `a?b`);
  - bounded: first 32 encoded chars + an 8-hex FNV-1a hash of the FULL id, so the
    filename can never exceed the filesystem per-component limit while staying
    distinguishing;
  - prefixed with `e`, so an empty id (encoded segment "") cannot alias a real id.

Format: `e{≤32 encoded}-{fnv32:08x}-{rand:016x}`.

## Policy / invariants
- Determinism: `capture_run_id` is pure; the only non-determinism (the RNG draw)
  lives in `unique_run_id`, which runs only on the real exec path
  (`tensorlake_exec`), never in the DST/sim path. No ambient time is read.
- Safety: the encoded token cannot contain `/`, whitespace, quotes, or `.`, so it
  cannot escape `/tmp/.paw-*` or break the shell redirection; length is bounded.

## Same-entity concurrency (the corrected understanding)
An earlier draft argued entity scope sufficed because Temper actors process one
message at a time. **That was wrong**, and the review panel (Codex + Fable) caught
it: the sandbox exec is a long-running side effect (it polls for up to 600
iterations) and does not block the entity actor for its whole duration — which is
exactly why a ~30-min run is a known problem (ARN-443 part D). So two dispatches
for the SAME entity CAN have their execs overlap in wall-clock time. A wall-clock
+ per-instance counter cannot separate them (both fresh instances read the same
millisecond and both counters start at 0). The per-dispatch random u64 closes
this: it is unique regardless of entity, clock, or instance.

## Deferred / out of scope
- Streaming / cancel / long-run (>poll-cap) exec — ARN-443 part D.
- Rebuilding + republishing the `computer_exec` blob to Genesis — done separately
  (Genesis already carries the rebuilt blob); not part of this repo-side change.
