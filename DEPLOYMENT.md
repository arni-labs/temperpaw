# TemperPaw Deployment Guide

TemperPaw is a **single-user, self-hosted** platform. Each deployment serves one operator (person or team) with one Turso database, one blob storage bucket, and one compute instance. There is no multi-tenant SaaS mode.

---

## Architecture Overview

```
                   GitHub Actions (CI)
                         |
                    Docker build
                         |
                         v
                  GHCR (ghcr.io/nerdsane/temperpaw)
                    :edge  :latest  :sha-*  :semver
                         |
                         v
         +----- Railway (compute) ------+
         |                              |
         |  temperpaw (main service)      |
         |    port 3467                 |
         |    /healthz liveness check   |
         |                              |
         |  otel-collector (sidecar)    |
         |    port 4318 (internal)      |
         |    portable-otel fallback    |
         |                              |
         |  datadog-runtime-agent       |
         |    4318 OTLP, 8126 APM       |
         |    Datadog-enhanced profile  |
         |                              |
         |  datadog-postgres-agent      |
         |    DBM only                  |
         +------------------------------+
                   |           |
                   v           v
           Turso (DB)    Cloudflare R2 (blobs)
```

**Current providers:**

| Component | Provider | Free Tier |
|-----------|----------|-----------|
| Compute | Railway | 512 MB RAM, 1 vCPU |
| Database | Turso (libSQL) | 9 GB, 500M rows |
| Blob storage | Cloudflare R2 | 10 GB |
| Container registry | GitHub Container Registry (GHCR) | Unlimited for public repos |
| Observability | Datadog (via OTEL) | Optional |

**Future direction:** The deployment CLI (`temperpaw deploy`) is designed to be extended to other cloud providers. The core app is a single static binary + dashboard assets + WASM modules, so it can run anywhere that supports Docker or bare metal. Turso and R2 could be swapped for any libSQL-compatible DB and S3-compatible storage.

---

## Quick Start: Automated Deploy

The fastest path from zero to a running instance:

```bash
# Install the CLI
cargo install temperpaw-cli

# Run the interactive deploy wizard
temperpaw deploy
```

This command:
1. Installs Railway CLI, Turso CLI, and Wrangler if not present
2. Authenticates with each service (interactive browser flows)
3. Creates a Turso database named `temperpaw-<username>`
4. Creates an R2 bucket named `temperpaw-fs-<username>`
5. Creates a Railway project named `temperpaw-<username>` with the app,
   collector fallback, and Datadog services when Datadog credentials are present
6. Seeds all environment variables (DB credentials, blob keys, LLM keys, OTEL config)
7. Deploys the OTEL collector fallback and Datadog Runtime Agent/DBM services
   when configured
8. Deploys the pre-built Docker image from GHCR
9. Assigns a public domain and polls `/healthz` until the new deployment is live,
   then verifies `/readyz` for core readiness (Discord state is in its body)
10. Prints the dashboard URL

Existing Railway deployments can enable the Datadog Runtime Agent path without
copying infrastructure secrets out of the app by calling:

```bash
curl -fsS -X POST \
  -H "Authorization: Bearer $TEMPER_API_KEY" \
  -H "Content-Type: application/json" \
  https://<your-temperpaw-domain>/paw/infra/railway/datadog-runtime-agent/ensure
```

That governed endpoint creates or reuses `datadog-runtime-agent` from
`datadog/agent:7.84.5` (an exact version: a redeploy never upgrades it), sets Agent and app Datadog variables, stores the Runtime
Agent service id, and redeploys the Agent plus the app. It does not return the
Railway token or Datadog API key.

### LLM credential auto-detection

The deploy wizard checks environment variables in this order:
- `ANTHROPIC_API_KEY` -> sets `LLM_PROVIDER=anthropic`
- `OPENROUTER_API_KEY` -> sets `LLM_PROVIDER=openrouter`
- `OPENAI_API_KEY` -> sets `LLM_PROVIDER=openai`
- OpenAI Codex subscription auth is configured in the dashboard via device-code login.

