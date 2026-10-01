export function validateRecording(v) {
  if (
    !v ||
    typeof v.id !== "string" ||
    !v.id ||
    v.schema !== "foresight-observatory-v1" ||
    !Array.isArray(v.candidates) ||
    !Array.isArray(v.events) ||
    !Array.isArray(v.forecasts)
  )
    throw Error("Expected a Foresight Observatory v1 recording.");
  if (
    v.candidates.length > 200 ||
    v.events.length > 5000 ||
    v.forecasts.length > 2000
  )
    throw Error("Recording exceeds display limits.");
  const ids = new Set();
  for (const c of v.candidates) {
    if (
      typeof c.id !== "string" ||
      ids.has(c.id) ||
      !Array.isArray(c.nodes) ||
      c.nodes.length > 100 ||
      typeof c.title !== "string"
    )
      throw Error("Invalid or duplicate candidate.");
    for (const key of ["claim", "mechanism", "scene", "falsifier"])
      if (typeof c[key] !== "string")
        throw Error("Missing candidate description.");
    if (
      !Array.isArray(c.signals) ||
      c.signals.some((s) => typeof s !== "string")
    )
      throw Error("Invalid candidate signals.");
    ids.add(c.id);
    const ns = new Set(c.nodes.map((n) => n.id));
    if (ns.size !== c.nodes.length || !ns.has(c.root))
      throw Error("Invalid candidate graph.");
    for (const n of c.nodes)
      if (
        typeof n.id !== "string" ||
        typeof n.statement !== "string" ||
        !Array.isArray(n.requires) ||
        n.requires.some((id) => !ns.has(id))
      )
        throw Error("Invalid prerequisite edge.");
  }
  for (const e of v.events)
    if (
      !Number.isFinite(e.at) ||
      e.at < 0 ||
      typeof e.function !== "string" ||
      typeof e.result !== "string" ||
      (e.depth !== undefined &&
        (!Number.isInteger(e.depth) || e.depth < 0 || e.depth > 20)) ||
      !ids.has(e.candidateId)
    )
      throw Error("Invalid trace event.");
  if (v.candidates.reduce((n, c) => n + c.nodes.length, 0) > 1500)
    throw Error("Too many nodes.");
  if (v.run)
    for (const k of [
      "seedSeconds",
      "semanticSeconds",
      "providerCalls",
      "cacheHits",
    ])
      if (
        v.run[k] !== undefined &&
        (typeof v.run[k] !== "number" ||
          !Number.isFinite(v.run[k]) ||
          v.run[k] < 0)
      )
        throw Error("Invalid run measurement.");
  if (v.benchmark)
    for (const k of [
      "completed_pairs",
      "time_reduction_percent",
      "shared_findings",
      "gpt_findings",
      "gpt_mean_seconds",
      "selective_mean_seconds",
      "skipped_checks",
      "checks",
      "transitions_requiring_gpt",
      "skipped_baseline_findings",
    ])
      if (
        typeof v.benchmark[k] !== "number" ||
        !Number.isFinite(v.benchmark[k])
      )
        throw Error("Invalid comparison measurement.");
  if (v.reviews !== undefined && !Array.isArray(v.reviews))
    throw Error("Invalid reviews.");
  for (const f of v.forecasts) {
    for (const k of ["registeredAt", "resolveBy", "resolvedAt"])
      if (f[k] !== undefined && f[k] !== null && typeof f[k] !== "string")
        throw Error("Invalid forecast date.");
    if (
      typeof f.id !== "string" ||
      typeof f.question !== "string" ||
      (f.probability !== null &&
        (typeof f.probability !== "number" ||
          !Number.isFinite(f.probability) ||
          f.probability < 0 ||
          f.probability > 1))
    )
      throw Error("Invalid forecast.");
  }
  if (v.baseline) validateRecording({ ...v.baseline, baseline: undefined });
  return v;
}
export function eligibleForecast(f, asOf = new Date().toISOString()) {
  const date = (s) => typeof s === "string" && Number.isFinite(Date.parse(s));
  return (
    typeof f.probability === "number" &&
    Number.isFinite(f.probability) &&
    f.probability >= 0 &&
    f.probability <= 1 &&
    ["yes", "no"].includes(f.outcome) &&
    f.evidenceKind === "observed" &&
    f.outcomeEvidenceKind === "observed" &&
    f.outcomeVerified === true &&
    Array.isArray(f.outcomeSources) &&
    f.outcomeSources.some((s) => /^https?:\/\//.test(s)) &&
    [f.registeredAt, f.resolveBy, f.resolvedAt, asOf].every(date) &&
    Date.parse(f.registeredAt) < Date.parse(f.resolvedAt) &&
    Date.parse(f.registeredAt) < Date.parse(f.resolveBy) &&
    Date.parse(f.resolveBy) <= Date.parse(f.resolvedAt) &&
    Date.parse(f.resolvedAt) <= Date.parse(asOf)
  );
}
export function accuracy(forecasts, asOf) {
  // One latest preregistered revision per event prevents revision counting from inflating N.
  const latest = new Map();
  for (const f of forecasts) {
    if (!eligibleForecast(f, asOf)) continue;
    const key = f.eventId || f.id,
      previous = latest.get(key);
    if (!previous || f.registeredAt > previous.registeredAt) latest.set(key, f);
  }
  const rows = [...latest.values()];
  const bins = Array.from({ length: 5 }, (_, i) => ({
    low: i / 5,
    high: (i + 1) / 5,
    n: 0,
    p: 0,
    y: 0,
  }));
  for (const f of rows) {
    const b = bins[Math.min(4, Math.floor(f.probability * 5))];
    b.n++;
    b.p += f.probability;
    b.y += Number(f.outcome === "yes");
  }
  return {
    n: rows.length,
    brier: rows.length
      ? rows.reduce(
          (s, f) => s + (f.probability - Number(f.outcome === "yes")) ** 2,
          0,
        ) / rows.length
      : null,
    bins: bins.map((b) => ({
      ...b,
      p: b.n ? b.p / b.n : null,
      y: b.n ? b.y / b.n : null,
    })),
  };
}
export function visibleEvents(recording, cursor) {
  return recording.events
    .filter((e) => e.at <= cursor)
    .sort((a, b) => a.at - b.at);
}
export function reviewSummary(reviews) {
  const valid = reviews.filter(
    (r) =>
      r &&
      ["a", "b", "tie"].includes(r.preference) &&
      ["usefulness", "vividness"].includes(r.dimension),
  );
  return {
    n: valid.length,
    a: valid.filter((r) => r.preference === "a").length,
    b: valid.filter((r) => r.preference === "b").length,
    ties: valid.filter((r) => r.preference === "tie").length,
  };
}
