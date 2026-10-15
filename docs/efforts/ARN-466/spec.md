# ARN-466 — Computer idle suspend and Effort ship

One contract, three expressions: this file, the Computer / Effort
machines, and the tests that refuse a Sleep without WASM or a Merge that
stops at Merged.

## Computer

States stay Created → Provisioning → Ready ↔ Sleeping → Destroyed
(plus Copying / Leased / Checkpointing / Terminating).

- `Sleep` is Ready → Sleeping and fires `computer_sleep`. That module
  POSTs `https://api.tensorlake.ai/sandboxes/{id}/suspend`. 200 already
  suspended, 202 accepted. Failure → `SleepFailed` (Sleeping → Ready).
- `Wake` is Sleeping|Ready → Ready and fires `computer_wake`
  (`/resume`). Failure → `WakeFailed` (Ready → Sleeping).
- Ready is not indefinite. `[[state_timeout]]` 180s → Sleep.
  `reset_on = ["Heartbeat", "Wake", "Copy"]`.
- `Heartbeat` is a self-loop from Ready and Leased. No WASM.
- Exec start accepts Sleeping, calls `sandbox_resume`, then starts the
  command. ExecStarted → Computer.Wake. Poll / Succeeded / Failed →
  Computer.Heartbeat so a long run does not get suspended at 180s.
- Sleeping sources are resumable. Disk stays. We stop paying compute.

Modal has no suspend in this change. Unsupported provider fails closed.

## Effort

WorkCycle is not this path.

- `ConfigureDeploy(computer_id, image_tag, deploy_max_checks, probe_id)`
  sets `deploy_configured`. Default computer is `arni-big`.
- `Merge` requires `deploy_configured` and goes to **Deploying**.
  `chain_merge_ready` still retracts to Proving on a record miss.
  An entity trigger (`patrol-release-service`) creates
  `TemperDeploy.Request` with `effort_id`, `image_tag`,
  `expected_sha=head_sha`, `max_checks`, `probe_id`.
- `Deploy` from Merged (after rollback) creates another TemperDeploy.
- Kernel Effort (`nerdsane/temper`): after the temper PR merges, pin
  that SHA in TemperPaw, wait for GHCR, `ConfigureDeploy` with the new
  TemperPaw `image_tag`, then Merge. Pin is not inside Merge WASM.

## DST / tests

- `tensorlake_control_accepted` is 200 and 202 only.
- `computer_is_runnable` is Ready, Leased, Sleeping.
- `handle_from_computer` accepts Sleeping.
- `paw_patrol_foundation` requires Merge → Deploying and
  `target_entity = "TemperDeploy"`.
