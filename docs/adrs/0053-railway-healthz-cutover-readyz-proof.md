# ADR-0053: Railway Cutover Uses Healthz, Readyz Remains Proof

## Status

Accepted. Superseded by ADR-0066 (cutover waits for core readiness).

## Context

TemperPaw exposes two useful probes:

- `/healthz` proves the HTTP process is alive.
- `/readyz` proves the app is ready and includes external transport state, including Discord.

Railway deployment health checks were using `/readyz`. During zero-overlap cutover this can create a deployment cycle: the old replica keeps serving traffic and holding the live Discord connection while the new replica waits for Discord readiness before Railway will move traffic.

## Decision

Railway deployment health checks use `/healthz`.

`/readyz` remains the stronger post-cutover readiness proof. Release verification must still check `/readyz`, `/paw/version`, metadata, and app usability after the new image is live.

## Consequences

- Railway can cut over once the new process is listening.
- Discord reconnect state no longer blocks the infrastructure health check.
- Operators still use `/readyz` to verify user-facing readiness after cutover.
- This does not change storage, app install semantics, or any database state.