If none are found, you can configure the LLM provider later via the dashboard Settings page.

---

## Docker Image

### Build pipeline

GitHub Actions (`.github/workflows/docker.yml`) builds the Docker image on:
- **Push to `main`** -> tagged `edge` and `sha-<short-hash>`
- **Published release** -> tagged `latest`, semver (`v1.2.3`, `v1.2`)

The image is pushed to `ghcr.io/nerdsane/temperpaw`.

### Multi-stage Dockerfile

The image is built in three stages:

1. **`dashboard-build`** (Node 22): Installs npm dependencies and builds the SvelteKit dashboard (`npm run build`)
2. **`rust-build`** (Rust 1.94): Compiles the `temperpaw-server` binary, embedding the pre-built dashboard assets
3. **Runtime** (Debian Bookworm slim): Copies the binary, dashboard build, and `os-apps/` (entity specs, WASM modules, Cedar policies)

Build args:
- `BUILD_VERSION` — version string (git tag or `sha-<hash>`)
- `BUILD_SHA` — full git commit SHA

The final image exposes **port 3467** and runs `./temperpaw`.

### Building locally

```bash
docker build -t temperpaw:local \
  --build-arg BUILD_VERSION=dev \
  --build-arg BUILD_SHA=$(git rev-parse HEAD) \
  .
```

---

## Railway Configuration

### `railway.toml`

```toml
[build]
builder = "dockerfile"
dockerfilePath = "Dockerfile"

[deploy]
healthcheckPath = "/healthz"
healthcheckTimeout = 3600
restartPolicyType = "ON_FAILURE"
restartPolicyMaxRetries = 3
```

Railway builds the Docker image from the repo's Dockerfile. The production `railway.toml` health check hits `/readyz`, so Railway moves traffic only after the new process has finished booting; `/healthz` answers as soon as the listener binds, while every other route still returns 503. `/readyz` is core readiness only: it reports Discord reconnect state in its body but never fails on it (ADR-0066). `overlapSeconds = 0` stops the old copy at cutover: a request in flight on it is cut off, which is preferred to two copies both answering Discord. TemperPaw cold boots can take a long time while the server restores state and reconciles apps, so the health window is intentionally set to one hour.

### Railway service architecture

1. **`temperpaw`** — The main application. Serves the dashboard, OData API, and agent runtime.
2. **`otel-collector`** — Portable OpenTelemetry fallback. Receives traces/metrics from temperpaw via Railway's private network (`otel-collector.railway.internal:4318`) and exports to Datadog (or debug logs if no DD_API_KEY).
3. **`datadog-runtime-agent`** — Datadog-enhanced Railway runtime Agent. Receives OTLP HTTP/gRPC from temperpaw via `datadog-runtime-agent.railway.internal`, exposes APM intake on `8126`, and handles Datadog logs/process/APM setup status.
4. **`datadog-postgres-agent`** — Datadog Postgres DBM Agent. Kept separate from runtime intake so database monitoring does not hide APM/OTLP setup state.

### Pre-built image deploy

The `temperpaw deploy` CLI does **not** build from source on Railway. Instead, it creates a thin Dockerfile that pulls the pre-built image from GHCR:

```dockerfile
ARG IMAGE_TAG=latest
FROM ghcr.io/nerdsane/temperpaw:${IMAGE_TAG}
```

This means deploys are fast — Railway just pulls the image instead of compiling Rust from scratch.

---

## Environment Variables

These are set on the `temperpaw` Railway service:

### Required

