# ADR-0027: Deployment, Dashboard Auth, Observability & Developer Experience

## Status

Proposed

## Context

OpenPaw currently runs only on Seshendra's machine. Going to production and opening the project to contributors requires solving five interconnected problems:

1. **No deployment path.** There's no way to deploy OpenPaw to production. The repo has a `railway.toml` but no tooling to provision the required infrastructure (database, blob storage) or seed configuration.

2. **No dashboard authentication.** The SvelteKit dashboard hardcodes `x-temper-principal-kind: admin` and has zero auth. In production with `TEMPER_API_KEY` set, bearer auth blocks all dashboard requests. Anyone who finds the URL has full admin access if the API key is unset.

3. **No post-deploy configuration UI.** The `openpaw setup` CLI wizard works in a terminal but not on a deployed instance (no TTY on Railway). Users need a web UI to configure API keys, messaging integrations, and soul personalization after deployment.

4. **No observability in production.** OTEL support exists (`otel_enabled` flag, collector configs, Datadog dashboard/monitor definitions) but there's no documented deployment path for the collector or the monitoring stack.

5. **Poor contributor DX.** The `[patch]` section in `Cargo.toml` (14 local path overrides) and `.cargo/config.toml` break builds for anyone who doesn't have `../temper/` cloned. `Cargo.lock` is gitignored (wrong for a binary crate). There's no `.env.example`, no `Makefile`, no `Dockerfile`, no CI, no contributing guide.

### Current state of the code

- **Storage defaults work locally:** If `TURSO_URL` is unset, the server defaults to local SQLite at `~/.local/share/openpaw/paw.db`. If `BLOB_ENDPOINT` is unset locally, Temper uses its internal blob route backed by the filesystem object store under the data directory. Railway deployments require `BLOB_ENDPOINT` so blob bytes go to R2/S3, not SQL.
- **Vault key auto-generates:** Three-tier fallback — env var, persisted file, generate-and-save (`startup.rs:174-247`). Changed 19 Nov 2026: a start makes no vault key, since one made over stored secrets opens none of them. `temperpaw deploy` makes one for a new service and local setup asks once for a new instance; a start with none stops and says how to give it one.
- **Dashboard is embedded:** The Rust binary serves the SvelteKit static build at `/dashboard` from `dashboard/build/` (`startup.rs:744-750`). One binary, one port, one service.
- **Bearer auth is optional:** If `TEMPER_API_KEY` is unset, all requests pass through (`bearer_auth.rs:43-46`). This is fine for local dev but not production.
- **Setup APIs exist:** `setup_api.rs` already has REST routes for reading/writing secrets (`/paw/setup/secrets`), connecting transports (`/paw/transports/discord/connect`), and checking status (`/paw/setup/status`). The dashboard settings page only needs a frontend.

## Decision

### 1. Single entry point with local/cloud choice

Combine setup and deployment into one flow. On first run (no API key found), `openpaw` presents:

```
◇ What would you like to do?
  ● Run locally (development)
  ○ Deploy to the cloud
```

Both paths share the same setup flow: API key selection, messaging config, soul interview with LLM-generated personality. The difference is what happens after:

- **Local:** Boot the server, seed secrets to local vault.
- **Cloud:** Provision infrastructure (Turso Cloud, Cloudflare R2, Railway), deploy the binary, wait for health check, seed secrets to the remote instance's vault via `POST /paw/setup/secrets`.

The soul interview runs before deployment using the API key the user just provided — no need to wait for the server. Generated content is held in memory and seeded after deploy.

Returning users (credentials already exist) skip the choice and boot directly.

### 2. Email + password authentication (embedded, no OAuth initially)

Auth is a platform primitive implemented in `crates/openpaw/src/auth.rs`, not a Temper app. Rationale:

- Auth runs before Temper middleware in the HTTP request path. JWT verification must be synchronous and fast.
- The Temper-native rule (ADR-0005) says Rust code is for "triggers, WASM host functions, platform primitives." Auth middleware is a platform primitive.
- OAuth (Google, GitHub) adds complexity with no benefit for single-user deployments. Each user deploys their own instance — they're the only one logging in. Email + password with first-user-wins admin is sufficient.

Resolution order in the auth middleware:
1. Health checks (`/healthz`, `/tdata` bare GET) → passthrough
2. Auth routes (`/auth/*`) → passthrough
3. `paw_session` cookie → validate JWT (signed with vault key) → inject admin headers → passthrough
4. Bearer token → existing flow (agent credentials, API key)
5. No auth → 401 (or redirect to `/dashboard/login` for dashboard paths)

First user to register becomes admin. No `ADMIN_EMAILS` env var. No OAuth app registration.

### 3. Dashboard settings page as web equivalent of `openpaw setup`

The dashboard gets a `/settings` page that reads/writes through the existing `setup_api.rs` routes. Sections: LLM provider, Discord, Slack, other integrations (GitHub token, Exa), soul personalization, account management.

The TUI (`openpaw setup`) and Dashboard Settings share the same secrets vault — whatever you configure in one is visible in the other. One source of truth.

The TUI collects secrets into memory. They reach the vault only after the server is running:
- **Local path:** Server boots → secrets seeded during startup Phase 5b.
- **Cloud path:** Server deployed → secrets seeded via `POST /paw/setup/secrets` after health check passes.

### 4. Infrastructure is infrastructure, not dashboard config

Turso Cloud (database) and Cloudflare R2 (blob storage) are infrastructure. They're set at deploy time by the `openpaw` deploy path, stored as Railway environment variables. They are NOT configurable through the dashboard.

