# Intent: land the paw-compute app (attach access + governed Exec)
Author: Claude (implementer, ARN-443 part A). Status: accepted.

## Problem
The `paw-compute` app — Computer attach access (Cedar) + the `Exec` entity and
`computer_exec` WASM (a governed shell command on a Computer's sandbox) — runs in
production via Genesis (`paw-compute@370cc794` installed) while its PR (#462) sits
unmerged. main and the Genesis shelf have diverged: the code is live but not on
main. That violates the reconcile rule (Genesis is source of truth for apps, but
main must reflect what runs).

## Proposed outcome
The paw-compute source lands on `main`, reconciled to match (or supersede) what
Genesis has installed, so main and the shelf agree. No new Genesis publish in this
effort — repo-side only.

## Affected users and systems
Third-party harnesses that attach a Computer and run governed Execs; the
`paw-compute` Genesis app; the `Computer`/`Exec` entities on temperpaw.

## Constraints
- No Genesis publish and no prod install in this effort (repo-side reconciliation).
- Genesis wins on divergence.
- Builds against current `main` (the merged ARN-401 wasm-helpers fix, #468).

## Open questions
- Does the branch match Genesis HEAD? (Answered in spec: yes, source is identical;
  only the compiled blob — gitignored here — differs.)
