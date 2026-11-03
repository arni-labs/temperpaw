# ADR-0001: Computer entity ops open to authenticated tenant principals

Date: 2026-08-23
Status: Accepted

## Context

The Computer row is the live mapping between agents and sandbox metal
(name, provider, sandbox_url, machine_id, project_harness_id). Third-party
harnesses (Claude Code, Codex) attach an existing computer by reading that
row by name — there is deliberately no sidecar API or repo that stores the
agent-to-sandbox mapping.

The shipped Cedar policy permitted lifecycle actions (Configure, Provision,
Sleep, Wake) for authenticated principals but granted no entity operations
at all. Every create, read, and list on /tdata/Computers returned 403, and
read denials do not raise pending decisions, so the registry was
unreachable even with a human approver in the loop. Attach was impossible
for any principal.

## Decision

Permit read and list on Computer unconditionally within the tenant (API
access is already credential-gated at the network edge), and create for
Agent principals — the same shapes the dsf tool policies use. Approval-worthy
transitions stay gated: ProvisionComplete, ProvisionFailed, and
CheckpointComplete remain admin-only.

A `principal.authenticated` condition was tried first and does not match on
the deployed principal model: the attribute is absent, and Cedar treats a
missing attribute as deny — which is also why the original
Configure/Provision permits never matched for any live principal.

An explicit Bind action recording which harness attached (an attach audit
trail) was considered and deferred: project_harness_id already carries the
project hook, and attach-by-read requires no state transition. If an attach
audit trail becomes a requirement, add Bind in a follow-up ADR rather than
widening this one.

## Consequences

- Any authenticated Agent (TemperPaw sessions, third-party harness sessions
  holding an issued AgentCredential) can resolve a computer by name and use
  the metal it points to.
- Tenant isolation is unchanged: Cedar evaluates within the tenant; other
  tenants and unauthenticated callers still get 403.
- Computer rows are not public; Genesis bundles remain the only public
  surface of paw-compute.
