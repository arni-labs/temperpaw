// Project Management — Label Cedar Policies
//
// Label had a schema and no state machine until its spec moved into specs/
// (the installer reads only specs/), so no policy was ever needed. Any agent
// makes and uses labels; supervisors, humans and Admin archive them.

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list", Action::"Create", Action::"IncrementUsage"],
  resource is Label
);

permit(
  principal is Admin,
  action,
  resource is Label
);

permit(
  principal,
  action == Action::"Archive",
  resource is Label
) when {
  principal has agent_type && ["supervisor", "human"].contains(principal.agent_type)
};
