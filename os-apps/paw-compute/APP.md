# paw-compute

Manages persistent cloud VMs for developer agents. VMs survive agent restarts — code, dependencies, and state persist across sessions.

## Entity Types

### Computer
Long-lived Linux VM lifecycle.

- **States**: Created -> Provisioning -> Ready <-> Sleeping -> Destroyed (+ Checkpointing from Ready; Leased copies)
- **Key actions**: `Configure`, `Provision`, `ProvisionComplete`, `Checkpoint`, `Sleep` (Tensorlake suspend), `Wake` (resume), `Destroy`
- **Idle**: Ready → Sleep at 180s with no Heartbeat / Wake / Copy. Exec against Sleeping resumes, then Wake/Heartbeat reset the clock.
- **Governance fields**: `tools_installed`, `credentials_scoped`, `network_allow`, `project_harness_id`

Computer provisioning is handled by the session's `sandbox_provisioner` module in paw-agent.

### Exec
One governed shell command on a Computer's sandbox (ADR-0002).

- **States**: Created -> Running -> Succeeded | Failed (both terminal)
- **Walk**: create an Exec, dispatch `Run(computer_id, command)`. The `computer_exec` WASM integration resolves the Computer row, executes the command on its sandbox via the shared provider abstraction, and reports back — `RunSucceeded(exit_code, stdout_tail, stderr_tail, stdout_path, stdout_bytes)` or `RunFailed(error)`.
- **Audit fields**: `computer_id`, `command`, `exit_code`, `stdout_tail`, `stderr_tail` (256 KB tails), `stdout_path`, `stdout_bytes`, `error`

Full output is never lost. Before running, `computer_exec` wraps the command so its full combined output is persisted to `~/.exec-out/<exec_id>.log` on the computer, then returns a 256 KB tail. `stdout_path` is that log path and `stdout_bytes` is its full byte count — so an agent can `grep`/`sed`/page the complete output via follow-up Execs even when it exceeds the tail. The wrapper preserves the original exit code (`exit $__rc`), so `exit_code` still reflects the user command, not the wrapper.

Security posture: Run and Exec entity operations are permitted for Agent principals only; the RunSucceeded/RunFailed callbacks are admin-only (WASM dispatch path). Provider credentials come from the trigger config overlay at runtime — harnesses never hold them. Every command is a Cedar-gated entity with a durable audit trail. Long-running/interactive commands are out of scope (v1).

### LatencyDiag
A learned, constrained diagnostic (`specs/latency_diag.ioa.toml`). One action, `RunScan`, takes no parameters and runs a canned, read-only Datadog p95 latency query on a pinned computer through the same `computer_exec` trigger — credentials stay on the computer and there is nothing to inject at dispatch time.

- **States**: Idle -> Scanning -> Ready | Failed; `RunScan` re-runs from Idle, Ready, or Failed.
- **Walk**: create a LatencyDiag, dispatch `RunScan()`. `computer_exec` runs the canned command on the computer named by the entity's `computer_id` and reports back — `RunSucceeded(exit_code, stdout_tail, stderr_tail)` or `RunFailed(error)`.
- **Governance**: create/read/list/RunScan permitted for Agent principals; callbacks are system-dispatched (mirrors Exec). The command and target computer are fixed on the entity — the caller cannot supply either.

## WASM Modules

- **computer_exec** — executes an Exec's (or LatencyDiag's) command on the target Computer's sandbox, persisting full output to a per-exec log file and returning a bounded tail (`wasm/computer_exec/`, built via `wasm/build.sh`).

## Setup

No dependencies. Create a Computer entity with `Configure`, then `Provision` to start the VM. The `sandbox_url` and `ssh_host` are recorded on `ProvisionComplete`. To run a command on a Ready computer, create an Exec and dispatch `Run`.

## Browser access to the desktop (noVNC)

Humans reach the same desktop the agents work on in a browser. Run
`scripts/setup-novnc.sh` on the computer (through a governed Exec), expose
the port once with `tl sbx port expose <name> 6080`, then open:

    https://6080-<machine_id>.sandbox.tensorlake.ai/vnc.html

TensorLake port exposure is unauthenticated by design; the desktop stays
gated by TigerVNC's VncAuth password. Rotate the password out-of-band with
`vncpasswd` (never through an audited command line), and remove the public
page any time with `tl sbx port rm <name> 6080` — the `tl sbx tunnel` path
keeps working. This mirrors TensorLake's own recommended pattern (their
computer-use docs: bring your own noVNC bridge).
