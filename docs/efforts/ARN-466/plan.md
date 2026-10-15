# ARN-466 — Plan

## What we are addressing

Sleep did not suspend Tensorlake. Merge did not start TemperDeploy.

## Expected end state

Ready sleeps at 180s and that Sleep suspends the sandbox. Wake and exec
resume it. Effort.Merge enters Deploying and creates a TemperDeploy.

## Steps

1. `sandbox_suspend` / `sandbox_resume` next to `sandbox_terminate`.
2. `computer_sleep` and `computer_wake` wasm32-wasip1 modules. Wire
   them on Sleep / Wake. 180s Ready timeout. Cedar for system Sleep
   and the new modules' http_call / secrets.
3. Exec start resumes a Sleeping box. ExecStarted wakes the Computer.
   Poll and completion Heartbeat so the 180s clock does not fire mid-run.
4. Effort.ConfigureDeploy + Merge → Deploying + TemperDeploy.Request.
   Foundation test locks that wiring.
5. Stack AGENTS.md: work and ship on `Computers('arni-big')`.
6. Build paw-compute blobs (wasi≥1, wbindgen=0). Publish paw-compute
   then paw-patrol to Genesis. Install. Confirm Tensorlake running
   count is still zero unless we Wake.
