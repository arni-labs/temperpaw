// Resident factory agents may ask people. Answers remain human decisions.
permit(principal is Agent,
  action in [Action::"create", Action::"Raise", Action::"RaiseBlocking", Action::"RecordFyi", Action::"Withdraw"],
  resource is Ask
) when {
  principal has agentTypeVerified && principal.agentTypeVerified == true &&
  principal has agent_type && principal.agent_type == "dsf-factory"
};

forbid(principal,
  action == Action::"Answer",
  resource is Ask
) when {
  principal has agent_type && principal.agent_type == "dsf-factory"
};
