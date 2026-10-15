# ARN-460 plan

## What we are addressing

A real DSF merge still fails if `github_token` is empty, even when the GitHub App can already see the repo. After Request, the implementer Agent cannot record that the merge started.

## Expected end state

`release_run_lifecycle` uses the App for GitHub HTTP. Cedar lets the implementer Agent and the kernel principals that actually fire the next action complete a DsfDeploy / TemperDeploy. ReleaseRun stays closed. No live Request in this effort.

## Steps

1. Add App mint to `release_run_lifecycle` (copy of the door’s JWT/install path; `rsa` only on this crate).
2. Wire `github_app_id` / `github_app_private_key` on DsfDeploy and ReleaseRun trigger configs.
3. Widen Cedar for DsfDeploy / TemperDeploy callbacks.
4. Foundation + unit tests. Rebuild and commit `release_run_lifecycle.wasm`.
5. Load-inline the spec + Cedar live after merge. Do not fire Request.
