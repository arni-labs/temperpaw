## Decisions & Tradeoffs

**Decision:** Grant the shadow-sweep write actions to `principal is Agent` gated
on `agent_type == "operator" && agentTypeVerified == true`, not to a broadened
Admin permit or a looser agent predicate.
**Came up because:** the sweep's key resolves to the operator AgentCredential
(verified empirically: the temperpaw secret == the prod openpaw operator key by
sha256; it bootstraps as AgentType "operator"). The denied actions need a permit
for THAT principal.
**Options:** widen the existing Admin permit to cover the operator (rejected -
the operator is an Agent, not Admin, and widening Admin is broader than needed);
grant any `principal is Agent` (rejected - any self-declared agent could then
write records; unsafe); mirror the kernel's verified-operator predicate (chosen).
**Chose the verified-operator predicate because:** it is the exact identity the
kernel already trusts for operator actions (`seed_operator_manage_policies` uses
the same `agent_type == "operator" && agentTypeVerified == true`), and the
`agentTypeVerified == true` clause excludes header-self-declared agents. Given
up: nothing - it is the narrowest grant that lets the sweep work.
**Where:** `os-apps/paw-patrol/policies/patrol.cedar`.

**Decision:** Three per-resource permits (each listing only that resource's
denied actions), not one permit with a combined action+resource set.
**Came up because:** "Ingest" exists on both ReviewRun and ProofPacket, but
Record/MarkAgree/MarkDisagree only on ShadowVerdict, and IngestRecord only on
ReviewRun / IngestProof only on ProofPacket.
**Chose per-resource permits because:** a single combined permit would nominally
allow non-existent pairs (e.g. ShadowVerdict.Ingest, ReviewRun.Record); per-
resource permits grant exactly the real denied pairs and read as the intent.
Given up: a few more lines.
**Where:** `os-apps/paw-patrol/policies/patrol.cedar`.

---

## Panel round fixes (#487)

**Act-on (names could reference a schema that never declares them):** refuted at
the source and proven by a real dispatch. temper-authz evaluates app policies
SCHEMA-LESS - `engine/mod.rs` builds the Cedar request with
`Request::new(..., None /* schema-less: actions/resources are tenant-defined */)`
and `Action::"{action}"` / `{type}::"..."` uids straight from the dispatch. There
is no schema/declaration section to match against; the names are the spec action
names (Ingest/IngestRecord/IngestProof/Record/MarkAgree/MarkDisagree) and the
entity/automaton types (ReviewRun/ProofPacket/ShadowVerdict), resolved at
dispatch. Proof: a LOCAL real dispatch (operator credential, patched policy,
isolated turso db) - `POST ReviewRuns('..')/Temper.Ingest` returned **HTTP 200**
(was 403 before the permit) and the entity advanced to **Recorded** (so the
kernel-dispatched `IngestRecord` callback, under the operator context, is
permitted too); `ShadowVerdict.Record` + `MarkAgree` returned 200 (agree:true
set). The AuthzEngine test builds the identical uids the kernel builds, so it
exercises the same path; the live dispatch removes any "hand-built entity" doubt.

**Consider (has-guards):** the three permits now use the kernel's canonical
`VERIFIED_OPERATOR_WHEN` predicate verbatim -
`principal has agent_type && principal.agent_type == "operator" &&
principal has agentTypeVerified && principal.agentTypeVerified == true` - so
Cedar attribute-absence can never error and the wording matches the kernel's own
operator policies.

**Consider (self-declared negative test):** the test now asserts a self-declared
operator (`agent_type == "operator"`, `agentTypeVerified == false` via the header
path) is DENIED on all seven pairs - the exact attack the verified flag exists
for.

**Consider (ShadowVerdict.Ingest):** the test now asserts `ShadowVerdict.Ingest`
(a nonexistent action the intent disclaims) is denied.
