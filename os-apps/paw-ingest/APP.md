# paw-ingest

Authenticated webhook pipeline. The Rust protocol trigger verifies the exact
request bytes and consumes replay/resource budgets before persisting an
immutable accepted envelope. Entity/WASM transitions then route and process it.

## Entity Types

### WebhookEvent
One incoming webhook flowing through validation, routing, and processing.

- **States**: Created -> Routing -> Processing -> Processed / Rejected
- **Key actions**: `Received` (authenticated payload + immutable route snapshot), `Routed` (created target entity), `Processed`
- **Failure actions**: `RouteFailed`, `ProcessFailed` — transition to Rejected
- **WASM**: `route_webhook` (uses the accepted snapshot; never re-reads the route), `process_webhook` (dispatches the target action)

### WebhookRoute
Configuration entity mapping route keys to target entities and actions.

- **States**: Active <-> Disabled
- **Key actions**: `Register` (unique route key, target capability, HMAC scheme, vault reference, signature/delivery headers, budgets), `Update`, `Disable`, `Enable`
- **Security**: Admin-only governance; secret values are never stored on the entity or fetched through HTTP admission
- **Options**: `monitor_resolution_enabled`, semantic `dedup_enabled`, `dedup_window_minutes`

## Setup

Register WebhookRoute entities for each external source and configure every
referenced vault secret. Providers must send a JSON-object body, the configured
signature over its exact bytes, and the configured delivery-ID header. Unsigned,
malformed, replay-mismatched, unconfigured, or over-budget requests do not
create a new WebhookEvent.