| Variable | Description |
|----------|-------------|
| `TEMPER_EVENT_STORE` | `postgres` for Railway Postgres |
| `TEMPER_PLATFORM_STORE` | `postgres` for platform specs, policies, WASM metadata, and secrets |
| `TEMPER_QUERY_PROJECTION_STORE` | `postgres` for OData/query projections |
| `DATABASE_URL` | Railway Postgres connection URL, usually `${{Postgres.DATABASE_URL}}` |
| `BLOB_ENDPOINT` | R2/S3-compatible endpoint URL |
| `BLOB_BUCKET` | Bucket name |
| `BLOB_ACCESS_KEY` | S3 access key ID |
| `BLOB_SECRET_KEY` | S3 secret access key |
| `PUBLISHED_BLOB_ENDPOINT` | R2/S3-compatible endpoint for immutable public artifacts |
| `PUBLISHED_BLOB_BUCKET` | Public artifact bucket name, separate from private TemperFS blobs |
| `PUBLISHED_BLOB_PUBLIC_BASE_URL` | Public/CDN base URL mapped to the public artifact bucket |
| `PUBLISHED_BLOB_ACCESS_KEY` | Optional public artifact access key; falls back to `BLOB_ACCESS_KEY` |
| `PUBLISHED_BLOB_SECRET_KEY` | Optional public artifact secret key; falls back to `BLOB_SECRET_KEY` |

The `PUBLISHED_BLOB_*` variables are generic Temper runtime infrastructure.
They are intentionally not tied to any one app: the runtime publishes immutable
file-derived artifacts into a separate public bucket so read-only surfaces can
serve stable bytes without sending browser traffic through governed file reads.
Do not point `PUBLISHED_BLOB_PUBLIC_BASE_URL` at the private TemperFS bucket.

Turso remains available only as an explicit legacy mode by setting
`TEMPERPAW_DEPLOY_STORAGE=turso` during deployment and providing `TURSO_URL` /
`TURSO_AUTH_TOKEN`.

### LLM Configuration

| Variable | Description |
|----------|-------------|
| `LLM_PROVIDER` | Provider name: `anthropic`, `openai`, `openai_codex`, `openrouter` |
| `ANTHROPIC_API_KEY` | Anthropic API key (if using Anthropic) |
| `OPENAI_API_KEY` | OpenAI API key (if using OpenAI) |
| `OPENAI_CODEX_TOKEN` | Legacy Codex OAuth token fallback. Prefer dashboard device-code login. |
| `OPENROUTER_API_KEY` | OpenRouter API key (if using OpenRouter) |

### Observability

| Variable | Description |
|----------|-------------|
| `OTEL_ENABLED` | Set to `true` to enable OTEL export |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://datadog-runtime-agent.railway.internal:4318` in `datadog-enhanced-railway`, or `http://otel-collector.railway.internal:4318` in `portable-otel` |
| `TEMPER_DATADOG_RAILWAY_PROFILE` | `datadog-enhanced-railway` when the Runtime Agent is active; `portable-otel` otherwise |
| `DD_AGENT_HOST` | `datadog-runtime-agent.railway.internal` in Datadog-enhanced Railway mode |
| `DD_TRACE_AGENT_URL` | `http://datadog-runtime-agent.railway.internal:8126` in Datadog-enhanced Railway mode |
| `DD_LLMOBS_API_ENABLED` | Enables direct Datadog LLMObs export when runtime OTLP bypasses the collector |
| `OTEL_RESOURCE_ATTRIBUTES` | Includes service identity plus `dd_llmobs_enabled=false` in Datadog-enhanced Railway mode so APM OTLP spans remain in APM while direct LLMObs API spans are the only LLMObs source |
| `DD_API_KEY` | Datadog API key (set on `temperpaw`, `datadog-runtime-agent`, and Datadog collector/DBM services when enabled) |
| `DD_SITE` | Datadog site (e.g., `datadoghq.com`) |

### Railway Integration (for dashboard redeploy)

| Variable | Description |
|----------|-------------|
| `RAILWAY_TOKEN` | Project-scoped Railway API token |
| `RAILWAY_PROJECT_ID` | Railway project UUID |
| `RAILWAY_ENVIRONMENT_ID` | Railway environment UUID |
| `RAILWAY_SERVICE_ID` | Railway service UUID for the temperpaw service |
| `RAILWAY_OTEL_SERVICE_ID` | Railway service UUID for the otel-collector |
| `RAILWAY_DATADOG_RUNTIME_AGENT_SERVICE_ID` | Railway service UUID for `datadog-runtime-agent` when Datadog is enabled |

