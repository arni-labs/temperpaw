// Foresight — Cedar Authorization Policies (corridor engine, 0.2)
//
// Legacy 0.1 retirement (ADR-003): the five old entity types persist in
// installed stores as inert residue. Reads stay open everywhere; mutation is
// system-principals-only so an image rollback (which restores the old
// bundle's WASM) keeps old system flows functional while sessions, agents,
// and humans can no longer drive the retired machinery.

permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is ForesightModel
);
permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Projection
);
permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Observation
);
permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Direction
);
permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is DirectionFeedback
);

permit(principal, action, resource is ForesightModel)
when { principal.agent_type == "system" };
permit(principal, action, resource is Projection)
when { principal.agent_type == "system" };
permit(principal, action, resource is Observation)
when { principal.agent_type == "system" };
permit(principal, action, resource is Direction)
when { principal.agent_type == "system" };
permit(principal, action, resource is DirectionFeedback)
when { principal.agent_type == "system" };

// ============================================================
// Corridor world engine (ADR-002). Separations that carry the
// engine's honesty: forecasts immutable, scoring system-only,
// publication WASM-gated, peers confirm what authors propose.
// ============================================================

// --- World ---
permit(principal is Admin, action, resource is World);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is World
);

// Operating a world: humans, supervisors, and system flows drive it.
permit(
  principal,
  action in [Action::"Configure", Action::"Seed", Action::"SampleEndpoints",
             Action::"IngestEvidence", Action::"Render", Action::"BeginUpdate",
             Action::"Archive", Action::"AnimateDwellers", Action::"AdjudicateNodes"],
  resource is World
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
};

// Session self-reports: the surveyor reports SeedComplete; spawn callbacks
// report EndpointsSampled.
permit(
  principal,
  action in [Action::"SeedComplete", Action::"EndpointsSampled", Action::"DwellersCast"],
  resource is World
) when {
  ["system", "agent"].contains(principal.agent_type)
};

// Pass results, update completion, and the diversity gate (ADR-006) are
// aggregate_costs / sample_endpoints / evidence territory — system only.
permit(
  principal,
  action in [Action::"PathsScored", Action::"UpdateComplete",
             Action::"SpawnNextDweller", Action::"GateDiversity",
             Action::"ResumeSeed", Action::"ResumeWorldCascade"],
  resource is World
) when {
  principal.agent_type == "system"
};

// Failure must never be blocked.
permit(principal, action == Action::"Fail", resource is World);

// --- EventNode ---
permit(principal is Admin, action, resource is EventNode);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is EventNode
);

// Peer confirmation: anyone may confirm — except the author (Gate 2).
permit(
  principal,
  action == Action::"Confirm",
  resource is EventNode
);

forbid(
  principal,
  action == Action::"Confirm",
  resource is EventNode
) when {
  resource.AuthorAgentId == principal.id
};

// Probability, edges, and resolution move only with evidence (system flows).
permit(
  principal,
  action in [Action::"UpdateProbability", Action::"UpdateEdges", Action::"Resolve",
             Action::"FlagForHuman"],
  resource is EventNode
) when {
  principal.agent_type == "system"
};

permit(
  principal,
  action == Action::"Retire",
  resource is EventNode
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
};

// --- Endpoint ---
permit(principal is Admin, action, resource is Endpoint);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is Endpoint
);

// The writer self-reports its bundle via BundleWritten (ADR-006); the endpoint
// then parks in Written for the diversity gate. The gate — never the writer —
// releases it via SubmitForRepair or sends it back via ReSteer, so those are
// system only.
permit(
  principal,
  action == Action::"BundleWritten",
  resource is Endpoint
) when {
  ["system", "agent"].contains(principal.agent_type)
};

permit(
  principal,
  action in [Action::"SubmitForRepair", Action::"ReSteer",
             Action::"ResumeWriter", Action::"ResumeEndpointScoring",
             Action::"PathsAttached", Action::"ClaimsAttached",
             Action::"SpawnNextBridge", Action::"ScoreComplete",
             Action::"MarkWeighted", Action::"Reweigh"],
  resource is Endpoint
) when {
  principal.agent_type == "system"
};

