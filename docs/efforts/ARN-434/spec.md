# Spec: Cedar permits for the shadow-sweep writes (ARN-434)

The contract for unblocking the ARN-431 S1 sweep: paw-patrol's `patrol.cedar`
must authorize EXACTLY the record-write actions the sweep dispatches, to EXACTLY
the principal it authenticates as, and nothing more. No entity, module, or
workflow change - only Cedar.

## The principal

The sweep authenticates with the prod deployment's `TEMPER_API_KEY`, which is
byte-identical (sha256) to the openpaw service key and bootstraps as the operator
`AgentCredential` (AgentType name "operator"). It therefore resolves to:

- `principal is Agent`
- `principal.agent_type == "operator"`
- `principal.agentTypeVerified == true` (credential-resolved, not header-declared)

The permits gate on the kernel's canonical predicate verbatim
(`temper_authz::VERIFIED_OPERATOR_WHEN`):

```
principal has agent_type && principal.agent_type == "operator" &&
principal has agentTypeVerified && principal.agentTypeVerified == true
```

The `has` guards make attribute-absence safe; the `agentTypeVerified == true`
clause means a self-declared (header) agent can never match.

## What is granted - the seven action x resource pairs

Three per-resource permits (per-resource, not one combined permit, so no
nonexistent pair - e.g. `ShadowVerdict.Ingest`, `ReviewRun.Record` - is ever
nominally allowed):

| Resource | Actions |
|---|---|
| `ReviewRun` | `Ingest`, `IngestRecord` |
| `ProofPacket` | `Ingest`, `IngestProof` |
| `ShadowVerdict` | `Record`, `MarkAgree`, `MarkDisagree` |

These are exactly the actions the sweep dispatches: `Ingest` (which fires
`record_ingest`, whose callback the kernel dispatches as `IngestRecord` /
`IngestProof` under the same operator context) and the three ShadowVerdict
writes. `Ingest`/`IngestRecord`/`IngestProof` are the S0 spec action names;
`ReviewRun`/`ProofPacket`/`ShadowVerdict` are the entity/automaton type names.

## What stays denied, and why

- A non-operator agent (any other `agent_type`) - denied; the sweep is the only
  writer of these records.
- A self-declared operator (`agent_type == "operator"` but
  `agentTypeVerified == false`, the header path) - denied; this is the exact
  case the verified flag exists for.
- Any other action on these resources (e.g. `ReviewRun.Supersede`,
  `ProofPacket.SupersedeProof`, `ReviewRun.Record`, `ShadowVerdict.Ingest`) -
  denied; the permits list only the seven pairs above.
- No existing permit is widened; the Admin and read/list permits are untouched.

## Why this is safe against undeclared names

temper-authz evaluates app policies SCHEMA-LESS
(`engine/mod.rs`: `Request::new(..., None /* actions/resources are
tenant-defined */)`), building `Action::"{action}"` / `{type}::"..."` uids from
the dispatch. There is no schema/declaration section the names must appear in;
they match by equality with the dispatch-built request. The live ARN-431 403s
(AuthorizationDenied, not unknown-action) already proved the kernel builds the
request with exactly these action/resource names.

## Test plan (red-green)

An AuthzEngine test (`verified_operator_may_write_shadow_records_narrowly`):
Deny(NoMatchingPermit) on all seven pairs BEFORE the permits, allowed AFTER; a
non-operator agent, a self-declared operator, and out-of-list actions/resources
(incl. `ShadowVerdict.Ingest`) stay denied. Plus a LOCAL real dispatch (operator
credential, patched policy, isolated turso db): `ReviewRun.Ingest` 403 -> 200 and
the entity reaches `Recorded`; `ShadowVerdict.Record`/`MarkAgree` return 200. The
"after 200" lands live once the lead publishes + installs the merged policy.
