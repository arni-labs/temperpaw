# ADR-012: Semantic programs and the Foresight Observatory

Status: experimental implementation; production adoption requires prospective evaluation.

## Problem

Jev-assisted whole reviews and selective category reviews still invoke reasoning on nearly every transition. The next architecture composes small semantic functions, including bounded recursion, and exposes their actual execution rather than adding another critic.

## Decision

Represent an exploration as an immutable evidence snapshot plus a bounded graph program. Each function has named inputs, a finite return type, explicit uncertainty, versioned model/question, and a recorded response. Program execution distinguishes semantic decisions, deterministic graph traversal, cached reuse and requests for generative reasoning. Recursion follows real prerequisite edges; missing prerequisites create an explicit research requirement. A cycle, depth limit or call budget produces an unresolved result, never success. A Jev distribution is never a forecast probability.

Execution that changes the installed application must be expressed by Temper entities, transitions and WASM. Offline experiments may run a bounded replay harness against frozen evidence; the browser is an observer and recording editor, never a provider-credential holder or hidden production orchestrator. Initial Observatory recordings may contain the existing review benchmark and an explicitly identified offline semantic-program experiment. Do not represent replay as live production or inferred graph order as measured execution.

The Observatory renders dependency branches, function calls, evidence, uncertainty, and reasoning escalation. Its recordings are portable and exportable, with playback controls and a presentation mode suitable for screen recording. Imported files are validated and rendered as text. Every visible metric has a stated denominator and provenance.

## Evaluation

Compare complete baseline and candidate runs under equal wall-clock and reasoning budgets, on multiple held-out worlds. Count distinct supported mechanisms rather than raw branches. Preserve rejected branches for independent audits. Assess usefulness through blinded human comparisons and decision consequences; assess vividness through concrete actors, dated measurable events and falsifiers, separately from truth. Register probabilities before outcomes and compute Brier scores only for eligible resolved forecasts with evidence and timestamps. Keep unknown, unresolved, simulated and retrospective cases out of prospective scores. Show missing measurements as unavailable.

No prototype score constitutes evidence that forecast accuracy has improved. Existing critic timing remains labeled critic-stage evidence. Native execution, live observation and retrospective recordings must be visually distinguished.