// The decomposer session self-reports its claims (mirrors SubmitForRepair).
permit(
  principal,
  action == Action::"DecompositionComplete",
  resource is Endpoint
) when {
  ["system", "agent"].contains(principal.agent_type)
};

permit(
  principal,
  action == Action::"Discard",
  resource is Endpoint
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
};

permit(principal, action == Action::"Fail", resource is Endpoint);

// --- Claim (ADR-004) ---
permit(principal is Admin, action, resource is Claim);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is Claim
);

// The search loop is system territory: only WASM submits bridges, records
// routes, settles, re-prices, or declares a claim unreachable. The single
// session-writable surface is AmendText, relayed below — and even that is
// priced (deformation flags).
permit(
  principal,
  action in [
    Action::"SubmitForBridge", Action::"RoutesAttached", Action::"RouteSettled",
    Action::"Settle", Action::"Reprice", Action::"MarkUnreachable", Action::"ResumeBridge"
  ],
  resource is Claim
) when {
  principal.agent_type == "system"
};

// Failure must never be blocked.
permit(principal, action == Action::"Fail", resource is Claim);

// --- Path ---
permit(principal is Admin, action, resource is Path);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is Path
);

// Only the assigned repairer reports its repair; only the assigned adversary
// reports its challenge. Assignment (repairer never the endpoint's author)
// is enforced by spawn_repairers at spawn time.
permit(
  principal,
  action == Action::"RepairComplete",
  resource is Path
) when {
  principal.agent_type == "system" || resource.RepairerAgentId == principal.id
};

permit(
  principal,
  action == Action::"ChallengeComplete",
  resource is Path
) when {
  principal.agent_type == "system" || resource.AdversaryAgentId == principal.id
};

// Costing and classification are deterministic WASM territory.
permit(
  principal,
  action in [Action::"Score", Action::"ClassifyCanonical", Action::"ClassifyTail",
             Action::"Reject", Action::"Rescore", Action::"RequestChallenge",
             Action::"Prune", Action::"RevisionRequested", Action::"ResumeRepair",
             Action::"ResumeCosting", Action::"AssignRepairer",
             Action::"AssignAdversary", Action::"AppendChallengeFlag"],
  resource is Path
) when {
  principal.agent_type == "system"
};

permit(principal, action == Action::"Fail", resource is Path);

// --- Artifact ---
permit(principal is Admin, action, resource is Artifact);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is Artifact
);

// Authors submit their own drafts.
permit(
  principal,
  action == Action::"SubmitForCheck",
  resource is Artifact
) when {
  principal.agent_type == "system" || resource.AuthorAgentId == principal.id
};

// Checker sessions self-report verdicts; the gate validates the schema.
permit(
  principal,
  action == Action::"CheckerVerdict",
  resource is Artifact
) when {
  ["system", "agent"].contains(principal.agent_type)
};

// Adjudicator sessions self-report judged verdicts; the module validates
// them and alone owns Resolve/FlagForHuman (ADR-004 commitment 2). Mirrors
// the CheckerVerdict permit: session principals may surface either as
// agents or through the wasm-runtime relay.
permit(
  principal,
  action == Action::"AdjudicationVerdict",
  resource is EventNode
) when {
  ["system", "agent"].contains(principal.agent_type)
};

// The gate itself: no session passes, publishes, or retcons directly.
permit(
  principal,
  action in [Action::"PassCheck", Action::"FailCheck", Action::"Publish",
             Action::"Retcon"],
  resource is Artifact
) when {
  principal.agent_type == "system"
};

permit(
  principal,
  action == Action::"Reject",
  resource is Artifact
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
    || resource.AuthorAgentId == principal.id
};

// --- Dweller ---
permit(principal is Admin, action, resource is Dweller);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is Dweller
);

permit(
  principal,
  action in [Action::"Activate", Action::"Retire"],
  resource is Dweller
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
};

// A dweller is animated only by its own backing agent (or system flows).
permit(
  principal,
  action in [Action::"RecordTraversal", Action::"RecordContradiction"],
  resource is Dweller
) when {
  principal.agent_type == "system" || resource.AgentId == principal.id
};

