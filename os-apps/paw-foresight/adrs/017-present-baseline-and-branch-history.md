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

The application stops ordinary exploration at 192 transitions and pair search at 224. It reserves 64 transitions for composition, 128 for world judgments, and 64 for writing; world evaluation stops by transition 416 and the app cap is 480, leaving 32 server hops of headroom. A reasoning phase permits 20 polls across all retries/corrections, 15 seconds apart: 40 polling actions plus at most six launch/spawn attempts and completion/correction edges fit the 64-transition reserve. A stalled writer reports an explicit exhausted budget, never manufactured results. Evaluation recording remains immediate.

Independent component judgments now share typed Jev requests. After temporal screening, each topological depth is evaluated in gap, likelihood, novelty and decision-value waves. A batch never crosses a wave or depth: each node sees its previous assessments and every prerequisite’s completed assessments. Common evidence is represented once without deletion; each typed result retains its exact individual request context and receipt. All batches checkpoint before another HTTP request, and malformed fan-out remains atomic and retryable. This improves judgments per callback without bypassing the server limit or changing probabilities.
