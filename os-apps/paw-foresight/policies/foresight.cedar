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

// Dashboard subscriptions are filtered to the authenticated tenant by the
// observe endpoint. This grants no generic Entity reads or mutations.
permit(
  principal is Admin,
  action == Action::"read_events",
  resource is Entity
);

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
             Action::"ResumeCosting"],
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
  action in [Action::"create", Action::"Resolve", Action::"Score", Action::"ScoreBatch", Action::"Void"],
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
   "grade_hindcast", "animate_dwellers", "adjudicate_nodes", "learning_prepare", "learning_train", "learning_evaluate", "forecast_outcome", "replay_open"].contains(context.module)
};

permit(
  principal is Agent,
  action == Action::"access_secret",
  resource is Secret
) when {
  ["seed_world", "sample_endpoints", "decompose_endpoint", "spawn_repairers",
   "spawn_adversaries", "aggregate_costs", "evidence_ingest",
   "register_forecasts", "render_artifacts", "consistency_gate",
   "grade_hindcast", "animate_dwellers", "adjudicate_nodes", "learning_prepare", "learning_train", "learning_evaluate", "forecast_outcome", "replay_open"].contains(context.module)
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

// Corridor sessions create skeleton EventNodes via temper.create in monty.
permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"create", Action::"read", Action::"list", Action::"Confirm",
             Action::"UpdateEdges", Action::"AdjudicationVerdict"],
  resource is EventNode
);

permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"RecordTraversal", Action::"RecordContradiction"],
  resource is Dweller
);

 // ARN-518: continuous learning and reviewed human outcome entry.
permit(principal is Admin, action in [Action::"create", Action::"read", Action::"list", Action::"Start"], resource is LearningRun);
permit(principal, action in [Action::"read", Action::"list"], resource is LearningRun);
permit(principal, action in [Action::"create", Action::"Start"], resource is LearningRun)
when { ["human","supervisor","system"].contains(principal.agent_type) };
permit(principal, action in [Action::"Prepared",Action::"Trained",Action::"CandidatePassed",Action::"ConfirmAdoption",Action::"Reject",Action::"Fail"], resource is LearningRun)
when { principal.agent_type == "system" };
permit(principal, action in [Action::"ConfigureLearning",Action::"OpenReplay",Action::"RegisterForecasts"], resource is World)
when { ["human","supervisor","system"].contains(principal.agent_type) };
permit(principal, action in [Action::"ReplayOpened",Action::"AdoptModel"], resource is World)
when { principal.agent_type == "system" };
permit(principal, action == Action::"RecordOutcome", resource is Forecast)
when { ["human","supervisor","system"].contains(principal.agent_type) };
permit(principal, action in [Action::"ResolveRevision",Action::"OutcomeVerified",Action::"RevisionVerified",Action::"OutcomeFailed"], resource is Forecast)
when { principal.agent_type == "system" };
// PATCH and PUT both authorize UPDATE_ACTION = "update" in the OData write handler.
// The precheck label "patch" is not the Cedar action.
forbid(principal, action == Action::"update", resource is Forecast);
forbid(principal, action == Action::"update", resource is LearningRun);

// World model parameters and mode are changed only by declared actions.
forbid(principal, action == Action::"update", resource is World);

// Immutable history remains available after a world is archived.
forbid(principal, action == Action::"delete", resource is Forecast);
forbid(principal, action == Action::"delete", resource is LearningRun);
forbid(principal, action == Action::"delete", resource is World);
// A new world begins with the identity model. Learned parameters enter via AdoptModel.
forbid(principal, action == Action::"create", resource is World)
when { resource has ModelJson && resource.ModelJson != "" };
forbid(principal, action == Action::"create", resource is World)
when { resource has AdoptedLearningRunId && resource.AdoptedLearningRunId != "" };


// Action integrations return through the kernel's named WASM service. Entity
// triggers keep their declared system principal; operator actions are separate.
permit(principal == Agent::"service:wasm-runtime",
  action in [Action::"Prepared", Action::"Trained", Action::"CandidatePassed",
             Action::"Reject", Action::"Fail"],
  resource is LearningRun);
permit(principal == Agent::"service:wasm-runtime",
  action == Action::"ReplayOpened", resource is World);
permit(principal == Agent::"service:wasm-runtime",
  action in [Action::"OutcomeVerified", Action::"RevisionVerified", Action::"OutcomeFailed"],
  resource is Forecast);

