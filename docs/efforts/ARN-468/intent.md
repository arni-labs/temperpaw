# ARN-468 — TemperDeploy can write IMAGE_TAG

ARN-466 shipped Sleep/Wake and Effort.Merge → TemperDeploy. The first
live Requests (`arn-466-temper-deploy`, `-2`) Failed on swap:
Railway GraphQL `Not Authorized`. GitHub Railway Redeploy wrote the
same IMAGE_TAG with a different token.

## Expected end state

- Vault `railway_token` is a workspace or account token Railway accepts
  as `Authorization: Bearer`.
- Service env `RAILWAY_TOKEN` is the same value, so the next boot does
  not reseed a dead credential.
- `TemperDeploy.Request` reaches Polling and Healthy without GitHub
  Redeploy.
