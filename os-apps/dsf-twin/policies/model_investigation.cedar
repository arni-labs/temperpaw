// The resident daemon reads evidence and queues investigations. Its reasoning
// agent uses a separate registered dsf-factory credential for model actions.
permit(principal, action in [Action::"read", Action::"list"], resource) when {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "worker" &&
  (resource is DsfObservation || resource is DsfModelSync || resource is DsfRailwayServiceInstance ||
   resource is DsfVercelProject || resource is DsfSupabaseProject || resource is DsfCloudflareR2Bucket ||
   resource is DsfDatadogMonitor || resource is DsfMediaPipeline || resource is DsfFlow || resource is DsfParticipant)
};
permit(principal, action in [Action::"RequestModelInvestigation", Action::"StartModelInvestigation", Action::"ReconcileModelWorker"], resource is PatrolRun) when {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "worker"
};
permit(principal, action in [Action::"ReportInvestigation", Action::"FailInvestigation", Action::"ReplayInvestigationResult", Action::"ReplayInvestigationFailure"], resource is WorkerRun) when {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "worker" &&
  ((resource has worker_id && resource.worker_id == principal.id) ||
   (resource has WorkerId && resource.WorkerId == principal.id))
};
permit(principal, action in [Action::"create", Action::"ConfigureModelInvestigation"], resource is WorkerRun) when {
  principal has role && principal.role == "service" && principal has agent_type && principal.agent_type == "dsf-factory-runtime" &&
  context has triggerDeclaredPrincipal && context.triggerDeclaredPrincipal == true &&
  context has triggerSourceEntityType && context.triggerSourceEntityType == "PatrolRun"
};
permit(principal, action in [Action::"CompleteModelInvestigation", Action::"FailModelInvestigation"], resource is PatrolRun) when {
  principal has role && principal.role == "service" && principal has agent_type && principal.agent_type == "dsf-factory-runtime" &&
  context has triggerDeclaredPrincipal && context.triggerDeclaredPrincipal == true &&
  context has triggerSourceEntityType && context.triggerSourceEntityType == "WorkerRun"
};
forbid(principal, action in [Action::"CompleteModelInvestigation", Action::"FailModelInvestigation", Action::"ConfigureModelInvestigation"], resource) when {
  resource is PatrolRun || resource is WorkerRun
} unless {
  principal has role && principal.role == "service" && principal has agent_type && principal.agent_type == "dsf-factory-runtime" &&
  context has triggerDeclaredPrincipal && context.triggerDeclaredPrincipal == true
};

forbid(principal, action in [Action::"ReportInvestigation", Action::"FailInvestigation", Action::"ReplayInvestigationResult", Action::"ReplayInvestigationFailure"], resource is WorkerRun) unless {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "worker" &&
  ((resource has worker_id && resource.worker_id == principal.id) ||
   (resource has WorkerId && resource.WorkerId == principal.id))
};
// A native investigation assignment cannot be replaced through the legacy action.
forbid(principal, action == Action::"Configure", resource is WorkerRun) when {
  (resource has model_investigation && resource.model_investigation == true) ||
  (resource has ModelInvestigation && resource.ModelInvestigation == true)
};

// Resource configuration is model data. The registered factory agent can write
// only its canonical config Files; provider actions recheck their pinned digest.
permit(principal is Agent, action in [Action::"create", Action::"update", Action::"read"], resource is File) when {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "dsf-factory" &&
  resource.id like "dsf-resource-config-*" && resource.id != "dsf-resource-config-"
};
