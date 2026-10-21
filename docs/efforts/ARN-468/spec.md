# ARN-468 spec

TemperDeploy swap authenticates to Railway GraphQL with vault
`railway_token` as `Authorization: Bearer`. That header is valid for
workspace and account tokens. Project tokens use
`Project-Access-Token` and a different query surface.

Vault IDs (`railway_project_id`, `railway_environment_id`,
`railway_service_id`) must match the openpaw production service. They
already did (same three UUIDs as the GitHub repo variables).

`seed_secret!` writes env `RAILWAY_TOKEN` into the vault on every boot.
A dead value in the service env comes back after every image swap.

## Invariants

- A Bearer token that can `variables` + `variableUpsert` on that
  project/env/service is what swap needs.
- After rotation, `TemperDeploy.Request` for a published `sha-*` tag
  reaches Healthy with `observed_sha == expected_sha`.
