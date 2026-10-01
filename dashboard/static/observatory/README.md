# Foresight Observatory

A portable observer for recorded and locally running semantic-program experiments. Open from the Foresight dashboard's **Open Observatory** link, or serve this directory with `python3 -m http.server 4179` and visit `http://localhost:4179`.

## Use

- **Explore:** play, pause, scrub and change playback speed. Select a graph node or a trace event to inspect hypotheses, exact model inputs/outputs, uncertainty, and prerequisite recursion. Space toggles playback; Escape leaves presentation mode.
- **Compare:** the completed 22-pair critic benchmark and the separate recursive experiment. Their timings are not a matched end-to-end comparison.
- **Forecast ledger:** 56 actual preregistered forecasts from the frozen world. No eligible outcomes exist in this snapshot. The evaluator scores only observed, verified, sourced, dated resolutions and deduplicates revisions per event.
- **Evaluation lab:** original endpoint excerpts versus newly proposed hypotheses, with source labels concealed. Record usefulness/vividness preferences and notes. This comparison is explicitly unmatched in generation budget and output format. Import another Observatory recording as a baseline for subsequent runs. Reviews remain in memory until exported.
- **Export:** self-contained interactive HTML, JSON recording, SVG graph, or browser screen video. Exports include input evidence and local reviews. Browser screen capture requires the user's screen picker and browser support. The HTML snapshot works without a server; optional Google fonts fall back offline. Live observation needs localhost/HTTPS.

## Real local execution

`explore_foresight_semantics.py` is an explicitly offline experiment, not the installed application's scheduler. It composes `compare_mechanism`, recursive `classify_gap`, and `choose_next_operation`; deterministic traversal tracks cycles, budgets and exact-input reuse. Follow-up research/repair choices are recorded requests, not executed research. A no-gap result is not forecast truth.

From the repository root:

```
python3 scripts/explore_foresight_semantics.py SEED_SESSION.json OUTPUT.json \
  --key-file PRIVATE_KEY_FILE \
  --watch-recording dashboard/static/observatory/live.json
```

Press **Follow local run** while the program is executing. Each event atomically replaces the live snapshot, and the viewer follows it until completion. No credentials enter the viewer. Live files are ignored by Git; remove them before packaging a deployment.

A seed file contains `session_id`, `elapsed_seconds`, `context`, and the completed reasoning Session's `result.result` JSON with eight candidates. Each candidate has named prerequisite nodes, a root, dated claim, mechanism, hypothesis scene, falsifier and early signals. The seed used here was generated through the governed Foresight MCP, with no new research or future factual assertions.

## Build a portable snapshot

```
python3 scripts/export_foresight_observatory.py dashboard/static/observatory observatory.html
```

`build_foresight_recording.py EVIDENCE_DIR RECORDING.json` packages the program trace, frozen forecast snapshot, critic benchmark and original endpoint excerpts. Source files are named in that script. Inputs and outputs are retained in Foundry's `arn518-observatory-evidence.tar.gz`.

## Scope

This is an experimental semantic-program runner and an operational viewer. It does not install a new native exploration engine or change production forecast authority. ADR-012 specifies that production execution must remain Temper-native entities, transitions and WASM. The UI never treats semantic distributions, vividness, human preference or branch count as predictive accuracy.
