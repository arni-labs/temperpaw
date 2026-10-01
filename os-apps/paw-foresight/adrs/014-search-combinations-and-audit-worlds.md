# Search combinations and audit worlds

The completed 303-question run contained 41 hypotheses and five composed worlds,
but no relationship or conditional-event checks. Every world received only a gap
classification and likelihood. Its 24 revisions did not establish that any
specific uncertainty had been resolved. The UI's inferred “gaps cleared” counter
therefore described neither research progress nor a verified resolution.

## Decision

Keep the native SemanticRun entity and WASM integrations. After exploration,
schedule compatibility questions across the current hypothesis frontier before
asking the writer to compose worlds. Cover pairs in round-robin order under the
remaining question budget. Build candidate sets from pairwise-compatible events
using different starting points. Retain all frontier events and unresolved pair
judgments in the composition input; a surprising idea is not discarded because
its probability or compatibility is uncertain. These candidate sets are search
seeds, not an exhaustive enumeration or a proof of joint consistency.

Each world must contain several facets, explicit assumptions and dated causal
links where relevant; parallel independent developments need no invented link. Facet names emerge from the question; there are
no prescribed economic, political, optimistic or pessimistic buckets. Validate
references, coverage, acyclicity and deadline ordering deterministically.
Source-support links are distinct from future causal prerequisites.

Jev evaluates all component pairs under the world's assumptions, every stated
transition, and the whole world jointly. The whole-set check can identify
contradictions that pairwise tests miss. For each transition, assess the target
both conditional on all explicit prerequisites occurring and conditional on
their conjunction failing. These are conditional model judgments, not identified
causal effects. Do not condition on the target or on its downstream consequences.
Never multiply these diagnostics into a world probability. The existing fresh
whole-world estimate runs after the audits and receives their results.

Conflicts or material unknowns return to composition with their exact subjects
and judgments. Revised worlds have immutable identities; previous worlds and
receipts remain in history. Bound this loop to three compositions and the actual
remaining budget. Unfinished audits stay explicit; a world revision never counts
as an uncertainty resolved. Active-world IDs determine the final answer.