Dashboard Settings is exclusively for personal configuration: API keys, messaging tokens, soul personalization. Things that might change after deployment without redeploying.

Two tiers:

| Tier | Where | When | What |
|------|-------|------|------|
| Infrastructure | Railway env vars (set by deploy flow) | Deploy time | `TURSO_URL`, `TURSO_AUTH_TOKEN`, `BLOB_ENDPOINT`, `BLOB_BUCKET`, `BLOB_ACCESS_KEY`, `BLOB_SECRET_KEY` |
| Personal config | Temper secrets vault (encrypted in DB) | Post-deploy | `ANTHROPIC_API_KEY`, `DISCORD_BOT_TOKEN`, `GITHUB_TOKEN`, soul content |

Auto-generated (no user action): `TEMPER_VAULT_KEY` (persisted to file), `TEMPER_API_KEY` (persisted to file, new — same pattern as vault key).

### 5. Embedded dashboard (not separate service)

The dashboard stays embedded in the OpenPaw binary, served as static files at `/dashboard`. One binary, one service, one URL. This is the standard pattern used by Grafana, Gitea, Minio, and Supabase.

Rationale against separate deployment:
- Adds CORS complexity, a second Railway service, split auth responsibility
- No benefit for single-user/single-tenant deployments
- SvelteKit code works with either `adapter-static` (embedding) or `adapter-node` (separate) — it's a config change if ever needed

Multi-user support (when needed) comes from adding user accounts and tenant switching to the same embedded dashboard, not from separating the service. Temper already has multi-tenant support (tenant IDs, Cedar authorization per tenant).

### 6. OTEL Collector as separate Railway service

The OTEL Collector is a standalone binary that can't be embedded. On Railway it's a second service in the same project, connected via private network. The deploy flow supports an optional `--with-datadog` flag to provision the collector alongside the main service.

### 7. Clean repo for contributors

- Remove the `[patch]` section from `Cargo.toml` and delete `.cargo/config.toml` from the repo. Add `.cargo/config.toml` to `.gitignore` with a `.cargo/config.toml.example` for local Temper development.
- Track `Cargo.lock` (binary crate — lockfile should be committed).
- Add `.env.example`, `Makefile`, `CONTRIBUTING.md`, `Dockerfile`, CI workflow.

## Consequences

### Positive

- **Stellar first-run UX:** `openpaw` → choose local/cloud → setup flow → done. One command from zero to a live, personalized Paw.
- **No pasting:** The deploy path provisions Turso and R2 programmatically, sets Railway env vars via CLI. The only semi-manual step is the R2 API token (Cloudflare doesn't support programmatic R2 token creation via wrangler).
- **No config duplication:** TUI and dashboard share the same secrets vault. Configure in one, see it in the other.
- **Clean contributor onboarding:** Fresh clone builds without `../temper/`. `make setup && make dev` works.
- **Auth without OAuth complexity:** Email + password with first-user-wins. No Google Cloud Console, no GitHub Developer Settings.

### Negative

- **Deploy flow requires three external CLIs:** `railway`, `turso`, `wrangler`. The deploy path checks for them and guides installation, but it's still three tools to install.
- **R2 API token is semi-manual:** Cloudflare's `wrangler` CLI can create buckets but not API tokens. The deploy flow guides the user to the Cloudflare dashboard for this one step.
- **No OAuth means no "Sign in with Google":** For single-user this is fine, but if the project grows to multi-user, OAuth needs to be added later.
- **Removing `[patch]` section** means local Temper development requires manually copying `.cargo/config.toml.example`. This is intentional — the default must work for contributors who don't have Temper cloned locally.

### Risks

- Railway volumes (for persisting vault key, API key, fallback SQLite) are GA but less battle-tested than traditional block storage. Mitigation: primary storage is Turso Cloud, not local SQLite.
- The deploy flow shells out to external CLIs whose interfaces may change. Mitigation: version-pin expected CLI behavior, handle parse failures gracefully.
- Soul interview requires a working LLM API key before deploy. If the key is invalid, the interview fails. Mitigation: validate the key with a lightweight API call before starting the interview.

## Alternatives Considered

### Separate dashboard service
Deploy the SvelteKit dashboard independently (Vercel, Cloudflare Pages). Rejected: adds CORS, split auth, two services to manage. No benefit for single-user deployments.

### OAuth from day one
Support Google and GitHub login immediately. Rejected: requires users to register OAuth apps with Google Cloud Console and GitHub Developer Settings — poor UX for personal deployments. Can be added later when multi-user is needed.

### Zero-config deploy (local SQLite, blobs in DB)
Default to embedded SQLite and database-stored blobs with no external dependencies. Rejected: user explicitly wants proper infrastructure (separate database, separate blob store). Railway volumes have durability concerns for primary storage.

### Infrastructure configurable via dashboard
Allow changing Turso URL, R2 credentials through Dashboard Settings. Rejected: infrastructure is set once at deploy time. Putting it in the dashboard mixes infrastructure concerns with personal config. Requires server restart to take effect, which is confusing UX.

### Standalone deploy script (`scripts/deploy.sh`)
A bash script instead of a Rust subcommand. Rejected: a proper CLI subcommand using `cliclack` provides the same polished TUI as the existing `openpaw setup`, integrates naturally with the binary, and can share code with the setup flow.

### Separate `openpaw deploy` and `openpaw setup` commands
Two distinct commands for deployment and setup. Rejected: confusing overlap. Both collect API keys and soul configuration. A single entry point with a local/cloud choice is cleaner.
