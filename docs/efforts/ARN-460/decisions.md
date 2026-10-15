# Decision log — ARN-460

**Decision:** (2026-09-03) DsfDeploy / ReleaseRun merge mints a GitHub App installation token; tenant `github_token` is fallback only.
**Came up because:** Rita installed two GitHub Apps. That solved `chain_github_ready`. `release_run_lifecycle` still read only `github_token`. Production vault has no PAT. She refused putting one in. She asked why merge does not use the App.
**Options:** (1) put a PAT in the vault; (2) keep documenting GitHub as unsolved; (3) mint an installation token the same way the door does.
**Chose (3) over (1) and (2) because:** the Apps are already the factory credential. (1) is the thing she forbade. (2) was the last report’s miss. What we gave up: a tenant with neither App nor `github_token` still cannot merge (correct fail).
**Where:** `os-apps/paw-patrol/wasm/release_run_lifecycle/src/github_app.rs`; DsfDeploy and ReleaseRun trigger configs.

---

**Decision:** (2026-09-03) Copy App JWT mint into `release_run_lifecycle` instead of extracting it into `wasm-helpers`.
**Came up because:** two modules now need the same mint. Extracting to `wasm-helpers` would compile `rsa` into every helper consumer.
**Options:** (1) extract to `wasm-helpers`; (2) a new shared crate; (3) copy the door’s mint into this module.
**Chose (3) over (1) and (2) because:** (2) is new machinery. (1) taxes every agent WASM for one merge path. A later extract is cheaper than pulling `rsa` through the helper graph now.
**Where:** `release_run_lifecycle/src/github_app.rs`; `chain_github_ready` unchanged.

---

**Decision:** (2026-09-03) `rsa` on this crate only, with a wasm32 `getrandom` custom stub that returns UNSUPPORTED.
**Came up because:** `rsa` 0.9 pulls `getrandom` 0.2, which `compile_error!`s on `wasm32-unknown-unknown`. JWT sign does not draw randomness. Putting `getrandom` on `wasm-helpers` already broke other modules (ARN-443).
**Options:** (1) extract App mint to wasm-helpers with getrandom; (2) hand-roll RS256; (3) crate-local `getrandom` custom feature + stub.
**Chose (3) because:** (1) is the ARN-443 regression. (2) is more crypto code. The stub is compile-only. What we gave up: this crate now has a wasm-only dep the door crate does not declare (the door blob was built in an environment that already satisfied getrandom).
**Where:** `release_run_lifecycle/Cargo.toml`; `github_app.rs`.

---

**Decision:** (2026-09-03) DsfDeploy / TemperDeploy machine callbacks are allowed for the implementer Agent and the kernel principals that actually apply them (`wasm-runtime`, `timeout-scheduler`, `system`, `patrol-release-service`). ReleaseRun stays service-only.
**Came up because:** the residual said “not for you.” Rita does not press Request. Her Agent does. Inline WASM `set_success_result` is dispatched as that Agent. Background WASM uses `AgentContext::for_service("wasm-runtime")`. Timers inherit the caller or fire as `timeout-scheduler`. Cedar only allowed `system` / `patrol-release-service`, so MergeSucceeded / Check / Fail were denied and the row sat Merging until timeout.
**Options:** (1) leave it (Rita would have to press); (2) kernel change so WASM callbacks always elevate to a service; (3) widen Cedar on the project tools only, including the principals the kernel actually uses.
**Chose (3) over (1) and (2) because:** (1) is the bug. (2) is Temper, not this repo. Widening only DsfDeploy / TemperDeploy keeps ReleaseRun closed. What we gave up: an Agent can name MergeSucceeded without the WASM having merged — same class as Request already being open to any Agent; the row still has to be in Merging.
**Where:** `os-apps/paw-patrol/policies/patrol.cedar`; `paw_patrol_foundation.rs` `effort_merge_permits_l0_l1_and_denies_l2`.

---

**Decision:** (2026-09-03) Rollback charset check accepts the 2026 stateless App installation token (`ghs_<appid>_<jwt>`, dots, up to 1024 chars).
**Came up because:** The ARN-460 panel (Grok) found `validate_github_token` still capped at 256 chars and rejected `.`. Merge HTTP does not call it. Rollback interpolates the minted token into `TOK='…'` after this check. GitHub’s April–June 2026 rollout issues ~520-char JWTs with two dots.
**Options:** (1) merge with the finding open; (2) skip charset check on App tokens; (3) allow `.` and raise the cap to 1024, keep rejecting shell metacharacters.
**Chose (3) over (1) and (2) because:** (1) is the bug this effort exists to close. (2) would put an unchecked string into a sandbox shell. What we gave up: a token longer than 1024 still fails (GitHub documents ~520).
**Where:** `os-apps/paw-patrol/wasm/release_run_lifecycle/src/lib.rs` `validate_github_token`.
