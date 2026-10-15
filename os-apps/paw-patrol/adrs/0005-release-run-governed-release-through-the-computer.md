# ADR-0005: ReleaseRun — governed release through the computer

**Status:** Accepted (2026-08-24)

## Context

On 2026-08-23 the deep-sci-fi latency fix was merged *through the computer*
(a governed `Exec` on `dsf` calling the GitHub merge API), which made the
merge Cedar-authorized and attributed to the agent's own credential. But that
was where governance stopped: Railway auto-deployed on the push, nothing
watched whether the new build came up healthy, and the only rollback was a
human re-pinning a deployment by hand. `DeployRun` (dsf-deploy) exists but
deploys by HTTP to configured URLs, not through the computer, and was never
used for that release.

The team rule for closing this: orchestration is declarative (automaton +
kernel timers + `[[action.triggers]]`), WASM modules do side effects only and
must not dispatch transitions on other entities.

## Decision

Add `ReleaseRun` to paw-patrol and extend `WorkCycle`:

- `WorkCycle.ConfigureRelease` records the release target (repo, PR number,
  computer, health URL, probe budget) and sets `release_configured`.
- `WorkCycle.Complete` carries an entity-kind trigger guarded by
  `bool_true release_configured` that creates a `ReleaseRun` and fires
  `Request` with `params_from` — no code involved.
- `ReleaseRun`: `Requested → Merging → Watching → Healthy`, with
  `Watching → Unhealthy → RolledBack` on a bad rollout and `Failed` when a
  step cannot run.
- A single WASM module, `release_run_lifecycle`, runs one side effect per
  trigger on the named `Computer`'s sandbox (the same `wasm_helpers::sandbox`
  path as `computer_exec`) and reports through the named callback:
  - `Request` → merge the PR via the GitHub API with the token stored on the
    computer → `MergeSucceeded(merge_sha)`.
  - `Check` → one `curl` of the health URL → `CheckHealthy` when HTTP 2xx,
    `status == healthy`, and the served `git_sha` equals `merge_sha`;
    `CheckPending` otherwise; `CheckUnhealthy` when the new commit serves
    degraded or the probe budget is spent.
  - `CheckUnhealthy` → `git revert -m 1 <merge_sha>` and push `main` →
    `RollbackPushed(revert_sha)`. The push takes the same GitHub-connected
    deploy path as the merge, so the platform redeploys the previous build.
- The watch loop is a kernel `state_timeout` on `Watching` (30s,
  `reset_on = ["Check", "CheckPending"]`) that fires `Check`. There is no
  polling loop in code.

The service under release reports the commit it is serving as `git_sha` on
its health endpoint (deep-sci-fi: `RAILWAY_GIT_COMMIT_SHA`), which is what
makes "the rollout landed" a checkable fact rather than a timer guess.

## Consequences

- Merge, watch, and rollback are each a Cedar-gated transition on one row,
  so the whole release reads as state transitions and the audit answers
  who requested it and what happened.
- Rollback is event-driven: an unhealthy rollout reverts itself without a
  human noticing first. Reverting is the rollback (not re-pinning an image)
  so it works for any GitHub-connected service and leaves the reason in git
  history.
- The revert's own rollout is not watched by this version; a follow-up can
  chain a second `ReleaseRun` (or a `Watching`-only mode) on `RolledBack`.
- Fix-specific verification (e.g. "did p95 improve") stays a learned tool
  (`LatencyDiag`), not part of the generic release gate — the generic gate is
  health + served commit only.
- `work_cycle_lifecycle` still dispatches cross-entity actions from WASM
  (`WorkerRun.Configure`, `WorkCycle.StartWork`, …). That predates this rule
  and is flagged here for a separate refactor; `release_run_lifecycle` does
  not follow that pattern.

## Hardening addendum (2026-08-24, post-review)

Three changes from the adversarial re-review, beyond the first-round P0 fixes:

- **Commit-binding.** The merge PUT pins the PR head sha it just read (`"sha"`),
  so GitHub refuses if the head moved between read and PUT (read→PUT TOCTOU).
  An optional `expected_head_sha` (set via `WorkCycle.ConfigureRelease` →
  `release_expected_head_sha`, carried on `Request`) binds the release to a
  specific reviewed commit; the merge refuses unless the PR head still matches.
  The merged head is recorded (`head_sha`, on `MergeSucceeded`) for audit.
  The merge PUT pins only the head, not the base, so after merging `merge()`
  re-reads the PR and refuses to emit `MergeSucceeded` unless it is now merged
  into `main` at the bound head (base-branch TOCTOU: a maintainer could retarget
  the PR's base between preflight and PUT; GitHub's merge API exposes no
  expected-base). The same re-read reconciles an ambiguous PUT whose connection
  dropped after the merge landed.
