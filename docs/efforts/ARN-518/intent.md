# ARN-518: Bold worlds with searched paths from the present

## Current accepted outcome — 2 October 2026

An ordinary question produces a variety of imaginative, materially different worlds.
Generate the worlds first, decompose their defining commitments, and search backward
for plausible chains from the sourced present. When a route fails, explore a different
mechanism rather than quietly weakening the imagined destination. Share unchanged
components and exact-context Jev evaluations across routes. Preserve original worlds,
explicit amendments, source provenance, assumptions and unresolved gaps. Show this
working in the deployed UI with readable whole-world answers, graph/matrix exploration,
causal paths and estimates whose limitations are clear.

The prior deployed game-making answer ran successfully but mostly described existing
practices. Functional completion did not meet this creative outcome. Restore the
world-first commitment of ADR-002/004 through the current native lifecycle; see
[ADR-018](../../../os-apps/paw-foresight/adrs/018-worlds-before-routes.md).

Compare the actual output with a strongly prompted GPT-6 Astra baseline at extra-high
effort, using frozen cross-domain questions and blind quality comparisons. Retain
failures and losing outputs. Distinguish imaginative quality, causal support, factual
fidelity and readability from predictive accuracy. Do not claim general superiority
or calibration without the corresponding evidence. Existing forecasting and learning
capabilities must remain intact.

Continue PR526 in arni-labs/temperpaw and PR121 in arni-labs/deep-sci-fi. Only the
dedicated Railway acceptance deployment and Vercel preview are authorized release
targets for this work; main and Genesis promotion remain deferred.

The sections below preserve the earlier learning objective and starting evidence;
they do not override this accepted correction or establish present delivery.

## User outcome

Evolve the existing Temper Foresight app so experience improves later predictions. Provide a working UI where the user can understand worlds and predictions, observe ongoing work, and inspect what the system learned with supporting evidence.

The user authorized implementation on 15 September 2026 and prioritized showing a complete end-to-end working flow as soon as possible. Visual polish and refinement follow that working result.

## Required behavior

- Explore an explicit world, its events, alternative paths, assumptions and predictions.
- Preserve prediction revisions, evidence and the model version used before resolution.
- Record outcomes with sources, distinguishing observations, historical replay and simulated experience.
- Perform real trainable updates, evaluate candidates against the preceding version, and adopt supported improvements.
- Make learning visible: examples, changed predictive behavior, evaluation results, limitations and rejected updates.
- Support accelerated historical replay without presenting simulation as independent truth.
- Show activity, progress and actionable failures.
- Demonstrate the complete UI-to-Temper-to-learning-to-subsequent-prediction flow before calling it delivered.

## Implementation boundary

The owning repository is nerdsane/temperpaw. Reuse paw-foresight and the authenticated TemperPaw dashboard. Preserve current application capabilities. No unrelated Deep Sci-Fi rewrite or platform redesign is authorized by this task.

## Evidence

- [Linear ARN-518](https://linear.app/arni-build/issue/ARN-518)
- [Source assessment](https://github.com/arni-labs/context/blob/a137b79c1e563f6f6712f13dfa45197e8c5881cc/research/foresight/2026-09-15-existing-app-assessment.md)
- Starting source: 07cbb1d84f7ea04ebada25723e11bc9a47c8ed1f.
- Live Forecast, Hindcast, World and execution contracts inspected through Temper MCP.
- The current default-tenant world remains in Seeding; existing data does not establish a healthy end-to-end flow.
- Historical Deep Sci-Fi PR 98 is closed and unmerged; its UI is reference material only.

Implementation and validation are pending. This draft records accepted intent and does not claim the feature is complete.

Author: Codex, GPT-6, Codex desktop harness.
