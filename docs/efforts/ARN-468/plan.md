# ARN-468 plan

What we are addressing: TemperDeploy cannot write IMAGE_TAG.

Expected end state: a live TemperDeploy row reaches Healthy using the
vault token. GitHub Redeploy is not the ship path.

1. Compare `/paw/infra/railway/status` IDs to GitHub
   `RAILWAY_*` variables.
2. Probe the service `RAILWAY_TOKEN` as Bearer and as
   `Project-Access-Token` without printing it.
3. Mint a workspace token (`apiTokenCreate` with workspaceId). Write it
   to `/paw/setup/secrets` `railway_token` and to service env
   `RAILWAY_TOKEN` with skip-deploys.
4. `TemperDeploy.Request` for the live image tag. Reinstall
   `paw-compute` if the swap rematerializes Computer without SleepFailed.
