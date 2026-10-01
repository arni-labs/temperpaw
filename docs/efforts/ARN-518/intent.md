# ARN-518: Observable continuous learning in Foresight

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
