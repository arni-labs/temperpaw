# ARN-460 spec

## GitHub for merge

`release_run_lifecycle` mints a GitHub App installation token the same way `chain_github_ready` does: `github_app_id` + `github_app_private_key` → JWT → list installs → mint for the repo owner. Tenant `github_token` is fallback when the App is not configured or that owner has no install.

Wired on every `release_run_lifecycle` trigger config on `DsfDeploy` and `ReleaseRun`. Merge already uses host `http_call`. Rollback still runs `git revert` on the computer; the token is charset-checked before it is interpolated into that one command.

## Deploy callbacks

On `DsfDeploy` and `TemperDeploy` only, the machine steps (`MergeSucceeded`, `Check`, `CheckPending`, `CheckHealthy`, `CheckUnhealthy`, `SwapSucceeded`, `RollbackPushed`, `Fail`) are allowed for:

- the same `Agent` that is allowed to `Request`
- `wasm-runtime` (background WASM `set_success_result`)
- `timeout-scheduler` / `system` (timers)
- `patrol-release-service` (elevated hops)

`ReleaseRun` is unchanged: create / Request / callbacks stay `patrol-release-service` or `system`. Ordinary Agents cannot drive it.

Effort `MarkDeployVerified` / `MarkDeployRolledBack` stay `patrol-release-service` (entity trigger `principal`). That hop is not this change.

## Out of scope

Live `DsfDeploy.Request` / `TemperDeploy.Request` unless Rita says Go. A new entity. A PAT in the vault. Opening `ReleaseRun` to implementer Agents.
