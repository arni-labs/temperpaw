# Plan: stage-3 S1 shadow verdicts (ARN-431)

## What we are addressing
Publish S0 to Genesis/prod (so the new entities exist in prod), then build the
shadow sweep that compares Temper's per-gate verdict against CI's and records
ShadowVerdict rows - shadow only, no blocking, no GitHub writes.

## Expected end state
- paw-patrol@7fcfae791 published to Genesis + installed on openpaw-production;
  ShadowVerdict/Adjudication/StandingDecision confirmed present in prod.
- `stack/shadow/shadow-sweep.py` + `stack/shadow/README.md` + unit tests, on a
  stack branch (not pushed to stack main).
- `temperpaw/.github/workflows/shadow-sweep.yml` (nightly + workflow_dispatch),
  in a draft PR off `claude/arn-431-s1-shadow`.
- Acceptance: sweep run for PRs 477 480 481 482 484 476; ShadowVerdict rows +
  agreement table reported.
- Design chain committed; decisions appended as I go.

## Steps (in the lead's order)
1. Design chain (this dir). [done first]
2. **Part 1 - Genesis publish + install + verify.** Report at this checkpoint.
3. **Part 2 - the sweep:** script + README + tests in stack (branch); workflow in
   temperpaw. Red-green on the pure pieces.
4. **Part 3 - acceptance:** workflow_dispatch run for the six PRs; capture rows +
   table.
5. Draft PR (temperpaw) with the design chain + workflow; report at this
   checkpoint with acceptance evidence. Stack branch handed to the lead (not
   pushed to stack main).

## Guardrails
- paw-patrol only on prod; standing grant; surface each prod push/install for
  authorization.
- Entities/modules unchanged - a spec gap is a FINDING, reported, not patched.
- Sweep never blocks, never writes to GitHub.
- One PR per repo; stack changes on a branch for the lead's review.