These enable the dashboard's "Deploy latest build" and "Update" buttons to trigger redeployments without leaving the browser.

### Build metadata

| Variable | Description |
|----------|-------------|
| `BUILD_VERSION` | Set at build time (git tag or sha) |
| `BUILD_SHA` | Set at build time (full commit SHA) |

---

## Dashboard Redeploy Integration

The running TemperPaw server exposes Railway management endpoints:

- **`POST /paw/infra/railway/redeploy`** — Triggers a Railway service redeployment via the Railway GraphQL API (`serviceInstanceRedeploy` mutation). Accepts an optional `image_tag` parameter (`"latest"` for stable releases, `"edge"` for latest main build).
- **`GET /paw/infra/railway/status`** — Returns current deployment status.
- **`POST /paw/infra/railway/set-var`** — Sets an environment variable on the Railway service.

These endpoints require the four `RAILWAY_*` secrets to be configured in the vault.

The dashboard sidebar shows:
- **"Update"** button — when a new release is available, redeploys with `image_tag=latest`
- **"Deploy latest build"** button — redeploys with `image_tag=edge` (latest `main` commit)

---

## Manual Deployment

### A CI-built image, checked, with the way back

The **Railway Redeploy** workflow (`.github/workflows/railway-redeploy.yml`, run by hand) deploys one `sha-*` image from GHCR to the staging or production main Temper and runs `scripts/railway_release.sh`:

