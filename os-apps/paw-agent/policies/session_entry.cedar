// SessionEntry -- hot operational session state.
//
// Entries are small, tenant-scoped records used by the Session WASM pipeline.
// Large governed artifacts remain in PawFS; this policy only covers the hot
// entry log needed to resume, compact, and recover sessions.

permit(
  principal is Admin,
  action,
  resource is SessionEntry
);

permit(
  principal,
  action in [Action::"create", Action::"read", Action::"list"],
  resource is SessionEntry
) when {
  ["agent", "system", "supervisor", "human", "admin"].contains(principal.agent_type)
};

// Transition callbacks use the authenticated runtime identity. The runtime
// appends and reads transcript entries; it cannot rewrite or delete history.
permit(
  principal == Agent::"service:wasm-runtime",
  action in [Action::"create", Action::"read", Action::"list"],
  resource is SessionEntry
);
