## Decisions & Tradeoffs

**Decision:** Extend `review_run` and `proof_packet` additively (keep their whole
existing lifecycle; add the record fields, a `Recorded/Superseded` sub-lifecycle,
and the ingest actions) instead of replacing their automata with the definitive
`Running > Recorded > Superseded` shape.
**Came up because:** the task and `stage3-spec.md`'s definitive shape describe
ReviewRun/ProofRun with a `Running > Recorded > Superseded` automaton, but that
is the later full-migration target; phase 1 is scoped "no Effort rename, no gate
changes."
**Options:** replace the automata now (rejected); additive extension (chosen).
**Chose additive because:** `work_cycle` references `review_run`/`proof_packet`
by id, `review_gate_lifecycle` wasm is fired by `review_run`'s existing actions,
and `crates/temperpaw/tests/paw_patrol_foundation.rs` asserts specific existing
actions (`ReviewRun.RequestChanges`, `ReviewRun.Escalate`) and states. Replacing
the automata would ripple into `work_cycle` and the gate wasm - both out of
phase-1 scope - and break the foundation test. AGENTS.md's rule applies: "if the
spec contradicts the repo's reality, follow the repo and record the deviation."
Given up: a single clean record automaton now; the full collapse lands with the
Effort migration (a later phase), where the gate wasm and work_cycle move too.
**Where:** `specs/review_run.ioa.toml`, `specs/proof_packet.ioa.toml`.

**Decision:** All record-shape validation (40-char commit sha; review has a
non-empty `reviewers_ran`; proof has a non-empty `changed_surface` and no
feature with `verdict=="fail"`) lives in the `record_ingest` boundary as
`parse_ok`, NOT in transition guards. The entities keep only a `record_present`
bool and an invariant that asserts it in the record states.
**Came up because:** the task asks for these as guards, but two facts about the
platform (temper-spec / temper-jit rev 43f9379) make them inexpressible as
guards: (1) the guard grammar has no string-length, regex, or per-element-array
predicate - only `state_in`, `min_count`/`max_count`, `is_true`/`is_false`,
`list_contains`, `list_length_min`, `cross_entity_state`; and (2) `build_eval_context`
builds the guard context from the entity's CURRENT state only - the incoming
action's params are NOT visible to guards, and guards are preconditions
evaluated before effects apply. So a `list_length_min reviewers_ran 1` guard on
`IngestRecord` would read the pre-ingest empty list and always fail; a guard
fundamentally cannot inspect the record being written.
**Options:** invent guard predicates + params-in-guard-context in the kernel
(rejected - out of scope, this is an app effort); split the record into `list`
fields plus a companion `counter` set via `set_counter_from_param` so an
invariant could assert `count > 0` (rejected - added redundant fields and an
artificial count, and still could not check the sha shape or per-feature
verdict); enforce everything in the ingest boundary and keep one `record_present`
invariant (chosen).
**Chose the ingest boundary because:** a record only reaches `IngestRecord` /
`IngestProof` when the module returned `parse_ok`, and `parse_ok` already
requires all four checks; so a malformed record cannot become a Recorded run.
`record_present` (set by the ingest action) plus the invariant `when [Recorded,
Superseded] assert record_present` make "a run in the record states went through
a checked ingest" a durable, expressible guarantee. Every record field is a
plain `string` (findings/features/tests as JSON), matching how `proof_packet`
already stores `proof_json` - no `list`/`counter` fields, so no first-adopter
risk on an unused field type. Given up: declarative, in-spec enforcement of the
shape - it is enforced in tested module code instead. Residual risk: someone
dispatching `IngestRecord` by hand with a bad record bypasses the module; noted
for the later phase that moves writes behind a governed (Cedar) action.
**Where:** `wasm/record_ingest/src/lib.rs` (`parse_record`, `record_shape_ok`,
`is_full_sha`); `record_present` invariant in `specs/review_run.ioa.toml` and
`specs/proof_packet.ioa.toml`.

