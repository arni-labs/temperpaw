// Project Management — Cycle Cedar Policies
//
// Cycle had a schema and no state machine until its spec moved into specs/
// (the installer reads only specs/), so no policy was ever needed. Any agent
// fills a cycle with issues; supervisors, humans and Admin start and close it.

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list", Action::"SetProject", Action::"AddIssueToCycle", Action::"MarkIssueComplete", Action::"RemoveIssueFromCycle"],
  resource is Cycle
);

permit(
  principal is Admin,
  action,
  resource is Cycle
);

permit(
  principal,
  action in [Action::"Start", Action::"Complete"],
  resource is Cycle
) when {
  principal has agent_type && ["supervisor", "human"].contains(principal.agent_type)
};
