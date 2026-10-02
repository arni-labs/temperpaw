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
