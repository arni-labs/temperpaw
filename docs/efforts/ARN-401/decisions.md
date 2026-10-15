# Decisions - unique sandbox exec capture id per entity

## Make the capture id globally unique per dispatch via a random u64
- **Decision:** Derive the capture id's uniqueness from a random u64 drawn once
  per dispatch (WASI `random_get` on wasip1, OS RNG on native), not from the
  entity id + wall-clock + per-instance counter.
- **Came up because:** the review panel (Codex + Fable) showed the entity+clock+
  counter scheme still collides for two execs of the SAME entity in the same
  millisecond (both fresh instances → counter 0, same clock). See the correction
  entry below.
- **Options:** entity id + clock + counter (insufficient) / the Exec row id where
  available (path-dependent — only unique when entity_id is the Exec id, not for
  the Session run_tools path) / a per-dispatch random u64 (uniform).
- **Chose the random u64 because:** it is globally unique per dispatch on BOTH
  exec paths, and the host already wires WASI (`temper-wasm` engine provides
  `random_get`), so it is available. It also removes the wall-clock read, keeping
  the module free of ambient time (DST discipline). Given up: the id is no longer
  a pure function of the inputs — but `capture_run_id(entity_id, rand)` stays pure
  and testable; only `unique_run_id` (impure, real-exec-path only) draws the RNG.
- **Where:** `os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs`
  `unique_run_id` / `random_u64` / `capture_run_id`; `getrandom` dep in Cargo.toml.

## Keep the entity id as a bounded, injective, readable label
- **Decision:** Keep an encoded entity id as a human-readable prefix on the
  capture filename — injective, capped at 32 encoded chars + an 8-hex FNV hash of
  the full id, prefixed with `e`.
- **Came up because:** temp files should be attributable to their entity; and the
  panel flagged that an unbounded entity id could exceed the filesystem per-
  component limit, plus the empty-id vs literal-`exec` alias nit.
- **Options:** drop the entity label entirely (random-only) / keep it unbounded /
  keep it bounded + hashed.
- **Chose bounded + hashed because:** attributable filenames aid debugging;
  bounding fixes the length limit; the full-id hash keeps the truncated segment
  distinguishing; the `e` prefix makes the empty id unambiguous. Injective
  encoding (`_`+hex for every non-[alnum-] byte) also closed the earlier Greptile
  lossy-sanitizer finding. Given up: a few extra chars per filename.
- **Where:** `capture_run_id`.

## CORRECTION: same-entity execs are NOT serialized (earlier reasoning was wrong)
- **Decision:** Reverse the earlier call that "entity scope suffices because the
  actor model serializes same-entity dispatch." It does not; the fix above adds
  per-dispatch uniqueness.
- **Came up because:** I first argued (from `actor/traits.rs` "one message at a
  time" + the absence of a per-message `tokio::spawn`) that a same-entity exec
  collision could not occur, and resolved the panel's P1 as invalid. The panel
  (Codex + Fable, independently) showed the window is real: the exec is a
  long-running side effect (polls up to 600 iterations) that does NOT hold the
  actor for its whole duration — which is precisely why a ~30-min run is a known
  limitation (ARN-443 part D). So two same-entity dispatches can have overlapping
  execs.
- **Options:** keep dismissing it (wrong) / add the per-dispatch discriminator.
- **Chose to fix because:** the collision is the exact ARN-401 class, one level
  down. The lesson: do not infer an execution-model guarantee from the absence of
  a spawn without tracing the integration dispatch path; a long-running
  integration can outlive the transition that fired it.
- **Where:** superseded by the random-u64 decision above; analysis in spec.md.

## Round 3: fail the exec on RNG error — delete the fallback
- **Decision:** If `random_get` errors, `random_u64` returns `Err` and the exec
  fails (`RunFailed`, "host RNG unavailable"); no fallback capture id.
- **Came up because:** the round-2 panel (Codex) flagged the heap-address fallback
  as not unique — allocator addresses can repeat across calls/instances, so the
  fallback could itself collide, defeating the fix.
- **Options:** cleverer fallback (a clock, a counter — all can repeat) / delete it
  and fail loudly.
- **Chose delete-and-fail because:** an exec that cannot obtain randomness must not
  risk a capture-file collision, and `random_get` erroring is already an abnormal
  host state (WASI wires it). Fix by deletion, not cleverness. Given up: nothing —
  the fallback only ever traded a hard failure for a silent collision risk.
- **Where:** `os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs` `random_u64`
  (now `Result<u64, String>`), propagated via `unique_run_id`? → `tensorlake_exec`.
