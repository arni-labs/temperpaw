# Decision log — ARN-468

**Decision:** Replace the vault/service `RAILWAY_TOKEN` with a workspace token. Do not change TemperDeploy WASM headers.
**Came up because:** Swap Failed with Railway GraphQL `Not Authorized`. IDs already matched GitHub. The 308-char service token returned `Not Authorized` as Bearer and `Project Token not found` as `Project-Access-Token`.
**Options:** (1) teach WASM `Project-Access-Token` and keep the 308-char value; (2) put Rita's short-lived CLI OAuth token in the vault; (3) mint a workspace token and store it in the vault and in service env.
**Chose (3) over (1) and (2) because:** (1) the 308-char value is not a project token. (2) expires the same day. What we gave up: a WASM that can use a project token later.
**Where:** `/paw/setup/secrets` `railway_token`; openpaw service `RAILWAY_TOKEN`; TemperDeploy `arn-468-temper-deploy` Healthy.