// Registration crosses entities through declared system triggers only.
permit(principal, action == Action::"Register", resource is Forecast)
when { principal.agent_type == "system" };
permit(principal, action in [Action::"ForecastRegistered", Action::"ForecastRegistrationFailed"], resource is World)
when { principal.agent_type == "system" };
permit(principal == Agent::"service:wasm-runtime",
  action in [Action::"ForecastPrepared", Action::"ForecastRegistrationComplete"], resource is World);

// Learning records and their inputs are produced by declared integrations.
// Explicit denies also constrain the existing broad dashboard Admin permits.
forbid(principal,
  action in [Action::"Prepared", Action::"Trained", Action::"CandidatePassed",
             Action::"ConfirmAdoption", Action::"Reject", Action::"Fail"],
  resource is LearningRun)
unless {
  (principal has agent_type && principal.agent_type == "system")
  || principal == Agent::"service:wasm-runtime"
};
forbid(principal,
  action in [Action::"ReplayOpened", Action::"AdoptModel", Action::"ForecastPrepared",
             Action::"ForecastRegistered", Action::"ForecastRegistrationComplete",
             Action::"ForecastRegistrationFailed"],
  resource is World)
unless {
  (principal has agent_type && principal.agent_type == "system")
  || principal == Agent::"service:wasm-runtime"
};
forbid(principal,
  action in [Action::"Register", Action::"Resolve", Action::"Score", Action::"ScoreBatch", Action::"Void",
             Action::"ResolveRevision", Action::"OutcomeVerified",
             Action::"RevisionVerified", Action::"OutcomeFailed"],
  resource is Forecast)
unless {
  (principal has agent_type && principal.agent_type == "system")
  || principal == Agent::"service:wasm-runtime"
};
permit(principal == Agent::"service:wasm-runtime",
  action == Action::"ForecastRegistrationFailed", resource is World);

// Dashboard-started research retains the verified admin identity during its
// seed_world internal calls. Grant workspace creation only; do not broaden
// workspace reads, management, or the existing agent permissions.
permit(
  principal is Admin,
  action in [Action::"create", Action::"Create"],
  resource is Workspace
);

// Only seed_world's runtime callback records the current research session.
// Explicit denies constrain the broad World Admin permit and generic creation.
permit(principal == Agent::"service:wasm-runtime",
  action == Action::"ResearchSessionStarted", resource is World);
forbid(principal, action == Action::"ResearchSessionStarted", resource is World)
unless { principal == Agent::"service:wasm-runtime" };
forbid(principal, action == Action::"create", resource is World)
when { resource has ResearchSessionId && resource.ResearchSessionId != "" };

// Session reports enter deterministic work through declared system entity triggers.
// The broad dashboard permits must not expose these internal entry actions.
permit(principal, action == Action::"CheckDiversityBarrier", resource is Endpoint)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action == Action::"CheckDiversityBarrier", resource is Endpoint)
unless { principal has agent_type && principal.agent_type == "system" };

permit(principal, action in [Action::"PrepareClaims", Action::"StartWriter", Action::"EvaluateEndpointScoring"], resource is Endpoint)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action in [Action::"PrepareClaims", Action::"StartWriter", Action::"EvaluateEndpointScoring"], resource is Endpoint)
unless { principal has agent_type && principal.agent_type == "system" };

permit(principal, action in [Action::"PrepareBridge", Action::"EvaluateRoutes"], resource is Claim)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action in [Action::"PrepareBridge", Action::"EvaluateRoutes"], resource is Claim)
unless { principal has agent_type && principal.agent_type == "system" };

permit(principal, action in [Action::"EvaluateRepair", Action::"EvaluateChallenge", Action::"RelayScoredRoute", Action::"RelayPrunedRoute", Action::"StartChallenge", Action::"LaunchChallenge", Action::"StartRepair"], resource is Path)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action in [Action::"EvaluateRepair", Action::"EvaluateChallenge", Action::"RelayScoredRoute", Action::"RelayPrunedRoute", Action::"StartChallenge", Action::"LaunchChallenge", Action::"StartRepair"], resource is Path)
unless { principal has agent_type && principal.agent_type == "system" };


// Progressive publication and background exploration use declared trusted entries.
permit(principal, action in [Action::"RequestForecastRegistration", Action::"StartForecastRegistration",
  Action::"MaybeDeepen", Action::"ContinueDeepening", Action::"CommitForecastSnapshot"], resource is World)
