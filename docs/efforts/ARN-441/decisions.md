# Decision log - ARN-441 step 1 (temperpaw side: rename migration + Effort lifecycle)

The canonical RFC/design chain is in `arni-labs/stack` `docs/efforts/ARN-441/`
(intent/spec/plan/decisions). This file is the temperpaw PR's decision log for the
paw-patrol spec+policy changes; the PR body carries these entries verbatim.

---

**Decision:** The WorkRequest->Intent / WorkCycle->Effort rename is a NEW entity
type per name; existing WorkRequest/WorkCycle prod rows are marked LEGACY, not
carried over.
**Came up because:** step 1 must decide "carry rows over or mark legacy - investigate
what the platform supports" (RFC naming section).
**Options:** (a) an automatic rename/migration that re-keys existing rows to the new
type; (b) new entity types, old rows left legacy under their old type names.
**Chose (b) over (a) because:** the platform has NO entity-type rename/alias/migration
mechanism - persistence keys on the type name (`persistence_id = "{tenant}:{entity_type}:{entity_id}"`,
temper `entity_actor/actor.rs:339`), and no genesis/registry rename path exists
(verified: no rename/alias handling in the kernel). Re-keying would be a kernel
change, which is explicitly out of scope for ARN-441. Marking legacy is safe here
because WorkRequest/WorkCycle are NOT yet the live stage-2 driver - stage 2 runs on
PR comments + check runs + bash routing (RFC "what we are addressing"), so no live
prod flow dispatches them; the renamed Intent/Effort entities are driven
synthetically in shadow (plan step 1). Given up: query continuity for any old rows
(they age out; the SDLC entities are short-lived per-effort rows, not durable state).
**Where:** paw-patrol `specs/intent.ioa.toml` (was work_request), `specs/effort.ioa.toml`
(was work_cycle), `specs/model.csdl.xml`, `policies/patrol.cedar`, wasm references.

---

