import {
  validateRecording,
  accuracy,
  visibleEvents,
  reviewSummary,
} from "./metrics.mjs";
const $ = (s) => document.querySelector(s),
  E = (v) =>
    String(v ?? "").replace(
      /[&<>"']/g,
      (c) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[c],
    );
let data,
  tab = "explore",
  selected = "",
  cursor = 0,
  playing = false,
  speed = 1,
  timer,
  lastTick = 0,
  ledgerPage = 0,
  ledgerFilter = "all",
  search = "",
  dimension = "usefulness",
  pairIndex = 0,
  recorder = null,
  videoChunks = [];
let followTimer = null,
  following = false;
const main = $("#main");
main.innerHTML = '<div class="spinner">Loading the recorded exploration…</div>';
function toast(message) {
  $("#toast").textContent = message;
  $("#toast").style.display = "block";
  setTimeout(() => ($("#toast").style.display = "none"), 4000);
}
function download(name, body, type) {
  const u = URL.createObjectURL(new Blob([body], { type })),
    a = document.createElement("a");
  a.href = u;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(u), 5000);
}
const duration = () => Math.max(1, ...data.events.map((e) => e.at));
const current = () =>
  data.candidates.find((c) => c.id === selected) || data.candidates[0];
const time = (s) =>
  `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
function intro(kicker, title, subtitle) {
  return `<div class="intro"><div><span class="eyebrow">${E(kicker)}</span><h1>${E(title)}</h1><p>${E(subtitle)}</p></div><span class="world-pill">◷ &nbsp; HORIZON / ${E(data.world?.target || "Not specified")}</span></div>`;
}
function stat(label, value, note, accent = false) {
  return `<div class="stat ${accent ? "accent" : ""}"><div class="label">${E(label)}</div><div class="value">${E(value)}</div><small>${E(note)}</small></div>`;
}
function render() {
  if (!data) return;
  document.querySelectorAll("[data-tab]").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === tab);
    b.setAttribute("aria-current", b.dataset.tab === tab ? "page" : "false");
  });
  main.innerHTML =
    tab === "explore"
      ? explore()
      : tab === "compare"
        ? compare()
        : tab === "forecasts"
          ? forecasts()
          : evaluate();
  bind();
  $("#provenance").textContent =
    `${data.world?.cutoff || "Unknown cutoff"} EVIDENCE SNAPSHOT · ${data.provenance?.mode || "IMPORTED RECORDING"}`;
}
function graph() {
  const W = 940,
    H = Math.max(460, data.candidates.length * 67 + 30),
    nodes = [],
    edges = [],
    seen = visibleEvents(data, cursor),
    status = new Map();
  for (const e of seen) if (e.nodeId) status.set(e.nodeId, e);
  const selectedC = current();
  data.candidates.forEach((c, ci) => {
    const y = 30 + ((ci + 0.5) * (H - 50)) / data.candidates.length;
    const positions = new Map(
      c.nodes.map((n, i) => [
        n.id,
        { x: 230 + i * (580 / Math.max(2, c.nodes.length - 1)), y },
      ]),
    );
    for (const [i, n] of c.nodes.entries()) {
      const p = positions.get(n.id),
        s = status.get(n.id),
        highlight = c.id === selectedC?.id,
        fill = !s
          ? "#fcfdf8"
          : s.result === "no_gap_identified"
            ? "#dceab7"
            : s.kind === "semantic"
              ? "#e8edda"
              : "#f4e8d5";
      if (!n.requires.length)
        edges.push(
          `<path class="edge ${highlight ? "highlight" : ""}" d="M98 ${H / 2} C155 ${H / 2} 130 ${y} ${p.x - 81} ${y}"/>`,
        );
      for (const id of n.requires) {
        const src = positions.get(id);
        if (src)
          edges.push(
            `<path class="edge ${highlight ? "highlight" : ""}" d="M${src.x + 82} ${src.y} C${(src.x + p.x) / 2} ${src.y} ${(src.x + p.x) / 2} ${p.y} ${p.x - 82} ${p.y}" marker-end="url(#arrow)"/>`,
          );
      }
      const words = (i === c.nodes.length - 1 ? c.title : n.statement).replace(
        /^By\s+[\d-]+,?\s*/,
        "",
      );
      nodes.push(
        `<g class="node ${highlight ? "" : "dim"}" tabindex="0" role="button" aria-label="Inspect ${E(c.title)}: ${E(n.statement)}" data-candidate="${E(c.id)}" data-node="${E(n.id)}"><title>${E(n.statement)}</title><rect x="${p.x - 82}" y="${p.y - 24}" width="164" height="48" rx="7" fill="${fill}" stroke="${highlight ? "#a8bb86" : "#dce2d3"}"/><circle cx="${p.x - 71}" cy="${p.y - 13}" r="2.5" fill="${s ? "#819958" : "#bec8b1"}"/><text x="${p.x - 63}" y="${p.y - 10}" class="node-sub">${E(i === c.nodes.length - 1 ? "FUTURE " + String(ci + 1).padStart(2, "0") : i === 0 ? "PREREQUISITE" : "MECHANISM")}</text><text x="${p.x - 71}" y="${p.y + 4}" class="node-label">${E(words.slice(0, 24))}</text><text x="${p.x - 71}" y="${p.y + 15}" class="node-label">${E(words.slice(24, 48))}${words.length > 48 ? "…" : ""}</text></g>`,
      );
    }
  });
  return `<svg class="graph" viewBox="0 0 ${W} ${H}" role="img" aria-label="Eight candidate futures and their prerequisite graphs. Click a node to inspect its evidence and checks."><defs><marker id="arrow" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0 0 L7 3.5 L0 7" fill="none" stroke="#9dab8c"/></marker></defs>${edges.join("")}<circle cx="75" cy="${H / 2}" r="24" fill="#293f2c"/><text x="75" y="${H / 2 + 6}" font-size="21" text-anchor="middle" fill="#d9ed8b">✳</text><text x="75" y="${H / 2 + 43}" text-anchor="middle" font-size="9" fill="#66755a">OBSERVED</text><text x="75" y="${H / 2 + 55}" text-anchor="middle" font-size="8" fill="#849078">WORLD</text>${nodes.join("")}</svg>`;
}
function inspector() {
  const c = current();
  if (!c) return '<p class="empty">No candidates in this recording.</p>';
  const events = visibleEvents(data, cursor).filter(
      (e) => e.candidateId === c.id,
    ),
    gap = events.filter((e) => e.function === "classify_gap"),
    novelty = events.find((e) => e.function === "compare_mechanism");
  return `<div class="inner"><div class="badge-row"><span class="badge">HYPOTHETICAL FUTURE</span><span class="badge orange">${events.length ? "UNDER EXAMINATION" : "AWAITING REPLAY"}</span></div><h2>${E(c.title)}</h2><p class="claim">${E(c.claim)}</p><div class="scene">${E(c.scene)}</div><div class="section-label">The mechanism</div><p class="detail-text">${E(c.mechanism)}</p><div class="section-label">What would we see first?</div>${(c.signals || []).map((s) => `<div class="signal">${E(s)}</div>`).join("")}<div class="section-label">What would prove it wrong?</div><p class="detail-text">${E(c.falsifier)}</p><div class="checks"><div class="check-row"><span>Mechanism distinction</span><span class="check-result">${E(novelty?.result || "Not replayed")}</span></div><div class="check-row"><span>Prerequisites inspected</span><span>${new Set(gap.map((e) => e.nodeId)).size} / ${c.nodes.length}</span></div><div class="check-row"><span>Forecast probability</span><span class="check-result">Not assigned</span></div></div><button data-evidence="${E(c.id)}" style="width:100%;margin-top:16px">Inspect full decision trace ↗</button><p class="small">Semantic judgments identify questions to investigate. They do not establish this future’s likelihood.</p></div>`;
}
function explore() {
  const scored = accuracy(data.forecasts, data.asOf);
  return (
    intro(
      "EXPLORE / ENTERPRISE AUTONOMY",
      "Watch possibilities become questions.",
      "Follow a recorded program as semantic functions compare futures, descend into prerequisites, and identify where evidence or reasoning is still needed.",
    ) +
    `<div class="stats">${stat("Candidate futures", data.candidates.length, "Generated hypotheses, not validated worlds")}${stat("Semantic function calls", data.run?.providerCalls ?? "—", "Actual recorded Jev calls")}${stat("Recursive depth", Math.max(0, ...data.events.map((e) => e.depth || 0)) + 1, "Levels of causal prerequisites")}${stat("Resolved predictions", scored.n, scored.n ? "Eligible for accuracy scoring" : "Accuracy is not yet measurable", true)}</div><div class="workspace"><div><section class="map-panel"><div class="panel-header"><h3>Possibility space <span class="small"> / ${data.candidates.length} branches</span></h3><button id="follow-live" ${location.protocol === "file:" ? 'disabled title="Live observation requires serving this page over localhost or HTTPS"' : ""} style="font-size:10px;padding:6px 8px">${following ? "Stop following" : "Follow local run"}</button><div class="legend"><span>Inspected</span><span class="uncertain">Open question</span><span class="unseen">Not replayed</span></div></div><div class="map-wrap" id="graph-wrap">${graph()}<span class="map-note">Click a node to follow its world · Edges are proposed prerequisites</span></div><div class="playback"><button id="play" aria-label="${playing ? "Pause" : "Play"} exploration">${playing ? "Ⅱ" : "▶"}</button><button id="restart" aria-label="Restart replay" style="background:transparent;color:#58694c">↺</button><input id="scrub" aria-label="Replay position" type="range" min="0" max="${duration()}" step="0.1" value="${cursor}"><span class="time" id="time">${time(cursor)} / ${time(duration())}</span><select id="speed" aria-label="Playback speed">${[1, 2, 4, 8].map((n) => `<option value="${n}" ${speed === n ? "selected" : ""}>${n}× replay</option>`).join("")}</select></div></section><section class="panel trace"><div class="panel-header"><h3>Inside the program</h3><span class="eyebrow" id="event-count">${visibleEvents(data, cursor).length} / ${data.events.length} EVENTS</span></div><div class="trace-list" id="trace-list">${trace()}</div></section></div><aside class="inspector" id="inspector">${inspector()}</aside></div>`
  );
}
function trace() {
  const events = visibleEvents(data, cursor).slice(-7).reverse();
  return events.length
    ? events
        .map(
          (e) =>
            `<div class="trace-row" tabindex="0" role="button" data-event="${data.events.indexOf(e)}"><time>${time(e.at)}</time><span>${"↳ ".repeat(Math.min(3, e.depth || 0))}<code>${E(e.function)}()</code><span style="color:#88927c"> · ${E(e.candidateId)}</span></span><span class="result">${E(e.result.replaceAll("_", " "))}</span></div>`,
        )
        .join("")
    : '<p class="empty">Press play to replay the actual semantic calls and recursive returns. No models run during playback.</p>';
}
function compare() {
  const b = data.benchmark;
  if (!b)
    return intro(
      "COMPARE",
      "No comparison attached.",
      "Import a recording with a measured baseline and candidate run.",
    );
  return (
    intro(
      "COMPARE / MEASURED, NOT ASSUMED",
      "Does the new program earn its complexity?",
      "Keep the measured critic experiment separate from the new recursive exploration. A faster check is not evidence of better predictions.",
    ) +
    `<div class="stats">${stat("Matched paths", b.completed_pairs, "Same frozen evidence in both arms")}${stat("Mean time saved", b.time_reduction_percent.toFixed(1) + "%", "Critic stage only")}${stat("Baseline findings retained", b.shared_findings + " / " + b.gpt_findings, "Agreement is not ground-truth accuracy")}${stat("Full-pipeline quality gain", "Not measured", "Needs an equal-budget prospective trial", true)}</div><div class="grid-two"><section class="panel"><div class="panel-header"><h3>Completed experiment · review time</h3><span class="badge">22 PAIRED PATHS</span></div><div class="panel-body"><div class="compare-bars">${[
      ["Full GPT review", b.gpt_mean_seconds, false],
      ["Jev → selective GPT", b.selective_mean_seconds, true],
    ]
      .map(
        ([label, value, c]) =>
          `<div><div class="bar-label"><span>${label}</span><b>${value.toFixed(1)} s</b></div><div class="bar-track"><div class="bar-fill ${c ? "candidate" : ""}" style="width:${(value / Math.max(b.gpt_mean_seconds, b.selective_mean_seconds)) * 100}%"></div></div></div>`,
      )
      .join(
        "",
      )}</div><div class="metric-list"><div><b>${b.skipped_checks} / ${b.checks}</b><span>Category checks handled by Jev</span></div><div><b>${b.transitions_requiring_gpt} / 171</b><span>Transitions that still needed GPT</span></div></div><div class="callout">${b.skipped_baseline_findings} baseline findings were lost on skipped checks. The remaining differences occurred on checks routed to GPT. Both arms passed strict output checks on 20 of 22 reviews.</div></div></section><section class="panel"><div class="panel-header"><h3>New experiment · composed functions</h3><span class="badge orange">EXPLORATORY</span></div><div class="panel-body"><h2>${data.candidates.length} alternative futures.<br>A recursive question tree.</h2><div class="metric-list"><div><b>${Math.round(data.run?.seedSeconds || 0)} s</b><span>One reasoning session proposing the forest</span></div><div><b>${(data.run?.semanticSeconds || 0).toFixed(1)} s</b><span>Semantic program execution</span></div><div><b>${data.run?.providerCalls ?? "—"}</b><span>Jev calls across named functions</span></div><div><b>${data.run?.cacheHits ?? 0}</b><span>Recorded cache reuses</span></div></div><div class="callout warning">These are new hypotheses, not independently verified discoveries. The recursive run has no matched full-pipeline baseline. Research and repair operations in the trace are recommendations, not completed work.</div><p class="small">Next comparison: fixed time and reasoning budget; distinct useful mechanisms; blind reviews; audited rejections; later outcome scoring. No claim of a 10× improvement is supported yet.</p></div></section></div>`
  );
}
function forecasts() {
  const score = accuracy(data.forecasts, data.asOf),
    filtered = data.forecasts.filter(
      (f) =>
        (ledgerFilter === "all" ||
          (ledgerFilter === "resolved" ? eligible(f) : !eligible(f))) &&
        f.question.toLowerCase().includes(search.toLowerCase()),
    );
  const pages = Math.max(1, Math.ceil(filtered.length / 10));
  ledgerPage = Math.min(ledgerPage, pages - 1);
  return (
    intro(
      "FORECAST LEDGER / ACCOUNTABILITY",
      "Reality gets the final vote.",
      "Probabilities belong to registered forecasts. Semantic-function scores never become probabilities that a future will happen.",
    ) +
    `<div class="stats">${stat("Registered forecasts", data.forecasts.length, "From the recorded Foresight world")}${stat("Eligible resolved events", score.n, "Observed outcomes with dated evidence")}${stat("Mean Brier score", score.brier === null ? "—" : score.brier.toFixed(3), "Lower is better · 0 is perfect")}${stat("Accuracy improvement", "Unknown", "Unresolved predictions cannot prove accuracy", true)}</div><div class="toolbar"><input id="search" aria-label="Search forecasts" placeholder="Search predictions, actors, or signals…" value="${E(search)}"><div class="filters">${["all", "pending", "resolved"].map((s) => `<button data-filter="${s}" class="${s === ledgerFilter ? "active" : ""}">${s[0].toUpperCase() + s.slice(1)}</button>`).join("")}</div></div><section class="panel"><div class="table-wrap"><table><thead><tr><th>PREDICTION / RESOLUTION CRITERION</th><th>PROBABILITY</th><th>RESOLVE BY</th><th>OUTCOME</th></tr></thead><tbody>${
      filtered
        .slice(ledgerPage * 10, (ledgerPage + 1) * 10)
        .map(
          (f) =>
            `<tr><td class="question">${E(f.question)}<div class="small">Registered ${E(f.registeredAt?.slice(0, 10) || "unknown")} · ${E(f.id)}</div></td><td><span class="prob">${f.probability === null ? "—" : Math.round(f.probability * 100) + "%"}</span></td><td style="white-space:nowrap">${E(f.resolveBy || "Unknown")}</td><td><span class="badge ${eligible(f) ? "" : "orange"}">${eligible(f) ? E(f.outcome) : "Pending / unscored"}</span></td></tr>`,
        )
        .join("") ||
      '<tr><td colspan="4">No forecasts match this filter.</td></tr>'
    }</tbody></table></div><div class="ledger-pager"><span>${filtered.length} forecasts · Page ${ledgerPage + 1} of ${pages}</span><div><button id="prev-page" ${ledgerPage === 0 ? "disabled" : ""}>← Previous</button> <button id="next-page" ${ledgerPage >= pages - 1 ? "disabled" : ""}>Next →</button></div></div></section><div class="callout">As of ${E(data.asOf?.slice(0, 10) || "unknown")}. Scoring requires an observed, verified outcome with a source URL, a resolution timestamp after registration and deadline, and a timestamp no later than the recording’s as-of date. Simulated outcomes and semantic confidence are excluded. Revisions count once per event.</div>`
  );
}
function eligible(f) {
  return accuracy([f], data.asOf).n > 0;
}
function pair() {
  const candidates = data.candidates;
  if (candidates.length < 2) return null;
  const a = data.baseline?.candidates?.length
      ? data.baseline.candidates[pairIndex % data.baseline.candidates.length]
      : candidates[pairIndex % candidates.length],
    b = data.baseline?.candidates?.length
      ? candidates[pairIndex % candidates.length]
      : candidates[(pairIndex + 1) % candidates.length];
  const aa = { ...a, _runId: data.baseline?.id || data.id },
    bb = { ...b, _runId: data.id };
  return pairIndex % 2 ? [bb, aa] : [aa, bb];
}
function evaluate() {
  const p = pair(),
    summary = reviewSummary(data.reviews || []);
  const preference = (d) => {
    const rs = (data.reviews || []).filter(
      (r) =>
        r.dimension === d &&
        r.kind === "human_cross_run_preference" &&
        r.preference !== "tie" &&
        r.candidateRun === data.id &&
        r.baselineRun === data.baseline?.id &&
        r.aRun &&
        r.bRun,
    );
    const wins = rs.filter(
      (r) => (r.preference === "a" ? r.aRun : r.bRun) === data.id,
    ).length;
    return rs.length ? wins + " / " + rs.length : "Not rated";
  };
  if (!p)
    return intro(
      "EVALUATION",
      "Two candidates are needed.",
      "Import a recording with at least two candidates.",
    );
  return (
    intro(
      "EVALUATION LAB / HUMAN JUDGMENT",
      "Useful. Vivid. And eventually, accurate.",
      "Judge candidate futures side by side without their titles or model assessments. These reviews measure human preference, not predictive accuracy.",
    ) +
    `<div class="stats">${stat("Local review judgments", summary.n, "Included in exported recordings")}${stat("Usefulness preference", preference("usefulness"), "Your candidate-over-baseline choices · ties excluded")}${stat("Vividness preference", preference("vividness"), "Your candidate-over-baseline choices · ties excluded")}${stat("Predictive accuracy", "Separate ledger", "Vivid prose earns no accuracy credit", true)}</div><div class="dimension"><button data-dimension="usefulness" class="${dimension === "usefulness" ? "active" : ""}">Usefulness</button><button data-dimension="vividness" class="${dimension === "vividness" ? "active" : ""}">Vividness</button><span class="progress-dots" style="margin:auto 0 auto auto">PAIR ${pairIndex + 1} · ${data.baseline ? "CROSS-RUN COMPARISON" : "CANDIDATE VS CANDIDATE"}</span></div><div class="grid-two">${p.map((c, i) => `<article class="panel evaluation-card"><span class="eyebrow">FUTURE ${i ? "B" : "A"}</span><h3>${E(c.claim)}</h3><p class="detail-text">${E(c.mechanism)}</p><div class="scene">${E(c.scene)}</div><div class="section-label">What would falsify it?</div><p class="detail-text">${E(c.falsifier)}</p></article>`).join("")}</div><div class="callout">${dimension === "usefulness" ? "Which future gives you a more consequential, distinct and evidence-grounded mechanism to investigate?" : "Which future makes actors, timing and real-world consequences more concrete without relying on unsupported detail?"}</div><textarea id="review-note" class="rating-note" placeholder="Why? Note the decision this informs, or the unsupported detail you noticed." aria-label="Review rationale" style="margin-top:16px"></textarea><div class="vote-buttons"><button data-vote="a">Prefer A</button><button data-vote="tie">Tie / neither</button><button data-vote="b">Prefer B</button><button id="skip-pair">Skip →</button></div><button id="load-baseline">Import baseline recording ↑</button><p class="small">${data.baseline ? data.comparisonLimitations || "Comparing two imported runs with labels concealed. Budget equality and matched questions must be verified separately." : "This is exploratory candidate review. Import a baseline recording to compare runs with their labels concealed."} Reviews stay in this page until you export them. Accuracy scoring remains independent.</p>`
  );
}
function openEvent(index) {
  const e = data.events[index];
  if (!e) return;
  const c = data.candidates.find((c) => c.id === e.candidateId),
    n = c?.nodes.find((n) => n.id === e.nodeId),
    call = data.calls?.[e.callIndex];
  let d = document.createElement("dialog");
  d.className = "detail-dialog";
  d.innerHTML = `<form method="dialog"><button class="close" aria-label="Close detail">×</button></form><span class="eyebrow">${E(e.kind || "EVENT")} · DEPTH ${E(e.depth)}</span><h2>${E(e.function)}()</h2><p>${E(n?.statement || c?.claim)}</p><div class="callout">Returned: <b>${E(e.result)}</b>${e.seconds !== undefined ? " · " + e.seconds.toFixed(3) + " s" : ""}<br>These choice scores describe a semantic classification, not a forecast probability.</div>${e.distribution ? `<pre class="evidence-pre">${E(JSON.stringify(e.distribution, null, 2))}</pre>` : ""}<details><summary style="margin-top:18px;cursor:pointer">Exact function input and output</summary><pre class="evidence-pre">${E(JSON.stringify(call || e, null, 2))}</pre></details><p class="small">${E(e.requestHash ? "Request SHA-256: " + e.requestHash : "Deterministic traversal or bookkeeping; no model call.")}</p>`;
  document.body.appendChild(d);
  d.addEventListener("close", () => d.remove());
  d.showModal();
}
function evidence(id) {
  const c = data.candidates.find((c) => c.id === id);
  if (!c) return;
  let d = document.createElement("dialog");
  d.className = "detail-dialog";
  d.innerHTML = `<form method="dialog"><button class="close" aria-label="Close evidence">×</button></form><span class="eyebrow">HYPOTHESIS / ${E(c.id)}</span><h2>${E(c.title)}</h2><p>${E(c.assumption)}</p>${c.nodes.map((n) => `<div class="section-label">${E(n.id)} · ${n.requires.length ? "Requires " + E(n.requires.join(", ")) : "Initial prerequisite"}</div><p class="detail-text">${E(n.statement)}</p><p class="small">${E(n.evidence_note)}</p>`).join("")}<details><summary style="margin-top:20px;cursor:pointer">Source evidence snapshot</summary><pre class="evidence-pre">${E(data.evidence?.observed_graph || "No source snapshot attached.")}</pre></details><p class="small">Evidence notes are generated explanations, not independent verification of the hypothesis.</p>`;
  document.body.appendChild(d);
  d.addEventListener("close", () => d.remove());
  d.showModal();
}
function bindGraph() {
  document.querySelectorAll("[data-candidate]").forEach((n) => {
    const pick = () => {
      selected = n.dataset.candidate;
      updatePlayback();
    };
    n.onclick = pick;
    n.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        pick();
      }
    };
  });
  document.querySelectorAll("[data-event]").forEach((n) => {
    n.onclick = () => openEvent(Number(n.dataset.event));
    n.onkeydown = (e) => {
      if (e.key === "Enter") openEvent(Number(n.dataset.event));
    };
  });
  document
    .querySelectorAll("[data-evidence]")
    .forEach((n) => (n.onclick = () => evidence(n.dataset.evidence)));
}
function updatePlayback() {
  if (tab !== "explore") return;
  $("#graph-wrap").innerHTML =
    graph() +
    '<span class="map-note">Click a node to follow its world · Edges are proposed prerequisites</span>';
  $("#inspector").innerHTML = inspector();
  $("#trace-list").innerHTML = trace();
  $("#event-count").textContent =
    `${visibleEvents(data, cursor).length} / ${data.events.length} EVENTS`;
  $("#scrub").value = cursor;
  $("#time").textContent = time(cursor) + " / " + time(duration());
  $("#play").textContent = playing ? "Ⅱ" : "▶";
  $("#play").setAttribute(
    "aria-label",
    playing ? "Pause exploration" : "Play exploration",
  );
  bindGraph();
}
function stop() {
  playing = false;
  clearInterval(timer);
  timer = null;
  if (following) {
    following = false;
    clearTimeout(followTimer);
    document.querySelector(".recorded").textContent = "Recorded experiment";
  }
}
function play() {
  if (playing) {
    stop();
    updatePlayback();
    return;
  }
  if (cursor >= duration()) cursor = 0;
  playing = true;
  lastTick = performance.now();
  timer = setInterval(() => {
    const now = performance.now();
    cursor = Math.min(duration(), cursor + ((now - lastTick) / 1000) * speed);
    lastTick = now;
    const last = visibleEvents(data, cursor).at(-1);
    if (last) selected = last.candidateId;
    if (cursor >= duration()) stop();
    updatePlayback();
  }, 150);
  updatePlayback();
}
async function followLive() {
  if (following) {
    following = false;
    clearTimeout(followTimer);
    render();
    return;
  }
  stop();
  following = true;
  async function poll() {
    try {
      const response = await fetch("live.json?t=" + Date.now(), {
        cache: "no-store",
      });
      if (!response.ok)
        throw Error(
          "No local live experiment is connected. Start the semantic program with --watch-recording pointing to live.json.",
        );
      const next = validateRecording(await response.json());
      if (!following) return;
      data = next;
      cursor = duration();
      selected = data.events.at(-1)?.candidateId || data.candidates[0]?.id;
      tab = "explore";
      if (data.provenance?.complete) following = false;
      render();
      document.querySelector(".recorded").textContent = following
        ? "● Live experiment"
        : "✓ Experiment completed";
      if (following) followTimer = setTimeout(poll, 500);
    } catch (err) {
      following = false;
      clearTimeout(followTimer);
      render();
      toast(err.message);
    }
  }
  poll();
}
function bind() {
  bindGraph();
  if ($("#follow-live")) $("#follow-live").onclick = followLive;
  if ($("#load-baseline"))
    $("#load-baseline").onclick = () => {
      $("#file-input").dataset.mode = "baseline";
      $("#file-input").click();
    };
  if ($("#play")) {
    $("#play").onclick = play;
    $("#restart").onclick = () => {
      stop();
      cursor = 0;
      selected = data.candidates[0]?.id;
      updatePlayback();
    };
    $("#scrub").oninput = (e) => {
      stop();
      cursor = Number(e.target.value);
      updatePlayback();
    };
    $("#speed").onchange = (e) => {
      speed = Number(e.target.value);
    };
  }
  if ($("#search"))
    $("#search").oninput = (e) => {
      const pos = e.target.selectionStart;
      search = e.target.value;
      ledgerPage = 0;
      render();
      $("#search").focus();
      $("#search").setSelectionRange(pos, pos);
    };
  document.querySelectorAll("[data-filter]").forEach(
    (b) =>
      (b.onclick = () => {
        ledgerFilter = b.dataset.filter;
        ledgerPage = 0;
        render();
      }),
  );
  if ($("#prev-page"))
    $("#prev-page").onclick = () => {
      ledgerPage--;
      render();
    };
  if ($("#next-page"))
    $("#next-page").onclick = () => {
      ledgerPage++;
      render();
    };
  document.querySelectorAll("[data-dimension]").forEach(
    (b) =>
      (b.onclick = () => {
        dimension = b.dataset.dimension;
        render();
      }),
  );
  document.querySelectorAll("[data-vote]").forEach(
    (b) =>
      (b.onclick = () => {
        const p = pair();
        data.reviews ??= [];
        data.reviews.push({
          dimension,
          preference: b.dataset.vote,
          a: p[0].id,
          b: p[1].id,
          aRun: p[0]._runId,
          bRun: p[1]._runId,
          pairIndex,
          note: $("#review-note").value,
          at: new Date().toISOString(),
          kind: data.baseline
            ? "human_cross_run_preference"
            : "human_candidate_preference",
          baselineRun: data.baseline?.id || null,
          candidateRun: data.id,
        });
        pairIndex++;
        render();
        toast("Review saved in this page. Export to keep it.");
      }),
  );
  if ($("#skip-pair"))
    $("#skip-pair").onclick = () => {
      pairIndex++;
      render();
    };
}
document.querySelectorAll("[data-tab]").forEach(
  (b) =>
    (b.onclick = () => {
      stop();
      tab = b.dataset.tab;
      history.replaceState(null, "", "#" + tab);
      render();
    }),
);
$("#present").onclick = () => {
  document.body.classList.toggle("present");
  toast(
    document.body.classList.contains("present")
      ? "Presentation mode · press Escape to exit"
      : "Workspace controls restored",
  );
};
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") document.body.classList.remove("present");
  if (
    e.code === "Space" &&
    tab === "explore" &&
    !["INPUT", "TEXTAREA", "BUTTON", "SELECT"].includes(
      document.activeElement?.tagName,
    ) &&
    !document.querySelector("dialog[open]")
  ) {
    e.preventDefault();
    play();
  }
});
$("#share").onclick = () => $("#export-dialog").showModal();
$("#export-json").onclick = () =>
  download(
    "foresight-recording.json",
    JSON.stringify(data, null, 2),
    "application/json",
  );
