# Imagine worlds before searching their routes

Status: Accepted for implementation
Date: 2026-10-02
Effort: ARN-518

## Problem

The question-driven SemanticRun flow introduced in ADR-013 constructs research-led
hypotheses, evaluates combinations, and composes worlds near the end. The deployed
game-making example completed its checks but mostly described existing practices.
Successful execution did not establish imaginative foresight. This direction also
lost the central commitment of ADR-002/004: imagine a distinct endpoint, then search
backward without silently repairing it into a conventional future.

## Decision

New runs establish the sourced present, imagine a portfolio of different future
worlds, and persist their original definitions and distinguishing commitments
before constructing components. The question determines the subject and possible
arrangements. No fixed optimistic/pessimistic, technical/social, or industry template
determines the worlds. Count and time limits are operational limits, not an agenda.

Each endpoint then receives backward route search. Routes reference shared event
nodes, dated causal links, the commitment they reach, present evidence, and unresolved
prerequisites. Evidence references are distinct from assumed future prerequisites.
A graph connection is a proposed causal path, not empirical proof or calibrated
probability. Failed or uncertain links prompt alternative mechanisms; an unsupported
endpoint remains visible as unresolved rather than being disguised as reachable.

Reuse a judgment only when its proposition, conditions, relevant evidence, model
contract and evaluation context match. Reusing an event ID alone is insufficient.
Shared pieces may support multiple routes and endpoints. Preserve the original
receipt; changed inputs invalidate affected reuse. Repeated estimates are not
independent evidence.

Reconstruction retains endpoint identity and selects explicit routes. Any change
to a distinguishing commitment is an explicit amendment with its original text,
replacement and reason. Jev assesses preservation separately from plausibility.
A weakened or changed endpoint cannot silently satisfy its original commitment.
Final worlds must remain different overall answers, not different topics within a
single conventional account. Do not fabricate a plausible path merely to meet a
desired world count or boldness target.

Implement this through the existing native SemanticRun lifecycle, versioned persisted
search state, reasoning modules and Jev evaluation machinery. Preserve current source
handling, recoverability, traceability and historical answers. Reserve actual work
for imagination and alternative route search before broad combination audits consume
the budget. The UI exposes original worlds, route development, amendments and final
estimates, including blocked paths and uncertainty.

### Corrections from the first deployed attempt

The first imagined set repeated its present-day research topics, then backward
generation exhausted its polling allowance before recording any routes. A
proposal is now preliminary until Jev separately assesses its consequential
change from the sourced present and whether the set contains alternative whole
trajectories. The classifications are fallible critique, not proof of novelty.
Rejected proposals and actual judgments remain visible; at most two revisions
use the original run's remaining resources before explicit quality exhaustion.

Initial three-commitment batches recorded paths but spent the available search
on the first two originals. New accepted sets request one original's missing
commitments together (at most eight), with at most 24 new shared hypotheses and
64 KiB of response. Alternative-route turns retain the three-commitment limit.
This makes a complete original the initial unit of work without increasing the
run's time, polling or transition limits. Historical accepted sets retain their
recorded selection contract. The backward scheduler does not use forward-search
novelty/value rankings, so new sets omit those two redundant per-piece tasks;
factual, temporal, likelihood and causal checks remain.

Both next live tests also failed when the composer omitted unexplored originals
without supplying omission receipts. The engine now derives those receipts from
the saved commitment and route records. It cannot discard a fully connected
original or fabricate a reconstructed world or estimate. This repairs structural
bookkeeping; fresh deployed results must still establish useful completion.

The next live pass showed that critique alone could accept still-incremental
ideas. First imagination therefore receives only the original question, horizon,
vantage date and hindcast setting; user constraints remain in the question.
Research findings and scope summaries belong to the subsequent evaluator and
route finder. Revision input preserves exact prior proposals and actual critique,
without repeating the research agenda. This separates creative proposal from
evidence testing; it does not establish creative superiority by itself.

Completed answers also retain every omitted original with its recorded reason.
Selected blocked routes remain conflicts in the final audit; unresolved or
missing routes cannot appear clear. Raw Jev estimates are retained separately
from path validity, and the UI exposes those limitations.

## Verification and delivery

Behavioral checks must distinguish this flow from the replaced one: endpoints exist
before components; a broken route produces alternative search; identical shared
evaluations are reused; changed evidence or conditions prevent stale reuse; silent
weakening is rejected; structural connectivity never implies factual proof.

Live acceptance uses ordinary questions across different domains and compares the
actual deployed output against a strongly prompted frontier-model baseline. Freeze
questions, baseline prompt and quality criteria before seeing candidate results.
Retain losing and failed runs. Blind comparisons assess distinctness, imaginative
change beyond the present, causal completeness, evidence fidelity, internal coherence
and readability. Report latency and resource use separately. A judge preference or
stable Jev score is not predictive accuracy; claims of accuracy require independently
resolved outcomes and a leakage-aware evaluation. Do not claim universal superiority
from a small sample.

Deployment remains the dedicated Railway acceptance service and Vercel preview.
Genesis and main promotion remain deferred by the accepted task scope.

