# ADR 011: Measure semantic evaluations independently of path decisions

Status: implementation draft; live provider evaluation pending.

Jev evaluates evidence contradiction, missing prerequisites and dependency
timing. It does not generate futures, estimate future-event probabilities or
replace the adversarial critic. Worlds opt into shadow mode before research.

`Path.StartChallenge` retains its original critic trigger and spawns a
`SemanticEvaluation`. That entity evaluates the copied repair reference and
required event IDs against the world's evidence snapshot, then becomes
Recorded, Skipped or Failed. A two-minute timeout bounds an abandoned run.
Terminal records reject further actions; generic update and deletion are denied.
No transition from this entity targets a path, forecast or learning model.

The successful record contains the exact request, its hash, pinned and returned
model, distributions, usage and provider elapsed time. The entity also preserves
the path parent, repair file and round. A late callback cannot alter a later
path revision. Failed evaluations count against coverage, not as clear judgments.

Inputs are capped at 24 KiB and 32 required events; oversize inputs fail explicitly
rather than being silently truncated. This may reduce coverage and must appear
in evaluation results. The TypeSafe key is the separate tenant secret
`foresight_typesafe_api_key`; no other application's credential is reused.

Compare optional human-labelled held-out snapshots with
`scripts/compare_foresight_semantics.py`. Existing critic judgments are useful
comparators but are not truth labels, and its extra research means it does not
necessarily receive identical evidence. A controlled quality comparison must
give both evaluators the same frozen packet. Whole-pipeline experiments keep
domain, evidence cutoff, generation model and search budget fixed and include
research, generation, queueing and revision time. Shadow mode makes no claim
to reduce whole-pipeline time or establish better forecasting accuracy.

## Revision: complete shared context

The original three-question, 24 KiB design above is superseded by D43/D44. StartChallenge now prepares a complete frozen packet before LaunchChallenge starts either consumer. Both use the same four-front critic rubric and hash-checked evidence. Full packet size is bounded at 128 KiB, without truncation. Shadow-mode reasoning tools are restricted to reporting, while off mode retains the existing research behavior. Evaluator data remains independent and cannot alter forecast decisions. A paired 22-path live replay validates input parity but is not a whole-pipeline or forecasting-accuracy benchmark.