- **Rollback isolation.** The revert runs in a fresh `mktemp -d` checkout per
  attempt (never a reused, potentially-poisoned dir), with `GIT_CONFIG_GLOBAL`/
  `GIT_CONFIG_SYSTEM` neutralized and `core.hooksPath=/dev/null`,
  `commit.gpgsign=false` so a planted global config or repo hook cannot execute
  under the release workflow. The token is supplied per-invocation via an
  `http.extraHeader` Authorization header (never written into the remote URL or
  `.git/config`, so it cannot leak through a config left behind by a kill).
  Idempotency is tip-only (HEAD's message), so a stale historical revert never
  skips a needed rollback. The revert adapts to the merge shape — `-m 1` for a
  true merge commit, plain `git revert` for a single-parent (squash/rebase)
  merge reconciled from an out-of-band merge — so no merge shape is
  un-rollbackable.
- **Per-repo serialization (ARN-397, reject).** Before merging, `merge()` reads
  other ReleaseRuns and refuses (→ Fail) if any for the same repo is active
  (Requested/Merging/Watching/Unhealthy). The query filters on active status
  only and matches the repo case-insensitively in code: the kernel's OData `eq`
  is case-sensitive, so pushing `repo eq …` server-side would drop an active
  `Owner/Repo` row before a new `owner/repo` run could be caught (GitHub
  owner/repo is case-insensitive — the same target). The active set is tiny
  (≤1 per repo), so status-only is page-safe, and the read fails closed on
  pagination or a malformed response. This is a read + the run's own Fail — no
  cross-entity dispatch. Residual TOCTOU (two Requests racing the check before
  either commits Merging) needs an atomic per-repo lane entity — the stronger
  form of ARN-397, tracked separately.

## Round-5 addendum (2026-08-24, post final review)

Two further correctness fixes from the round-5 review, plus documented residuals.

- **Merge confirmation binds to the pinned head and tolerates a lagging read.**
  The post-merge re-read now (a) requires the merged head to equal the head we
  pinned in the PUT — so if the PUT was refused (head moved) and a *different*
  head was merged out-of-band, we refuse to watch a commit we did not merge,
  closing the reconcile head-bypass even when `expected_head_sha` is unset; and
  (b) is retried a few times, so a transient GET blip or GitHub read-after-write
  lag no longer strands an actually-merged release as `Failed`.
- **Rollback auto-runs only for a true merge commit.** `-m 1` reverts the whole
  PR merge. A single-parent tip means the PR was merged out-of-band via
  squash/rebase; a rebase tip is only the last of N commits and is
  indistinguishable from a squash tip, so a plain revert could silently leave
  earlier commits deployed while reporting success. We refuse (→ Fail) and
  escalate to a human instead. Our own workflow always merges via
  `merge_method=merge` (a 2-parent merge commit), so the normal release path is
  always fully rollbackable; only out-of-band reconciled squash/rebase merges
  escalate.

### Documented residuals (tracked, not fixed here)

- **Base-retarget has no compensation (detection only).** GitHub's merge API
  pins the head but exposes no expected-base, so a maintainer retargeting the
  PR's base in the window between our preflight read and the PUT can cause a
  merge into a non-main branch. The confirm gate PREVENTS the dangerous outcome
  (we never watch or revert a non-main merge onto `main`) — the run just Fails
  visibly — but it cannot PREVENT the accidental merge itself or undo it.
- **Repo identity is textual, not canonical.** The per-repo guard compares
  owner/name case-insensitively; it does not resolve GitHub's numeric repo id,
  so renaming a repo mid-release (old and new names both redirect to the same
  repo) can let two runs proceed. Closing this needs the atomic per-repo lane
  entity below.
- **ARN-397 guard bounds at scale / cold start.** The kernel's projection
  coverage check (which lets the guard see a committed-but-unprojected active
  run) is bypassed when total ReleaseRuns exceed the scan candidate budget
  (10× max_entities) or right after a cold restart (empty in-memory index);
  terminal rows are never GC'd, so this is a slow-burn condition. Separately,
  because the guard now filters on status only, >100 *active* rows tenant-wide
  (e.g. runs stuck by the ARN-396 restart residual) fail every repo's releases
  closed rather than just their own. A periodic sweep that Fails stuck runs, and
  the per-repo lane entity, close both.

The **atomic per-repo lane entity** is the stronger form of ARN-397 and the
single fix for the TOCTOU race, canonical-repo-identity, and at-scale coverage
gaps together; tracked separately.

## Round-6 addendum (2026-08-25, PR review)

- **Health-probe SSRF guard (fixed).** `validate_url` restricted the health URL's
  syntax but not its host; because the probe is `curl`ed from the credentialed
  computer sandbox, a configured loopback/private/link-local/metadata URL could
  reach internal services. `validate_url_host` now pins the host to a public
  endpoint — refusing `localhost`, `127.`/`10.`/`192.168.`/`172.16–31.`/`0.`,
  the `169.254.` link-local+metadata range, IPv6 loopback/ULA/link-local, and
  bare single-label names.
- **ConfigureRelease target binding (residual).** The `ConfigureRelease` permit
  authorizes the *caller* (Admin/system/patrol-release-service/supervisor) but
  does not yet bind the release *target* (repo/PR/computer) to the WorkCycle's
  own produced work artifact — a trusted supervisor could name an unrelated
  target. Caller-authorization is the governed control today; target-provenance
  binding needs the WorkCycle to model its output artifact and is tracked as a
  follow-up (it applies to the whole WorkCycle→ReleaseRun completion path, not
  just this permit).
- Two Greptile findings were verified false positives against the merged code:
  the health-URL "shell injection" (the `validate_url` allowlist rejects
  `'`/`` ` ``/`$`/`;`/`|`/space and is called on both the merge and check paths,
  with a dedicated exploit test) and "any Agent can create/Request a ReleaseRun"
  (create/Request is gated `when principal.agent_type == "patrol-release-service"`;
  only read/list are tenant-open).

Known residual (kernel, ARN-396): `state_timeout`s are in-memory and arm only
on dispatch, so a `Requested`/`Watching` run can still hang across a restart
until durable timeout delivery lands.