**Decision:** Register the three new entities (`Adjudication`,
`StandingDecision`, `ShadowVerdict`) in `policies/patrol.cedar`'s Admin permit
and the any-principal read/list permit only; do not add them to the
action-bound system-agent permit.
**Came up because:** the phase-1 scope says "no Cedar", but a Temper entity with
no matching permit is default-denied and cannot be acted on at all - the same
Admin catch-all that governs every existing paw-patrol entity has to list them
or they are dead.
**Options:** leave them out of Cedar entirely (rejected - they would be
un-actionable, even in tests/e2e); add them to every permit including the
system-agent action-bound block (rejected - that block enumerates specific
actions per principal, so adding the new actions there is authorization *logic*,
which is the Cedar work phase 1 excludes); register them under the Admin +
read/list permits (chosen).
**Chose the minimal registration because:** it makes the entities governable and
readable exactly like their siblings without introducing new authorization
logic. In S0 nothing writes these entities as a system agent (record_ingest only
parses and returns; the write-back orchestration is a later phase), so the
system-agent write permits are not needed yet and are deferred to the phase that
wires their writers. Given up: system-agent write access now.
**Where:** `os-apps/paw-patrol/policies/patrol.cedar` (Admin permit + read/list
permit).

**Decision:** Implement `record_ingest` with the SDK's `temper_module!` macro
(returns a `Value`), not the manual `#[no_mangle] fn run` pattern the other
paw-patrol modules use.
**Came up because:** every existing module is a trigger that makes OData writes
via `Context::from_host`; `record_ingest` must instead be a pure parser that only
returns fields.
**Options:** copy the manual pattern (rejected - it is built around dispatching
writes, the opposite of this module); use `temper_module!` (chosen).
**Chose the macro because:** it is exactly "read context, return a Value", which
is this module's whole job, and it keeps the parser a plain unit-testable
function. Given up: visual consistency with the sibling modules. The parsing lives
in `fn parse_record(body: &str) -> ParsedRecord`, host-testable with no wasm.
**Where:** `wasm/record_ingest/src/lib.rs`.

**Decision:** Pin `record_ingest`'s `temper-wasm-sdk` to rev
`43f9379cc51545b9a47b8a28ccb202c31957a0e9` (the server's current rev), while the
existing modules are still pinned to `b0c79312...`.
**Came up because:** the task specifies the server's current rev; the other
modules have not been re-pinned since their last bump.
**Options:** match the siblings at `b0c79312` (rejected - not the server rev);
pin to `43f9379` (chosen).
**Chose 43f9379 because:** it is the rev the running server builds against, so the
committed `.wasm` matches the host ABI it will load under. Given up: a single
uniform SDK rev across the app until the others are bumped - noted as a follow-up,
not blocking, since the ABI is compatible.
**Where:** `wasm/record_ingest/Cargo.toml`.

---

## Round 2 - panel round 1 fixes (ARN-430)

**Decision:** Merge `origin/main` into the branch to clear two "CI act-ons".
**Came up because:** the branch predated the merge of #479 (the CI fast/full
lane split), so the PR diff appeared to revert `ci.yml`. Two panel act-ons were
this stale-branch artifact, not real changes.
**Options:** rewrite ci.yml to re-add the fast lane (rejected - it would be
duplicating a merge, and the effort must not touch ci.yml); merge origin/main
(chosen).
**Chose the merge because:** after it, `git diff origin/main..HEAD --
.github/workflows/ci.yml` is empty - the PR touches no CI file, which is the
correct end state. Given up: nothing.
**Where:** merge commit on `claude/arn-430-stage3-s0`.

