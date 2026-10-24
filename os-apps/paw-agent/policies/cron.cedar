// CronJob + CronScheduler — Cedar Authorization Policies

// Admins can do everything
permit(
  principal is Admin,
  action,
  resource is CronJob
);

permit(
  principal is Admin,
  action,
  resource is CronScheduler
);

// Only supervisors/humans can configure and activate cron jobs
permit(
  principal,
  action in [Action::"create", Action::"Configure", Action::"Activate", Action::"Pause", Action::"Resume", Action::"Expire"],
  resource is CronJob
) when {
  ["supervisor", "human"].contains(principal.agent_type)
};

// System agents can trigger cron jobs (called by CronScheduler WASM)
permit(
  principal,
  action in [Action::"Trigger", Action::"TriggerComplete", Action::"TriggerFailed"],
  resource is CronJob
) when {
  principal.agent_type == "system"
};

// RecordLastSession rewrites a job's record of which Session its last run
// spawned. It repairs records a kernel replay bug left wrong, so only the
// verified operator may do it (the owner's decision, 24 Oct 2026): not Admin,
// people or agents, and not a self-declared operator claim. Both spellings:
// the OData route passes the bare name; one passed type-qualified gets the
// same answer instead of falling to Admin's catch-all.
permit(
  principal is Agent,
  action in [Action::"RecordLastSession", Action::"CronJob.RecordLastSession"],
  resource is CronJob
) when {
  principal has agent_type && principal.agent_type == "operator" &&
  principal has agentTypeVerified && principal.agentTypeVerified == true
};

forbid(
  principal,
  action in [Action::"RecordLastSession", Action::"CronJob.RecordLastSession"],
  resource is CronJob
) unless {
  principal is Agent &&
  principal has agent_type && principal.agent_type == "operator" &&
  principal has agentTypeVerified && principal.agentTypeVerified == true
};

// Any authenticated agent can read cron jobs
permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is CronJob
);

// System agents manage the scheduler lifecycle
permit(
  principal,
  action in [Action::"create", Action::"Start", Action::"CheckComplete", Action::"CheckFailed", Action::"ScheduledCheck", Action::"ScheduleFailed"],
  resource is CronScheduler
) when {
  principal.agent_type == "system"
};

permit(
  principal,
  action in [Action::"read", Action::"list"],
  resource is CronScheduler
);
