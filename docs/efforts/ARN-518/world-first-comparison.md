# World-first comparison protocol

Frozen before candidate implementation outputs on 2 October 2026.

## Baseline

Requested model: GPT-6 Astra, extra-high reasoning. An independent context receives
the ordinary questions and a strong prompt, can research public sources, and cannot
inspect Foresight outputs or project files. Up to 3,500 words per question. The prompt
explicitly requests bold distinct worlds, changes beyond the present, backward paths,
alternative routes, interacting detail, assumptions, challenges and source support.
This is a strong direct-answer baseline rather than a deliberately weak prompt.

Frozen prompt SHA-256:
`e05b5429932bc4a540679a99e746d127fafdf953fc1a38f04e9df1e965fd81ea`.
Frozen inputs: `/private/tmp/arn518-world-first/baseline/`. Baseline answers are
written to the workspace-local `benchmark-artifacts/baseline/` directory because
the baseline agent’s patch tool rejected writes outside its project boundary.
No candidate output is supplied to that context.

Questions, unchanged for both systems:

1. How will people make video games in 2030?
2. What will eating at home be like in 2035?
3. How will children learn in 2035?
4. What will living in a big city be like in 2040?
5. What will small music venues be like in 2030?

Games and music are known development examples. Food, education and city living are
held-out questions for this revision; do not represent the entire set as held out.
Preserve all attempts and revisions, including interrupted and failed runs. Report
actual model identity/effort, source vantage, elapsed time, requests and token/cost
figures when available. Unknown resource figures remain unknown.

## Comparison

Use blinded answer labels and counterbalanced order. Judge imaginative change beyond
the present, distinctness of whole worlds, causal completeness and alternative routes,
evidence fidelity and chronology, internal coherence, concrete interacting detail,
and readability. Require concrete examples for preferences; answer length and number
of model calls are not quality scores. Inspect cited evidence independently for a
bounded factual check. Keep creative preference separate from factual defects.

Expose per-question findings, ties and losses rather than reporting only an aggregate
win. This small benchmark can establish a measured advantage on its questions, not
superiority for any possible question. A self-score, judge preference, repeated Jev
estimate or fictional scenario is not predictive accuracy. Accuracy claims require
independently resolved outcomes and an evaluation that addresses historical leakage.

## User-flow acceptance

Run the actual deployed question form through completed worlds. Inspect immutable
original endpoints, connected proposed routes, explicit unresolved prerequisites,
amendments, reuse receipts and whole-world estimates. Verify the overview and details
visually. A local unit test or successful deployment alone does not satisfy this step.

## First deployed development attempt

Backend `c7a7a4a81` was activated through native module uploads on the dedicated
acceptance service; preview UI `2ad79601` was verified in the browser. The normal
form accepted “How will people make video games in 2030?” and displayed four
imagined worlds before route estimates. The world is
`foresight-b0bb7b3b-4f9f-4423-9ed0-4a148c83b76e`.

The proposals failed the accepted quality bar: creator platforms, supervised AI
helpers, integrated art tools and provenance paperwork were separate research
topics, largely already represented in that run's own present baseline. Richer
paragraphs did not create sufficiently different futures. This is a development
failure against the brief, not a completed blind comparison. The independent
baseline answers remain sealed from implementation work.

The next correction adds actual proposal-level Jev critique before freezing
endpoints for route search. Preserve rejected attempts and their judgments; do
not represent a model verdict as proof of novelty. Keep the current run and clock
intact. Ordinary-question completion, the held-out questions and the blind
comparison remain pending. No comparative win or delivery is claimed.

The same run later reached terminal Failed: “Reasoning phase exhausted its
reserved polling budget; saved work is preserved”, with zero Jev judgments and
zero routes. The native active-run list was empty. Preserve this functional
failure alongside the proposal-quality failure. No retry or clock reset was
performed. The request for all backward routes in one generation is a workload
hypothesis to test with bounded incremental batches, not a verified provider
failure cause.

## Second development pass

Backend `222dd6e31` was deployed as Railway
`90f3770b-939f-4f70-b798-45781e81cf13`, with all 70 manifest files matching
and readiness 200. The normal form started games world
`foresight-dbd62a8a-d97a-4f6f-8a22-e063e3f27099` and food world
`foresight-717259e6-d662-4fc9-be9e-d95f28f23549`. Neither was restarted.

Food exercised actual proposal rejection/revision, then saved three backward
routes and reached 129 Jev judgments across 31 HTTP requests. This proves an
incremental route batch completed, not a complete answer or resolved paths.
The accepted games and food proposals still looked too incremental. In food,
Jev accepted a revised thrift-cooking idea after its earlier form was rejected;
the human assessment remains that the set falls short of the requested ambition.
Provider acceptance is not the quality benchmark.

This evidence motivated separating first imagination input from the research
agenda, while retaining sourced Jev evaluation and backward search. Food was
held out for `222dd6e31`; after using that result to improve the generator it is
a development example for the next revision. Education and city living remain
unseen candidate questions. Baseline comparator answers remain sealed.

