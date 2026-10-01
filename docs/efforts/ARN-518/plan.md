# Plan

## What we are addressing

ARN-518 adds actual continuous learning and an understandable UI to the existing Foresight app. The end state is the complete verified flow described in spec.md, shown to the user before visual refinement.

## Implementation

1. Implement the learner as bounded pure Rust shared by WASM integrations and deterministic tests. Establish failing acceptance tests for temporal separation, improvement/rejection, replay determinism and malformed or simulated evidence.
2. Express learning preparation, fitting, evaluation and adoption in Temper entities and callbacks. Preserve an immutable run record and versioned parameters. Connect recorded outcomes to learning and adopted models to prediction revisions.
3. Add a focused Foresight workspace to the authenticated Svelte dashboard. Reuse OData and SSE. Build world, prediction, activity and learning views and controls against the actual app contracts.
4. Run the real app and dashboard in the governed execution environment. Drive the complete flow and validate persistence, failure states, duplicate delivery and stale candidate behavior. Record evidence and any necessary implementation decisions as they arise.
5. Run the configured review panel, triage within accepted scope, complete required checks, publish the app to Genesis and deploy the dashboard through the governed release. Verify the installed version, live user flow and Datadog evidence. Show the working result.

## Delivery priority

Complete one usable learning and observation flow first. Keep presentation simple and readable using the existing dashboard plus the Galley design tokens. Defer decorative refinement, not required behavior or correctness.

## Approved delivery acceleration

Publish accepted path predictions as they become available. Reduce the initial exploration to three futures with up to three key claims each, followed by one repair-and-challenge pass per claim. Start bounded deeper searches after the first result is usable. Preserve saved paths and prediction history. Repair provider protocol and recovery failures before timing a fresh complete run; report actual first-prediction and first-pass elapsed times separately from implementation time.


## Persistent delivery and original UI

The user requested the original Temper-backed Deep Sci-Fi presentation after inspecting the temporary dashboard. Reuse its existing deployment branch and OData adapter, preserving the separate fiction application. Display real forecasts, calibration runs, durable session status and paginated run history.

Recovered preview exports are dated, read-only snapshots stored through existing persistent file storage. Preserve their original identifiers and known failures, and explicitly identify absent stories/history; do not manufacture engine events or import snapshots as successful live runs.

Run the repaired kernel and final app closure in an isolated persistent Railway acceptance environment with its own PostgreSQL and external file storage. Verify the complete first pass and deeper exploration without manual recovery, then restart and re-read original records and story bytes. Keep the browsable URL and data available. Release the same verified artifact to the dedicated Foresight service after required review, with production and fiction data preserved.