1. **snapshot**: what runs now (its deployment and image), its route count from its log, and one answer from each probe in `PROBE_ROUTES`.
2. **forward**, only for the Factory's `stage` and `go_live` deploys: when the commit `/paw/version` reported in the snapshot already contains the release's commit (GitHub's compare), the release is live already: nothing deploys or restarts, and the result is `already_live`. A release deploys only when its commit contains the commit that serves, so a branch that forked before it deploys nothing; production also requires `main` to contain the release. Production `roll_back` and manual runs with an empty `step` on either environment skip both the backward-deploy protection and the production main-membership check, allowing deliberate deploys of older images or commits outside `main`.
3. **deploy**: the service's source set to the image, with the `railway.toml` deploy settings (`/readyz`, one hour, overlap and draining 0, three restarts), and a new deployment. Railway keeps the old one serving until `/readyz` answers.
4. **check**: `/healthz` and `/readyz`, `/paw/version` naming the tag's commit, the new boot's route count, every probe and the guest read against the snapshot. Memory (Railway's figure and the build's Datadog gauges) is reported after a soak.
5. **rollback**, on any miss: a deployment that never took over is stopped and nothing restarts; one that took over is replaced by the image that ran before, checked against the snapshot.

`mode: check` runs the checks against what runs now, deploying nothing. `drill` (staging only) fails the check on purpose to prove the rollback. Each run ends with a release record (job outputs and run summary). An image whose kernel adds a storage migration has a way back only when the migration is listed in `scripts/additive_migrations.txt`. A kernel whose migrator ignores unknown migrations then boots on the migrated database as it is. An older one refuses it (on staging, `sha-3d79f6e` after `sha-6f410a8`: "migration 19 was previously applied but is missing"), so the way back first deletes the newer rows from `_sqlx_migrations` where `MIGRATION_ROLLBACK` is `auto`, and otherwise stops and names them.

### Using Railway CLI directly

If you have the Railway CLI linked to your project:

```bash
# Trigger a redeploy of the current image
railway redeploy -y

# Or deploy from the repo (builds from Dockerfile on Railway)
railway up -s temperpaw
```

### Using Docker directly

```bash
docker run -d \
  -p 3467:3467 \
  -e TEMPER_EVENT_STORE=postgres \
  -e TEMPER_PLATFORM_STORE=postgres \
  -e TEMPER_QUERY_PROJECTION_STORE=postgres \
  -e DATABASE_URL=postgres://user:pass@host:5432/db \
  -e BLOB_ENDPOINT=https://your-r2-endpoint \
  -e BLOB_BUCKET=your-bucket \
  -e BLOB_ACCESS_KEY=your-key \
  -e BLOB_SECRET_KEY=your-secret \
  -e LLM_PROVIDER=anthropic \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  ghcr.io/nerdsane/temperpaw:edge
```

### First-time setup

After the server starts:

1. Navigate to `http://<host>:3467/dashboard`
2. The login page detects no accounts exist and shows "Create your account"
3. Create the admin account (email + password)
4. Configure LLM provider and API key in Settings (if not set via env vars)
5. The Paw agent is automatically bootstrapped on first boot

---

## OTEL Collector And Runtime Agent

The collector runs as a separate Railway service with two modes:

- **Datadog mode** (when `DD_API_KEY` is set): Exports traces, metrics, and logs to Datadog
- **Debug mode** (no `DD_API_KEY`): Logs traces to stdout for troubleshooting

The collector is reachable from the temperpaw service via Railway's private network at `otel-collector.railway.internal:4318`. It remains the `portable-otel` fallback.

When Datadog credentials are present, the deploy flow also creates
`datadog-runtime-agent` and points TemperPaw at
`http://datadog-runtime-agent.railway.internal:4318` for OTLP and
`http://datadog-runtime-agent.railway.internal:8126` for Datadog trace intake.
USM is only supported if Railway can provide system-probe host mounts and Linux
capabilities. Continuous `ddprof` profiling is opt-in canary work; on-demand
profiling through `/_admin/profile/cpu` remains supported.

---

## Development

### Running locally

```bash
# Start the server (uses .env for credentials)
cargo run -p temperpaw --bin temperpaw-server

# Dashboard dev server (hot reload)
cd dashboard && npm run dev
```

The local server runs on `http://localhost:3467`. The dashboard dev server proxies API calls to the Rust server.

### Required local env vars

Create a `.env` file in the repo root:

```
TEMPER_EVENT_STORE=postgres
TEMPER_PLATFORM_STORE=postgres
TEMPER_QUERY_PROJECTION_STORE=postgres
DATABASE_URL=postgres://user:pass@host:5432/db
BLOB_ENDPOINT=https://...
BLOB_BUCKET=...
BLOB_ACCESS_KEY=...
BLOB_SECRET_KEY=...
PUBLISHED_BLOB_ENDPOINT=https://...
PUBLISHED_BLOB_BUCKET=...
PUBLISHED_BLOB_PUBLIC_BASE_URL=https://assets.example.com
LLM_PROVIDER=anthropic
ANTHROPIC_API_KEY=sk-ant-...
```

---

## Troubleshooting

**Server won't start / panics on boot:**
- Check that all required env vars are set, especially `DATABASE_URL` when `TEMPER_EVENT_STORE=postgres`
- Check Railway logs: `railway logs` or view in the Railway dashboard

**"Unresolved secret template" error in Paw chat:**
- The configured LLM provider's API key is missing or empty
- Go to Settings in the dashboard and verify the LLM provider and API key are set
- The WASM auto-fallback will try other providers if available, but at least one must have a valid key

**Health check failing:**
- The server takes time to boot (compiling WASM, running migrations)
- Railway's health check timeout is 60 seconds — increase if needed
- Check logs for startup errors

**Dashboard shows "Loading..." forever:**
- The API may be unreachable — check that the server is running
- Check browser console for CORS or network errors

**Deploy buttons not working:**
- Ensure `RAILWAY_TOKEN`, `RAILWAY_PROJECT_ID`, `RAILWAY_ENVIRONMENT_ID`, and `RAILWAY_SERVICE_ID` are set
- These are seeded automatically by `temperpaw deploy` but need manual setup if you deployed differently