## Capacity and initial coverage (audit policy 2)

Pass15 exposed two different omissions in the same lifecycle. Education accepted a
second backward response at minute 59.16 with 87 candidate checks outstanding. Its
recorded nine-minute generation estimate described one child, while corrections
made each observed generation interval about 21 minutes. Music reached the
exploration cutoff with 14 candidate checks outstanding; the route-only drain did
not include prerequisite checks. Its preserved complete remaining workload is 102
transitions under the new mandatory policy, not merely the four novelty transitions.
Neither historical failure is erased or claimed recoverable.

New runs reserve a useful unit through generation, candidate admission, route
checks, current novelty comparison, and final writing. Generation timing includes
corrective children from the initial request until acceptance. The original run
clock, 480-transition hard bound, and existing finalization reserve remain unchanged.
The generator receives available evaluation capacity; the receiver computes the
mandatory workload before applying a reply. An oversized proposal is corrected
within the existing correction allowance, not partially applied. Contingent work
has an explicit conservative bound; admission is not a provider-time guarantee.

The first generation reconstructs two complete originals within the same 24-node
and 64-KiB response bounds. Currently passed novelty comparisons receive priority
only when their exact evidence/context request remains current. Exact prerequisites
may be shared; commitments are never dropped to fit. Historical programs retain
their recorded batch policy. Targeted evidence research addresses missing bridges;
relevant evidence is never removed to avoid reassessment.

The mandatory-work allowance spans candidate, route, and deferred-comparison
stages. A declined later generation cannot erase the allowance for accepted work.
Grounding and causal checks remain required; optional diagnostic estimates do not
certify missing mechanisms. The shared search task policy supplies execution,
admission and reporting rather than counting optional work as completed checks.
The native receiver may still decline work that cannot fit, and any unresolved
path remains visible alongside model estimates.

`audit_policy_version = 2` is stamped only on new runs. `semantic_search` owns
both mandatory and optional task selection. Initial coverage excludes conditional
on/off sweeps; after active worlds have fresh joint estimates, diagnostics target
unresolved transitions or competing routes. Reporting distinguishes selected
experiments from `not_run` experiments. Historical runs keep their original full
sweep. Mandatory candidate/route/comparison work stops with the existing ten-minute
finalization time and 120-transition tail still protected.

Pass16 showed that this admitted unit also needs a completion boundary. Both runs
had two complete compatible originals with current novelty receipts and no pending
mandatory checks, but attempted a third original before composing an answer.
New findings reopened the existing graph: its conservative revalidation cost alone
was 200/206 transitions against 158/154 reserved. Removing every new prerequisite
could not satisfy the correction request.

Policy 2 now sends that completed two-original unit through the existing composition,
world evaluation and writing lifecycle before further-original research. This is
structural readiness, not proof that the paths will occur: uncertain route judgments
remain uncertain, and other original proposals remain available as unconstructed
alternatives. The answer may still fail its distinctness or world checks.

Capacity rejection separates existing-graph revalidation under the proposed present
context from additional graph work. When existing-graph cost alone exceeds the
allowance, the receiver stops without futile shrink corrections. Retrieved findings
and the unaccepted draft remain in the saved reasoning result; it neither applies
part of the draft nor presents saved odds as rechecked against that evidence.
These costs are conservative admission estimates, not measured provider work.

Shared bridge producer contract
-------------------------------

Backward generation declares an exactly shared causal mechanism once in a bounded
`bridges` list, then names it with `bridge_ref` from every dependent route. Stored
bridge IDs remain immutable and distinct from candidate reference aliases. The
producer supplies causal ancestors, assumptions, milestone and the complete set of
endpoint dependencies explicitly. Changed mechanisms, conditions or deadlines
require a new declaration; paraphrasing a bridge per route does not establish a
new mechanism. Signed conditions come from canonical candidate branch ancestry.
The declaration contract changes representation, not the existing response or
execution budgets. These producer instructions do not establish causal validity;
native validation and actual judgments remain required.

Targeted repair generation receives the exact obligation fingerprint, affected
commitments and recorded categorical distributions. It may retrieve specific
missing evidence, propose a materially different immutable mechanism, or report
`no_change` explicitly. A deadline, test plan or rephrasing is not a causal repair.
The producer emits a fingerprint-bound disposition; it cannot invent Jev reasoning,
claim a failed bridge makes a world impossible, or reset the original budgets.

The existing Expanded callback may preserve one validated answer and its exact
assessment context before one optional repair. The checkpoint is immutable and
bounded to 8 MiB; it retains the original clock and counters. Admission reserves
existing-graph revalidation, 44 generation transitions, at least 120 finalization
transitions (88 generation transitions plus the larger of 32 or the packed final
audit cost), and a minimum of 16
transitions for new work. Time admission retains the larger finalization reserve
or observed compose-plus-synthesize duration. Native application checks actual
cost again. Admission is not guaranteed: the captured pass15 food history exceeds
the checkpoint bound and declines repair while retaining the completed answer.
During active or failed repair, the UI labels the saved answer as the initial
assessment and uses its saved sources and audit context. Later findings are not
presented as incorporated until a new validated answer completes.
