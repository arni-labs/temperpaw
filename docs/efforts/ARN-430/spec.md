# Spec: stage-3 S0 - record entities + shadow ingest (ARN-430)

This is the RFC for phase 1 of stage 3. It implements exactly the S0 "mirror-in"
row of the shadowing plan in `stack/docs/stage3-spec.md`: the record entities and
the `record_ingest` module, testable against already-merged PRs. It deliberately
does not touch CI, Cedar, the `WorkCycle -> Effort` rename, or any gate behavior.

## The contract

One module and five entity changes, all in `os-apps/paw-patrol`.

### record_ingest (new WASM module, temporary - retired at S3)

Fired by `ReviewRun.Ingest` / `ProofPacket.Ingest` on a comment body. The parsing
is a pure function; the `run` wrapper turns it into a callback the kernel applies.

- INPUT: `comment_body` - a raw GitHub issue/PR comment body string (from
  `ctx.trigger_params`); plus `ctx.entity_type` (the entity it was fired on).
- OUTPUT (the returned callback): a callback *action* and its *params*.
  - On a valid record whose kind matches the entity: action `IngestRecord`
    (ReviewRun) / `IngestProof` (ProofPacket), with params = the decoded fields
    flattened (arrays/objects serialized to JSON strings to match the string
    fields), plus a derived `open_act_on_count` for review.
  - Otherwise: an EMPTY action, which the kernel does not dispatch (a bare
    `"callback"` would NOT be inert on a plain trigger - it would try to dispatch
    a non-existent action - so the no-op is the empty string).

Behavior of the pure parser (`parse_record`):

1. Scan for a marker `<!-- sdlc-review-record-b64\n<base64>\n-->` first, then
   `<!-- sdlc-proof-record-b64\n<base64>\n-->`; `kind` is `review`/`proof`/`none`.
2. Base64-decode the payload, parse JSON, read `commit`.
3. `parse_ok` is true only when: a marker was found, the payload decodes to a
   JSON object, `commit` is a 40-character lowercase-hex sha, AND the record-shape
   checks hold (review: non-empty `reviewers_ran`; proof: the stack proof rules,
   see ProofRun below).

The kernel builds `callback_action` from the result's `action` (defaulting to
empty) and only dispatches when it is non-empty, and it prefers a static
`on_success` over the module's returned action. The wiring therefore uses a
dynamic action and no `on_success`: the module returns the write action ONLY when
`parse_ok` AND the kind matches `ctx.entity_type`, and an EMPTY action otherwise,
so a comment with no record, a malformed record, or a record fed to the wrong
entity writes nothing and raises no error. The module creates no entities and makes no OData calls; the state
machine writes, through `IngestRecord` / `IngestProof`.

### ReviewRun (was review_run) - additive record lifecycle

Existing states and actions (`Requested/Claimed/Reviewing/ChangesRequested/
Approved/Escalated/Failed`, and their `review_gate_lifecycle` triggers) are
unchanged. Added:

- States: `Recorded`, `Superseded` (both indefinite).
- Fields (all `string` except the bool): `commit`, `reviewers_ran` (JSON array),
  `findings` (JSON array of `{severity, by, file_line, claim, failure_scenario,
  resolved}`), `risk`, `open_act_on_count`, `record_present` (bool).
- `Ingest`: `from [Requested] -> Requested` (self-loop), params `comment_body`,
  effect fires the `record_ingest` trigger. The module returns the `IngestRecord`
  callback + fields when the comment holds a valid review record; otherwise an
  empty action (no dispatch), so the run stays in Requested.
- `IngestRecord`: `from [Requested] -> Recorded` (once per run - S0 creates a fresh run per record), params
  `commit, reviewers_ran, findings, risk, open_act_on_count`. Effect:
  `set_bool record_present true`. Dispatched by the kernel from the module's
  callback.
- `Supersede`: `from [Recorded] -> Superseded`, when a newer record for a later
  head replaces this one.
- Invariant `RecordedHasRecord`: `when [Recorded, Superseded] assert
  record_present` - a run in the record states must have gone through
  `IngestRecord`, which is only dispatched on a `parse_ok` record (checked
  commit sha + non-empty reviewer set). The record-shape checks are NOT
  transition guards: the guard grammar has no string/array predicates and cannot
  see action params (see decisions.md).

