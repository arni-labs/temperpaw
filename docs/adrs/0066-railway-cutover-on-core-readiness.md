# ADR-0066: Railway Cutover Waits for Core Readiness

## Status

Accepted. Supersedes ADR-0053; amends ADR-0054 (Discord state moves into the `/readyz` body).

## Context

ADR-0053 moved the Railway health check from `/readyz` to `/healthz` because `/readyz` failed while Discord was reconnecting, and the new copy could never become healthy while the old one held the Discord connection. ARN-432 then moved deploy verification to `/healthz` for the same reason, as an interim until `/readyz` reflected core readiness only.

`/healthz` answers 200 as soon as the listener binds, which is early in boot. Until startup finishes, the startup gate answers 503 on every other route. With `overlapSeconds = 0`, Railway stopped the old copy the moment `/healthz` passed, so every deploy had a window with no working copy. The 15 Oct 2026 01:50 UTC deploy: the new copy logged first at 01:51:51, the old copy's last response was at 01:52:09, and the new copy began serving at 01:52:29, about 20 seconds of 503s.

## Decision

- `/readyz` is core readiness: 503 until the startup gate opens, then 200. Discord state is still reported in its body (`discord.connected`, `discord.connection_state`, `discord.last_error`) but never fails readiness.
- The production `railway.toml` health check uses `/readyz`, so traffic moves only to a copy that has finished booting.
- `overlapSeconds = 15` keeps the old copy up for in-flight requests after cutover. TemperPaw has no SIGTERM handler, so `drainingSeconds` would not drain anything.
- The release deploy job verifies on `/readyz` plus the `/paw/version` identity check.

## Consequences

- No 503 window during a normal deploy; a copy that never finishes booting never takes traffic, and the old copy keeps serving until the health window ends.
- Both copies run against the same Postgres for the whole boot plus 15 seconds instead of about 18 seconds. Event appends stay safe (expected-sequence check plus the unique `(tenant, entity_type, entity_id, sequence_nr)` index); per-process actor caches and background loops are not instance-aware, as before, for a somewhat longer window.
- A Discord outage no longer shows as a non-200 `/readyz`; read `discord.connected` in the body to see it.
- The `RAILWAY_DEPLOYMENT_OVERLAP_SECONDS` service variable, where set, must agree with `railway.toml`.
- The in-app setup deploy (`crates/temperpaw/src/deploy.rs`) and paw-patrol's TemperDeploy still use `/healthz`; they are unchanged here.
