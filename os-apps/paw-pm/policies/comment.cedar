// Project Management — Comment Cedar Policies
//
// The installer reads both the root and specs/: this entity already had a
// state machine, but no permit, so only System could move it.
// Accepted policy decision: https://github.com/arni-labs/temperpaw/pull/660
// (Change, and the reason for each). Contributors may maintain shared work;
// retiring shared records and changing a sprint's lifecycle belong to its
// operators: supervisors, humans and dashboard Admin.
// Delete retires a discussion record shared by the whole issue team.

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list", Action::"Create", Action::"Edit", Action::"React"],
  resource is Comment
);

permit(
  principal is Admin,
  action,
  resource is Comment
);

permit(
  principal,
  action == Action::"Delete",
  resource is Comment
) when {
  principal has agent_type && ["supervisor", "human"].contains(principal.agent_type)
};
