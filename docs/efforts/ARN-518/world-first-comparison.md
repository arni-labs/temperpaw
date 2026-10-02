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
Local execution artifacts: `/private/tmp/arn518-world-first/baseline/`.

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

Results are pending. No comparative win is claimed by this protocol.
