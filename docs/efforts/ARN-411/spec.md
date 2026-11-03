# Spec: the SDLC gates
Status: accepted. Intent: docs/efforts/ARN-411/intent.md

## Requirements
Five required checks on every PR: sdlc-planning (intent/spec/plan/decisions exist), sdlc-decisions (shaped decision log), sdlc-verification (proof, scope-aware skip), sdlc-review (fixed panel, cost-tiered), sdlc-automerge (risk-tiered routing).

## Design
Gate logic lives once in arni-labs/stack; each workflow clones stack at run time. Evidence rides in PR comments (hidden JSON marker + human summary) and Vercel; nothing committed. Review runs local -> computer -> cursor -> api, first available.

## Policy / invariants
Fixed review panel from panel.json; no self-review (synthesis != author, commit == head); required checks block merge via branch protection.

## Deferred / out of scope
Deploy/verify/rollback (its own effort); check-run mirror (ARN-419); instruction evals (ARN-414).
