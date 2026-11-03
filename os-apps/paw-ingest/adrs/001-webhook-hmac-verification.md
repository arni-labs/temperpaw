# ADR-001: Authenticated webhook admission

**Status:** Accepted
**Scope:** `paw-transport` webhook trigger and `paw-ingest`
**Date:** 2026-07-07 (revised 2026-07-11)
**Tracking:** ARN-168 (Class B, epic ARN-165); paired with Temper ARN-171 / PR #340

## Context

`POST /triggers/webhook/{route_key}` is public because external providers cannot
present a Temper bearer credential. The original pipeline created a durable
`WebhookEvent` first and deferred verification to `validate_webhook` WASM. That
module only checked for a signature header. PR #451 replaced that check with a
real HMAC, but still left the security boundary structurally incomplete:

- every shipped route configured an empty secret and therefore skipped HMAC;
- an invalid request was persisted before authentication;
- the validator and router independently re-read mutable `WebhookRoute` state,
  so validation and execution could observe different targets;
- exact signed deliveries could be replayed;
- literal secrets and route governance were broadly readable and mutable; and
- the public body inherited the application's 50 MiB body allowance.

The Temper kernel receiver in PR #340 establishes the correct order for static
`[[webhook]]` declarations: authenticate raw bytes, authorize, derive durable
idempotency, and only then dispatch. TemperPaw routes are dynamic entities, so
they cannot use the kernel's static route lookup directly, but they must use the
same boundary ordering and must not retain a second downstream verifier.

## Decision

The HTTP trigger is the sole webhook admission boundary. It remains a protocol
bridge: after read-only route/secret resolution it creates one entity and
dispatches one action. Business processing remains in entity transitions and
WASM integrations.

### Governed route capability

Every active route must declare all of the following:

- `auth_scheme = "hmac-sha256"`;
- a non-empty `secret_ref` containing only a vault key, never a secret value or
  `{secret:...}` template;
- an explicit `signature_header`;
- an explicit `delivery_id_header` supplied by the provider;
- bounded `max_body_bytes` and `max_deliveries_per_minute` budgets; and
- a fixed target entity type and action.

Route/source names and target identifiers must satisfy path-safe identifier
grammars; target actions must be dot-qualified identifiers. Boolean options are
exactly `true` or `false`, and the deduplication window is a positive bounded
budget. Configuration typos therefore fail admission instead of silently
disabling controls or becoming internal URL fragments. Duplicate signature or
delivery-ID headers are rejected as ambiguous.

`route_key` is a declared unique key. Route creation, reads, updates, and state
changes are Admin-only. Shipped seed routes use governed vault references.
Admission refuses a delivery with 503 when its route's referenced secret is
absent. `/readyz` and setup status report the missing references, but `/readyz`
still answers 200: it is core readiness only (ADR-0066), and a missing webhook
secret must not stop a deploy from taking over.

Startup injects a tenant-scoped in-process secret resolver into the trigger.
The resolver closes over the active tenant and vault, and accepts only the
validated route key. Webhook admission never retrieves signing secrets through
the setup HTTP API, so public trigger authentication cannot be converted into
a secret-exfiltration dependency.

### Admission order

For each request the trigger:

1. applies a hard global body ceiling before extraction;
2. resolves exactly one active route through the bounded keyed lookup;
3. validates route configuration and its per-route body budget;
4. resolves the referenced secret through its injected tenant-vault
   capability;
5. verifies `HMAC-SHA256(secret, raw_body)` using decoded bytes and
   `Mac::verify_slice`;
6. requires a bounded, non-empty provider delivery ID;
7. consumes the route's admission-rate budget, including for authenticated
   malformed traffic;
8. requires the authenticated UTF-8 body to parse as a JSON object and derives
   a canonical `normalized_payload` without changing `raw_payload`;
9. creates the deterministic `WebhookEvent` with the provider delivery ID,
   payload digest, route ID, route key, authentication scheme, and route-
   snapshot digest atomically bound into its initial fields, then dispatches
   `Received` once.

Unknown, malformed, unsigned, mis-signed, non-object, over-budget, or
unconfigured requests create no durable entity.

### Replay boundary

The event ID is derived from a domain-separated SHA-256 hash of tenant, route
entity ID, and provider delivery ID. Entity creation is the durable compare-and-
set. Temper's collection POST is an atomic get-or-create and returns the
authoritative stored winner even when the caller-selected ID already exists.
The trigger compares that response before any dispatch. The initial create
atomically stores the immutable admission fingerprint, including payload and
route-snapshot digests. The stable request identity is the event/route identity,
delivery ID, payload digest, and authentication scheme:

- any stable-identity mismatch returns HTTP 409 and never dispatches;
- a stable match after transition returns the existing event as a duplicate,
  even if an administrator has since changed the route;
- a stable match still in `Created` may retry the interrupted `Received`
  dispatch only when the current route-snapshot digest also matches the stored
  digest; otherwise it returns HTTP 409 rather than dispatching a changed
  capability.

The payload digest is deliberately not part of the event ID because a provider
delivery ID identifies one delivery. Binding the digest inside that reserved
identity lets the server distinguish an exact retry from changed content trying
to reuse the same delivery ID.

### Immutable accepted envelope

The trigger snapshots the governed route fields into `WebhookEvent.Received`,
including the route ID, target capability, source type, operational options,
payload digest, delivery ID, and a digest of the route snapshot. Downstream
WASM never re-reads `WebhookRoute`. A concurrent route mutation can affect the
next delivery but cannot change the capability already admitted for this one.

Raw request headers are not persisted. The exact accepted raw body remains
available to the entity pipeline because it is functional input. The normalized
body is a canonical JSON object, and downstream routing fails closed if a
manually created or corrupted event violates that contract. WebhookEvent reads
are Admin-only.

### Kernel relationship

This is not an alternative to Temper PR #340. The two routes serve different
configuration models (static spec declarations versus dynamic route entities)
but share the same security contract and cryptographic primitive. TemperPaw
pins the current kernel and keeps the custom trigger only for the dynamic
entity-first ingress model. The obsolete `validate_webhook` WASM verifier is
removed so authentication has one owner.

## Consequences

- Unsigned shipped routes no longer work. Operators must configure the named
  vault secrets and providers must send both signature and delivery headers.
- Seed data does not rewrite an existing record, so a route created before this
  change answers 503 until an Admin runs `Update` with `auth_scheme`,
  `secret_ref`, `signature_header` and `delivery_id_header`.
- Route changes are governance operations, and existing literal-secret routes
  must be migrated to secret references before traffic is accepted.
- Exact replay is durable across restarts because it is represented by entity
  identity; the in-process rate budget protects the single deployed trigger
  service from authenticated floods, while the platform's request admission
  controller remains the outer concurrency boundary.
- Expired route-rate windows are evicted, and the trigger fails closed once its
  bounded tracked-route budget is full, so governed route churn cannot grow the
  process map without bound.
- Tests must prove no persistence before authentication, duplicate delivery
  suppression, rejection of changed content under a consumed delivery ID,
  recovery after an interrupted create-before-dispatch, snapshot stability
  under route mutation, no HTTP secret-fetch dependency, malformed/non-object
  rejection, Cedar restrictions, bounded bodies/rates, and a signed local
  HTTP-to-entity flow.