when { principal.agent_type == "system" };
permit(principal == Agent::"service:wasm-runtime",
  action in [Action::"BeginDeepening", Action::"DeepeningClaimPrepared", Action::"RegistrationSnapshotPrepared"], resource is World);
forbid(principal, action in [Action::"RequestForecastRegistration", Action::"StartForecastRegistration",
  Action::"MaybeDeepen", Action::"ContinueDeepening", Action::"CommitForecastSnapshot"], resource is World)
unless { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action in [Action::"BeginDeepening", Action::"DeepeningClaimPrepared", Action::"RegistrationSnapshotPrepared"], resource is World)
unless { principal == Agent::"service:wasm-runtime" };
permit(principal, action == Action::"BeginDeepening", resource is Claim)
when { principal.agent_type == "system" };
forbid(principal, action == Action::"BeginDeepening", resource is Claim)
unless { principal has agent_type && principal.agent_type == "system" };

// The sequence-checked timer inherits the trusted identity that last reset it.
permit(principal, action == Action::"ForecastRegistrationTimedOut", resource is World)
when { principal.agent_type == "system" || principal == Agent::"service:wasm-runtime" };
forbid(principal, action == Action::"ForecastRegistrationTimedOut", resource is World)
unless { (principal has agent_type && principal.agent_type == "system") || principal == Agent::"service:wasm-runtime" };

permit(principal, action == Action::"EvaluateWorldCascade", resource is World)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action == Action::"EvaluateWorldCascade", resource is World)
unless { principal has agent_type && principal.agent_type == "system" };

// Dashboard questions are explicit operator inputs. Worker-created requirements
// must pass path evaluation and cannot claim this reserved manual author marker.
forbid(principal, action == Action::"create", resource is EventNode)
when { resource has AuthorAgentId && resource.AuthorAgentId == "dashboard" }
unless { principal is Admin || (principal has agent_type && ["human","supervisor","system"].contains(principal.agent_type)) };

// Declared corridor workers reuse their tenant's world workspaces. This does
// not expose workspace browsing or management to model sessions or operators.
permit(principal is Agent, action in [Action::"read", Action::"list"], resource is Workspace)
when { principal has agent_type && principal.agent_type == "system" };

// Jev is a shadow evaluator. Its records cannot mutate paths or forecasts.
permit(principal, action in [Action::"read", Action::"list"], resource is SemanticEvaluation);
permit(principal, action in [Action::"create", Action::"Evaluate", Action::"Timeout"], resource is SemanticEvaluation)
when { principal has agent_type && principal.agent_type == "system" };
permit(principal == Agent::"service:wasm-runtime",
  action in [Action::"Record", Action::"Skip", Action::"Fail"], resource is SemanticEvaluation);
forbid(principal, action in [Action::"update", Action::"delete"], resource is SemanticEvaluation);
permit(principal is Admin, action == Action::"ConfigureSemanticEvaluation", resource is World);
permit(principal, action == Action::"ConfigureSemanticEvaluation", resource is World)
when { principal has agent_type && ["human","supervisor","system"].contains(principal.agent_type) };
permit(principal is Agent, action == Action::"http_call", resource is HttpEndpoint)
when { context.module == "evaluate_semantics" };
permit(principal is Agent, action == Action::"access_secret", resource is Secret)
when { context.module == "evaluate_semantics" };

permit(principal == Agent::"service:wasm-runtime", action == Action::"ChallengePrepared", resource is Path);
forbid(principal, action == Action::"ChallengePrepared", resource is Path)
unless { principal == Agent::"service:wasm-runtime" };
permit(principal is Agent, action == Action::"http_call", resource is HttpEndpoint)
when { context.module == "prepare_challenge" };
permit(principal is Agent, action == Action::"access_secret", resource is Secret)
when { context.module == "prepare_challenge" };

// Native semantic exploration: readers inspect; operators start/cancel;
// provider callbacks and frozen results belong to the machine.
permit(principal, action in [Action::"read", Action::"list"], resource is SemanticRun);
permit(principal, action in [Action::"create", Action::"Start", Action::"Cancel"], resource is SemanticRun)
when { ["human", "supervisor", "system"].contains(principal.agent_type) };
permit(principal, action, resource is SemanticRun)
when { principal.agent_type == "system" };
permit(principal is Admin, action in [Action::"create", Action::"Start", Action::"Cancel"], resource is SemanticRun);