Additional final-answer fixes preserve omitted original endpoints and recorded
reasons, and carry selected path barriers into final audits. A new estimate must
not erase an existing grounding failure. Both tests subsequently failed composition because omitted originals lacked
writer-supplied receipts: games recorded 9 routes, 336 Jev judgments and 127 HTTP
requests; food recorded 9 routes, 381 judgments and 116 requests. Native active
run listing was empty afterward. No completed answer or comparison win exists.

The engine now derives omission receipts from saved route coverage. New initial
batches cover one original's complete commitment set within unchanged byte/node
bounds, and omit unused forward-ranking checks. The previous declaration-order
three-commitment batches explain why later originals had no routes. Transition
cost is a likely search limit (127 provider requests alone require at least 254
Evaluate/Recorded transitions), but the exact terminal admission reason was not
available in the visible UI. A fresh deployed run must establish whether these
changes deliver a complete, useful answer.


## Pass 3: contract failures, no completed answers

Revision `497c0726d` produced no completed answers across games, food, education, cities and music. Games and education exhausted correction polling because backward research emitted stored source records rather than the required response schema. Food and cities could not correct a projection horizon mistakenly stored as an observation period. Music rejected honest unsourced roots with empty mechanisms, despite the prompt permitting explicit unresolved questions. No quality comparison or frontier-model win is established. Education and cities have now informed debugging and are no longer untouched holdouts.

The repair shares an exact research response contract, batches validation diagnostics, permits explicit append-only projection corrections while excluding superseded sources from active evidence, and accepts explicit unresolved roots without promoting them to connected paths. Existing uncertainties, original sources and run clocks remain preserved. Seven module suites passed (707 test executions; 32 ignored), seven WASM builds and clippy completed with warnings, and the rebuilt seed handler passed an actual WASM replay. Fresh deployed runs remain necessary.

## Pass 4: composition and request-size failures

Acceptance backend `0e6244a585d8311f14b61ef4d04616e76ae9a736` again produced zero completed answers across the five questions. Games, music and cities exhausted composition polling. Education exhausted corrections for an invalid causal chronology. Food reached synthesis but failed an inconsistent final validator: its valid reconstructed world had 14 events, while that validator still allowed only 12 instead of the endpoint contract's 32. Cities also recorded an individual Jev request exceeding 128 KiB before its final polling failure. Authentication was ready in the timed-out sessions; the exact cause of provider latency is not established.

The runs preserved substantial work: games 15 routes / 345 Jev checks / 104 provider requests; food 15 / 497 / 116; education 5 / 160 / 29; cities 15 / 508 / 122; music 16 / 494 / 119. These counts establish executed checks, not validated futures or completed answers. No run was restarted or had its clock reset. The native active-run list was empty after all five terminated.

Food's accepted proposal set remained too incremental, despite favorable Jev labels. Its saved proposal receipts motivated contract 2: compare a broader candidate pool against retrieved present findings and against one another, then enrich a selected distinct set without changing its defining claims. Comparator answers remain sealed. No frontier-model advantage has been demonstrated.

The next repair assembles world graphs directly from selected stored routes instead of asking the writer to copy already validated components and links. A captured food replay preserves the exact 14- and 10-event worlds. Synthetic replay establishes graph integrity, not live generation quality or faster provider completion. The individual request-size repair must preserve semantic evidence and causal state while leaving original audit records available for inspection. Fresh deployed completion and the quality comparison remain outstanding.

Native action dispatch timestamps subsequently showed correctly spaced 60-second polls. Games and music consumed the ten-check phase budget as 4 + 4 + 2 across three composer sessions: two corrections rejected a copied causal link before the final response arrived. Cities consumed 3 + 3 + 3 + 1, including the oversized-request rejection. All three final Sessions later completed. The old timeout diagnostic looked only at `response_correction`, hiding the actual `composition_correction`; it now reports the relevant correction. No timer acceleration was observed and no polling or run-time budget was increased.

The captured city audit needs 153 judgments. Its inherited byte cap of 47,456, learned after an actual route-stage token overflow, packs those into 126 requests and exceeds the remaining transition budget. At the default byte ceiling the same planner packs 19 requests; this is a packing measurement, not provider acceptance. Contract 2 therefore keeps separate learned caps for proposal, route, event and world requests, with the same ceiling and actual-error backoff in each domain. Returning to a domain retains its learned cap. Legacy runs retain their global cap. This avoids treating one context shape's byte-to-token ratio as a universal provider limit; fresh live acceptance remains necessary. The [provider documents](https://docs.typesafe.ai/models) token limits of 32k for state plus the longest question and 64k for the complete request, rather than a universal byte limit.


## Pass 5: quality defects observed during live execution

Backend `370a885a6632f2c46379b17cd5bda9150100f3bf` is deployed as Railway `b418dbb0-883c-44d0-85ac-369b7d60a008`; all 74 manifest files matched and readiness returned 200. Five ordinary questions started through the preview form. Games, food, education and cities handed off from research automatically. Music failed during an attempted retirement of a malformed hypothesis node: native approval callback registration returned `GD query failed (HTTP 404)`. The denied action was not bypassed; the system-tenant diagnostic read was unauthorized.