**Decision:** Wire the ingest with an `Ingest` self-loop action on ReviewRun
(from Requested) and ProofPacket (from Drafting) that fires `record_ingest`;
the module returns a dynamic callback action + the decoded fields, and the
kernel dispatches `IngestRecord` / `IngestProof` to write them.
**Came up because:** panel act-on - no entity transition invoked record_ingest
or mapped its output onto entity fields; the module was unreferenced.
**Options:** static `on_success = "IngestRecord"` on the trigger (rejected - a
single static action cannot route review vs proof, cannot reject a
cross-fed/invalid record without an error, and on_success fires on ANY module
success regardless of parse_ok); the dynamic callback action (chosen).
**Chose the dynamic callback because:** the kernel's rule is "prefer static
on_success, else the module's returned callback_action; a bare `callback`
dispatches nothing". So the module returns `IngestRecord` / `IngestProof` only
when the record is valid AND its kind matches the entity it was fired on
(checked via `ctx.entity_type`), and the inert `callback` otherwise. A comment
with no record, a malformed record, or a record fed to the wrong entity writes
nothing and raises no error - no spurious failures on the Discord channel. This
is the sanctioned "module returns a callback, the kernel applies it" pattern
(same as `workspace_provisioner` -> `WorkspaceReady`); sequencing stays in the
state machine, the module makes no OData call and creates nothing. Given up: a
purely declarative on_success wire.
**Where:** `specs/review_run.ioa.toml` (`Ingest` + trigger), `specs/proof_packet.ioa.toml`
(`Ingest` + trigger), `wasm/record_ingest/src/lib.rs` (`ingest_action`, `run`).

**Decision:** Implement `record_ingest` with the manual `#[unsafe(no_mangle)]
run` pattern, not the `temper_module!` macro.
**Came up because:** the dynamic callback action above needs the module to set a
custom callback action (`IngestRecord`/`IngestProof`); the macro hardcodes the
action to `"callback"`.
**Options:** the macro (rejected - cannot set the callback action, so it can
only work with a static on_success, which the routing rules out); the manual
pattern (chosen), which the other paw-patrol modules already use.
**Chose the manual pattern because:** it lets the module return the right write
action per entity+kind. The parsing stays in pure, unit-tested functions
(`parse_record`, `record_shape_ok`, `write_params`, `open_act_on_count`); only
the thin `run` wrapper is host-only. Given up: the macro's brevity. (This
reverses the round-1 decision to use the macro.)
**Where:** `wasm/record_ingest/src/lib.rs`.

**Decision:** Align the proof `parse_ok` checks with `stack/proof/validate.py`,
not just "non-empty changed_surface + no failing feature".
**Came up because:** panel act-on - `parse_ok` was true for proofs the stack
validator would reject.
**Options:** keep the loose checks (rejected); mirror the validator's
record-intrinsic rules (chosen).
**Chose the validator's rules because:** the entity should only Record a proof
the gate would accept. `proof_shape_ok` now enforces: non-empty
`changed_surface`; every changed + blast_radius feature present in `features[]`
with `verification == "rerun"`; `independent_verifier.agrees` and its `reran`
covers changed + blast; no `verdict == "fail"`; a `verified-unreachable`
feature carries a reason; every UI feature has screenshots and no failed
judgment; `tests.result == "pass"`. The features-dir and URL-evidence rules are
CI-only (need external context) and stay out of the module. Each rule has a
rejection test. Given up: nothing - the two real proof fixtures (PR 477, 480)
still pass. **Where:** `wasm/record_ingest/src/lib.rs` (`proof_shape_ok`).

**Decision:** The module emits the flat top-level fields the ingest transitions
need, including a derived `open_act_on_count`.
**Came up because:** panel act-on - the parser did not emit the fields the write
actions consume.
**Options:** map fields in the spec (not possible - specs cannot transform);
emit them from the module (chosen).
**Chose module emission because:** `write_params` returns exactly the
`IngestRecord` / `IngestProof` param names, arrays/objects serialized to JSON
strings to match the entities' string fields, and derives `open_act_on_count`
as the number of `severity == "act-on"` findings that are not `resolved`. Tested
against the real PR-477 (1 open act-on) and PR-480 (0) records plus a synthetic
mix. Given up: nothing. **Where:** `wasm/record_ingest/src/lib.rs`
(`write_params`, `open_act_on_count`).

