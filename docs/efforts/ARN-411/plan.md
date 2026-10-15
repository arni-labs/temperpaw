# Plan: wire the gates
Spec: docs/efforts/ARN-411/spec.md

## What we are addressing
Make the advisory SDLC flow enforced, proven on temperpaw first, then rolled to the other three repos.

## Approach
Build the gates in stack; pilot on temperpaw; fix what the pilot exposes; turn on branch protection; roll out.

## Steps
1. Build the five gate scripts + workflows in stack.
2. Vendor into temperpaw; get all green on a real PR.
3. Branch protection: mark the checks required.
4. Roll the workflows to temper, katagami, deep-sci-fi.

## Files / surfaces touched
.github/workflows/sdlc-*.yml (each repo); stack/gates, stack/proof, stack/review, stack/autonomy.yaml.

## Expected end state
A PR cannot merge without the design chain, a decision log, proof, and review; low-risk PRs auto-merge, risky ones elevate to a human.