// Track records are graded, never self-reported.
permit(
  principal,
  action == Action::"UpdateTrackRecord",
  resource is Dweller
) when {
  principal.agent_type == "system"
};

// --- Forecast (the scoreboard: immutable once registered) ---
permit(principal is Admin, action, resource is Forecast);

permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Forecast
);

// Only register_forecasts creates; only evidence resolves and grades.
permit(
  principal,
  action in [Action::"create", Action::"Resolve", Action::"Score", Action::"Void"],
  resource is Forecast
) when {
  principal.agent_type == "system"
};

// --- Hindcast ---
permit(principal is Admin, action, resource is Hindcast);

permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Hindcast
);

permit(
  principal,
  action in [Action::"create", Action::"Start", Action::"Grade"],
  resource is Hindcast
) when {
  ["system", "supervisor", "human"].contains(principal.agent_type)
};

permit(
  principal,
  action == Action::"ScoreComplete",
  resource is Hindcast
) when {
  principal.agent_type == "system"
};

permit(principal, action == Action::"Fail", resource is Hindcast);

// --- Lens ---
permit(principal is Admin, action, resource is Lens);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list",
             Action::"Activate", Action::"Reconfigure", Action::"Archive"],
  resource is Lens
);

// --- Corridor WASM module HTTP + secret access ---
permit(
  principal is Agent,
  action == Action::"http_call",
  resource is HttpEndpoint
) when {
  ["seed_world", "sample_endpoints", "decompose_endpoint", "spawn_repairers",
   "spawn_adversaries", "aggregate_costs", "evidence_ingest",
   "register_forecasts", "render_artifacts", "consistency_gate",
   "grade_hindcast", "animate_dwellers", "adjudicate_nodes"].contains(context.module)
};

permit(
  principal is Agent,
  action == Action::"access_secret",
  resource is Secret
) when {
  ["seed_world", "sample_endpoints", "decompose_endpoint", "spawn_repairers",
   "spawn_adversaries", "aggregate_costs", "evidence_ingest",
   "register_forecasts", "render_artifacts", "consistency_gate",
   "grade_hindcast", "animate_dwellers", "adjudicate_nodes"].contains(context.module)
};

// ── Session relay (v1 trust model) ─────────────────────────────
// Entity actions dispatched from inside sessions arrive as the
// wasm-runtime relay principal, not as the agent itself, so the
// agent_type- and id-bound permits above cannot see them. Until the
// dispatch context carries verifiable agent attributes end to end
// (follow-up: bind RepairComplete/ChallengeComplete to the assigned
// agent via context attrs), session self-reports are permitted at
// the relay principal — the same de facto trust model the 0.1 engine
// ran with. System-only transitions (scoring, classification,
// publication, forecast lifecycle) remain closed: they are
// dispatched by app WASM, never by sessions.

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"SeedComplete", Action::"EndpointsSampled", Action::"DwellersCast", Action::"Fail"],
  resource is World
);

// The endpoint writer self-reports BundleWritten through the relay (ADR-006);
// the decomposer self-reports DecompositionComplete. SubmitForRepair and
// ReSteer are NOT here — the diversity gate (system) dispatches them, never a
// session.
permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"BundleWritten", Action::"DecompositionComplete", Action::"Fail"],
  resource is Endpoint
);

// The repairer's drift-constrained amendment channel (ADR-004): AmendText
// carries the diff; the deformation flag prices it. Settle/Reprice/
// MarkUnreachable stay system-only — sessions never decide verdicts.
permit(
  principal == Agent::"service:wasm-runtime",
  action == Action::"AmendText",
  resource is Claim
);

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"RepairComplete", Action::"ChallengeComplete", Action::"Fail"],
  resource is Path
);

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"SubmitForCheck", Action::"CheckerVerdict", Action::"Reject"],
  resource is Artifact
);

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"Confirm", Action::"UpdateEdges", Action::"AdjudicationVerdict"],
  resource is EventNode
);

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"RecordTraversal", Action::"RecordContradiction"],
  resource is Dweller
);
