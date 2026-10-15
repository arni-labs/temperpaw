# ARN-441 spec (temperpaw slice)

Canonical RFC: `arni-labs/stack` `docs/efforts/ARN-441/spec.md`. This file is the
temperpaw-implementable slice for the paw-patrol app; read the stack RFC for the
full design and acceptance criteria.

## Owner ruling that shapes this PR (2026-08-31)

Option **B, additive-then-flip** (see decisions.md). The rename premise recorded in
the foundation ("WorkRequest/WorkCycle are not the live driver") was contradicted by
source: `crates/paw-codex-worker` (temper_api.rs:111, `entity_url("WorkCycles")`) and
`dashboard/` (`queryEntities('WorkCycles')`) drive/render the live WorkCycle via the
CSDL EntitySet names. A hard rename would 404 the live worker and blank the dashboard.
So Intent and Effort are created as NEW types the new lifecycle speaks; WorkRequest/
WorkCycle, the worker, and the dashboard stay untouched and retire at the phase-3
flip (like risk_rule and the CI gates).

## PR 1a scope (this PR)

Additive, declarative, no wasm change:

- **Intent** (`specs/intent.ioa.toml`) - the WorkRequest shape in SDLC vocabulary.
  `Accept` creates an Effort (create + Seed) under `patrol-intake-service`.
- **Effort** (`specs/effort.ioa.toml`) - WorkCycle EXTENDED to the full lifecycle:
  `Intended (initial) -> Specified -> Planned -> Building -> InReview -> Proving ->
  Merged -> Deploying -> Verified`, with `Stalled` (recoverable) and `Abandoned`
  (terminal). Chain reference fields (intent_id, review_run_ids, proof_packet_ids,
  deployment_id, adjudication_ids, pm_issue_id), design-chain refs (spec_ref,
  plan_ref), and gate markers back the state guards. Pre-attachment states
  (Intended/Specified/Planned) are allow_indefinite (no owner, no lease).
- **CSDL** - Intent/Effort EntityTypes + EntitySets Intents/Efforts.
- **Cedar** - Intent intake for agents; Effort lifecycle for the system/patrol
  surface; Effort birth (create + Seed) locked to `patrol-intake-service`.
- **GitHub doors** - `intent_ref` / `spec_ref` / `plan_ref` / `decisions_ref`
  are git paths (`docs/efforts/<issue>/*.md`). Attach* runs
  `chain_github_ready`. Accept / Specify / Plan / Merge refuse unless that
  path is a file on GitHub. ReviewRun / ProofPacket attach an HTML or JSON
  Temper File; those are not committed.
- **Panel** - one ReviewRun per panel agent. Each run is written with
  `RecordPanel` (Requested → Recorded). PassReview is called after three
  attaches. The kernel door is `panel_started` (verifier counter bound is
  2, so it cannot require count 3).
- **Merge** - Cedar door. L0/L1 permit; L2+ denies and surfaces as MCP
  elicitation. Merge does not call GitHub.
- **Deploy** - Effort.Deploy stores an opaque `deployment_id`. The implementer
  creates `DsfDeploy` (DSF merge+watch+revert) or `TemperDeploy` (GHCR
  IMAGE_TAG → Railway → /paw/version). Child Healthy → Effort.Verified;
  RolledBack → Merged; Failed → Stall. Live ReleaseRun / DeployRun rows
  are not renamed. No ConfigureRelease on Effort.

## Not required to use

A WorkerRun heartbeat lease on Effort. The implementer is the harness; #492
stays unmerged.

## Deferred (recorded, not dropped)

- WorkCycle's `AwaitingHuman*` pause states: completion-approval is Cedar-deny
  -> MCP elicitation on Merge; a start-approval pause, if wanted, becomes a
  StartBuild guard in a later step.
- `Resume` out of Stalled will require an Adjudication guard (a later step).
- Intake routing for Intent (the new-type equivalent of patrol_request_router)
  generalizes in a later step; the Effort is born at Accept without it.
- github_mirror, gate_render-to-GitHub, kernel entity namespacing (ARN-28).
  ARN-28 is blocked until this SDLC is proven in use.

## Acceptance (this PR)

L0-L3 cascade + composite cross-entity verification pass with Intent/Effort; all
wasm still build; the app boots on an isolated TURSO_URL; one synthetic Effort is
hand-driven Intent.Accept -> Effort created (Intended) -> ... -> Verified, read back via
OData at each step. Genesis publish -> install after merge.
