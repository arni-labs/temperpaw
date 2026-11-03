# ARN-460 — App-token merge and implementer deploy callbacks

Rita named two leftovers after ARN-441 / #497.

The GitHub Apps she installed already open the factory door (`chain_github_ready` on Specify / Plan / Accept). `DsfDeploy.Request` is a different module (`release_run_lifecycle`). It still only reads vault `github_token`. A real DSF merge looks for that one secret. If it is empty, merge fails before GitHub is called. Use the App. Do not put a PAT in the vault.

The Ask residual was written as if Rita pressed buttons. She does not. Her implementer Agent fires `DsfDeploy.Request` / `TemperDeploy.Request`. Cedar then allows only `system` or `patrol-release-service` to land “merge started,” “healthy,” “fail.” The kernel applies those callbacks as the calling Agent (inline WASM) or `wasm-runtime` (background WASM). The row can sit in Merging until it times out and Stalls while the PR is already on main.

ReleaseRun stays service-only. Do not fire a live Request unless Rita says Go.
