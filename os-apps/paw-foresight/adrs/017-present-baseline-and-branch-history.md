# Establish the present before forecasting; condition on branch history

Date: 2026-09-29

An ordinary question must not depend on a carefully written prompt to distinguish today's observed conditions from future changes. Previously the structured baseline arrived only during world composition. Conditional link estimates saw direct prerequisites but omitted earlier events in the same path.

The existing native SemanticRun lifecycle now establishes a sourced, dated baseline in its seed phase. Observations, assumptions and unknowns remain distinct. Every proposed future event receives a temporal classification before forecasting. Already-observed and mixed claims do not receive future likelihood or novelty estimates and cannot become defining world components. Mixed claims return to the reasoning model for decomposition; uncertain claims remain explicitly uncertain. Source identifiers are validated; source contents and model classifications are not treated as proof.

Each causal link carries a reproducible hypothetical branch state derived from its immutable world DAG. Later checks inherit upstream assignments and their deadlines. They exclude the target, descendants and unrelated components. The alternative condition leaves direct prerequisites unassigned and states that at least one fails, without inventing which. These are conditional model judgments over proposed histories, not sampled trajectories or empirically validated simulations. All branches remain hypothetical. Whole-world probability is still a separate joint judgment, never a product or average of link odds.

The UI exposes the baseline while exploration runs, temporal classifications and the actual conditions behind causal estimates. Counters distinguish Jev judgments, failed attempts and Jev HTTP requests. An exploration round is not an LLM request count.

## Sources and scope