**Decision:** Make ShadowVerdict `agree` a real boolean, set through
`MarkAgree` / `MarkDisagree` self-loops (set_bool), not a string param.
**Came up because:** panel act-on + nit - the contract (and spec.md) says
boolean; it was declared string.
**Options:** keep string (rejected - wrong type); pass a bool param (rejected -
booleans are not populated from action params in this kernel; only `set_bool`
writes the boolean store); the set_bool flag pattern (chosen).
**Chose the flag pattern because:** it is how every other spec sets a boolean
(distinct actions with a `set_bool` effect), so `agree` is a true `Edm.Boolean`
that guards/OData see correctly. `Record` writes the string fields; `MarkAgree`
/ `MarkDisagree` set the flag. Given up: recording agreement in one action.
ShadowVerdict is written by nothing in S0 (its writer is S1), so this only
fixes the shape. **Where:** `specs/shadow_verdict.ioa.toml`, `Agree` ->
`Edm.Boolean` in `specs/model.csdl.xml`.

**Decision (the panel "consider"):** Ingest only from Requested (ReviewRun) /
Drafting (ProofPacket), not from the terminal review/proof states.
**Came up because:** the reviewer noted records arrive after runs reach terminal
outcome states, so ingest might need to fire from those too.
**Options:** allow Ingest from every state incl. terminals via a transient
"Ingesting" state (rejected - a multi-`from` self-loop is impossible, so it
needs a transient state plus an origin-restoring failure path, which is real
machinery); ingest from the initial state only (chosen).
**Chose initial-state-only because:** the S0 shadow mirror creates a FRESH
ReviewRun/ProofPacket per record (the shadow is a separate authority from the
operational review workflow - "exactly one authority per gate"), so the
record-carrying run starts at its initial state and ingests there. Reusing the
operational run that already went through review is not the S0 flow. Ingest is
a clean self-loop on the initial state; a comment with no record leaves it
there. Given up: ingesting onto an already-terminal operational run - not needed
in S0, and revisited if the flow ever reuses those runs.
**Where:** `specs/review_run.ioa.toml`, `specs/proof_packet.ioa.toml`.

---

## Round 3 - panel round 2 fixes (ARN-430, terminal batch)

**Decision:** The no-record path returns an EMPTY callback action, not `"callback"`.
**Came up because:** a reviewer (codex) found the "inert callback" was not inert:
for a non-Composite integration the kernel does dispatch the returned action.
**Verified against the kernel (rev 43f9379):** `engine/telemetry.rs` builds
`callback_action` as `result.get("action").as_str().unwrap_or("")`, and
`dispatch/wasm.rs` only dispatches `if !callback_action.is_empty()`. The
"callback" -> "" zeroing at wasm.rs:1291 is guarded by `composite_result_consumed`,
which is false for a plain trigger - so returning `"callback"` WOULD try to
dispatch a non-existent `callback` action (an error). An empty action is the
sanctioned no-op.
**Options:** return `"callback"` (rejected - dispatches and errors); return an
error so `on_failure` fires (rejected - a no-record comment is normal, not a
failure to surface on the Discord channel); return an empty action (chosen).
**Chose the empty action because:** it dispatches nothing, stays success, raises
no error. `NO_DISPATCH = ""`; a test asserts the no-record, malformed, and
wrong-entity cases all yield exactly `""`. Given up: nothing.
**Where:** `wasm/record_ingest/src/lib.rs` (`run`, `ingest_action`, `NO_DISPATCH`).