Independent structural questions share provider requests, following the
[documented fan-out interface](https://docs.typesafe.ai/patterns/fan-out).
Validate the complete response before advancing the cursor. Each question keeps
its own answer, task, evidence context and request hashes, with a shared HTTP
call ID. The UI counts Jev checks separately from provider requests. Dependent
node decisions and whole-world likelihoods remain sequential.

When source evidence changes, requeue the same hypotheses' gap and likelihood
questions and clear their stale current results. Preserve the previous answers
in the trace. Evidence additions and revised possibilities are shown as research
progress; there is no inferred “gaps cleared” count.

## Verification and limits

Tests reject cycles, impossible date order, invalid causal links, missing facet
coverage, malformed fan-out answers and invalid subject references. Higher-order
conflicts must trigger fresh composition even when every pair was compatible.
Conditional requests must exclude the target from their conditioning set.
Actual WASM integration tests exercise the native handoffs. Browser verification
distinguishes synthetic layout fixtures from provider-generated output.

“No conflict found” means no conflict was found in the recorded checks. It is not
proof, forecast calibration, or an exhaustive search. Worlds may overlap, so
their probabilities do not form a distribution summing to one. This change does
not yet maintain a claim-specific ledger proving that research questions were
answered; unknowns remain visible rather than being counted as cleared.


### World-set comparison input boundary

The set-difference judgment receives a structural projection, not full operational
snapshot records. It retains complete world definitions, mechanisms, narrative,
assumptions, facets, chain links, signed branch conditions and all defining/counter
event statements, scopes, dates and evidentiary qualifications. Baseline and source
claim limitations remain available. Presentation cards, prior scores, session IDs
and duplicate serialized graph data do not determine whether the proposed worlds
answer the same question differently. No prose is truncated. Likelihood and
individual evidence checks keep their existing inputs.

This boundary addresses a captured singleton request rejected with
`max_tokens_exceeded`; reducing question count cannot split a singleton. Artifact
verification compares exact retained values and record-ID sets and mutates omitted
fields to establish invariance. Byte reduction is measured, not claimed as a token
count or live provider acceptance; supported native continuation verifies that.

### Confidence passes reuse unchanged audits

The fresh living run completed 96 world tasks, but repeating all audits required
179 transitions with only 35 available. Later confidence passes now request only
fresh whole-world likelihoods when every structural and conditional audit has an
exact input fingerprint. The fingerprint binds the model and question contract,
baseline, full source records, defining events, world and branch records, and
actual per-task request; only prior model judgments are excluded. Missing legacy
fingerprints require a full audit. Changed inputs invalidate current odds and
audits even when the budget prevents replacement, without altering old receipts.

Each new round reports successful fresh judgments and reused audit identities,
original round and fingerprint. Reused checks remain prior judgments, not new
HTTP calls or independent confirmation. `stable_world_estimates` describes only
small changes in whole-world odds; uncertainty and probability-coherence warnings
remain. An interrupted pass has null current odds and intact prior history.
Admission still reserves writing and two extra attempts and cannot guarantee
completion. No time, call, transition or retry limit changes.

The actual-WASM fixture exercises first-pass stamping, fresh-only subsequent
calls, exact history preservation and interruption. A separately labelled
simulation over captured living content creates new receipts through mocked
provider calls and estimates 13 of 35 transitions; the original captured legacy
receipts do not qualify for reuse. This is not a live second-pass result.

### Historical rankings guide search without repeated whole-pool scoring

In the captured living exploration, 40 repeated novelty/decision judgments used
25 HTTP batches. New evidence still invalidates and refreshes temporal, support,
marginal and conditional judgments. Unchanged older candidates' ranking scores
instead move to `historical_search_guidance`, retaining their exact available
evaluation/context, recorded round and evidence IDs with `current:false`.
Missing provenance stays unknown; a complete comparison sample is not inferred.
These values are separate generator/challenge guidance, never current prerequisite
judgments, forecast evidence or component probabilities. New and revised
candidate IDs retain fresh ranking tasks. Admission and resumed exploration use
the same rule; source acquisition and all resource limits remain unchanged.

Actual-WASM replay of the saved second living research output removes 32 old
ranking tasks while retaining 12 new ranking tasks and all temporal/support/
probability refreshes. Old artifacts fail the same provenance check. The recorded
25-batch counterfactual is not a promised live saving or proof of richer futures.

### Reconcile later research with the current baseline

Ordinary exploration that adds typed findings returns the current baseline and scope review in the same response. The consumer resolves existing aliases against the pre-expansion catalog and new local source references against their round-qualified identities, then validates the summaries against the candidate snapshot before accepting any nodes. Missing or invalid summaries use the existing bounded response correction; rejected findings remain unaccepted. Leads-only batches do not require a rewrite.

`baseline_history` retains each predecessor baseline and scope review, accepted replacement, source Session, round and added finding identities. The original scope-repair receipt remains historical; an updated summary does not certify that gaps are resolved. Current temporal/probability judgments still refresh on the changed source basis. This repairs stale statements such as “only unverified companion records exist” after an actual limited survey finding arrives; it does not establish future novelty or forecast accuracy.

### Use the reserved challenge for signed causal consequences

The existing challenge Session selects an unresolved, future-eligible, unreplaced candidate by its recorded Jev decision-value ranking, with identity as a deterministic tie-break. The receipt distinguishes current ranking evidence from historical or unknown provenance. Selection directs investigation; it does not establish the premise. Without an eligible ranked candidate the original challenge remains available.

The producer receives two exact existing-type branch roots inheriting the selected candidate's prior conditions: its event occurs, or that exact event does not occur. A compound event's complement does not assert a particular opposite outcome. Every proposed challenge hypothesis is bound to one root or its descendants, while the existing premise critiques and alternative links remain. Empty hypotheses with an explanation are permitted rather than forcing fabricated consequences. The consumer checks the roots and bindings before accepting any additions; existing conditional requests and composition compatibility propagate the signed conditions. Whole-world odds remain fresh joint-event estimates, never a normalized partition of these two branches.

No Session, phase or budget is added. Bound candidates do add conditional-evaluation tasks (at most one per hypothesis within the existing 128-item generation bound), so preserving one Session is not a claim of equal execution cost or guaranteed completion. Existing call/transition/time bounds still apply. Captured-input synthetic producer/consumer regression demonstrates the structural change and its packing cost; the earlier paired native probe demonstrated contract viability only. Richer final-world quality remains unproven.

### Experimental comparison bindings (not quality validation)

New compositions carry one comparison frame, anchored by the engine to the exact original question and horizon and citing existing evidence. Each world names a counterpart in that revision and binds organizing components or inherited hypothetical branches to downstream defining events. The engine records support only when an existing declared chain path or branch ancestry connects them. This does not prove causation, specialize event scope, add assumptions, or establish a substantive alternative by itself.

The focal Jev question compares the named pair with the complete set as context. Missing or unsupported bindings keep the aggregate unresolved even when a raw focal choice says alternative; the raw judgment remains intact. One optional set correction total is available for this contract through the existing revision/admission path, whether triggered by bindings or a complementary-slice finding. Exhaustion preserves usable worlds and unresolved findings. Historical compositions keep their original read and audit behavior. The contract is a bounded experiment; synthetic producer/consumer verification does not demonstrate richer model-generated worlds.

The challenge completion receipt also preserves its actual pending trigger, distinguishing budget-reserved challenge work from generator-reported saturation.