$("#export-svg").onclick = () =>
  download(
    "foresight-graph.svg",
    graph()
      .replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" ')
      .replace(
        "<defs>",
        "<style>text{font-family:Arial,sans-serif}.edge{fill:none;stroke:#c4d0b8;stroke-width:1.2}.edge.highlight{stroke:#8fa658;stroke-width:2}.node-label{font-size:10px;fill:#34412e}.node-sub{font-size:7px;fill:#7e8975}.dim{opacity:.72}</style><defs>",
      ),
    "image/svg+xml",
  );
$("#import").onclick = () => {
  $("#file-input").dataset.mode = "recording";
  $("#file-input").click();
};
$("#file-input").onchange = async (e) => {
  try {
    const f = e.target.files[0];
    if (!f) return;
    if (f.size > 12 * 1024 * 1024) throw Error("Recording exceeds 12 MB.");
    const next = validateRecording(JSON.parse(await f.text()));
    stop();
    if (e.target.dataset.mode === "baseline") {
      if (!next.id || next.id === data.id)
        throw Error("Baseline must have a different run ID.");
      delete next.baseline;
      data.baseline = next;
      tab = "evaluate";
    } else {
      data = next;
    }
    selected = data.candidates[0]?.id;
    cursor = 0;
    ledgerPage = 0;
    pairIndex = 0;
    $("#export-dialog").close();
    render();
    toast("Recording imported.");
  } catch (err) {
    $("#export-status").textContent = err.message;
  } finally {
    e.target.value = "";
  }
};
$("#export-html").onclick = async () => {
  try {
    const packed = window.__OBSERVATORY_ASSETS__ || {
      css: await (await fetch("observatory.css")).text(),
      js: await (await fetch("observatory.js")).text(),
      metrics: await (await fetch("metrics.mjs")).text(),
    };
    let html = document.documentElement.cloneNode(true);
    html.querySelectorAll("script").forEach((s) => s.remove());
    html.querySelectorAll("dialog").forEach((d) => d.removeAttribute("open"));
    html.querySelector("body").classList.remove("present");
    html.querySelector("#toast").style.display = "none";
    html.querySelector("link[rel=stylesheet]")?.remove();
    const style = document.createElement("style");
    style.textContent = packed.css;
    html.querySelector("head").append(style);
    const script = document.createElement("script");
    script.type = "module";
    script.textContent =
      "window.__OBSERVATORY_RECORDING__=" +
      JSON.stringify(data).replace(/</g, "\\u003c") +
      ";\nwindow.__OBSERVATORY_ASSETS__=" +
      JSON.stringify(packed).replace(/</g, "\\u003c") +
      ";\n" +
      packed.metrics.replaceAll("export function", "function") +
      "\n" +
      packed.js.replace(
        /^import[\s\S]*?from\s*['"]\.\/metrics\.mjs['"];?\s*/,
        "",
      );
    html.querySelector("body").append(script);
    download(
      "foresight-observatory.html",
      "<!doctype html>\n" + html.outerHTML,
      "text/html",
    );
    toast("Interactive snapshot exported.");
  } catch (err) {
    $("#export-status").textContent = "Export failed: " + err.message;
  }
};
$("#capture").onclick = async () => {
  if (recorder?.state === "recording") {
    recorder.stop();
    return;
  }
  try {
    if (!navigator.mediaDevices?.getDisplayMedia)
      throw Error(
        "Screen capture is unavailable here. Open this page on localhost or HTTPS, or use your device’s screen recorder.",
      );
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: { frameRate: 30 },
      audio: false,
    });
    videoChunks = [];
    recorder = new MediaRecorder(stream);
    recorder.ondataavailable = (e) => {
      if (e.data.size) videoChunks.push(e.data);
    };
    recorder.onstop = () => {
      download(
        "foresight-exploration.webm",
        new Blob(videoChunks, { type: recorder.mimeType }),
        recorder.mimeType,
      );
      stream.getTracks().forEach((t) => t.stop());
      $("#capture").textContent = "Record screen video ●";
      toast("Screen recording downloaded.");
    };
    stream.getVideoTracks()[0].onended = () => {
      if (recorder.state === "recording") recorder.stop();
    };
    recorder.start();
    $("#capture").textContent = "Stop recording ■";
    $("#export-dialog").close();
    document.body.classList.add("present");
    toast("Recording. Use the browser’s Stop sharing control to finish.");
  } catch (err) {
    $("#export-status").textContent = err.message;
  }
};
try {
  data = validateRecording(
    window.__OBSERVATORY_RECORDING__ ||
      (await (await fetch("recording.json")).json()),
  );
  selected = data.candidates[0]?.id;
  const hash = location.hash.slice(1);
  if (["explore", "compare", "forecasts", "evaluate"].includes(hash))
    tab = hash;
  render();
} catch (err) {
  main.innerHTML = `<div class="input-error"><h2>Could not open this recording.</h2><p>${E(err.message)}</p><p>Use Export → Import recording, or open the self-contained HTML export.</p></div>`;
}
