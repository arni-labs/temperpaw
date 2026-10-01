import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  accuracy,
  eligibleForecast,
  validateRecording,
  visibleEvents,
} from "../static/observatory/metrics.mjs";
const forecast = {
  id: "f",
  eventId: "e",
  probability: 0.8,
  question: "Will the event happen?",
  registeredAt: "2025-01-01",
  resolveBy: "2025-06-01",
  resolvedAt: "2025-06-02",
  outcome: "yes",
  evidenceKind: "observed",
  outcomeEvidenceKind: "observed",
  outcomeVerified: true,
  outcomeSources: ["https://example.org/outcome"],
};
test("Brier uses resolved observed evidence and does not turn pending into failures", () => {
  assert.equal(accuracy([], "2026-01-01").brier, null);
  assert.equal(accuracy([{ ...forecast, outcome: "" }], "2026-01-01").n, 0);
  assert.ok(Math.abs(accuracy([forecast], "2026-01-01").brier - 0.04) < 1e-10);
});
test("reject future resolutions, hindsight, simulated evidence and invalid probabilities", () => {
  for (const f of [
    { ...forecast, resolvedAt: "2027-01-01" },
    { ...forecast, registeredAt: "2025-06-03" },
    { ...forecast, evidenceKind: "simulated" },
    { ...forecast, outcomeVerified: false },
    { ...forecast, outcomeSources: [] },
    { ...forecast, probability: NaN },
    { ...forecast, probability: 1.01 },
  ])
    assert.equal(eligibleForecast(f, "2026-01-01"), false);
});
test("one latest eligible revision per event; legitimate zero remains zero", () => {
  const score = accuracy(
    [
      forecast,
      { ...forecast, id: "f2", registeredAt: "2025-02-01", probability: 1 },
      { ...forecast, id: "f3", eventId: "e2", probability: 0, outcome: "no" },
    ],
    "2026-01-01",
  );
  assert.equal(score.n, 2);
  assert.equal(score.brier, 0);
});
test("recording preserves unresolved real forecasts and bounded actual trace", () => {
  const r = validateRecording(
    JSON.parse(
      readFileSync(
        new URL("../static/observatory/recording.json", import.meta.url),
      ),
    ),
  );
  assert.equal(r.candidates.length, 8);
  assert.equal(r.run.providerCalls, 40);
  assert.equal(r.events.length, 88);
  assert.equal(accuracy(r.forecasts, r.asOf).n, 0);
  assert.equal(visibleEvents(r, -1).length, 0);
  assert.equal(visibleEvents(r, Infinity).length, 88);
});
test("malformed recording and dangling edges are rejected before rendering", () => {
  assert.throws(() => validateRecording({ schema: "other" }));
  const r = JSON.parse(
    readFileSync(
      new URL("../static/observatory/recording.json", import.meta.url),
    ),
  );
  r.candidates[0].nodes[0].requires = ["missing"];
  assert.throws(() => validateRecording(r), /edge/);
});
