# Plan: stage-3 S0 (ARN-430)

## What we are addressing
paw-patrol cannot hold or ingest the SDLC records yet. Add the record entities
and the `record_ingest` parser so the S0 mirror-in step exists and is testable
against merged PRs. Additive only - no CI/Cedar/rename/gate change.

## Expected end state
- `specs/review_run.ioa.toml`: existing lifecycle intact + record fields, the
  `IngestRecord`/`Supersede` actions, and the `RecordedHasRecord` invariant.
- `specs/proof_packet.ioa.toml`: existing lifecycle intact + proof.json fields,
  `IngestProof`/`SupersedeProof`, and the `ProofRecorded` invariant.
- New specs: `adjudication.ioa.toml`, `standing_decision.ioa.toml`,
  `shadow_verdict.ioa.toml`.
- `specs/model.csdl.xml`: new properties on ReviewRun/ProofPacket + three new
  EntityTypes and EntitySets.
- `wasm/record_ingest/`: the module (pure `parse_record` + `temper_module!`
  wrapper) with a committed `record_ingest.wasm`, pinned to the server rev.
- `app.toml` + `wasm/build.sh`: register `record_ingest`.
- `crates/temperpaw/tests/paw_patrol_foundation.rs`: extended to assert the new
  entities/fields/module (keep it green).
- Design chain committed; draft PR open; not merged.

## Steps
1. Design chain (this dir). [done first]
2. Specs: extend review_run, proof_packet; add the three new entities.
3. CSDL: properties + entity types + entity sets.
4. `record_ingest` module: `parse_record` pure fn + macro wrapper + Cargo.toml
   pinned to rev 43f9379..., committed `.wasm`.
5. Tests: real-PR fixtures (475/477/480) + malformed + no-marker; run red-green.
6. Register module in app.toml + build.sh; update foundation test.
7. `cargo fmt --check`, clippy, run touched tests + foundation test.
8. Stage explicit paths, commit (conventional), push as rita-aga, open draft PR.

## Out of scope (later phases, recorded so they are not dropped)
- S1 ShadowVerdict writer + nightly disagreement sweep.
- S2 per-gate CI flips to OData reads; S3 composite check + retiring
  record_ingest and ShadowVerdict.
- Effort state machine (`WorkCycle -> Effort`), Merge/Deploy governed actions,
  Cedar policies, the webhook transport (paw-triggers/paw-ingest).
