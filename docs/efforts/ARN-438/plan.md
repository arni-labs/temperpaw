# Plan - temperpaw CI for ARN-438

1. Worktree off `origin/main`, branch `claude/arn-438-shadow-multirepo`. [done]
2. Extend `.github/workflows/shadow-sweep.yml` to a two-repo matrix; all git/gh
   calls move to `STACK_TOKEN`; add the `repo` dispatch choice. [done]
3. Add `.github/workflows/temper-pin-bump.yml`. [done]
4. Commit the design chain (this folder) so future bump PRs' planning gate
   resolves ARN-438. [in progress]
5. Draft PR early; ping the lead.
6. Panel at 3/3 (lead runs the panel), fix all findings, land.

## Verification before PR-ready

- `python3 -c 'import yaml; ...'` parses both workflows. [done]
- Simulate the pin-bump PR body generation and run the real
  `gates/check-decision-log.py` on it -> passes. [done]
- Confirm the pin-bump `cargo update` refreshes `Cargo.lock` correctly against the
  real drift (pin `43f9379…` -> temper main `a500e5b7…`) in a throwaway clone.
- Confirm the two-repo sweep produces distinct rows for temper vs temperpaw (the
  stack-side dry run already showed this; re-confirm the workflow wiring).

## Expected end state

One temperpaw PR: `shadow-sweep.yml` sweeps both repos nightly, and
`temper-pin-bump.yml` keeps the kernel pin current by auto-PR. No new secret (uses
the existing `STACK_TOKEN`). Nothing blocks merges; the pin-bump PR rides the
normal gates.