### ProofRun (was proof_packet) - additive record lifecycle

Existing states/actions (`Drafting/Ready/Rejected`) unchanged. Added:

- States: `Recorded`, `Superseded` (both indefinite).
- Fields (all `string` except the bool): `commit`, `changed_surface` (JSON
  array), `blast_radius` (JSON array), `features` (JSON array of the proof.json
  feature objects), `tests` (the proof.json tests object), `independent_verifier`
  (the proof.json object), `record_present` (bool).
- `Ingest`: `from [Drafting] -> Drafting` (self-loop), params `comment_body`,
  fires `record_ingest`; the module returns the `IngestProof` callback + fields
  when the comment holds a valid proof record, else an empty action (no dispatch).
- `IngestProof`: `from [Drafting] -> Recorded` (once per packet - S0 creates a fresh packet per record), params
  `commit, changed_surface, blast_radius, features, tests, independent_verifier`.
  Effect: `set_bool record_present true`. Dispatched by the kernel from the
  module's callback.
- `SupersedeProof`: `from [Recorded] -> Superseded`.
- Invariant `ProofRecorded`: `when [Recorded, Superseded] assert record_present`.

The proof-shape checks are enforced in `record_ingest`'s `parse_ok` (not as
transition guards - the grammar cannot inspect per-element array verdicts or read
action params), and they mirror the record-intrinsic rules of
`stack/proof/validate.py`: non-empty `changed_surface`; every changed + blast
feature present in `features[]` with `verification=="rerun"`;
`independent_verifier` agrees and re-ran changed + blast; no `verdict=="fail"`; a
`verified-unreachable` feature has a reason; every UI feature has screenshots and
no failed judgment; `tests.result=="pass"`. A proof that fails any check returns
`parse_ok=false`, so `IngestProof` is never dispatched.

### Adjudication (new)

`Active` (single indefinite state, initial). Fields: `scope` (file:line or a
class name), `ruling` (string), `source` (the owner who ruled), `effort_ref`.
Action `Record`: `from [Active] -> Active`, params `scope, ruling, source,
effort_ref`.

### StandingDecision (new)

`Active -> Retired`. Fields: `text` (the ruling injected into every panel
prompt). Actions: `Adopt` (`Active -> Active`, param `text`), `Retire`
(`Active -> Retired`).

### ShadowVerdict (new, temporary - retired at S3)

`Recorded` (single indefinite state, initial). Fields: `effort_ref`, `pr`,
`gate`, `temper_verdict`, `ci_verdict` (strings), `agree` (bool, `Edm.Boolean`).
`Record` (params `effort_ref, pr, gate`) writes the identity; `MarkAgree` /
`MarkDisagree` (params `temper_verdict, ci_verdict`, `set_bool agree`) each write
the two verdicts AND the flag in one action, so `agree` can never diverge from
the verdicts it summarizes - booleans are set by effects, not params.

## Model / CSDL

`specs/model.csdl.xml` gains the three new EntityTypes + EntitySets and the new
properties on ReviewRun/ProofPacket. The foundation test asserts these by name,
so CSDL and the ioa specs move together.

## What this is not

No CI change, no Cedar policy, no rename, no gate flip. Those are later phases
(S1-S3 and the Effort state machine). ShadowVerdict is written by nothing in S0;
its writer is S1. The entities are a shadow mirror in S0 - nothing reads them to
gate a merge.

## Test plan (red-green)

Rust tests that feed `record_ingest`'s parser the real base64 comment bodies from
merged PRs 477 and 480 (475 carried no `-b64` markers) and assert the decoded
fields (kind, commit, reviewers_ran / changed_surface, parse_ok) match the
comments; a malformed-record case (marker present, payload not valid base64)
asserting `parse_ok == false`; a no-marker case asserting `kind == "none"`; the
no-op routing (empty action) for no-record / malformed / wrong-entity; the
derived `open_act_on_count`; and a strict-extraction rejection test per class
(missing field, wrong JSON type, bad enum) asserting the reason names the field.