**Decision:** parse_ok is strict, typed extraction with a field-naming reason,
not a loose value check.
**Came up because:** panel act-ons - the checks skipped the schema-level typing
the stack validator's jsonschema pass does (a field could be the wrong JSON type
and still pass).
**Options:** port the whole JSON schema (rejected - heavy, and validate.py runs
jsonschema separately); make extraction strict (chosen).
**Chose strict extraction because:** every field the write actions map is now
required with the right JSON type, else `parse_ok=false` and `reason` names the
offending field. Review: `commit` (string, 40-hex), `reviewers_ran` (non-empty
array of strings), `findings[]` (objects with string `severity`/`file_line`, bool
`resolved`), `risk` in {low, medium, high}. Proof: `commit`; `changed_surface`
(non-empty array of strings), `blast_radius` (array of strings); `features[]`
(objects with string `key`/`verification`/`verdict`, `verdict` in
{pass, fail, verified-unreachable}); `tests` object with `result` in {pass, fail};
`independent_verifier` object with `reran` (array of strings) and bool `agrees` -
then the stack proof rules over that surface. The `verdict` enum check subsumes
fable's verified-unreachable point (an out-of-enum verdict fails extraction).
There is a rejection test per class (missing key, wrong type, bad enum). Given
up: nothing - both real records (PR 477, 480) still validate.
**Where:** `wasm/record_ingest/src/lib.rs` (`validate_review`, `validate_proof`,
the typed `*_field` accessors).

**Decision (consider):** ShadowVerdict `MarkAgree` / `MarkDisagree` write the two
verdicts AND the boolean in one action each; `Record` writes only the identity.
**Came up because:** a reviewer noted the verdict pair and the agree flag could
diverge if set by separate actions.
**Chose one-action-each because:** the grammar allows params (the two verdict
strings) and a `set_bool` effect on the same action, so the comparison and its
summary flag are written atomically and cannot drift. Given up: nothing.
**Where:** `specs/shadow_verdict.ioa.toml`.

**Decision (consider):** State the re-ingest and Superseded semantics in the
specs. Recorded -> Recorded is a re-ingest (a corrected record for the SAME head
overwrites in place, last write wins); Superseded is terminal (a record retired
by a NEWER record for a LATER head, which lives on its own run/packet; this one
is kept for history, no outgoing transitions).
**Where:** comments in `specs/review_run.ioa.toml`, `specs/proof_packet.ioa.toml`.

---

## Round 4 - terminal micro-batch (ARN-430)

Owner ruling: full proof-schema parity in WASM is out of scope - strict typed
extraction of the fields the write actions map IS the contract. The other three
findings are one-line fixes, applied here.

**Decision:** Enum-check `findings[].severity` in {act-on, consider, nit} during
extraction (out-of-enum -> parse_ok=false naming the field).
**Came up because:** `open_act_on_count` trusts `severity == "act-on"`; an
unchecked severity could silently mis-count.
**Where:** `wasm/record_ingest/src/lib.rs` (`validate_review`), with a rejection
test.

**Decision:** Drop the dead `Recorded` re-ingest from-state from IngestRecord /
IngestProof; a run/packet ingests once from its initial state.
**Came up because:** the S0 shadow creates a FRESH run/packet per record (this
effort's own design), so a Recorded -> Recorded re-ingest is a path nothing
takes - dead surface, not a feature.
**Options:** keep it and document (rejected - documenting a dead path); remove it
(chosen). **Chose removal** so the reachable states are exactly Requested ->
Recorded (-> Superseded). A corrected record is a new run; the prior run is
retired with Supersede (last-created wins). The spec comments and spec.md now say
this. **Where:** `specs/review_run.ioa.toml`, `specs/proof_packet.ioa.toml`,
`docs/efforts/ARN-430/spec.md`.

**Decision:** Reject duplicate `features[].key` in a proof record.
**Came up because:** duplicate keys would let one feature's verdict/verification
mask another under the by-key lookup.
**Where:** `wasm/record_ingest/src/lib.rs` (`validate_proof`), with a rejection
test.

**Decision (consider):** Document that ShadowVerdict `agree` is COMPUTED by the
S1 comparison sweep from the two verdicts; the entity cannot verify the equality
itself until the kernel has param-aware guards (gap already filed).
**Where:** `specs/shadow_verdict.ioa.toml`.
