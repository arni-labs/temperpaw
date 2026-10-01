# Jev and the Foresight critic: shared evidence

Foresight now prepares one complete evidence packet before either evaluator starts. `Path.StartChallenge` runs `prepare_challenge`; its `ChallengePrepared` callback persists the packet and SHA-256, rejects a callback for an obsolete repair log, and enters the system-only `LaunchChallenge` action. That action launches the reasoning critic and an independent SemanticEvaluation with copies of the same packet.

The packet contains the complete repair narrative, complete endpoint bundle, observed skeleton, required causal nodes, repairer's existing flags and the exact four questions: contradiction, incentive, lag and unexplained discontinuity. No document is silently truncated. A 128 KiB application cap bounds the complete serialized packet; this byte cap is not a guarantee of fitting the provider's separate token limits. Missing/empty documents and oversized packets fail explicitly.

Both consumers verify the stored hash. In shadow mode the critic has only reporting tools, so neither evaluator can independently fetch different evidence. They use the same rubric and clear/defect/unknown choices. The critic additionally provides explanations and reports only costs not already acknowledged by the repairer; Jev's judgments never change scores, routing or forecasts. Existing off-mode research behavior is retained.

Each SemanticEvaluation becomes Recorded, Skipped or Failed, with a two-minute timeout and immutable terminal record. A successful record retains the packet hash, exact provider request, pinned and returned model, distributions, usage and elapsed time. Accepted rounding is preserved in raw records; semantic Brier calculations normalize the rounded distribution. Forecast probabilities are never inferred from Jev confidence.

## Matched live comparison, 2026-09-18

The prior structured-only comparison omitted narratives and is superseded. All 26 source documents were retrieved with the Foresight MCP's `navigate("Files('…')/$value")`; the earlier FileVersions denial did not prevent reading these current files.

All 22 completed paths from run C were replayed against GPT-5.5 through Foresight Sessions and Jev 1.13.0 through the compiled evaluation WASM, using identical frozen packets and question sets. One incomplete source path was excluded explicitly. Both models completed all 22 cases. Packet identities and Jev request contents are checked programmatically.

Agreement was 63/88 judgments: contradiction 22/22, incentive 18/22, lag 17/22, unexplained discontinuity 6/22. Jev marked lag and discontinuity defects on every path; GPT-5.5 did so on 17 and 6 respectively. Agreement is not accuracy: there are no independent gold labels, and the paths share one world and three endpoints. This does not justify replacing the critic.

The production-host WASM Jev replay had a 0.249 s median invocation, 0.561 s p95 and 309,371 input tokens. At the documented $0.042 per million input tokens, estimated cost is $0.012994, not a billing readback. An earlier direct-HTTP preflight agreed on 86/88 categorical judgments with a 0.526 s median; connection reuse and transport differ. For 18 timed reasoning Sessions, median observed completion was bounded between 25.2 and 31.4 seconds by polling; four initial cases lack precise timing. Those clocks are not equivalent. Session token counters returned 1/1 and cannot support a reasoning-cost estimate.

This is a complete paired replay of the critic stage with common upstream paths held fixed, not a newly generated research-to-forecast pipeline run. It does not measure whole-pipeline speedup or future forecast accuracy. The app changes are tested locally; the installed acceptance app has not been replaced.

The report and exact evidence are in Foundry: `arn518-fair-comparison-report.md`, `arn518-fair-packets.json`, `arn518-fair-jev-wasm-results.json`, `arn518-fair-reasoner-sessions.json`, and `arn518-fair-comparison-final.json`. Recompute with `scripts/compare_foresight_evaluators.py`. API model/pricing source: https://docs.typesafe.ai/models.

## Verification

The shared-packet guest test exercises preparation and both consumers, checks that no consumer refetches documents, compares their complete input and rubric, and verifies tampering rejection. Runtime tests prove the callback/system-entry/spawn sequence, terminal outcomes and callback permissions. A stale preparation cannot overwrite a newer repair. The explicit ignored live test replays frozen packets through the actual compiled Jev guest using ProductionWasmHost; credentials are supplied only through a restricted file path and never appear in artifacts.

## Transition-localization replay

An additional live API experiment evaluated 171 recorded internal dependencies across the same 22 paths, preserving all document text as indexed verbatim passages. The four questions were asked per dependency; each resulting defect then received a separate passage-selection question with every passage plus `none`. This experimental harness is in `scripts/localize_foresight_jev.py`; calculations are in `scripts/summarize_foresight_localization.py`. It is not deployed application behavior.

Jev returned 76 timing flags (54 with a selected passage) and 67 missing-step flags (12 with a selected passage); no local contradiction or incentive flags. Ten of the original 25 broad disagreements had a corresponding local flag with a citation. The repeated broad questions matched 85/88 prior Jev judgments. A selected passage is not validation: several missing-step citations describe timing concerns rather than omitted prerequisites. Existing author cost flags remain visible, so retrieving them is not independent discovery.

All final cases completed. Four initial citation requests exceeded the token budget, resolved by batching four citation questions per request with full context. One inconsistent choice/probability response was rejected and the classification rerun once; failed attempts are retained. Source reconstruction, offsets, packet hashes, exact edge selection, citation coverage and summary totals were checked. External references (274 edges) were recorded but not individually assessed, and no new matched GPT run or gold-label accuracy claim is made.

Foundry holds `arn518-jev-localization-report.md`, `arn518-jev-localization-results.json.gz`, `arn518-jev-localization-summary.json` and `arn518-jev-localization-pilot.json`. The design supported by this experiment is specific transition flags with inspectable citations and reasoning review before any cost or forecast changes.

## Matched review-workflow timing

