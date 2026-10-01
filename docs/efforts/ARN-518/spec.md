# Foresight learning and observation

## Outcome

A user can inspect a world and its predictions, follow the engine's activity, run accelerated learning from dated experiences, and see evidence of what changed. New resolved outcomes drive subsequent learning. Model parameters actually change; stored prose alone is insufficient.

## Contract

1. Preserve the current World, EventNode, Endpoint, Path, Claim and Forecast capabilities.
2. Every prediction is immutable and records its raw input probability, applied model version, evidence provenance and registration time. A changed probability or model produces a revision linked to the previous prediction. Repeating the same registration is idempotent.
3. An outcome identifies the question, resolution time and sources. Observed, historical and simulated experiences remain distinguishable. Voided, duplicated, undated, future or unsupported outcomes cannot silently become training truth.
4. Learning has visible preparation, training, evaluation and final states. A run retains its input identities, preceding model, candidate parameters, sample counts, evaluation results and reason for adoption or rejection.
5. Train a bounded predictive component from outcomes. Evaluate on examples excluded from that candidate's fitting. Temporal evaluation must not use an outcome before it was available. Compare candidate and preceding model using identical examples and features.
6. Adopt only supported improvements. Insufficient evidence, malformed data, non-finite values and failed evaluations preserve the incumbent and explain why. Concurrent or repeated completion must not replace a newer model with an older candidate.
7. Historical replay advances a supplied logical time and exercises the same learner. It does not wait for wall time. Simulation data may demonstrate mechanics but must be labeled and must not promote a live model as empirical evidence.
8. New verified outcomes can trigger another learning cycle through declared Temper transitions. The engine's control flow lives in entity specs and WASM callbacks, without an independent background orchestrator.
9. Applied models affect later registered predictions. The interface shows the input probability, revised probability, model identity and supporting run.
10. Authenticated UI: worlds and dependencies; prediction detail and revisions; activity and failures; learned changes and evaluation. Reuse the existing dashboard client and event stream. No server credential enters browser code.
11. All queries and computation have explicit limits. Truncation or skipped examples are reported. Failures remain visible.

## First end-to-end acceptance case

Create or load a world. Display its events, paths and registered predictions. Supply a dated replay dataset with explicit provenance. Run a real parameter update through Temper. Observe training and evaluation, an adopted candidate or a justified rejection, and a subsequent prediction that uses the adopted model. Refresh the page and verify the records persist. Resolve a new forecast with evidence and prove that feedback reaches the learning path. Repeat delivery and completion events to verify no duplicate scoring or stale model adoption.

A deterministic simulation fixture demonstrates mechanics and is labeled as such. Historical or live quality claims require independently sourced outcomes and comparable scores. No generated display content stands in for a missing engine result.

## Invariants and model

- Registered prediction contents never change.
- Each event contributes at most once across an adopted model's fitting/evaluation lineage. Later runs exclude consumed identities; rejected candidates leave that lineage unchanged.
- Candidate evaluation examples do not enter that candidate's fitting set.
- A prediction uses only the adopted model available at registration. Its resolution deadline must be later than the frozen registration clock; earlier path prerequisites remain context. Date-only deadlines include their UTC day, while UTC timestamps are exact.
- A rejected or failed candidate leaves that adopted model unchanged.
- A stale completion cannot overwrite a newer adopted model.
- Simulated feedback never counts as observed evidence.
- Replaying identical inputs in identical order gives identical parameters and scores.
- Terminal learning runs cannot be rewritten.

The implementation's entity state model and deterministic tests must express and check these same invariants. The formal expression will use the project's existing model and verification machinery, without a parallel handwritten orchestration implementation.

## Limits

The first trainable component improves bounded event prediction and calibration. Its coefficients establish learned predictive associations, not discovered causal laws. The design must support future improvements to consequence models and research choice without claiming those research ambitions have already been validated.

### Current learning bounds

A run accepts at most 512 examples and one MB of uploaded JSON. A model retains at most 512 consumed event identities across its adopted lineage. A run that would exceed that lineage bound fails visibly without replacing the incumbent or forgetting old identities. Forecast-history queries likewise reject more than 512 rows rather than silently sampling or truncating them. These are explicit limits of this first implementation, not a claim of unlimited lifetime learning.

Observed prediction registration uses the host-recorded triggering event time. Historical and simulated registration require an explicit logical time; invalid time or unavailable inputs return the world to Active with an error so the caller can correct and retry. Legacy hindcast configuration is normalized to historical learning provenance through the trusted registration callbacks. A registration pass retains one model snapshot while concurrent adoption preserves the world's operational state.

## Progressive exploration

The observed vertical slice includes research, three distinct saved futures, extracted claims, backward paths, independent challenges, cost-based path classification, and immutable event predictions. A new first pass evaluates at most three ranked claims per future. Each claim receives one repair and challenge before its first verdict; revision rounds and alternate paths run afterward.

A Canonical or Tail path requests prediction registration after its classification commits. Unclassified or rejected worker requirements are excluded. Explicit dashboard-authored questions remain eligible. The registration queue coalesces requests arriving during a batch. A failed or timed-out batch drains a request that arrived after the batch started; without new queued work, it remains Active with its error instead of retrying forever. It freezes eligible inputs and the model in a durable snapshot before taking the observed prediction timestamp. Replay retains its explicit logical clock. Attempt guards prevent an old batch from completing a newer one.

After every active sampled future has attached its claims, all attached claims are present and have committed a terminal verdict, and the pending registration batch completes, the World starts background exploration once. Each eligible claim reopens once while retaining its previous routes. Existing route and revision ceilings bound this deeper pass. Cursor guards reject repeated advancement; completion waits until all planned claims actually started and finished. A phase guard prevents a delayed first-pass completion from overwriting background progress. Prior forecasts remain immutable.

Foresight first-pass sessions explicitly select a 180-second provider invocation limit and at most one sequential session-level retry. Existing bounded transport retries occur inside that invocation limit. Other sessions retain their normal limit. An active invocation cannot be restarted concurrently. Matched historical tool calls and outputs remain structured; terminal provider events end the stream read without waiting for transport EOF.

## Native Foresight decision outlook

The native semantic run retains its original evidence and every Jev assessment. Research, uncertain and repair recommendations can select at most eight distinct scenarios for one deepening round. Only live-world deepening may use existing read-only web search/fetch, at most eight Session turns with a prompt budget of two searches and four source reports. A reported source is not a validated future fact. Original gaps remain, new revisions record their parent assessment, and new nodes receive fresh Jev assessments. Unavailable research remains an unanswered question. Hindcasts must not use fresh web evidence.

Final output is a `foresight-outlook-v1` JSON answer: headline (160 characters), summary (400), horizon, probability_basis=`subjective_model_estimate`, calibrated=false, evidence_limits, research_questions and three to five outcomes. Each outcome contains id, title (70), definition (240), probability, scenario_ids, narrative (360), signals and falsifiers (one to three strings, 160 each). Exactly one outcome is residual `other` with no modeled scenario IDs; the other buckets partition all modeled scenarios without overlap or invented IDs. Weights must be finite, in [0,1], and sum to one. They are model judgment, never Jev choice distributions or a claim of empirically measured calibration.
