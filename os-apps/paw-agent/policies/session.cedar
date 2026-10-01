// Session — Cedar Authorization Policies
//
// Controls who can create, configure, and interact with spec-driven sessions.
// Callback actions are permitted for system agents to enable the dispatch pipeline loop.
// Steering is permitted for supervisors, humans, and parent sessions.

// --- Creation and Configuration: admins, supervisors, humans, and runtime agents ---

permit(
  principal is Admin,
  action,
  resource is Session
);

permit(
  principal,
  action in [Action::"create"],
  resource is Session
) when {
  ["supervisor", "human", "system", "agent"].contains(principal.agent_type)
};

permit(
  principal,
  action in [Action::"Configure", Action::"Provision", Action::"ProvisionWorkspace", Action::"Resume"],
  resource is Session
) when {
  ["supervisor", "human", "system"].contains(principal.agent_type) ||
  principal == resource ||
  context.parent_session_id == principal.id
};

// --- Read and List: any authenticated agent ---

permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is Session
);

permit(
  principal,
  action in [Action::"read_specs"],
  resource is Spec
);

// --- Steering: supervisors, humans, system, and self (session acting on itself) ---

permit(
  principal,
  action in [Action::"Steer", Action::"SwitchProvider", Action::"SwitchMode"],
  resource is Session
) when {
  ["supervisor", "human", "system"].contains(principal.agent_type) ||
  principal == resource ||
  context.parent_session_id == principal.id
};

// --- Dispatch pipeline callbacks: system agents ---
// These actions are triggered by WASM integration callbacks, not by users.

permit(
  principal,
  action in [
    Action::"SandboxReady",
    Action::"ProvisionPending",
    Action::"CheckSandboxReady",
    Action::"WorkspaceReady",
    Action::"ContextReady",
    Action::"ContextReadyAuthSkipped",
    Action::"ProviderAuthReady",
    Action::"ProviderAuthExpired",
    Action::"ProviderResponseReady",
    Action::"ProcessToolCalls",
    Action::"HandleToolResults",
    Action::"CheckpointToolBatch",
    Action::"RecordResult",
    Action::"RecordResultNoReply",
    Action::"RecordResultInlineReply",
    Action::"NeedsCompaction",
    Action::"CompactionAuthReady",
    Action::"CompactionAuthExpired",
    Action::"CompactionComplete",
    Action::"CheckSteering",
    Action::"ContinueWithSteering",
    Action::"FinalizeResult",
    Action::"FinalizeResultNoReply",
    Action::"DeliveryFailed",
    Action::"Heartbeat",
    Action::"ProgressMade",
    Action::"TimeoutFail",
    Action::"PauseForApproval",
    Action::"PauseForPlanApproval",
    Action::"RecoverFromRestart",
    Action::"RecoveryComplete",
    Action::"ResumeFromCheckpoint",
    // Wake-stalled actions fired by route_message when a DM arrives on a
    // session whose last_progress_at has passed its deadline. Self-loops
    // that re-trigger the corresponding driver. Without these permits Cedar
    // denies, the session falls through to TimeoutFail, and the user sees
    // the canned "tool execution made no progress for 300s" reply.
    Action::"ResumeTools",
    Action::"ResumeProvider",
    Action::"ResumeContext",
    Action::"ResumeCompacting",
    Action::"ResumeSteering"
  ],
  resource is Session
) when {
  principal.agent_type == "system" ||
  principal == resource
};

// --- Cedar approval resume: any privileged principal ---
// Dispatched by Discord interaction handler after human clicks Approve.
permit(
  principal,
  action in [Action::"ResumeAfterApproval", Action::"ResumeWithPlanApproval", Action::"ResumeWithPlanChanges"],
  resource is Session
) when {
  ["supervisor", "human", "system", "admin"].contains(principal.agent_type)
};

// --- WASM module HTTP call authorization ---

permit(
  principal is Agent,
  action == Action::"http_call",
  resource is HttpEndpoint
) when {
  ["agent_reply", "sandbox_provisioner", "workspace_provisioner", "context_preparer", "provider_auth_gate", "provider_caller", "provider_response_applier", "tool_runner", "monty_repl", "workspace_restorer", "context_compactor", "steering_checker", "session_link_monitor", "cron_scheduler_check", "cron_scheduler_heartbeat", "request_approval", "request_plan_review", "plan_approval_handler", "plan_review_feedback_handler", "session_recoverer", "emit_ots_trajectory", "openai_codex_auth"].contains(context.module)
};

// Allow agent WASM modules to access secrets
permit(
  principal is Agent,
  action == Action::"access_secret",
  resource is Secret
) when {
  ["sandbox_provisioner", "workspace_provisioner", "provider_caller", "tool_runner", "monty_repl", "workspace_restorer", "context_compactor", "session_recoverer", "openai_codex_auth"].contains(context.module)
};

// --- Failure and cancellation: admins, system callbacks, or the owning parent/self ---

permit(
  principal,
  action in [Action::"Fail", Action::"Cancel"],
  resource is Session
) when {
  principal.agent_type == "system" ||
  context.parent_session_id == principal.id ||
  principal == resource
};

// Only declared system entries may launch bounded calls or replay their callbacks.
permit(principal, action in [Action::"CallProviderStandard", Action::"CallProviderForesight", Action::"CallProviderForesightRetry", Action::"RetryForesightProvider", Action::"ForesightProviderFailed", Action::"ForesightProviderRetryFailed", Action::"ForesightProviderResponseReady", Action::"ForesightProviderAuthExpired"], resource is Session) when { principal.agent_type == "system" };
forbid(principal, action in [Action::"CallProviderStandard", Action::"CallProviderForesight", Action::"CallProviderForesightRetry", Action::"RetryForesightProvider", Action::"ForesightProviderFailed", Action::"ForesightProviderRetryFailed", Action::"ForesightProviderResponseReady", Action::"ForesightProviderAuthExpired"], resource is Session) unless { principal is Agent && principal.agent_type == "system" };
