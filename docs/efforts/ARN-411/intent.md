# Intent: enforced SDLC gates in temperpaw
Author: Rita (via Fable). Status: accepted.

## Problem
The SDLC flow was advisory - an agent could skip decisions, proof, or review. Nothing stopped a PR from merging without them.

## Proposed outcome
The stages that agents skip become required CI checks: planning artifacts, a decision log, proof of implementation, an independent review, and risk-tiered auto-merge.

## Affected users and systems
Every PR to temperpaw (and later the other three repos); the shared review computer; CI.

## Constraints
Harness-agnostic (Claude/Codex/Cursor/Grok); nothing committed for evidence (PR comments + Vercel); the review runs on flat subscriptions via the computer.

## Open questions
Deploy/verify/rollback is Genesis-first and prod-fragile - how it triggers (not a plain GitHub merge) is resolved in the deploy effort, not here.
