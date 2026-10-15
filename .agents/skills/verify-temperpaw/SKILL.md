---
name: verify-temperpaw
description: Drive and prove the TemperPaw server the way a user or operator does - launch it locally, health-check it, exercise the HTTP/OData surface, capture evidence. Use for "verify temperpaw", the verification step of any temperpaw change, or before calling temperpaw work done.
---

# Verify TemperPaw

TemperPaw's primary surface is an HTTP server (`crates/temperpaw`): a governed OData API (`/tdata`), platform endpoints (`/paw`, `/setup`, `/dashboard`), and transport triggers (Discord, Slack, webhooks). This skill proves behavior on a locally running server. Production verification (Railway + Datadog + Genesis pinned refs) is the Definition of Done's separate step.

## Launch

```bash
cp -n .env.example .env   # first time only; then edit:
#   PORT=3477 (any free port), TEMPER_*_STORE=turso  (local libsql - no external DB)
#   TURSO_URL=file:.scratch/paw.db  - ISOLATE: without this the server uses the
#   shared ~/.local/share/temperpaw/paw.db (someone's real local state; never
#   drive or mutate it from verification)
rustup target add wasm32-unknown-unknown wasm32-wasip1   # first time only
make wasm                 # REQUIRED before first run: os-apps declare AppRequired
                          # WASM modules and a boot without the artifacts shuts
                          # down. Builds every os-apps/*/wasm/build.sh; slow once.
cargo run -p temperpaw    # make dev uses the same entry point
```

Ready when the log prints the bound port and `GET /healthz` returns 200. First `make wasm` + build are slow (full workspace, two targets); later runs are seconds. The dashboard (`make dashboard`) is NOT needed for API verification.

Teardown: kill the PID you started (never by name - other agents run servers on this machine).

## Doctor

One read-only check before driving anything:

```bash
curl -sf http://localhost:$PORT/healthz
```

200 means: process up, storage reachable. Non-200 or connection refused: read the boot log before driving anything. Run Doctor again after any failed drive.

## Drive

- Unauthenticated: `/healthz` only. Everything else goes through auth (`crates/temperpaw/src/auth.rs`).
- Operator surface: OData reads and action dispatches with `Authorization: Bearer $TEMPER_API_KEY` (from `.env`) + `X-Tenant-Id: $PAW_TENANT`:
  ```bash
  curl -s -H "Authorization: Bearer $KEY" -H "X-Tenant-Id: default" http://localhost:$PORT/tdata/<EntitySet>
  ```
- Actions: `POST /tdata/<Set>('<id>')/Temper.<Action>` - then READ THE ENTITY BACK and check the state transition; a 200 on dispatch is not proof the machine moved.
- Transports (Discord/Slack) need real tokens in `.env`; without them, verify at the trigger boundary with a signed webhook POST where a feature file documents one.

## Evidence

- Save every response and relevant log excerpt under `/tmp/verify-temperpaw/<date>/` and reference the files in the PR or effort summary.
- Prove the real path: entity state transitions read back via OData, not just HTTP 200s. Side effects (rows created, files written, messages sent) are checked alongside what is visible.
- Evidence survives cleanup - teardown kills the server, never deletes `/tmp/verify-temperpaw/`.

## Cleanup

Kill the server PID you captured at launch. Remove nothing else. `.env` stays (shared local state).

## Feature map

`features/` - one file per user-facing feature; each says how to reach it, how to drive it, and what observable end state proves it. A proof that drives one convenient entry point is incomplete when the map lists others.
