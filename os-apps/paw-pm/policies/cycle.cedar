// Project Management — Cycle Cedar Policies
//
// The installer reads both the root and specs/: this entity already had a
// state machine, but no permit, so only System could move it.
// Accepted policy decision: https://github.com/arni-labs/temperpaw/pull/660
// (Change, and the reason for each). Contributors may maintain shared work;
// retiring shared records and changing a sprint's lifecycle belong to its
// operators: supervisors, humans and dashboard Admin.
// Start/Complete change the shared sprint lifecycle, including carrying
// unfinished work into the next cycle.

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