The early games and food candidate sets remained too incremental. Games compared reusable game-component sales with Roblox avatar goods, while the [Unity Asset Store](https://assetstore.unity.com/tools) already lists game toolkits, behavior systems and scripting components. This establishes a missing present comparison, not adoption or earnings. Food selected assembly of meal components, a guided kitchen and budget visibility. Passing model labels do not establish the requested ambition. These observations motivate the stronger present comparison and substantive development change in ADR 019.

The initial research prompt also still requested inferred future hypotheses. Commit `d51bf735c` removes that instruction for world-first runs while preserving the legacy corridor prompt. Fifteen seed tests and an actual WASM evidence-only prepare/baseline/imagine replay passed; restoring the old prompt failed the new regression. This correction is held from deployment while the existing runs finish.

For the previous city replay, 153 judgments/19 requests describes the structural audit subset. The complete native contract-2 replay includes utility tasks and admitted 21 batches with 48 transitions from a state with 80 remaining. Neither replay is live provider acceptance. Comparator answers remain sealed; no frontier-model advantage is established.

Pass 5 ended with zero completed answers. Games saved 13 routes and 437 Jev judgments/119 HTTP requests, but rejected composition because the final selection omitted commitments; the preceding full selection had incompatible combined milestone timing. Food saved 15 routes and 485 judgments/106 requests, then failed the writing provider context limit. Education saved eight routes and 315 judgments/107 requests, then its synthesis stream failed (`host rc -4`) at transition 380. Cities admitted only one of ten provisional ideas, exhausting comparison without development. These are failures, not benchmark candidates. Original clocks were preserved.

The next repair validates a proposed original's combined route graph before saving/evaluating it. Native bounded search supplies a compatible route bundle as composer guidance; another complete selection is permitted if the same graph and milestone checks pass. It does not rank plausibility or silently change stored dates. A reversed pair of deadlines is underspecified timing, not proof that a future is impossible. The captured games failure reproduces; an explicit timing change in a test proposal accepts and assembles all five commitments. That establishes the mechanism, not a live model repair.

Writing now receives its purpose-specific evidence, component, lineage and estimate projection without appending the full search histories. Exact repeated values use reversible references; different signed conditions and incomplete results remain distinct. Food's captured native input fell from 876,544 to 219,940 bytes. Restoring the unconditional histories fails the WASM regression. The captured round trip preserves the projected semantic data; live provider acceptance remains outstanding.

If fewer than three initial candidates pass, one provisional development turn is now possible within existing remaining budgets. A city-like one-of-ten case selects three provisional candidates at 88 transitions used, retains failed receipts, then requires fresh comparisons and pair checks. Development is not acceptance and cannot repeat. Removing this branch fails the regression. The final route suite passed 172 tests (13 ignored); the synthesis suite previously passed 169 (8 ignored), with targeted native replay and mutation checks. Release builds and deployed fresh questions must still establish delivery. Baseline answers remain sealed.

## Pass 6: ongoing acceptance and causal-role correction

The next acceptance build is `aeac15cee7bcd8c803f3a52434cdd9722ca9b680` (Railway `ae99163d-1ef9-4c4d-bac1-a46c797e759a`), verified against 75 installed files and changed native module hashes. Ordinary games, food, city, education and music questions started through the form. Initial games research saved 17 typed findings and zero future-hypothesis nodes; research handed off automatically. No completed answer is established yet.

Games and food exposed a contradictory proposal constraint: the producer correctly represented c1,c2 → c3 and c1,c2,c3 → c4 while all four commitments defined the world. Validation wrongly required consequent commitments to be outside the defining set. Repeated correction consumed the phase polling limit before Jev proposal checks. The repair allows overlapping causal and defining roles, while rejecting self-dependency, unknown references, repeated relations and cycles. Exact selected relations and all commitments remain in the Jev dependence request. It does not relax novelty or evidence checks, change probabilities or reset run clocks. Native module update will avoid restarting healthy provider sessions; any failed-run continuation must retain the original deadline and avoid overlapping provider work.

UI revision `bc77d8aeee92426968e0be051ef00d264b78db41` preserves the native evidence envelope in research progress. The prior projection flattened snake-case fields; a first attempted PascalCase selection was also wrong because the pinned kernel uses exact stored keys. Both failures now reproduce in regression tests. The stopped music run visibly renders its saved qualified source findings after deployment. Direct browser opening of its progress API was blocked; that route was not bypassed.

Approval handling repair `95941e67f` is committed but not deployed. A request-approval callback404 now preserves the pending human-decision state and exposes callback/notification errors; actual WASM tests cover404/200 and notification failures, and restoring the terminal-error behavior fails the regression. This does not repair inaccessible governance infrastructure, grant approval, retry a denied action or suspend the parent Foresight clock.
