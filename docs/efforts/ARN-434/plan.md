# Plan: ARN-434 Cedar permits for the shadow-sweep writes

## Expected end state
- `policies/patrol.cedar` gains three narrow permits (verified operator):
  ReviewRun {Ingest, IngestRecord}; ProofPacket {Ingest, IngestProof};
  ShadowVerdict {Record, MarkAgree, MarkDisagree}.
- An AuthzEngine red-green test: the operator is DENIED these before the permits
  and ALLOWED after; scoped-out cases (unverified agent, wrong action, wrong
  resource) stay denied.
- Design chain committed; draft PR; not merged.

## Steps
1. Design chain (this dir).
2. Add the AuthzEngine test; run it against the current policy -> RED (deny).
3. Add the three permits to patrol.cedar; run the test -> GREEN (allow).
4. Capture the before/after transcript pair as proof.
5. fmt/clippy the touched crate; draft PR; report.

## Out of scope
No entity/module/workflow change. Publish/install + acceptance rerun happen after
the lead merges (their approved prod pattern). The kernel guards-cant-read-params
gap (ShadowVerdict.agree computed by the sweep) stays as-is.
