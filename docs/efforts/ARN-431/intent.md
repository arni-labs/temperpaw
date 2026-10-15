# ARN-431: stage-3 phase 2 - S1 shadow verdicts

## Problem
S0 (ARN-430) shipped the record entities and `record_ingest`, but two things are
still missing before Temper can ever become an authority on a gate:

1. The S0 app is only merged to GitHub. Genesis is the source of truth that
   production loads pinned versions from; until paw-patrol is published to
   Genesis and installed on openpaw-production, the new entities
   (ReviewRun/ProofRun record fields, Adjudication, StandingDecision,
   ShadowVerdict) do not exist in prod.
2. Nothing yet compares what Temper's state machine would conclude against what
   CI actually concluded. Without that comparison we cannot know whether Temper
   is ready to flip a gate (S2).

## Proposed outcome
- paw-patrol at temperpaw `main` (the S0 merge) is published to Genesis and
  hot-installed on openpaw-production; the pinned ref serves the S0 specs
  (ShadowVerdict/Adjudication/StandingDecision present in prod).
- A **shadow sweep** runs per PR (nightly + on demand): it reads the four gate
  check conclusions from GitHub (the CI verdicts), ingests the PR's b64 record
  comments into prod Temper through the S0 Ingest actions (pull-mode, no webhook),
  computes Temper's verdict per gate from entity state, writes a
  `ShadowVerdict{pr, gate, temper_verdict, ci_verdict, agree}` row, and prints a
  disagreement table.
- The sweep **never blocks and never writes to GitHub** - it is shadow only. It
  is a script now; it becomes a Temper reaction later (per the stage-3 spec).

## Affected users and systems
- Genesis + openpaw-production: a governed publish/install of paw-patrol ONLY,
  under the standing grant. Nothing else on prod is touched.
- temperpaw: one new scheduled workflow (`.github/workflows/shadow-sweep.yml`).
- stack: the sweep script + README (`shadow/`), reviewed on a stack branch (stack
  has no gate loop; not pushed to stack main by me).

## Constraints
- Modules and entities are UNCHANGED - S0 shipped them. A spec gap is reported,
  not patched around (that would be a new S0 change, out of this phase).
- The sweep is a script, not a WASM module (it becomes a reaction later).
- One PR per repo: temperpaw gets the workflow. Stack changes go on a branch.
- Shadow only: exactly one authority per gate stays CI. No GitHub writes, no
  merge blocking.

## Open questions (answered in spec.md / decisions.md)
- How is each gate's Temper verdict computed from entity state, and precisely
  what does the sweep mirror for the planning / decision-log gates (which have no
  b64 record)?
- How does the sweep dispatch Ingest against prod without a webhook (pull-mode)?
