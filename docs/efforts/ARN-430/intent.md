# ARN-430: stage-3 phase 1 - record entities + shadow ingest (S0)

## Problem
Today the SDLC loop's trust state lives in PR comments: each review and proof
record is a base64 blob inside an HTML comment, parsed by workflow bash. Stage 3
moves that state onto Temper so the whole loop is one queryable state machine.
The first step (S0, "mirror-in") has to exist before anything can read Temper
instead of comments: paw-patrol needs entities that can hold a review record and
a proof record, and a module that turns today's comment blob into those fields.
Until that exists there is nothing to compare CI against and no path to the
later shadow and flip phases.

## Proposed outcome
paw-patrol carries the record shape as governed entities and can ingest a raw
GitHub comment body into them:

- `ReviewRun` (was `review_run`) holds a full commit sha, the reviewers that
  ran, the findings inline, the synthesized risk, and the open act-on count,
  with a `Running -> Recorded -> Superseded` record lifecycle.
- `ProofRun` (was `proof_packet`) holds the `proof.json` shape: changed surface,
  blast radius, features with verdicts and steps, tests, and the independent
  verifier.
- `Adjudication` and `StandingDecision` are the two small new entities the panel
  loop needs (owner rulings and the global standing decisions).
- `ShadowVerdict` is the temporary S1 comparison row, added now so the shape is
  in place; it is retired at S3.
- `record_ingest` is a WASM module that parses a comment body's `-b64` markers
  and returns the decoded record fields; the state machine writes them. It never
  creates entities and never dispatches transitions.

Verifiable by replaying already-merged PRs (475, 477, 480): the module's decoded
fields match the comment records.

## Affected users and systems
paw-patrol only. No CI workflow, no Cedar policy, no `WorkCycle -> Effort`
rename, no gate-behavior change. Nothing reads these entities to gate a merge in
S0 - they are written as a shadow mirror. The existing `review_run` /
`proof_packet` lifecycles and the `review_gate_lifecycle` wasm stay untouched.

## Constraints
- Additive only: the existing entities keep every state, field, action, and
  trigger they have today (the foundation test and `review_gate_lifecycle`
  depend on them). New record fields and a new record sub-lifecycle are added
  beside them.
- The record shape must match the live `-b64` markers exactly (stack
  `proof/schema.json` and `review/schema.json`), so replayed PRs decode.
- `record_ingest` parses and returns; it does not write. One concern.
- WASM pinned to the temper server's current rev.

## Open questions (answered in spec.md / decisions.md)
- Replace or extend `review_run` / `proof_packet`? -> extend (repo reality).
- How are "40-char sha" and "reviewers_ran non-empty" enforced when the guard
  grammar has no string predicates? -> see decisions.md.
