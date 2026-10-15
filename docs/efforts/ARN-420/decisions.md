# Decisions & Tradeoffs

**Decision:** Trigger deploy on merge to a `release` branch, not on merge to `main`.
**Came up because:** the loop needs an automatic deploy, but not every main merge
should hit prod.
**Options:** auto-deploy on main; explicit `workflow_dispatch`; a dedicated release
branch.
**Chose release branch over the others because:** it gives a clear, gated release
action (merge to release) while keeping main fast, and the existing ancestry guard
already enforces that release contains main. Gave up the one-click simplicity of
deploying straight from main.
**Where:** `.github/workflows/docker.yml` `deploy` job `if:` guard.

**Decision:** Append the deploy as a job in `docker.yml` (needs build-and-push)
rather than a separate `workflow_run` workflow.
**Came up because:** the deploy must run only after the image exists.
**Options:** a separate `workflow_run`-triggered workflow; an appended job with
`needs`.
**Chose the appended job because:** build and deploy share one run so ordering is
guaranteed and there is no cross-workflow race; `workflow_run` also only fires from
the default branch, which is an easy footgun. Gave up separating deploy into its own
file.
**Where:** `.github/workflows/docker.yml`.

**Decision:** Roll back by restoring the exact previously-deployed tag, with a
`sha-<short7>` reconstruction as fallback.
**Came up because:** rollback must target a real GHCR tag.
**Options:** always reconstruct from the sha; read the live tag; keep a separate
last-good record.
**Chose read-live-tag-then-fallback because:** the tag Railway currently holds is the
ground truth of what is deployed; the sha reconstruction (verified against real GHCR
tags as `sha-<7hex>`) covers the case where the variable read is unavailable. Gave up
a separate state store.
**Where:** `stack/deploy/railway-deploy-verify-rollback.sh` rollback block.

**Decision:** Scope this effort to the server image path; leave the os-app publish
path to a second part.
**Came up because:** the two deploy paths are independent and the os-app path is
MCP/Temper-driven, not a CI shell script.
**Options:** do both now; split.
**Chose split because:** the server path is a clean, testable CI deliverable that
closes the concrete no-rollback gap today; bundling the larger os-app runner would
delay it. Gave up closing the whole deploy stage in one PR.
**Where:** this effort; os-app runner tracked as ARN-420 part 2.

**Decision:** Cut Datadog alert-gating from the bash deploy driver (owner ruling A, 2026-08-28, after the convergence breaker at round 3).
**Came up because:** two consecutive panel rounds found real holes in the gating (tag filter covered 5 of 79 monitors; no pre-deploy baseline, so one standing alert blocks all deploys) while the rest of the pipeline reviewed clean.
**Options:** harden the bash (tag re-provisioning, baseline comparison - rejected); cut the feature.
**Chose the cut because:** monitoring-informed gating belongs to stage 3's Deployment entity, where the AlertWatch guard has real coverage; hardening bash that stage 3 deletes is spend without a keep. Given up: automated monitor gating until the Deployment entity lands - deploys still verify identity and roll back on failed verification.
**Where:** stack deploy driver 58b4be9; this workflow's DD plumbing removed.
