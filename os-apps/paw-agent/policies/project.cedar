// Project — coordinated effort container (ADR-0018)
permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list", Action::"Configure", Action::"Update"],
  resource is Project
);

// Browser sign-in supplies a typed Admin with no agent_type.
permit(
  principal is Admin,
  action in [Action::"Pause", Action::"Resume", Action::"Complete", Action::"Archive"],
  resource is Project
);

permit(
  principal,
  action in [Action::"Pause", Action::"Resume", Action::"Complete", Action::"Archive"],
  resource is Project
) when {
  ["supervisor", "human", "admin"].contains(principal.agent_type)
};
