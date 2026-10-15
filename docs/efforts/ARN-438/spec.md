# Spec - temperpaw CI for ARN-438

## Item 1: shadow sweep across two repos (one workflow)

`.github/workflows/shadow-sweep.yml` gains a job matrix over
`repo: [nerdsane/temperpaw, nerdsane/temper]`. Each matrix leg:

- checks out ITS repo (`repository: ${{ matrix.repo }}`) so the planning /
  decision-log mirror scripts read that PR's real `docs/efforts/` tree;
- resolves a PR window for that repo (nightly: PRs updated in the last ~48h,
  newest-updated first so the `--limit` cap cannot drop in-window PRs);
- runs `shadow-sweep.py --repo <repo> --repo-dir .` per PR, checking each PR head
  out from that repo's `origin`.

The sweep mints repo-qualified entity ids (`sv-<slug>-prN-gate`, from the stack
half already merged), so temper#5 and temperpaw#5 never collide in prod Temper.

**Token.** `nerdsane/temper` is PUBLIC, so GITHUB_TOKEN (and anonymous) can already
read its PRs and check it out - the token is not about read access. The sweep must
clone the PRIVATE `arni-labs/stack` for its script anyway, so it reuses that one
`STACK_TOKEN` (a rita-aga PAT) for every git and `gh` call rather than mixing a
token per repo.

**Dispatch.** `workflow_dispatch` gains a `repo` choice (`both`/`temperpaw`/
`temper`). Explicit `pr_numbers` require a single repo (otherwise which repo owns
the numbers is ambiguous - fail loudly). A targeted dispatch skips the other leg.

Still shadow only: never blocks a merge, never writes to GitHub.

## Item 2: temper pin-bump automation

`.github/workflows/temper-pin-bump.yml`, scheduled daily + `workflow_dispatch`,
with a `concurrency` group so a manual dispatch never races the cron:

1. Read temper main HEAD (`git ls-remote` - temper is public) and the current pin.
   The pin is read ONLY from the `nerdsane/temper.git` dependency lines, and each
   manifest must carry at least one such line (else fail - no vacuous pass), and
   the rev must be uniform across both manifests (a half-drifted pin fails loudly).
2. If equal, stop. Forward-only: if temper main is not strictly `ahead` of the pin
   (reverted/force-pushed/diverged), do not bump backward. Then dedupe on **PR
   existence**, not branch existence: skip only if a PR for `bot/temper-pin-<12hex>`
   already exists in ANY state (open = in flight, closed = a human rejected this
   rev); a merged bump is unreachable because the pin would already match. A prior
   run whose `gh pr create` FAILED leaves a stranded branch with no PR, so keying on
   the branch would skip forever - keying on the PR proceeds and reclaims it.
3. Otherwise bump the rev on the temper.git lines of BOTH manifests (line-scoped
   sed), re-assert uniformity == NEW, then `cargo update -p <crate>` for the crate
   list DERIVED from the temper.git dependency keys (no hardcoded list to drift) to
   refresh `Cargo.lock` (a pure sed would miss new transitive deps and break
   `--locked`).
4. Commit, push the bot branch with plain `git push --force` (a shallow fresh
   checkout has no remote-tracking ref for a lease; the PR-existence dedupe already
   proved the rev-specific bot branch is ours), and open ONE PR with `STACK_TOKEN`. When it opens a new PR it supersedes (closes + deletes the branch of) any older open `bot/temper-pin-*` PR, so successive temper advances never stack multiple open bumps.

**Why STACK_TOKEN, not GITHUB_TOKEN.** temper is PUBLIC, so this is NOT about read
access. A PR created with GITHUB_TOKEN does not trigger `pull_request` workflows, so
the bump's own gates would silently never fire. STACK_TOKEN is a real PAT whose
events are not suppressed (the same reason `sdlc-decision-intake.yml` already uses
it for body edits). The rust toolchain is installed only when a bump will happen
(after the no-drift early-exit).

**Gates.** The bump PR touches `Cargo.toml`, so the planning gate is not exempt and
needs `docs/efforts/<id>/`. The PR body references ARN-438, whose design chain
lives on main (this folder), so the planning gate resolves it. The body carries a
`## Decisions & Tradeoffs` section ("No decisions - mechanical bump") for the
decision-log gate. CI compiling and testing `--locked` against the new kernel IS
the proof.

## What this is NOT

No cross-repo event plumbing (no repository_dispatch from temper). A scheduled diff
check is enough and simplest. The pin bump is the deploy leg; the release itself
follows temperpaw's existing release flow once the bump merges.
