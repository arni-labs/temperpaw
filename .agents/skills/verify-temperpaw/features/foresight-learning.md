# Foresight worlds and learning

## Sub-features

Authenticated dashboard at /dashboard/foresight; world and evidence inspection; immutable prediction revisions; observed research setup; historical and simulated replay; outcome entry; model fitting and held-out evaluation; adoption history and live activity.

## Driving it

Use an isolated server and database with the paw-foresight app installed. Sign in through /auth/login and use the session cookie for both browser and HTTP verification. The browser must never receive a server API key. Verify the running binary and installed WASM identities before testing.

1. Create a simulated world, set an explicit replay clock, and add a question with a probability strictly between zero and one. Register predictions and read the resulting Forecast back.
2. Load the canonical labeled fixture at os-apps/paw-foresight/fixtures/simulated-calibration.json. Run learning after its outcomes are available. Inspect preparation, candidate coefficients, training/validation counts, the identical held-out examples used for both scores, and the final adoption decision.
3. Advance the clock beyond the model's evaluation boundary. Register again and inspect the earlier and later predictions, their model/run identities and revision link. Repeat the same registration and verify no duplicate.
4. Record a dated outcome with sources. Read every revision back, independently recompute its Brier score, and verify that the feedback starts learning. An insufficient-evidence run must reject without changing the incumbent.
5. Submit a replay clock older than the adopted model and retry with a valid clock. The failed request must be visible and leave the world usable.
6. Reuse consumed events in a later dataset alongside fresh events. Consumed events must not refit the model or become held-out evidence again. Preserve cumulative lineage and report the explicit bounds.
7. Check World, Forecast and LearningRun PATCH, PUT and DELETE denials. Attempt internal learning/model/registration/scoring callbacks with the same dashboard cookie; these must be denied while operator actions still work.
8. In observed mode, verify the configured provider/model is shown and passed to research. Without provider setup, show the actionable setup state; with legitimate test credentials, start research and follow it through event/prediction creation. The first registration uses the host event timestamp. A simulated fixture does not prove this provider path.
9. Refresh the page and verify selected world, replay time, tab, prediction history and learning results persist. Inspect real world data and any honest empty states. Confirm authenticated live events arrive.

## What proves it

Actual parameter changes and predictions, independently recomputed scores on the same examples, immutable persisted records, denial responses with unchanged state, and a browser the verifier has driven. Save screenshots of the world, expanded prediction history and learned changes. Record failed and rejected runs as outcomes rather than calling all runs successful.

Synthetic fixture gains demonstrate the loop only. They do not establish live predictive accuracy or a learned causal world model.

## Adjacent behavior

The dashboard session adapter affects authenticated OData reads/actions and event subscriptions. Recheck tenant isolation and method/URI/replay binding with the composed router. Hindcast batch grading must retain every revision's score and start one learning cycle for the batch. Registering forecasts must remain idempotent when a list response lags an authoritative row read.

## Progressive first pass

Create a new observed world and complete its normal Seed action; reusing an existing corpus is permitted, but do not synthesize SeedComplete. Sample three futures once. Verify distinct saved bundle bytes, at most three claims per future, and a challenged first-pass route before any additional revision round. Read the first registered Forecast while other claims are still running. Its required EventNode must belong to a Canonical/Tail path (or be an explicit dashboard-authored question).

Record first prediction and first-pass elapsed times. Verify the first pass becomes usable before background claims reopen, and that the UI shows background exploration with the earlier forecasts still present. Confirm final background completion, retained path IDs, and immutable earlier forecasts. Exercise concurrent registration requests, stale registration callbacks and a stale first-pass callback after deepening begins. They must not lose pending requests, reset the phase, or backdate a newly selected input.

Provider checks must use the exact installed module: preserve real multi-turn function calls, terminate on completed/failed/incomplete response events, record progress under the trusted entry, and allow at most one session-level retry after invocation termination. A 180-second hard call limit is not a measured idle timeout or a promise of end-to-end latency.
