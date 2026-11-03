## Decisions & Tradeoffs

**Decision:** The planning and decision-log gates' "Temper verdict" in the sweep
is a mirror computed from PR content with the gates' own stack scripts, not read
from entity state.
**Came up because:** S0 only added record entities for review and proof; there is
no planning or decision-log entity, but the shadow needs a Temper verdict for all
four gates to compare against CI.
**Options:** invent planning/decision entities now (rejected - that is new S0
work, and this phase changes no entities); skip those two gates (rejected - the
shadow must cover every required gate); recompute them from PR content with the
same gate scripts (chosen).
**Chose the mirror because:** it keeps the phase entity-free and still produces a
verdict for all four gates. It is stated plainly (spec + README) that review and
proof are the true state-machine shadow while planning and decisions are a
consistency check on the gate scripts - so a planning/decisions disagreement would
mean the script is non-deterministic, not that Temper and CI differ. Given up: a
"pure" entity-derived verdict for those two, which waits for the Effort state
machine (a later phase).
**Where:** `stack/shadow/shadow-sweep.py`, `docs/efforts/ARN-431/spec.md`.

---

## Acceptance finding (Part 3, live run 2026-08-29)

**Finding:** The shadow sweep's prod principal (`TEMPER_API_KEY`) is Cedar-denied
for the paw-patrol write actions. `ReviewRun.Ingest`, `ReviewRun.IngestRecord`,
`ProofPacket.Ingest`, and `ShadowVerdict.Record` / `MarkAgree` / `MarkDisagree`
all return `403 AuthorizationDenied: no matching permit policy`. Only lazy entity
creation and reads succeed.
**Consequence in the acceptance run (PRs 477/480/481/482/484/476):**
- `review` / `proof` shadow verdicts are `na` — the ReviewRun/ProofPacket
  entities are lazy-created but stuck at their initial states (`Requested` /
  `Drafting`) because `Ingest` is denied, so nothing advances them to `Recorded`.
- 24 ShadowVerdict rows (4 gates x 6 PRs) exist but are EMPTY — they lazily mint
  at ShadowVerdict's initial state `Recorded`, but `Record`/`MarkAgree` (which set
  pr/gate/verdicts) were denied, so every field is null.
- `planning` / `decision-log` verdicts are computed locally (from PR content via
  the gate scripts), so they show correctly and agree with CI 12/12 — but their
  ShadowVerdict rows also could not be populated.
**This is not a Temper-vs-CI disagreement.** The review/proof `DIFF` rows are
`na` vs `pass` because the sweep could not write to Temper, not because the state
machine concluded differently. The real S0 machine output was never produced in
prod for these PRs.
**Not patched here (by constraint - modules/entities/Cedar unchanged in S1):**
the fix is an owner authorization decision - grant a shadow-sweep identity (or
the sweep's principal) permission for the Ingest/IngestRecord/IngestProof and
ShadowVerdict write actions in paw-patrol's Cedar policy. This is the same
"writes behind a governed action" work S2 needs, and it connects to ARN-430's
recorded residual. Recommend a follow-up issue; S1's sweep + workflow are ready
and will produce real entity-derived verdicts once the grant lands.
**Where:** live run against openpaw-production; evidence in the ARN-431 report.

---

## Panel round fixes (#486)

**Decision:** Pass the PR list to the sweep via an `env:` var read into a bash
array, not a `${{ }}` splice into the `run:` script; sanitize the dispatch input
and write `$GITHUB_OUTPUT` with a heredoc delimiter.
**Came up because:** the panel flagged `steps.window.outputs.prs` spliced as a
literal `${{ }}` into the shell (workflow-injection class), and `INPUT_PRS`
written into `$GITHUB_OUTPUT` unquoted (output-injection class).
**Fix:** `PRS: ${{ steps.window.outputs.prs }}` in `env`, then
`read -ra pr_args <<< "$PRS"` + `"${pr_args[@]}"`; the dispatch input is
`tr -cd '0-9 '`-sanitized before it reaches the shell, and the output is written
with a `<<SHADOW_PRS_EOF` delimiter so no value can inject an extra key. Values
are data, never executable text. **Where:** `.github/workflows/shadow-sweep.yml`.

**Decision (considers):** Widen the nightly window to ~48h and note in the
workflow that review/proof read `na` until ARN-434's permits land.
**Where:** `.github/workflows/shadow-sweep.yml` (since window + header comment).

Stale-branch artifacts (apparent ci.yml/READY_PATH revert + docs/efforts/ARN-432
deletion) dissolved by merging origin/main - the PR diff is now only the
shadow-sweep workflow + the ARN-431 design chain.

---

## Panel delta fixes (#486)

**Act-on (input sanitization concatenated):** replaced `tr -cd '0-9 '` (which
could fuse "12;34" into a different valid PR number "1234") with PER-TOKEN
validation - split on whitespace/commas, each token must be 1-6 digits or the
run fails loudly. Verified under bash: "12;34" and "477 && rm -rf x" are
rejected, valid lists accepted, blank -> nightly.

**Act-on (limit truncates before the window filter):** the nightly query now
fetches newest-updated-first (`--search "sort:updated-desc"`) with a 200 cap, so
the limit cannot drop in-window PRs before the `updatedAt` filter runs, and it
warns loudly if the full cap is returned (possible truncation).

**Consider (gh failure not caught):** the `gh pr list` result is captured with an
explicit `|| { echo ::error; exit 1; }` and the step runs `set -euo pipefail`.

**Consider (nightly cron):** kept enabled (486 merges after 434's permits, so the
na-window is nil-to-short) and the run step now logs the na-constraint line at
start.

**Nit (permissions):** dropped the unused `contents: read` (no repo checkout -
stack is cloned with STACK_TOKEN); kept `pull-requests: read` + `checks: read`
which gh needs via GITHUB_TOKEN.

---

## Terminal act-on (#486): the mirror needs a repo tree

**Act-on:** the sweep ran `check-effort-artifacts.py` / `check-decision-log.py`
(the planning + decision-log mirrors) with NO checkout of this repo, so they
computed against an empty tree (git diff empty -> exempt -> a hollow pass).
**Fix:** the job now `actions/checkout`s this repo, and the run step sweeps ONE
PR at a time - fetching each PR's head (`refs/pull/<n>/head`, depth 1) and
checking it out - so each PR's mirror reads THAT PR's `docs/efforts/<id>/`. This
is why a single checkout wasn't enough: the sweep covers a multi-PR window and
each PR's design chain lives on its own head.
**Reverts the earlier nit:** ARN-431's #486 round dropped `permissions:
contents: read` as unused; the checkout now needs it, so it is restored. The
history reads: removed when there was no checkout, restored when the mirror fix
added one.
**Considers taken:** the token-validation loop uses `read -ra` (no pathname
expansion can fire - `*` is rejected, not globbed); the nightly sort/cap path's
first live exercise is on the first nightly run, noted in the workflow.
**Where:** `.github/workflows/shadow-sweep.yml`.
