# ADR-013: Question-driven semantic exploration in the existing Foresight app

Status: open exploration revision under verification, 2026-09-19.

World.RequestSemanticExploration records the request before research. SeedComplete starts a SemanticRun. Research follows the question and discovered evidence; it does not select fixed axes or require determined milestones. Observations, contested reports, weak signals and hypotheses retain distinct provenance. Hindcasts use only their frozen corpus.

A reasoning Session proposes explicit event hypotheses and their causal dependencies. The engine appends them without overwriting earlier evidence or assessments. The planner orders actual prerequisites before dependent nodes and records missing references and cycles. Jev Choice diagnoses gaps and recommends useful further work; Noul estimates the explicitly stated event; Score evaluates comparative novelty and decision value. The model judgments are not observations or empirical calibration. Jev evaluates hypotheses; reasoning Sessions generate their text and retrieve sources.

After evaluation, another reasoning Session can introduce new hypotheses, connect mechanisms, challenge earlier claims, or research outside the existing frontier. There is no fixed Cartesian space, topic taxonomy, parent-selection gate or single deepening round. Existing assessments are reused. Sessions are spawned by declared effects and observed through bounded scheduled actions. The browser never orchestrates the loop; provider credentials remain on Temper.

One Calling/Recorded transition records up to eight sequential provider evaluations. Each request is rebuilt after the preceding answer, preserving dependent reasoning. Every actual call has a trace entry with its response, request hash and references into the immutable snapshot, including assessments as used. A provider error preserves completed evaluations and records the failed attempt explicitly.

Synthesis selects a useful portfolio of futures rather than partitioning all generated nodes along one convenient axis. Each v2 outcome identifies an evaluated hypothesis. The engine attaches its exact Jev event estimate; synthesis cannot invent or renormalize it. Futures may overlap and their probabilities need not sum to one. Historical v1 partitions remain readable.

The existing /worlds and /world/[id] UI displays actual durable progress in a full-viewport matrix, graph and answer inspector. Pagination and graph bounds are explicit. Candidate count alone does not establish useful diversity or forecast accuracy.

Operational bounds: 2,048 nodes, 5,000 actual provider calls, 64 exploration rounds, one hour after semantic preparation, 128 new nodes per generation batch and 24 MiB trace payload. There is no prescribed dependency depth. A model may finish when further exploration has low expected value, with a stated reason. Budget and provider stops are incomplete work, not convergence. Unknown premises remain visible.

## Verification

The earlier deployed v1 run completed 226 actual Jev calls but produced a narrow compliance-led partition from 81 prescribed combinations. That established execution, not adequate exploration breadth. This revision is tested for repeated rounds, append-only evidence, real event probability attachment, overlapping outcome semantics and large-run runtime capacity. Simulated capacity and UI fixtures are separate from live provider evidence. Deployment receipts and fresh live-run results must identify the revision before this revision is described as delivered.

Genesis publication remains deferred under the user's explicit authorization; delivery uses the existing dedicated acceptance engine and connected preview. The denied old production module-registration route is not bypassed.
