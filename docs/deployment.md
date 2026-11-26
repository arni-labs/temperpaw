# Deployment

## Preferred path

Run `temperpaw deploy`. The CLI is responsible for:

1. Checking prerequisite CLIs.
2. Provisioning Turso and R2.
3. Creating the Railway project.
4. Seeding the admin account and dashboard-managed secrets.

## Railway fallback

If you prefer the one-click path, use the Railway button in the README and provide the infrastructure variables manually:

- `TURSO_URL`
- `TURSO_AUTH_TOKEN`
- `BLOB_ENDPOINT`
- `BLOB_BUCKET`
- `BLOB_ACCESS_KEY`
- `BLOB_SECRET_KEY`

## Genesis app source

A server's apps are its install records. Boot restores exactly the apps those
records name, at the versions they name, and installs nothing else. A new server
has no apps until whoever installs them does (the Factory line, UniKon, or an
operator): `POST /paw/apps/install-from-genesis` with a pinned `owner/app@hash`
ref, from the registry the server installs from (`TEMPER_GENESIS_REGISTRY_URL`).
The install puts in the app's specs, Cedar, modules and the routes its
`routes.json` declares, all or nothing. It answers 200 with the install and its
`install_id` when it ends within 25 s, otherwise 202 with the `install_id`; the
installs list, `GET /api/genesis/apps/installs`, then gives the app's last
install in its `installs`: `running`, `installed`, or `failed` with its
`reason`.

`/paw/apps/install-from-genesis` takes only the operator credential. Any other
key, such as the Factory line's installer key, installs through the kernel's
own `POST /api/genesis/apps/install` (the same body, `tenant` being the key's
own) and reads `GET /api/genesis/apps/installs`. Cedar decides both, as
`install_app` on `App` and `read_app_installs` on `AppInstall`, so a key does
exactly what its tenant's policy permits its agent type.

Boot fetches each installed Genesis app's files again before restoring it,
bounded by `TEMPERPAW_GENESIS_CACHE_RESTORE_TIMEOUT_SECS` (default 20). If that
fetch stalls, TemperPaw logs a warning and continues to readiness with the
installed state already in the database. Do not reset, wipe, replace, or restore
over a production database.

## Optional Datadog

Telemetry goes to Datadog through the `datadog-runtime-agent` Railway service (see `DEPLOYMENT.md`). Without a Datadog key, set `OTEL_ENABLED=false`.
