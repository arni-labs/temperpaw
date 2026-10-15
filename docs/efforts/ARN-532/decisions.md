# Decisions

## Restore the intended human approver in the app policy

**Decision:** Permit only the registered verified operator to invoke Ask.Answer.

**Came up because:** A signed-in Foundry test answer returned403, local hash comparison identified its existing human credential as operator, and actual Cedar evaluation denied that principal.

**Options:** Substitute an agent credential, broadly grant operator factory permissions, or add an exact Answer permit to the owning app policy.

**Chose the exact permit because:** It repairs the human flow while preserving resident-agent prohibitions, generic-write restrictions and native permission decisions. This is a narrow permission omission correction, not a new architecture; a separate ADR is unnecessary.

**Where:** os-apps/paw-patrol/policies/patrol.cedar; crates/temperpaw/tests/dsf_factory_policy.rs; companion Foundry PR33.
