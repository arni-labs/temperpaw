// Project Management — Comment Cedar Policies
//
// Comment had a schema and no state machine until its spec moved into specs/
// (the installer reads only specs/), so no policy was ever needed. Like
// Issue's comments: any agent writes and reacts; supervisors, humans and
// Admin delete.

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