A subsequent controlled batch ran all 22 paths twice: GPT alone, and live Jev localization/citation followed by GPT review. Both reviewed all 171 internal transitions under the same four-category rubric and returned cited findings plus repair recommendations. The assisted model received full evidence and was required to inspect what Jev missed. Jev preprocessing is included in its elapsed time.

Mean time to the first returned review was 54.7 s for GPT alone versus 52.4 s assisted (4.1% lower). Median paired saving was -0.9 s. Polling bounds distinguish nine faster assisted pairs, seven slower pairs and six ambiguous pairs. Both models completed all 22 reviews and covered every transition. Strict citation/structure checks passed for 15 GPT-only outputs and 21 assisted outputs; on the 15 pairs where both passed, means were 49.7 versus 49.8 s. Thus this test does not demonstrate a consistent speed advantage for the same usable output. Correcting invalid outputs was not timed.

Judgment agreement was 664/684 (97.1%), which is not independently labeled accuracy. The assisted reviewer retained 47 of 148 Jev candidates and added eight findings outside them. The primary batch used unique early prompt markers and fresh instances of the same governed native MCP connector after an initial batch encountered a cumulative interpreter-allocation failure. All initial results remain in the evidence but are excluded from primary timing. Backend caching and runtime variance were not fully controlled.

Foundry artifacts: `arn518-controlled-review-report.md`, `arn518-controlled-review-final-summary.json`, `arn518-controlled-review-protocol.json`, and `arn518-review-workflow-evidence.tar.gz`. The archive contains exact inputs, Session IDs, raw outputs, timing bounds, Jev calls and reproducible harness/analysis sources. This measures review and repair recommendations, not a complete research-to-registration pipeline or automatic repair execution.

## Selective review timing

A fresh 22-pair experiment replaced some GPT checks with Jev judgments. The fixed experimental rule skips a transition/category only when Jev selects clear with `probabilities.clear >= 0.80`; every other check goes to GPT. This score is not calibrated correctness. GPT receives the complete evidence but reviews only assigned checks, without Jev verdict hints. The portable offline policy is `scripts/route_foresight_jev.py`; this does not change installed application routing.

All 44 Sessions completed. Jev skipped 324/684 checks (47.4%), yet 168/171 transitions and all 22 paths still required GPT. Mean elapsed time, including fresh Jev screening, was 50.8 s for full GPT versus 46.5 s selective: an observed 8.4% reduction. Median paired saving was 2.2 s; completion brackets distinguish 11 faster selective pairs, four slower and seven overlapping pairs. On 19 pairs where both outputs passed strict checks, means were 49.7 versus 46.4 s (6.8% reduction). Screening itself averaged 0.892 s.

Both arms returned 52 findings, with 44 shared (84.6% baseline-relative retention). None of the eight absent baseline findings was skipped by Jev: all were routed to GPT and judged differently. There were eight additional selective findings and 667/684 matching verdicts. The prior full-GPT run had 63 findings versus the new full-GPT run's 52, with 46 shared; baseline variability is substantial. These are agreement/retention measures, not independent accuracy or proof of equal quality.

Strict schema, coverage and exact-quote checks passed for 20/22 outputs in each arm. All five nonverbatim quotation instances differed only by Markdown bold markers; they remain failures in the recorded validation. Four policy tests passed, and packet hashes, identical decoded evidence, exhaustive disjoint routing, all 44 unique completed sessions, timing arithmetic and archive hashes were verified. An initial native MCP submission/parser failure was fixed by splitting Python string literals across lines; excluded attempts remain in the evidence.

Foundry holds `arn518-selective-review-report.md`, `arn518-selective-review-summary.json`, `arn518-selective-review-protocol.json`, and `arn518-selective-review-evidence.tar.gz`. The archive includes runnable analysis and exact live-harness sources. This demonstrates a modest observed critic-stage saving on one reused world, not a newly generated full pipeline, production threshold validation or deployment.

## Semantic-program Observatory

The Foresight dashboard now links to a portable Observatory with an animated prerequisite graph, exact function-call inspection, presentation mode, JSON/HTML/SVG export, browser screen capture, local live observation, comparison, a forecast ledger and source-concealed human evaluation. A self-contained HTML artifact and short MP4 are published in Foundry. ADR-012 distinguishes this offline experiment from future Temper-native production execution.

A real Foresight reasoning Session proposed eight alternative hypotheses in 92.7 s. Forty Jev calls composed mechanism comparison, recursive gap checks and next-operation selection in 16.3 s, yielding 88 events over 24 prerequisite nodes. Another 40-call run verified live browser observation in 16.7 s. All candidates remain unresolved; no forecast probability or accuracy gain was inferred. Exact-input reuse and cycle/depth/call guards are tested, but these runs had no cache hits.

The human evaluation view includes excerpts from the original three endpoints as an explicitly unmatched baseline. It records run/candidate identities, usefulness/vividness preferences and rationale; imported recordings support future comparisons. The existing 56 forecasts remain unscored. Outcome scoring rejects missing evidence, simulated outcomes, hindsight and future resolutions and counts one eligible revision per event.

All 54 dashboard Foresight tests and five recursive-program tests pass. The production dashboard build and Svelte checks pass; browser verification covers desktop/mobile, replay, selection, live following, source inspection and portable exports preserving review provenance. No production exploration engine was replaced and no full-backend boot is claimed for this UI/experiment change. See `dashboard/static/observatory/README.md` for execution and export instructions.

Foundry: `arn518-foresight-observatory.html`, `arn518-foresight-observatory-demo.mp4`, `arn518-observatory-recording.json`, `arn518-observatory-report.md`, `arn518-observatory-preview.png`, and `arn518-observatory-evidence.tar.gz`.
