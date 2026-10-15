# Decision log — ARN-466 (Computer Sleep / Effort.Merge ship)

**Decision:** Sleep and Wake are one WASM module each; they do not dispatch.
**Came up because:** Sleep was a bare Ready → Sleeping transition. The 180s rule was prose.
**Options:** (1) keep Sleep as a state change and cron keepwarm; (2) one module that suspends and also Heartbeats; (3) computer_sleep on Sleep, computer_wake on Wake, Ready timeout 180s.
**Chose (3) over (1) and (2) because:** (1) is the hole. (2) hides sequencing in WASM. What we gave up: a Suspending intermediate state.
**Where:** `os-apps/paw-compute/specs/computer.ioa.toml`; `wasm/computer_sleep`; `wasm/computer_wake`.

---

**Decision:** Wake is Sleeping|Ready → Ready, not Sleeping → Provisioning.
**Came up because:** Wake used to go to Provisioning (re-provision). Resume is not provision. ExecStarted must be able to fire Wake on a warm box (idempotent 200).
**Options:** (1) keep Provisioning; (2) Sleeping only → Ready; (3) Sleeping|Ready → Ready with /resume.
**Chose (3) over (1) and (2) because:** (1) would rebuild the VM. (2) makes ExecStarted → Wake fail on Ready. What we gave up: Wake on a Leased copy (would steal it to Ready). Leased execs Heartbeat instead.
**Where:** `computer.ioa.toml` Wake; `exec.ioa.toml` ExecStarted trigger.

---

**Decision:** Effort.Merge goes to Deploying and creates TemperDeploy. WorkCycle is not this path.
**Came up because:** Merge → Merged left MarkDeployVerified stuck. ReleaseRun already shipped DSF from WorkCycle.Complete; Effort never got the child.
**Options:** (1) allow MarkDeployVerified from Merged; (2) Merge → Merged plus a second Deploy action the agent remembers to fire; (3) Merge requires ConfigureDeploy, enters Deploying, spawns TemperDeploy.
**Chose (3) over (1) and (2) because:** (1) papers over the missing child. (2) is the hole Rita named. What we gave up: Merge without a ship target; DSF ReleaseRun from Effort (still WorkCycle until someone adds a second trigger).
**Where:** `os-apps/paw-patrol/specs/effort.ioa.toml`.

---

**Decision:** Kernel pin stays a ConfigureDeploy input, not Merge WASM.
**Came up because:** A temper PR still has to pin and image-swap TemperPaw after merge.
**Options:** (1) Merge WASM edits temperpaw Cargo.toml and opens a pin PR; (2) agent pins, waits GHCR, ConfigureDeploy(image_tag), Merge.
**Chose (2) over (1) because:** (1) is several concerns in one module and hides the pin from the machine. What we gave up: one-click kernel ship.
**Where:** Effort ConfigureDeploy hint; this log.

---

**Decision:** Encode the shared computer in stack AGENTS.md, not only dd-computer docs.
**Came up because:** Codex does not pick arni-big if the rule is only in `stack/docs/dd-computer/`.
**Options:** (1) leave it in dd-computer docs; (2) put it in the Temper operating layer section of stack AGENTS.md.
**Chose (2) over (1) because:** that file is what every harness loads. What we gave up: nothing.
**Where:** `arni-labs/stack` AGENTS.md (worktree `cursor-arn466-agents-computer`).
