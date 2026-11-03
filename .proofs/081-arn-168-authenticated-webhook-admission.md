# Proof Report: 081 — ARN-168 Authenticated Webhook Admission

## Date

2026-07-11

## Branch / Commit

- Repository: `nerdsane/temperpaw`
- Existing PR: #451
- Remote branch: `claude/arn-168-webhook-hmac`
- Local review branch: `codex/pr451-security-review`
- Commit: pending final review gate at proof capture time

## What Was Done

- Moved HMAC verification to the public HTTP trigger so invalid traffic creates
  no entity.
- Injected a tenant-scoped in-process vault resolver so webhook signing secrets
  never traverse the setup HTTP API.
- Required explicit HMAC scheme, vault reference, signature header, delivery-ID
  header, body budget, and rate budget on every active route.
- Replaced literal/empty shipped secrets with governed vault references and
  exposed all required references in setup/readiness.
- Derived deterministic WebhookEvent IDs from tenant, route ID, and provider
  delivery ID. The atomic get-or-create response is validated against the
  stored payload/route fingerprint before any dispatch.
- Required authenticated bodies to be JSON objects, preserved exact raw bytes,
  and passed canonical JSON downstream; malformed/scalar requests fail before
  persistence.
- Snapshotted the route target capability and its digest into `Received`, then
  removed the downstream mutable route lookup.
- Restricted WebhookRoute and WebhookEvent access to Admin plus the named WASM
  transition owners.
- Removed the duplicate `validate_webhook` WASM module and its HMAC/SHA/hex/
  subtle dependency set.
- Updated the webhook smoke harness to use the governed seeded routes, sign raw
  payloads, prove forged/malformed rejection, prove changed-content replay
  conflict, and prove exact replay suppression.

## Verification Flow

1. Start a local TemperPaw server with a fresh Turso database and built WASM.
2. Resolve the four seeded Patrol webhook routes.
3. Store their five referenced signing keys in the tenant vault.
4. POST forged, signed-malformed, and signed-scalar requests and compare
   WebhookEvent count before and after.
5. POST a signed request, then reuse its delivery ID with changed signed content
   and require HTTP 409 with no event or dispatch.
6. Replay the exact signed request and require the original event ID with
   `status=duplicate`.
7. POST signed Datadog, GitHub, and Discord payloads with unique
   delivery IDs.
8. Wait for every WebhookEvent to reach `Processed` and every WorkRequest/Signal
   to reach `Linked`.

## Verification Results

| Step | Expected | Actual | Status |
|------|----------|--------|--------|
| Forged HTTP delivery | 401 and no durable event | 401; event count stayed 0 | PASS |
| Signed malformed/scalar body | 400 and no durable event | Both 400; event count stayed 0 | PASS |
| Signed request route | Processed -> WorkRequest Linked | Processed; WorkRequest Linked | PASS |
| Signed Datadog route | Processed -> Signal Linked | Processed; Signal Linked | PASS |
| Signed GitHub route | Processed -> Signal Linked | Processed; Signal Linked | PASS |
| Signed Discord route | Processed -> Signal Linked | Processed; Signal Linked | PASS |
| Exact replay | Original event, no redispatch | Same deterministic ID; `duplicate` | PASS |
| Changed body, consumed delivery ID | 409, no new event/dispatch | 409; original remained sole event | PASS |
| Mutation TOCTOU test | Accepted target stays immutable | Original target dispatched after route mutation | PASS |
| Webhook trigger tests | Crypto, replay, HTTP, budgets, logging | 10 passed, 0 failed across focused modules | PASS |
| Full paw-transport crate | No transport regression | 45 passed, 0 failed | PASS |
| Paw Patrol contracts | Manifest/seed/boundary + Cedar matrix | 2 passed, 0 failed focused; full suite green | PASS |
| WASM native + release | route/process compile for host and wasm32 | PASS | PASS |
| Required-secret setup schema | All five required references visible | 1 passed, 0 failed | PASS |

## What Worked

- The deterministic event ID was honored by the live OData create path. The
  replay returned `wh-a8db5fd0...506ac` without dispatching again.
- The real get-or-create response retained the original payload fingerprint;
  changed signed content under the same delivery ID returned HTTP 409.
- The immutable envelope flowed through real route/process WASM into Patrol,
  producing linked WorkRequest/Signal entities and FactoryCase/WorkCycle state.
- Raw headers were unnecessary after admission and are no longer persisted.

## What Didn't Work

- The original smoke script created duplicate routes even though Patrol already
  seeds them. The new fail-closed route lookup surfaced this as a configuration
  error. The harness now resolves and exercises the real seeded routes.
- Its original five-minute startup window expired while compiling every OS app,
  and load-only correctly rejected unrelated missing required artifacts. The
  run built missing artifacts once and then completed against persisted WASM.
- An intermediate implementation assumed duplicate entity POST returned HTTP
  409. Live Temper correctly returns 201 with the authoritative existing state.
  Admission now validates that atomic response before dispatch; the final live
  replay-mismatch test passes.

## Limitations

- The per-route rate window is process-local because TemperPaw currently runs a
  single webhook trigger service. It evicts expired entries and fails closed at
  a 4,096-route tracking budget. Durable replay is not local: the shared store's
  atomic get-or-create selects the stored fingerprint. A future horizontally
  scaled trigger should move the rate counter into a shared Temper admission
  primitive.
- Static kernel `[[webhook]]` declarations use Temper PR #340. Dynamic
  WebhookRoute entities cannot use that static lookup directly, but follow the
  same authenticate/authorize/idempotency/dispatch ordering.

## What Still Doesn't Work

- No production deploy was performed because PR #451 must remain open. Live
  Railway/Datadog verification is therefore pending merge and deployment.
- All five configured webhook secret references must be populated in the
  deployment vault; `/readyz` reports them in its body and admission fails closed
  while any are missing. `/readyz` stays 200 per ADR-0066.

## Artifacts

- Executable proof: `crates/paw-codex-worker/scripts/webhook-intake-smoke.sh`
- Machine summary: `/tmp/paw-patrol-webhook-smoke-proof-4531-56005/summary.json`
- Entity snapshots and visual proof: `/tmp/paw-patrol-webhook-smoke-proof-4531-56005/`
- Server log: `/tmp/paw-patrol-webhook-smoke-server.log`
- WASM build log: `/tmp/paw-patrol-webhook-smoke-wasm-build.log`

## Architecture Diagram

```text
Public POST
  -> bounded raw body
  -> unique governed route snapshot
  -> vault secret resolution
  -> in-process tenant vault secret
  -> HMAC-SHA256 verify_slice
  -> delivery/rate/JSON-object budgets
  -> deterministic WebhookEvent atomic get-or-create
  -> authoritative fingerprint comparison
  -> Received(immutable target + digests)
  -> route_webhook WASM
  -> process_webhook WASM
  -> WorkRequest / Signal
```
