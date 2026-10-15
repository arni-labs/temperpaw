# ARN-441 step 1 (temperpaw) — plan: additive Intent + Effort

Owner ruling: **Option B, additive-then-flip** (see decisions.md). CREATE `Intent`
and `Effort` as NEW entity types that the new lifecycle speaks; leave
`WorkRequest`/`WorkCycle` specs, wasm, the paw-codex-worker, and the dashboard
UNTOUCHED (they retire at the phase-3 flip, like risk_rule and the CI gates).

## What we are addressing

The SDLC's execution unit becomes a first-class, queryable lifecycle that begins at
intent and ends at a verified deploy, with the design chain (intent/spec/plan) as
gated states and orphan detection as a state-machine property (the lease). Named in
the domain vocabulary from birth so no NEW machinery hardcodes the legacy names.

## Expected end state

- `Intent` entity (the renamed WorkRequest shape) accepts intake. `Accept`
  creates an `Effort` (entity-trigger, principal-elevated) seeded with
  `intent_id` + intent.md.
- `Effort` entity drives `Intended → Specified → Planned → Building → InReview →
  Proving → Merged → Deploying → Verified`, with `Stalled` reachable from the active
  states (lease timeout) and from InReview, and `Abandoned` as the failure terminal.
- Design-chain doors: `Specify` refuses unless spec.md is on GitHub;
  `Plan` refuses without plan.md; `Accept` refuses without intent.md;
  `Merge` refuses without decisions.md. `chain_github_ready` retracts the
  ready bool if the git path is missing. Review/proof stay Temper Files.
- Chain reference fields on Effort: `intent_id`, `review_run_ids`,
  `proof_packet_ids`, `deployment_id`, `adjudication_ids`, `pm_issue_id`.
- No WorkerRun lease on Effort. The implementer is the harness (Computer/Exec).
- One ReviewRun per panel agent, written with `RecordPanel` (Requested →
  Recorded) plus a Temper File. PassReview after three attaches; kernel
  door is `panel_started`.
- Merge is Cedar (L0/L1 permit; L2+ elicitation). Deploy records a `DsfDeploy`
  or `TemperDeploy` id; those tools report Healthy / RolledBack / Failed.
- CSDL: Intent/Effort + DsfDeploy/TemperDeploy EntityTypes and sets. Cedar
  permits for the new surfaces. Genesis publish → install; one synthetic Effort
  hand-driven through the states.

## Proposed Effort automaton (for lead confirmation before build)

States (initial `Intended`; terminals `Verified`, `Abandoned`):

| State | Meaning | Entered by | Guard/door |
|---|---|---|---|
| Intended | born at intent acceptance; intent.md attached | Intent.Accept (create) | — |
| Specified | spec.md written | Specify | wasm refuses if spec.md absent |
| Planned | plan.md written | Plan | wasm refuses if plan.md absent |
| Building | implementer working | StartBuild (guard has_plan) | — |
| InReview | under independent review | SubmitForReview (guard worker_done) | — |
| Proving | review passed, proof/eval | PassReview + e2e | guard review_passed |
| Merged | PR merged (governed at step 3) | Merge (guard records/proof) | Cedar at step 3; state guard now |
| Deploying | deploy in flight (step 4 links Deployment) | Deploy | guard merged |
| Verified | deploy verified — success terminal | Verify | guard deployment verified |
| Stalled | owner missed lease TTL, or review stall | state_timeout(active) / Stall | Resume needs an Adjudication (step later) |
| Abandoned | given up — failure terminal | Abandon (from any active) | — |

Design-chain refs and the bool markers (`spec_written`, `plan_written`,
`worker_done`, `review_passed`, `proof_attached`, `deploy_verified`) back the
guards. The implementer is a harness Agent: work is Computer/Exec; this row is
the record. No WorkerRun lease.

## Diff-size recommendation (lead's call, deferred to me)

Estimated ~1000 lines across Intent spec, Effort spec, CSDL, Cedar, a new
effort_lifecycle wasm, and tests. Both halves are ADDITIVE (create-only, no rename),
so splitting carries none of the rename risk that folding them avoided. Proposed
split for reviewability:
- **PR 1a** — Intent + Effort entity TYPES (full state/field shape) + CSDL sets +
  Cedar + the Intent.Accept→Effort birth handoff. Proof: cascade + all-wasm build +
  app boot + hand-dispatch an Effort through the states with the wasm stubbed/minimal.
- **PR 1b** — chain-file guards + `effort_lifecycle` wasm (generalized) + WorkerRun
  Heartbeat + the state_timeout lease. Proof: synthetic drive incl. a forced
  lease-timeout → Stalled and a missing-spec.md refusal.

Awaiting lead confirm on the automaton + the split before writing spec/wasm.

## Verify (Definition of Done for step 1)

L0–L3 cascade (verify-temperpaw / edit hook) · build all wasm wasm32-wasip1, zero
wbindgen · boot on isolated TURSO_URL · hand-drive one synthetic Effort through every
state (transcript) · genesis publish → install → verify pinned ref.
