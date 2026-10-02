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

New accepted sets request backward paths in deterministic batches of at most
three commitments. Missing paths come first, then least-explored unresolved
alternatives. All original worlds and shared components remain in context;
batching limits generation work rather than narrowing the original ideas.
Responses outside the selected commitments or above the per-turn bounds are
rejected before mutation. Existing time and transition limits remain unchanged.
This addresses an oversized-work hypothesis; only a fresh live run can establish
whether it resolves the observed timeout. Historical accepted sets retain their
original contract.

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
