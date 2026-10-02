# World-first comparison protocol

Frozen before candidate implementation outputs on 2 October 2026.

## Baseline

Requested model: GPT-6 Astra, extra-high reasoning. An independent context receives
the ordinary questions and a strong prompt, can research public sources, and cannot
inspect Foresight outputs or project files. Up to 3,500 words per question. The prompt
explicitly requests bold distinct worlds, changes beyond the present, backward paths,
alternative routes, interacting detail, assumptions, challenges and source support.
This is a strong direct-answer baseline rather than a deliberately weak prompt.

Frozen prompt SHA-256:
`e05b5429932bc4a540679a99e746d127fafdf953fc1a38f04e9df1e965fd81ea`.
Frozen inputs: `/private/tmp/arn518-world-first/baseline/`. Baseline answers are
written to the workspace-local `benchmark-artifacts/baseline/` directory because
the baseline agent’s patch tool rejected writes outside its project boundary.
No candidate output is supplied to that context.

Questions, unchanged for both systems:

1. How will people make video games in 2030?
2. What will eating at home be like in 2035?
3. How will children learn in 2035?
4. What will living in a big city be like in 2040?
5. What will small music venues be like in 2030?

Games and music are known development examples. Food, education and city living are
held-out questions for this revision; do not represent the entire set as held out.
Preserve all attempts and revisions, including interrupted and failed runs. Report
actual model identity/effort, source vantage, elapsed time, requests and token/cost
figures when available. Unknown resource figures remain unknown.

## Comparison

Use blinded answer labels and counterbalanced order. Judge imaginative change beyond
the present, distinctness of whole worlds, causal completeness and alternative routes,
evidence fidelity and chronology, internal coherence, concrete interacting detail,
and readability. Require concrete examples for preferences; answer length and number
of model calls are not quality scores. Inspect cited evidence independently for a
bounded factual check. Keep creative preference separate from factual defects.

Expose per-question findings, ties and losses rather than reporting only an aggregate
win. This small benchmark can establish a measured advantage on its questions, not
superiority for any possible question. A self-score, judge preference, repeated Jev
estimate or fictional scenario is not predictive accuracy. Accuracy claims require
independently resolved outcomes and an evaluation that addresses historical leakage.

## User-flow acceptance

Run the actual deployed question form through completed worlds. Inspect immutable
original endpoints, connected proposed routes, explicit unresolved prerequisites,
amendments, reuse receipts and whole-world estimates. Verify the overview and details
visually. A local unit test or successful deployment alone does not satisfy this step.

## First deployed development attempt

Backend `c7a7a4a81` was activated through native module uploads on the dedicated
acceptance service; preview UI `2ad79601` was verified in the browser. The normal
form accepted “How will people make video games in 2030?” and displayed four
imagined worlds before route estimates. The world is
`foresight-b0bb7b3b-4f9f-4423-9ed0-4a148c83b76e`.

The proposals failed the accepted quality bar: creator platforms, supervised AI
helpers, integrated art tools and provenance paperwork were separate research
topics, largely already represented in that run's own present baseline. Richer
paragraphs did not create sufficiently different futures. This is a development
failure against the brief, not a completed blind comparison. The independent
baseline answers remain sealed from implementation work.

The next correction adds actual proposal-level Jev critique before freezing
endpoints for route search. Preserve rejected attempts and their judgments; do
not represent a model verdict as proof of novelty. Keep the current run and clock
intact. Ordinary-question completion, the held-out questions and the blind
comparison remain pending. No comparative win or delivery is claimed.
