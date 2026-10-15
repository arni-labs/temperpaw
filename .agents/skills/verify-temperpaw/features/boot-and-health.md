# Server boot and health

## Sub-features
Config parsing (.env), storage init (turso local / postgres prod), HTTP bind, health endpoint.

## How to get to it (user POV)
An operator starts the server and checks it is alive before pointing anything at it.

## Driving it
`cargo run -p temperpaw` with a seeded `.env`; then `curl -sf http://localhost:$PORT/healthz`.

## What proves it
`/healthz` returns 200 and the boot log shows the configured tenant and storage backend with no error lines. Evidence: the curl output and the first 30 boot-log lines.

## Gotchas
Default storage in `.env.example` is postgres (production shape); local runs set `TEMPER_*_STORE=turso`. Port collisions with other local servers - pick a fresh PORT. `make wasm` must run before first boot: AppRequired modules missing from the bundle trigger shutdown (and, as of 2026-08-25, that shutdown path stack-overflows - reported as a product bug, not papered over here).