permit(principal, action == Action::"RequestSemanticExploration", resource is World)
when { ["human", "supervisor", "system"].contains(principal.agent_type) };
permit(principal, action == Action::"StartSemanticExploration", resource is World)
when { principal.agent_type == "system" };
// WASM result callbacks are delivered by the runtime relay, not browser callers.
permit(principal == Agent::"service:wasm-runtime",
 action in [Action::"Prepared",Action::"ResumePrepared",Action::"SearchPlanned",Action::"Evaluate",Action::"Recorded",Action::"Reason",Action::"LaunchReasoning",Action::"ReasoningPending",Action::"ReasoningProgress",Action::"ReasoningRetry",Action::"ReasoningComplete",Action::"CompositionRejected",Action::"Expanded",Action::"Complete",Action::"Fail"],
 resource is SemanticRun);
forbid(principal,
 action in [Action::"Prepared",Action::"ResumePrepared",Action::"SearchPlanned",Action::"Evaluate",Action::"Recorded",Action::"Reason",Action::"LaunchReasoning",Action::"ReasoningPending",Action::"ReasoningProgress",Action::"ReasoningRetry",Action::"ReasoningComplete",Action::"CompositionRejected",Action::"Expanded",Action::"Complete"],
 resource is SemanticRun)
unless { principal == Agent::"service:wasm-runtime" };

// Six modules share no ambient network capability. Only the snapshot/session
// readers may read the local engine; only the one-call provider may reach Jev.
permit(principal is Agent, action == Action::"http_call", resource is HttpEndpoint)
when {
 ["semantic_prepare", "semantic_session"].contains(context.module) &&
 context.method == "GET" && context.url like "http://127.0.0.1:3467/tdata/*"
};
permit(principal is Agent, action == Action::"http_call", resource is HttpEndpoint)
when {
 context.module == "semantic_call" && context.method == "POST" &&
 context.url == "https://api.typesafe.ai/v1/systemone"
};
permit(principal is Agent, action == Action::"access_secret", resource == Secret::"foresight_typesafe_api_key")
when { context.module == "semantic_call" };

permit(principal is Agent, action == Action::"access_secret", resource == Secret::"temper_api_url")
when { ["semantic_prepare", "semantic_session"].contains(context.module) };

// The server-side question route uses the tenant's verified operator credential.
permit(principal is Agent,
  action in [Action::"ConfigureLearning", Action::"Configure", Action::"RequestSemanticExploration", Action::"Seed"],
  resource is World)
when {
  principal has agent_type && principal.agent_type == "operator" &&
  principal has agentTypeVerified && principal.agentTypeVerified == true
};

permit(principal, action == Action::"StartSeed", resource is World)
when { principal has agent_type && principal.agent_type == "system" };
forbid(principal, action == Action::"StartSeed", resource is World)
unless { principal has agent_type && principal.agent_type == "system" };

// The verified dashboard operator may retry an existing world's exploration.
permit(principal is Agent,
  action in [Action::"create", Action::"Start"],
  resource is SemanticRun)
when {
  principal has agent_type && principal.agent_type == "operator" &&
  principal has agentTypeVerified && principal.agentTypeVerified == true
};

// Scheduled after LaunchReasoning has durably saved its prompt parameters.
forbid(principal, action == Action::"SpawnReasoning", resource is SemanticRun)
unless { principal has agent_type && principal.agent_type == "system" };

forbid(principal, action == Action::"Fail", resource is SemanticRun)
unless { principal == Agent::"service:wasm-runtime" || (principal has agent_type && principal.agent_type == "system") };
forbid(principal, action == Action::"CheckReasoning", resource is SemanticRun)
unless { principal has agent_type && principal.agent_type == "system" };

// The app monitor may inspect only the current researcher through existing system reads.
permit(principal, action == Action::"CheckResearchSession", resource is World)
when { principal.agent_type == "system" };
permit(principal == Agent::"service:wasm-runtime",
 action in [Action::"ResearchPending", Action::"ResearchIncomplete", Action::"ResearchFailed", Action::"ResearchMonitorUnavailable", Action::"ResearchMonitorInvocationFailed"], resource is World);
forbid(principal,
 action in [Action::"ResearchPending", Action::"ResearchIncomplete", Action::"ResearchFailed", Action::"ResearchMonitorUnavailable", Action::"ResearchMonitorInvocationFailed"], resource is World)
unless { principal == Agent::"service:wasm-runtime" };