**Decision:** Execute the rename as an atomic total rename across the declarative
layer AND the wasm layer in one PR, verified by cascade + wasm build + app boot -
NOT a partial "spec-only" pass.
**Came up because:** the reference survey found the rename is far larger than the
plan's "spec change only, no wasm" framing: ~700 references across ~30 files,
including 10 wasm Rust modules (review_gate_lifecycle 157, patrol_run_lifecycle 100,
work_cycle_lifecycle 39, worker_run_lifecycle 36, patrol_request_router 35,
signal_router 28, daily_brief_lifecycle 28, finding_lifecycle 26, repo_sweep 17),
plus work_cycle.ioa.toml (21), patrol.cedar (19), model.csdl.xml (18), and ~10 other
specs that cross-reference the types. The wasm modules reference WorkRequest/WorkCycle
as entity-type strings (cross-entity dispatch/guards) and field names (work_cycle_id
etc.), so a declarative-only rename would fail to install (cross-entity references to
a renamed type break; wasm dispatch to the old type 404s).
**Options:** (a) declarative-only step 1, wasm later (leaves a broken intermediate
that can't shadow-drive); (b) atomic total rename in one PR.
**Chose (b) because:** shadow-first requires nothing breaks - a half-rename breaks.
The rename must be complete (all 700 refs) to install and be driven. Given up: a
smaller first PR - but a rename cannot be half-done.
**Rename map:** `WorkRequest`->`Intent`, `WorkCycle`->`Effort`; reference fields
`work_request_id`->`intent_id`, `work_cycle_id`->`effort_id`; file renames
work_request.ioa.toml->intent.ioa.toml, work_cycle.ioa.toml->effort.ioa.toml. ADRs
are historical records - left with the then-current names (a note added). release_run
->Deployment is step-4 scope; only the `deployment_id` reference field on Effort lands
now.
**Where:** all os-apps/paw-patrol specs + model.csdl.xml + patrol.cedar + the 10 wasm
modules + APP.md.

---

**Decision:** Split ARN-441 step 1 into TWO PRs: a pure mechanical rename PR first,
the Effort-lifecycle additions second (owner ruling, 2026-08-31).
**Came up because:** the rename is ~700 refs across 10 wasm modules; the earlier
"one atomic PR" plan mixed a huge mechanical change with judgment work.
**Options:** (a) one PR (rename + lifecycle); (b) rename-only PR first, lifecycle
PR second.
**Chose (b) over (a) because:** owner ruling - a ~700-ref/10-wasm mechanical change
wants isolation. PR 1 = pure rename (types + fields + files + cedar + csdl + wasm
refs), ZERO behavior change, proven by cascade + all-wasm build + app boot +
synthetic hand-dispatch drive; it merges + publishes, then PR 2 adds the six Effort
states + chain-file guards + chain reference fields + WorkerRun Heartbeat on top as
the judgment PR. Isolating the blast radius makes both reviewable and revertible.
Given up: nothing - two smaller PRs are strictly easier to review than one.
**Where:** PR 1 (rename) then PR 2 (lifecycle) on temperpaw, `claude/arn-441-*`.

---

**Decision:** This effort is handed off to a successor session (owner-accepted,
2026-08-31); the authoring session did NOT execute the rename surgery.
**Came up because:** the mandate is ARN-441 all 5 steps to merged+published+installed;
the authoring session had run the full ARN-438 arc and was context-heavy with a
non-zero error rate at quality-critical moments, and step 1 is a prod-breaking
~700-ref rename.
**Options:** (a) push the tired context through the multi-PR mandate; (b) hand off
to a fresh-context successor with a full brief.
**Chose (b) because:** the ownership rule (quality bar over raw progress); a clean
context lowers error risk on a prod-breaking multi-step effort. Owner accepted and
recorded it as the model for the honest hand-off call.
**Where:** handoff brief at scratchpad/ARN-441-handoff-brief.md; foundation
(migration decision, rename survey/map, this chain) preserved on
`claude/arn-441-entity-loop`.

---

**Decision:** (BLOCKER surfaced to owner, 2026-08-31, successor session) The
recorded migration premise - "WorkRequest/WorkCycle are NOT the live stage-2
driver; renamed entities driven synthetically in shadow" - is CONTRADICTED BY
SOURCE. A pure paw-patrol-only, zero-behavior-change, shadow-safe rename PR is
therefore not achievable; escalated to the team-lead/owner for an A-vs-B ruling
before any rename surgery.
**Came up because:** the reference survey went repo-wide (not just paw-patrol) and
found WorkCycle/WorkRequest are live-driven by two OTHER deployable units.
**Evidence (verified in source):**
- crates/paw-codex-worker (compiled prod worker): `temper_api.rs:111`
  `config.entity_url("WorkCycles", work_cycle_id)`; `fetch_work_cycle_until_review_passed`
  polls WorkCycle (`temper_api.rs:123-137`); `event_loop.rs` claims queued WorkerRuns
  and drives the cycle; posts `work_cycle_id` / `Fail`. ~70 refs.
- dashboard/ (Svelte): `stores/workcycles.ts` `queryEntities('WorkCycles', ...)` +
  `getEntity('WorkCycles', id)`; `app-views/paw-patrol.ts` addresses the `WorkCycles`
  set and `WorkCycleId` columns + relation graph.
- CSDL: `<EntitySet Name="WorkCycles" EntityType="TemperPaw.Patrol.WorkCycle"/>`
  (+ WorkRequests). Worker + dashboard address the OData SET names; a clean rename
  makes the set `Efforts` and 404s the live worker / blanks the dashboard.
- Also independent, MUST-NOT-TOUCH WorkCycle concepts in other apps:
  `os-apps/paw-harness/specs/work_cycle.ioa.toml` (+ its cedar/csdl) and
  `reference-projects/deep-sci-fi/dsf-harness/specs/dsf_work_cycle.ioa.toml` - separate
  entities, out of ARN-441 scope.
**Options:** (A) coordinated hard cutover across 3 pipelines (app publish+install,
paw-codex-worker rebuild+deploy, dashboard deploy) in a quiet window - clean end
state, but no atomic flip => breakage window for in-flight efforts, worker/dashboard
come into scope; (B) additive-then-flip - add Effort/Intent as new types for the new
lifecycle, keep WorkCycle/WorkRequest live for worker+dashboard until the phase-3
flip (same "retire AT the flip" discipline as gate_render/merge-permit/risk_rule),
no prod breakage, two names coexist only during the shadow window.
**Chose escalate-and-hold over proceeding on the recorded premise because:** the
premise is prod-breaking if wrong (a clean rename 404s the live worker and blanks
the dashboard), and the A-vs-B fix is an owner call, not an implementer default;
the cost was time-to-first-commit, the gain was correctness on a prod-breaking
scope decision. Recommended B (shadow-first, matches the rest of ARN-441) pending
the owner ruling.
**Where:** finding messaged to team-lead 2026-08-31; no code touched yet.

---

**Decision:** (OWNER RULING, team-lead 2026-08-31) Option B - additive-then-flip.
**Came up because:** the source-contradiction blocker above needed an A-vs-B call.
**Options:** (A) coordinated hard cutover of all three units (paw-patrol app +
paw-codex-worker + dashboard) in a quiet window - the rejected default of a literal
rename; (B) additive-then-flip - new Intent/Effort types now, legacy retires at the
phase-3 flip.
**Chose B because:** (1) the approved spec's spine is "nothing existing retires until
its entity twin proves out live" - A (hard 3-unit cutover with a breakage window)
contradicts the spec Rita approved; B is what the spec implies. (2) Rita's naming
ruling's REASON was "before more machinery hardcodes legacy names" - B honors it fully:
Intent/Effort are created NOW and every NEW piece speaks ONLY the domain names (zero
new legacy hardcoding); the legacy names survive only in already-deployed consumers
(worker, dashboard) and retire AT the flip, like risk_rule and the CI gates. Bounded
coexistence, not a permanent tax. (3) At flip time the retirement of
WorkCycles/WorkRequests is a normal coordinated change on by-then read-mostly
machinery - no quiet-window heroics.
**Scope for PR 1a (folds old 1a+1b into one additive PR):** CREATE Intent + Effort
types at full shape - Effort with Intended-initial six-stage lifecycle, chain-file
guards, chain reference fields, WorkerRun Heartbeat + state_timeout lease; Intent as
the renamed WorkRequest shape with Accept->Effort handoff - plus Cedar, CSDL sets
Intents/Efforts, and the wasm handlers for the NEW types (work_cycle_lifecycle logic
GENERALIZED, not moved). WorkCycle/WorkRequest specs/wasm/worker/dashboard UNTOUCHED.
A flip-time retirement note is added to the RFC Not-in-scope.
**Where:** team-lead ruling 2026-08-31; PR on claude/arn-441-entity-loop.

---

**Decision:** (lead confirm + lease refinement, 2026-08-31) Effort automaton
approved as proposed; lease timeout applies ONLY to owned states.
**Came up because:** the lease exists for orphan PICKUP, not death; pre-attachment
states have no owner, so a timeout there is noise, not orphan detection.
**Options:** for the pre-attachment states (Intended/Specified/Planned): (a) a long
state_timeout -> Stalled (the rejected default of "every state gets a lease"); (b)
allow_indefinite (no lease at all).
**Chose (b) allow_indefinite over (a) because:** it is the honest "no owner, no
lease" model, and the Intent entity's own intake timeouts already cover "nobody
picked this up"; given up automatic surfacing of a forgotten spec'd-but-unbuilt
effort (acceptable - with no owner there is nothing to be orphaned from). The owned
states Building/InReview/Proving/Merged/Deploying get state_timeout -> Stalled in 1b;
Stalled/Verified/Abandoned are allow_indefinite resting states. Timeout target is
Stalled (recoverable), never Fail/Abandoned.
**WorkCycle AwaitingHuman* pause states (where they went):** NOT dropped - the
completion-approval pause becomes step 3's Cedar-deny->MCP elicitation; a
start-approval pause, if still wanted, becomes a guard on StartBuild in a later step.
Both recorded in the RFC deferred list.
**Split (approved):** PR 1a = Intent + Effort types (full shape) + CSDL sets Intents/
Efforts + Cedar + Intent.Accept->Effort birth. PR 1b = chain-file guards +
effort_lifecycle wasm (generalized) + WorkerRun Heartbeat + state_timeout lease, with
a forced timeout->Stalled drive as proof. Both additive/create-only.
**Where:** lead ruling 2026-08-31; building 1a now.

---

**Decision:** (2026-08-31, successor - during the synthetic drive) The Effort
lifecycle Cedar permit grants the verified operator (agent_type "operator" &&
agentTypeVerified) the full Effort transition set, alongside Admin/system/supervisor.
**Came up because:** the verify-temperpaw drive authenticates with Bearer
TEMPER_API_KEY, which the platform resolves to agent_type "operator" (confirmed in
the boot log: `Intent.Accept ... agent_type:"operator"`). The first drive 403'd on
`Effort.Specify` - the initial permit only allowed Admin/system/supervisor.
**Options:** (a) broaden the permit to the verified operator; (b) find a way to drive
as a system agent (blocked - temperpaw's bearer path resolves to operator, and
"system" is never accepted from inbound headers per ADR-0157); (c) weaken to any
Agent (rejected - too broad).
**Chose (a) over (b)/(c) because:** the verified operator is the SAME trusted
sweep/verification authority ARN-434 already grants the S0/S1 record writes
(`agent_type == "operator" && agentTypeVerified == true`, so a self-declared header
agent never matches) - it is the legitimate driver of the shadow SDLC entities, and
(b) is not reachable from the operator credential. The per-gate authority flips
(steps 3-4) put stricter permits on Merge/Deploy, tightening this surface there.
Given up: nothing for the shadow phase; the operator surface narrows at the flips.
**Where:** `os-apps/paw-patrol/policies/patrol.cedar` (Effort lifecycle permit block).

---

**Decision:** (#491 panel round 1, 2026-08-31) Split the Effort Cedar permits: birth
(create + Seed) ONLY for patrol-intake-service; the lifecycle permit drops Seed.
**Came up because:** both reviewers found the lifecycle grant let verified operators
Seed, bypassing the birth rule (an Effort is born only by accepting an Intent).
**Options:** (a) keep one combined permit (rejected - it lets a lifecycle driver
conjure Efforts); (b) split birth from lifecycle.
**Chose (b) because:** it makes the birth rule hold in policy, not just convention.
Also dropped the Effort.Seed-vs-Seed dual-id hedge (the live drive log shows the bare
`Seed` fires: temper.action "Seed"), and hoisted `principal has agent_type` before
every == (schema-less Cedar must deny-on-missing explicitly). Given up: nothing.
**Where:** patrol.cedar Effort lifecycle + birth permits; verified by a live negative
drive (operator POST /tdata/Efforts -> 403).

---

**Decision:** (#491 panel round 1) Chain-id collections are real `list` fields, not
JSON-in-a-string.
**Came up because:** both reviewers flagged review_run_ids/proof_packet_ids/
adjudication_ids as JSON-blob strings - an entity-first violation (outside code must
parse/rewrite blobs).
**Options:** (a) keep JSON strings; (b) real list fields appended per-link via
list_append.
**Chose (b) because:** the kernel has a real list type + list_append effect (ARN-92:
the kernel takes real arrays). The attach actions append one element each; CSDL is
Collection(Edm.String). Verified live: review_run_ids reads back as ["rev-1"] (a JSON
array, not a string). Given up: nothing. **This decision flows into 1b's chain-file
guards** (they read the lists, not blobs).
**Where:** effort.ioa.toml (3 list fields + list_append on AttachReviewRun/
AttachProofPacket/Resume), model.csdl.xml (Collection).

---

**Decision:** (#491 panel round 1) pm_issue_id has a single source - the Intent -
and is dropped from the Effort row.
**Came up because:** pm_issue_id was copied to the Effort at birth, but Intent.
LinkPmIssue can set it AFTER Accept, so the copy goes stale.
**Options:** (a) copy at birth (stale); (b) add an Effort update path when the Intent
links later (more machinery + the Intent->Effort reverse-ref problem); (c) drop the
copy, read via intent_id.
**Chose (c) because:** at Accept the pm_issue_id is not even set yet (LinkPmIssue is
Accepted->Linked, after Accept), so the birth copy is premature AND stale-prone;
reading it from the Intent via intent_id is the single source. Given up: pm_issue_id
is not directly on the Effort row (one join via intent_id) - acceptable, the Intent
IS the Linear mirror holder.
**Where:** effort.ioa.toml (dropped field + Seed param), model.csdl.xml, intent params_from.

---

**Decision:** (#491 panel round 1) A failed Effort birth is recoverable via a
RebirthEffort repair action; intent.md is referenced at birth; AttachSignal is a
self-loop.
**Came up because:** three reviewer findings - (2) Accept commits before the birth
trigger with no repair path; (5) the birth did not reference intent.md (an acceptance
criterion); (6) AttachSignal from Triaged regressed the Intent to Submitted.
**Options:** (2) entity-trigger on_failure (uncertain support) vs a repair action;
(5) copy the ref vs attach the file; (6) self-loop vs split per-from-state.
**Chose:** (2) a RebirthEffort action (Accepted->Accepted) carrying the same create
trigger - a supported, explicit repair (semantics: use only when no Effort exists for
the Intent, so it cannot double-birth); (5) copy intent_ref through Seed (guaranteed
at birth, verified live); (6) drop AttachSignal's `to` so it stays in the current
state. Given up: nothing.
**Where:** intent.ioa.toml (RebirthEffort, intent_ref, AttachSignal self-loop),
effort.ioa.toml (intent_ref), paw_patrol_foundation.rs (birth-wiring test - the
"take it" consider).

---

**Decision:** (#491 panel round 2, 2026-08-31) Close the second birth door with a
Cedar `forbid`, remove Effort from the generic system-agent permit, and restrict
RebirthEffort to the intake service; widen RebirthEffort to be reachable from Linked.
**Came up because:** round 2 found (1) the generic system-agent permit granted
`create` on Effort - a second birth door; (2) RebirthEffort was callable by any
Agent; (4) RebirthEffort was unreachable after LinkPmIssue->Linked.
**Options:** (1) narrow the generic permit vs add a forbid - the lead said forbid
beats permit, use it if the permit can't be narrowed cleanly; (4) widen from-states
with an explicit `to` (regresses Linked->Accepted) vs a self-loop (no `to`, stays put).
**Chose:** BOTH narrow AND forbid for (1) - removed Effort from the generic list AND
added `forbid(create/Seed on Effort) unless patrol-intake-service`, so every door
(Admin blanket, system, future strays) is closed; verified live (operator POST
/tdata/Efforts -> 403, birth via Accept still works). (2) moved RebirthEffort into an
intake-service-only permit. (4) self-loop (dropped `to` and the trigger's optional
`to_state`) so RebirthEffort stays in Accepted/Linked and is reachable from both.
Given up: nothing.
**Where:** patrol.cedar (forbid + permits), intent.ioa.toml (RebirthEffort self-loop).

---

**Decision:** (DESIGN POINT to owner, #491 round 2) The idempotency guard for
RebirthEffort (#2) and the intent_ref-non-empty enforcement (#3) need the
wasm-validated-guard pattern; recommend implementing them in 1b's effort_lifecycle
wasm, not 1a.
**Came up because:** both fixes hit kernel mechanism limits (verified in source):
kernel ACTION guards have NO string check (only Always/StateIn/CounterMin,Max/
BoolTrue,False/ListContains/ListLengthMin/CrossEntityStateIn/And - temper-jit
table/guard.rs), so "effort_id != ''" / "intent_ref != ''" cannot be a kernel guard;
and the entity-trigger (resolve_target=create) cannot write the created id back to
the source (only the `spawn` effect's store_id_in can, but spawn's copy_fields copies
same-named parent STATE fields and cannot map the Intent's `Id` to Seed's intent_id).
**Options:** (a) switch birth to `spawn` (gets effort_id back-ref) + solve the
intent_id mapping somehow; (b) enforce both via a wasm that validates (query-before-
create for idempotency; non-empty for intent_ref) - the SAME pattern 1b's
effort_lifecycle wasm already uses for spec.md/plan.md; (c) a kernel change (out of
scope).
**Chose (b), recommend to 1b because:** the Cedar/declarative hardening that closes
the SECURITY-critical open door (#1 forbid, #2 permit, #4) lands in 1a NOW; the
wasm-validated integrity guards (idempotency, non-empty) are the identical pattern to
1b's chain-file guards and belong with them, keeping 1a declarative. Pending owner
adjudication (round 3 residual).
**Where:** recommendation messaged to team-lead; guards to land in 1b effort_lifecycle wasm.

---

**Decision:** (owner ruling, 2026-08-31, Cursor takeover) Delete RebirthEffort.
Birth is Accepting → ConfirmBirth → Accepted; fail or timeout returns to Triaged.
Retry is Accept. Temper has no source-transition rollback (create/spawn are
post-transition), so Accepted is the state that means an Effort exists — not a
repair button on an already-Accepted Intent.
**Came up because:** a reviewer invented RebirthEffort after Accept committed
before create. Everything that followed (Cedar lock, Linked widen, idempotency
wasm, Seed↔LinkEffort cycle, false ARN-448 blame, two deferrals) was that
decision compounding. Rita: birth should be atomic or rolled back.
**Options:** (1) keep the repair button and finish its idempotency; (2) kernel
transaction (out of scope, not a primitive); (3) Accepting handshake — Accept
does not mean done; Seed confirms; timeout/fail back to Triaged.
**Chose (3) because:** it is the honest machine given the kernel. What we gained:
one birth door, no second button, no idempotency wasm, no cycle. What we gave
up: a create-then-Seed-fail orphan Effort with no back-ref (rare; retry Accept
creates another). Not worth a repair action.
**Where:** intent.ioa.toml (Accepting, ConfirmBirth, BirthTimedOut, 30s timeout);
effort.ioa.toml (Seed → ConfirmBirth); patrol.cedar; paw_patrol_foundation.rs.

---

**Decision:** Disaster inventory (same takeover). One-bad-decision cascades to
delete or hold; not more machinery.
**Came up because:** Rita asked to find every RebirthEffort-class disaster, not
only the button.
**Inventory:**
- D1 RebirthEffort + lock + widen + idempotency wasm + LinkEffort cycle —
  DELETED (this PR). Do not re-land.
- D2 paw-fs verified-operator File-create grant (#492) — HOLD. Punched a hole
  in another app so a drive identity could create chain files. Not on main.
  Do not merge #492 as-is.
- D3 Seed↔LinkEffort cyclic back-ref — already stripped on #492; do not
  re-land. The boot crash was later shown to be base paw-fs (ARN-448), which
  does not restore the cycle.
- D4 "1b must finish before any real shadow Effort" — HOLD. Lease + chain-file
  doors are real factory work; they are not a gate on birth. #492 stays open
  and unmerged. Do not race entity-loop-2 on arni-big.
**Keep (not disasters):** intake-only Effort birth forbid; AttachSignal
self-loop; list chain fields; pm_issue_id single-sourced on Intent; Computer/
Exec; #494 C+D (real 120s WASM / copy-won't-kill-source). Additive Intent/
Effort (the 700-ref rename never shipped).
**Where:** this file; GitHub #492 hold comment; Linear ARN-441 takeover comment.

---

**Decision:** (owner, 2026-08-31) The Accepting/ConfirmBirth/BirthTimedOut
handshake is also deleted. Accept goes to Accepted and creates the Effort.
No confirm, no timer, no extra Cedar.
**Came up because:** Rita: "i dont want any of the birth stuff -- its still
overcomplicated." The handshake was compensation for a kernel gap that has
not shown up, same class as RebirthEffort.
**Options:** (1) keep the handshake; (2) Accept + create trigger only, live
with a rare failed create.
**Chose (2) because:** one action, one trigger. The failure case does not
earn a protocol. Kernel transaction stays a temper item if it ever matters.
**Where:** intent.ioa.toml, effort.ioa.toml (Seed trigger removed),
patrol.cedar, foundation test, APP.md, spec.md, plan.md.

---

**Decision:** (owner, 2026-08-31) Temper does not spawn the implementer.
paw-codex-worker is not the Effort path. Strip WorkerRun-lease fields from
Effort; any tenant Agent may write the Effort record (create/Seed stay
intake-only).
**Came up because:** the missing-piece story was "StartBuild must spawn a
worker." Rita: the implementer harness does the work through Computer/Exec
and uses Temper to record it. Stage 3: machine work = Exec; agent spawn =
panel/arbiter only; the gate never spawns.
**Options:** (1) keep AttachWorkerRun + owner_worker_run_id + operator-only
lifecycle so a drive identity walks the row; (2) delete the WorkerRun lease
from Effort and let the implementer Agent write the record.
**Chose (2) because:** (1) is the old Mac-mini worker. WorkCycle keeps that
shape until the flip; Effort does not copy it. What we gave up: a lease
heartbeat on Effort (not needed for a harness that is already in session).
**Where:** effort.ioa.toml, model.csdl.xml, patrol.cedar Effort permit,
plan.md, spec.md.

---

**Decision:** (2026-08-31) Effort.Deploy creates the existing ReleaseRun.
Effort.Merge is the authorization door only — it does not call GitHub.
**Came up because:** Rita asked to get Stage 3 working with the settled
topology. Merge/Deploy were empty doors; ReleaseRun is already live and
proven (merge + watch + rollback through the computer).
**Options:** (1) new Deployment entity (rejected — no-orphans: Deployment
is release_run extended); (2) put the create trigger on Merge (collapses
the two doors); (3) ConfigureRelease + Deploy creates ReleaseRun.Request
the same way WorkCycle.Complete does.
**Chose (3) because:** it reuses the proven machine and keeps Merge as
the Cedar gate. ReleaseRun.work_cycle_id receives the Effort id (WASM
does not read that field). Reverse Healthy→MarkDeployVerified is not
wired — the implementer marks verified after the ReleaseRun is Healthy.
**Where:** effort.ioa.toml (ConfigureRelease + effort_deployed_requests_release),
model.csdl.xml, patrol.cedar, paw_patrol_foundation.rs.

---

**Decision:** (2026-09-01) Cursor cannot complete Temper MCP elicitation, so
Rita asked this harness to resolve pending decisions in-session with the
operator approver key. Approve Intent denials as broad for `Agent::"cursor"`.
Do not approve the `manage_policies` PD. Install the missing Intent/Effort
Cedar as the operator instead.
**Came up because:** Observe/UI approval is unusable from this chat; Cursor's
host drops `elicitation/create` (~30–60s, `-32001`). Five Intent-create PDs
and one PolicySet `manage_policies` PD were pending. Live `paw-patrol-patrol`
Cedar has no Intent/Effort rules (specs were hot-loaded; policy was not).
**Options:** (1) tell Rita to click Observe; (2) approve every PD including
`manage_policies` so the harness becomes a policy admin; (3) operator-approve
the Intent PDs, leave `manage_policies` pending, append Intent/Effort Cedar
as the operator.
**Chose (3) because:** (1) is the UX she rejected; (2) is a standing PolicySet
grant this harness does not need (claude-code already has that grant from
earlier elicitations). What we gained: Intent walk without UI. What we gave
up: Cursor still cannot complete elicitation itself — later denials still
need this same operator resolve, or a host fix.
**Where:** live `decision:PD-01a05a94-…` (and four sibling Intent PDs);
`arn-441-intent-effort` policy appended via operator
`POST /api/tenants/default/policies/create`; PD-01a05a60-ce89 left pending.

---

**Decision:** (2026-09-01) File doors land now. `intent_ref` / `spec_ref` /
`plan_ref` are Temper File ids. Attach* runs `chain_file_ready` (GET File,
require Ready/Locked); on_failure retracts the ready bool. Specify / Plan /
Accept also have `is_true *_file_ready` plus `cross_entity_state File … Ready`.
**Came up because:** Rita: do not punt the file-existence door. Kernel guards
cannot read file bytes or action params; empty `spec_ref` would vacuous-pass
a cross-entity guard alone.
**Options:** (1) `effort_lifecycle` kitchen-sink (rejected); (2) handshake
states (rejected — same class as ConfirmBirth); (3) Attach + check + retract
+ Specify/Plan/Accept kernel guards.
**Chose (3) because:** the GET is one concern; the door that refuses a missing
File is the kernel guard on the next action. Residual: a lying Mark/Attach
bool with an empty id can race Specify before retract if integrations are not
awaited. Retract clears the bool.
**Where:** intent.ioa.toml, effort.ioa.toml, wasm/chain_file_ready,
patrol.cedar, model.csdl.xml.

---

**Decision:** (2026-09-01) One ReviewRun per panel agent (Grok, Codex, Claude).
`PassReview` requires `review_run_ids` length ≥ 3 and each ReviewRun
Approved or Recorded.
**Came up because:** Rita: cleaner to query than one run with `reviewers_ran[]`.
**Options:** (1) one run per round with a reviewers array; (2) one run per agent.
**Chose (2) because:** each agent's write is its own row; the Effort list is
the panel. What we gave up: a single ingest of today's combined review.json
still lands as one Recorded run (S0) — that run is one of the three only if
the other two agents also write.
**Where:** review_run.ioa.toml, effort.ioa.toml PassReview, APP.md.

---

**Decision:** (2026-09-01) Review and proof records are Temper rows plus
optional Temper Files (HTML or JSON). GitHub comments and Vercel are not
the record. GET File `$value` returns the File's `mime_type` — `text/html`
renders as HTML in a browser, `application/json` as JSON.
**Came up because:** Rita: if Temper can display the file, do not keep a
GitHub comment / Vercel copy as the source.
**Options:** (1) keep dual-home comment + Vercel; (2) Temper entity + File
as the artifact, surfaces render from that.
**Chose (2) because:** that is the Stage 3 record. Vercel stays only if we
choose to host a File whose mime is HTML there — not as a second source.
**Where:** APP.md ReviewRun; this log. github_mirror not added.

---

**Decision:** (2026-09-01) TemperPaw production deploy is not "Railway
watches main." The stack driver default is `DEPLOY_MODE=image`: upsert
`IMAGE_TAG` (skipDeploys) + redeploy, verify `/paw/version` sha, roll back
by restoring the previous tag. `source` mode only verifies a release-branch
push that already started a Railway source build. Genesis install is a
different path (hot-install an app pin, no Railway redeploy).
**Came up because:** the ReleaseRun preview described merge-to-main + curl
SHA + git revert as how TemperPaw deploys. Rita: it is more complicated.
**Options:** (1) keep ReleaseRun's merge/revert as the TemperPaw deploy;
(2) treat that WASM as one profile and name the image-tag driver as the
actual TemperPaw ship path.
**Chose (2) because:** that is what `stack/deploy/railway-deploy-verify-rollback.sh`
does. ReleaseRun as written matches `source` mode, not the default image
path. Not implemented in this change — recorded so the next deploy wiring
does not lie.
**Where:** stack/deploy/railway-deploy-verify-rollback.sh; genesis-temperpaw-deploy
skill (install-from-genesis).

---

**Decision:** (2026-09-01) Do not start Temper entity namespacing until the
SDLC is proven in use. Until then, two deploy tools must not share an
automaton name or EntitySet. Those names are storage keys, not different
verbs. The shared class is the Genesis app. After the kernel change, both
automata become `Deploy` under an app prefix.
**Came up because:** `ReleaseRun` vs `Deployment` as two domain words was a
collision workaround. Rita: same class; kernel should namespace; this is
part of the effort but not before we are actually using the SDLC; many
`/tdata` callers.
**Options:** (1) implement app-qualified types in Temper now; (2) unique
flat keys now, kernel namespacing after SDLC is live (ARN-28); (3) appended
ids (`ReleaseRun_dsf`) forever.
**Chose (2) because:** (1) is a big kernel + caller migration with no
working loop to enumerate callers; (3) is the same flat map. Interim keys:
live DSF merge+watch stays `ReleaseRun`/`ReleaseRuns` (do not rename live
rows); Howl HTTP tool stays `DeployRun`/`DeployRuns`. New types use parallel
names, not `Deploy` vs `DeployRun`: `TemperDeploy`/`TemperDeploys` and
`DsfDeploy`/`DsfDeploys`. Never mint a flat `Deploy`/`Deploys` — that name
is what both tools take after ARN-28. Effort stores opaque `deployment_id`.
No `ConfigureRelease` on Effort.
**Where:** [ARN-28](https://linear.app/arni-build/issue/ARN-28) (blocked on
ARN-441); registry strip at temper-server `registry/mod.rs` `rsplit('.')`.

---

**Decision:** (2026-09-01) Effort is the shared SDLC parent. Project deploy
tools are children, not extra Effort verbs. `DsfDeploy` reuses
`release_run_lifecycle`. `TemperDeploy` is the IMAGE_TAG path. Effort.Deploy
only stores `deployment_id`. No ConfigureRelease. No repo string-match.
**Came up because:** Rita rejected DeployImage/DeployDsf, Effort-in-a-project,
and ReleaseRun-vs-Deployment as three shapes. Same Effort lifecycle; ship
differs per project.
**Options:** (1) Effort.Deploy creates ReleaseRun (previous, wrong for
TemperPaw); (2) one Deploy type with repo matching; (3) two named children
that report back.
**Chose (3) because:** DSF ship is merge+watch+revert on the computer;
TemperPaw ship is GHCR tag → Railway IMAGE_TAG → /paw/version. The join is
opaque id forward + entity-trigger back. What we gave up: Effort cannot
create the child (kernel `target_entity` is monomorphic) — the implementer
creates the tool, then records its id.
**Where:** effort.ioa.toml; dsf_deploy.ioa.toml; temper_deploy.ioa.toml;
temper_deploy_lifecycle; patrol.cedar; model.csdl.xml.

---

**Decision:** (2026-09-01) Deploy failure: child Healthy →
Effort.MarkDeployVerified → Verified. RolledBack → MarkDeployRolledBack →
Merged (retry Deploy). Failed → Stall only when rollback did not finish.
**Came up because:** Stall-on-clean-rollback was wrong. A proper deploy
rolls production back; the Effort is not dead, it is unshipped.
**Options:** (1) any child terminal Stalls the Effort; (2) only Failed
Stalls; RolledBack returns to Merged.
**Chose (2) because:** Stall is for a world that may be inconsistent, not
for a finished rollback. What we gave up: a single terminal on Effort for
every deploy outcome.
**Where:** effort.ioa.toml MarkDeploy*; dsf_deploy / temper_deploy Fail and
RollbackPushed triggers.

---

**Decision:** (2026-09-01) `pm_issue_id` and `decisions_ref` live on Effort
again, with `LinkPmIssue` and `AttachDecisions`.
**Came up because:** #491 dropped pm_issue_id from Effort (single-source on
Intent). The Stage 3 chain Rita locked lists pm_issue_id on the Effort row,
and decisions.md is a gated file like spec/plan.
**Options:** (1) keep the join via intent_id only; (2) put both on Effort
with explicit attach/link actions.
**Chose (2) because:** the Effort is “everything about this work”; Linear
and the decision log attach after birth and would go stale if only copied
once. What we gave up: one source for the Linear id (Intent and Effort can
diverge if someone links only one).
**Where:** effort.ioa.toml; model.csdl.xml.

---

**Decision:** (2026-09-01) PassReview’s kernel door is `panel_started` (at
least one AttachReviewRun). Three ReviewRuns stay the implementer rule.
`panel_count` is visible but not a guard.
**Came up because:** live Effort was 423 Locked. L1 cannot require
`panel_count >= 3` — temper-verify’s default counter bound is 2 — and
ReviewRun cross-entity over a list is a dead guard.
**Options:** (1) live with 423; (2) three named attach actions / three
bools; (3) local bool door plus a visible counter.
**Chose (3) because:** it is the File-door pattern and it verifies. (2)
is more machinery for a bound we do not own. What we gave up: the kernel
does not prove “three Approved runs.”
**Where:** effort.ioa.toml PassReview; temper-verify default_max_counter=2.

---

**Decision:** (2026-09-01) ReviewRun.RecordPanel writes the panel verdict
and moves Requested → Recorded. It does not call review_gate_lifecycle.
**Came up because:** Approve fans into WorkCycle and errors without
work_cycle_id. Rita locked “write the ReviewRun directly”; GitHub comments
are not the record.
**Options:** (1) Approve and ignore the WASM error; (2) reuse IngestRecord
(operator + GitHub body); (3) a panel action on the row.
**Chose (3) because:** one concern, no WorkCycle dispatch, agents can call
it. What we gave up: Approve still exists for the old loop.
**Where:** review_run.ioa.toml RecordPanel; patrol.cedar.

---

**Decision:** (2026-09-01) intent/spec/plan/decisions doors are GitHub
paths, not Temper Files. Review and proof stay Temper Files.
**Came up because:** Rita: if those four are committed, GitHub is enough;
do not duplicate them into Temper Files.
**Options:** (1) keep Temper File copies; (2) drop the door to convention;
(3) WASM GET of the committed path.
**Chose (3) because:** Specify still refuses a missing spec, and the
source of truth is the repo. What we gave up: the kernel cannot see git
without this call.
**Where:** chain_github_ready; intent.ioa.toml; effort.ioa.toml.

---

**Decision:** (2026-09-01) Agents do not record each mid-effort call as a
Temper Decision entity. The record is `docs/efforts/<id>/decisions.md`.
Adjudication stays stall/resume, not a per-call row.
**Came up because:** Rita asked whether agents also write each decision as
a Temper entity.
**Options:** (1) a Decision entity per call; (2) keep the git log (+ PR
`## Decisions & Tradeoffs` for the CI gate).
**Chose (2) because:** the git file is already the durable home and the
gate already reads it. (1) is a second write of the same fact. What we
gave up: Temper cannot query “every call” as rows.
**Where:** docs/efforts/ARN-441/decisions.md; stack AGENTS.md.

---

**Decision:** (2026-09-01) Operator installed `arn-441-stage3-full` and
approved the AttachSpec PD as all Effort actions for `Agent::"cursor"`.
`manage_policies` PDs stay pending.
**Came up because:** live Cedar had Specify/Plan/StartBuild but not
AttachSpec. Cursor elicitation still times out (`-32001`). Rita already
asked this harness to resolve denials in-session.
**Options:** (1) Observe UI; (2) grant manage_policies to the harness;
(3) operator policy + Effort grant, same as Intent.
**Chose (3) because:** it matches the earlier ruling. What we gave up:
this harness can now call MarkDeployVerified (forbid still blocks
create/Seed). Production ships still go child Healthy → service callback.
**Where:** live policy `arn-441-stage3-full`; PD-01a05ea7-1647.

---

**Decision:** (2026-09-01) TemperDeploy polls `/healthz`, not `/readyz`.
**Came up because:** live `/readyz` is 503 (Discord 401). ARN-432 already
moved the bash driver to `/healthz` for that reason. `/healthz` is 200
and `/paw/version` returns a sha.
**Options:** (1) keep `/readyz` and wait for Discord; (2) poll `/healthz`
+ `/paw/version` like the driver.
**Chose (2) because:** Request would otherwise sit in Polling until
`max_checks` and roll back every time Discord is degraded. What we gave
up: Discord-not-ready will not fail a TemperPaw image swap.
**Where:** temper_deploy.ioa.toml ready_path; temper_deploy_lifecycle.

---

**Decision:** (2026-09-01) Do not fire DsfDeploy/TemperDeploy Request in
this session. How we will know a real Request worked: the child row
leaves Requested (Swapping/Merging, then Polling/Watching, then Healthy)
with empty error_message; Effort becomes Verified via the service
callback; for TemperDeploy, `/paw/version` `.sha` equals `expected_sha`
and `/healthz` stays 200. Fail is also knowledge: Failed + error_message,
or Cedar 403, or stuck Swapping for 10m then Fail.
**Came up because:** Rita asked if there is a reason to think Request
will not work, and how she will know.
**Options:** (1) fire Request now; (2) say the failure modes from live
evidence without shipping.
**Chose (2) because:** Request is Go. Live evidence already names two
real stops: `/readyz` 503 (fixed to `/healthz`) and a live ReleaseRun
that Failed with GitHub Bad credentials (same WASM DsfDeploy reuses).
Tenant secret names are not listable (403). Missing
`railway_token` / ids would Fail on the first swap with that string in
error_message.
**Where:** this entry; live `/paw/version`, `/readyz`, ReleaseRuns.

---

**Decision:** (2026-09-01) DsfDeploy/ReleaseRun merge uses the tenant
`github_token` secret via host `http_call`, not `~/.git-credentials` on
the computer.
**Came up because:** Rita asked to fix DsfDeploy. A live ReleaseRun
Failed with GitHub Bad credentials. The WASM took the first
`github.com` line from the computer's credential file.
**Options:** (1) rotate the computer file; (2) use the same tenant
secret `cicd_merger` already uses.
**Chose (2) because:** it is the class. A stale first line on one
sandbox cannot fail every merge. Rollback still runs on the computer
but with that same token, not the file. What we gave up: merge now
depends on the vault secret being present and valid.
**Where:** release_run_lifecycle; dsf_deploy.ioa.toml; release_run.ioa.toml.

---

**Decision:** (2026-09-03) Commit the rebuilt release_run_lifecycle.wasm. The source already uses tenant github_token and a concurrent-set scan; the bundled artifact on stage3 was still the 2026-08-25 build that reads ~/.git-credentials.
**Came up because:** Fable strings on the committed blob found ~/.git-credentials and no github_token / concurrent_entity_set. The rebuilt artifact sat dirty in the worktree and was not committed.
**Options:** (1) merge 497 with the old blob; (2) commit the rebuilt module so DsfDeploy.Request matches the source and the recorded decision.
**Chose (2) because:** (1) ships the exact Bad credentials failure the 2026-09-01 decision said this change fixes.
**Where:** os-apps/paw-patrol/wasm/release_run_lifecycle/release_run_lifecycle.wasm.

---

**Decision:** (2026-09-03) Per-repo merge serialization scans ReleaseRuns and DsfDeploys on every Request. concurrent_entity_set still names this row's set (typos fail closed).
**Came up because:** Grok, Codex, and Fable showed a DsfDeploy-only scan cannot see an in-flight ReleaseRun for the same repo, and the reverse. Either watch can revert the other's merge.
**Options:** (1) keep one set per caller; (2) always read both sets; (3) a new per-repo lane entity.
**Chose (2) because:** ARN-397 already fails closed on a paginated page. (3) is the stronger form already tracked. (1) is the hole.
**Where:** release_run_lifecycle active_release_conflict.