- [Christopher Lee's Patientic post](https://x.com/chris_not_busy/status/2102349211127955925) and [published walkthrough](https://patientic.ai/blog/how-we-created-a-patient-world-and-gave-grok-4-7-a-mission): preserve state, time, disclosure and alternatives separately from model proposals. The walkthrough describes eight authored branches and 58 events, not a demonstration of predictive accuracy. We adopt explicit history and bounded judgments, not medical assumptions or performance claims.
- [TypeSafe primitives](https://docs.typesafe.ai/introduction): small typed questions, independently evaluated against explicit state; independent questions can share a request. Existing structural batching is retained.
- [JEV-as-a-Judge, version 2](https://arxiv.org/html/2609.26550v2): evidence-grounded judgments and confidence routing have a limited operating envelope. Reference-free prose and elaborate wrong answers can remain confidently wrong. We retain uncertainty and the reasoning model's role; no paper threshold is installed as an unvalidated forecast-accuracy guarantee.
- [webctl](https://github.com/dorkitude/webctl): focused evidence retrieval can reduce context noise. This does not justify pruning a novel future because it lacks a source already predicting it. The existing research path remains; no new retrieval platform is added.

Verification must distinguish a present claim from a future increment, preserve unknowns, reject unsourced baseline references, inherit multilevel ancestors, exclude outcomes from conditioning, distinguish on/off conditions, and retain separately evaluated world probabilities. A live ordinary-question run is required for delivery; local fixtures alone do not establish output quality or calibration.


## Bounded native callback lineage

The server admits 512 callback hops per inherited request context. Scheduling does not reset it. SemanticRun counts every native action, including polls, retries and corrections; the count is a conservative bound on its parent lineage. An explicit new run has its own counter while retaining a resumed checkpoint’s original clock and receipts.

The application stops ordinary exploration at 232 transitions and pair search at 264. It reserves 44 transitions for composition, 128 for world judgments, and 44 for writing; world evaluation stops by transition 436 and the app cap is 480, leaving 32 server hops of headroom. A reasoning phase permits ten polls across all retries/corrections, 30 seconds apart. Provider retry delay remains 15 seconds. The worst supported phase consumes 39 transitions on success or 40 on poll exhaustion: entry Reason/Launch/Spawn (3), ten check/callback pairs (20), four possible correction relaunches (12), three retry spawns (3), and final Expanded (1); the eleventh timeout check plus Fail replaces the final pending/success tail. Separate JSON and typed corrections can each occur twice. Reserve44 retains four transitions of margin. Neither retries nor corrections reset phase polls, the original clock, or the total transition counter. Poll spacing preserves roughly five minutes of polling opportunity, not a strict wall-clock completion guarantee: retries and scheduling add delay. A stalled writer reports an explicit exhausted budget, never manufactured results. Evaluation recording remains immediate.

Independent component judgments now share typed Jev requests. After temporal screening, each topological depth is evaluated in gap, likelihood, novelty and decision-value waves. Temporal screening can batch across depths because it reads only the claim, dated baseline and evidence. Subsequent batches never cross a wave or depth: each node sees its previous assessments and every prerequisite’s completed assessments. Common evidence is represented once without deletion; each typed result retains its exact individual request context and receipt. All batches checkpoint before another HTTP request, and malformed fan-out remains atomic and retryable. This improves judgments per callback without bypassing the server limit or changing probabilities.


World components need not form a connected causal graph. A coherent joint scenario may contain parallel developments or shared background causes. Facets must still cover every component, while the optional causal-link list contains only asserted dependencies (zero to twenty-four). Existing link identity, date, acyclicity and reference checks remain. Pairwise and whole-set audits still evaluate all components, followed by a fresh joint likelihood; conditional branch judgments are made only for links actually claimed. This avoids inventing causation merely to satisfy graph connectivity.

## A shared comparison question

The composer now selects a shared central question from the user question and explored possibilities, and each world supplies its overall trajectory answer before detailing its implications. The question may involve interacting uncertainties; it imposes no domain checklist, fixed axis, or exclusive outcomes. The engine preserves this comparison on each immutable world and passes it to the set-level judgment and writer. The set-level criterion distinguishes materially different overall answers from changing subjects or stakeholders within one common account. Missing new composition fields fail atomically through the existing bounded correction path. Historical composed worlds remain readable without invented comparison fields. Contract tests establish propagation and validation, not creative quality; that still requires a live run.

The existing contrastive challenge receives current candidate novelty and decision-value judgments with their exact category definitions, but no likelihoods. These are fallible critiques of repeated mechanisms, not thresholds or targets; the pass and resource budget are unchanged. Composition and writing also receive the original exploration-admission receipt so a later phase label cannot disguise budget-limited search as convergence.

### Source findings, research leads and chronology

New seed records use `evidence_json`, normalized into `evidence_metadata` throughout the semantic graph and Jev input. One typed record has one source reference. `kind` distinguishes a substantive finding from a source lead; publication date, observation-period endpoints and retrieval date are separate nullable values. Publication and observation preserve year/month/day precision; retrieval is a day. Unknown dates remain unknown, and indexed publication metadata requires source checking. A lead stays visible as research context but cannot establish a baseline observation. Known publication or observation intervals later than the vantage are ineligible; later retrieval of a frozen source does not change its historical vantage.

Newly prepared snapshots declare evidence contract v1. Historical untyped records remain explicitly `legacy_unverified`; new baseline construction may report them as unresolved research but cannot silently upgrade them into findings. Already saved snapshots without the version marker keep their historical baseline readable. Later research uses the same metadata format; legacy sessions may retain missing metadata as unverified rather than fabricating it. Malformed new metadata is rejected atomically. This validates representation and eligibility, not truth: a researcher could still falsely label a title as a substantive finding, and dates may be wrong despite valid syntax. No claim of independent source verification follows from this contract.

### Generate consequences under recorded hypothetical branches

Previously, inherited conditions existed only in audits after world composition.
Exploration's `parent` meant revision lineage and `requires` linked support or
prerequisites; neither represented the opposite outcome of an uncertain premise.
Ordinary exploration can now propose immutable `snapshot.branches` records and
associate new consequences through `branch_id`. A branch names its parent,
`all_occurring` or `not_all_occurring` premise IDs, and a dated milestone. The
engine derives parent-first conditions and readable event descriptors. A failed
conjunction does not assign every premise false. Conditions remain hypothetical,
separate from the sourced baseline; no topic axes or branching quota are imposed.

One shared validator rejects foreign references, cycles, self/downstream
conditioning, malformed dates/types and directly contradictory signed clauses
before committing any generated nodes. Branch premise links affect evaluation
ordering without being rewritten into a candidate's marginal prerequisites.
Each branched candidate receives a separately keyed `estimate_conditional` Jev
question alongside its unchanged marginal estimate. Its current typed receipt
contains the exact conditioning state. Existing lossless request packing and
admission estimates include these additional tasks; clocks and native transition
limits do not reset. Changed evidence still invalidates current judgments.

Composition uses only currently eligible future candidates and future premises.
It carries the union of inherited conditions into `world.branch_conditions`,
rejecting incompatible assignments. A fresh whole-world probability assesses the
joint defining events **and** these signed conditions, not probability assuming
those conditions true. The engine copies conditions and their event descriptions
to the final answer, so readers do not need a separate hypothesis lookup.
Historical snapshots without branches remain readable without invented history.

Verification exercises opposite premises and three-level inheritance through
actual generation-input, expansion, Jev-request, composition and final-answer
WASM boundaries. It preserves a marginal estimate while recording a different
conditional estimate, rejects incompatible composition, and preserves judgments
when the original clock is exhausted. These tests establish state propagation
and accounting, not creative quality, calibration or causal identification.


### Comparable probability judgments

Whole-world and constituent estimates are independent model judgments, so they can violate the conjunction upper bound. New likelihood receipts fingerprint the exact question/horizon, baseline, full source contents and proposition. Only matching current typed receipts are compared; legacy or changed contexts remain unassessed. The engine preserves raw odds and records `joint_exceeds_component` as probability uncertainty, not logical impossibility or calibration. The diagnostic travels through existing refinement history and answer audits. It adds no retry loop and never clamps estimates. At evidence-limit capacity the mandatory evaluation note still carries the warning without deleting another limitation.


### Poll spacing across bounded corrections

The observed three composition attempts took 137.469, 112.199 and 109.172 seconds.
Ten shared 30-second polls exhausted the phase after only 30 seconds of its third
attempt. CheckReasoning now waits 60 seconds, still sharing ten checks across
corrections and retries. The 44-transition reservation, 480-action cap, 900-second
reasoning timeout and original run clock remain unchanged. Polling rejects an
expired original one-hour deadline, including an already completed child result.
A measured-duration simulation through the actual monitor WASM needs seven polls
at the new spacing; the old spacing fails at ten. This trades up to another
30 seconds of completion-detection latency for less premature abandonment, not
additional model attempts or a promise that arbitrary corrections will finish.

### Source-driven scope narrowing

Baseline review records the original question separately from the scope supported
by retrieved evidence. A narrowed or uncertain scope caused by source availability
requests one research-only use of the existing explore lifecycle before seed
hypotheses are evaluated. This is a fallible model review, not coverage certification.
The repair may revise the sourced baseline; its original version remains in the
receipt. New sources use a separate identity prefix, existing sources stay immutable,
and ordinary exploration rounds, the original clock and budgets are not reset.
Malformed repair responses receive the existing bounded corrections, then continue
with the original baseline and explicit limitations. Frozen or budget-limited runs
skip external repair honestly. A completed repair can remain limited; only typed,
dated findings can support an addressed disposition, and unresolved scope remains
visible in baseline unknowns. No domain checklist or future outcome is prescribed.

A live repair confused review status with disposition status, then returned 17
observations against the existing 16-observation bound. The repair producer now
receives the complete output contract: scope enums and limits share an owner with
validation, and baseline bounds come from the existing baseline validator. Errors
name the invalid field and accepted values or bound. Existing findings and new
same-response finding IDs are distinguished in the repair contract.

Corrections retain the exact rejected draft as unaccepted data in the next
reasoner's user message, so a fresh Session can repair retrieved material without
repeating research. Rejected drafts never enter the snapshot automatically. The
existing 256 KiB correction-context bound applies without truncation; an oversized
scope repair records the actual limit error and continues on the original baseline.
Actual-WASM tests cover the observed enum/list errors, draft producer-consumer
round-trip, and unchanged snapshot on oversized rejection; the previous producer
artifact fails the contract test. This does not certify the model's repaired scope.

### Separate contributory routes

A captured composition supplied school projects and work placements as separate
routes to portfolio records. Rejecting the shared target forced the correction to
drop one route. Links now retain distinct identities when they share a target;
multiple `from_ids` within one link still mean that link's joint prerequisites.
Separate routes are not assumed exhaustive, exclusive, or collectively necessary.

Automatic hypothetical ancestry follows unique incoming routes as before. At a
convergence with multiple incoming routes, their origins remain unassigned. The
exact links are retained in `branch_state.unassigned_upstream_routes` as context,
with `assumed:false`; provider input includes the referenced event descriptions.
A separately required direct event remains assigned even if it also appears in
an unassigned route. Off conditions remain “not all direct prerequisites occur,”
without inventing which fails. No route-selection API is added.

Each link date remains inside the world interval. Nondecreasing dates are checked
along unambiguously inherited routes, not across alternative origins that were
never assumed. Cycle rejection still covers the entire claimed graph. Per-link
task identities and fresh joint-world estimation are unchanged. Portable tests
cover converging routes, diamond ancestry, off conditions, route descriptors,
cycles and reversed unique-route dates; actual expansion preserves both captured
links where the previous artifact rejects them. The replay uses the later saved
catalog and assessments with local correction cursors cleared, not an untouched
capture of the original failed execution.


### Preserve active reasoning across the former ten-poll cutoff

The ten shared polls still abandoned a third corrective Session while it was
CallingProvider; that same Session completed after its parent failed. Poll count
is now diagnostic only. The monitor continues checking the same child and consumes
its completed result without launching a replacement merely for being slow.
The original one-hour clock, 480-transition cap, 60-second poll cadence, bounded
correction/provider-retry counts, and 900-second native state timeouts are unchanged.
Completed results after the original deadline are still rejected.

The unchanged 44-transition allowance is named `REASONING_ADMISSION_RESERVE`:
it plans admission of optional work, not a guaranteed upper bound on active
reasoning. Remaining transitions are rechecked before subsequent work; a slow
child can use evaluation headroom and exhaust the overall run budget. This does
not guarantee delivery or add model attempts. Earlier worst-case phase arithmetic
above describes the superseded poll-count cutoff, not the current lifecycle.
