// TemperFS File Authorization Policies

// Any authenticated principal can read/list files
permit(
    principal,
    action in [Action::"read", Action::"list"],
    resource is File
);

// Session tool reads (temper.read) authorize content access through a
// family of capitalized action names. They are reads like `read` above,
// so any principal may make them; without this permit every session file
// read dies in WaitingForApproval.
permit(
    principal,
    action in [
        Action::"Read", Action::"Download", Action::"GetContent",
        Action::"GetValue", Action::"Stream", Action::"Open",
        Action::"GetText", Action::"FetchContent", Action::"Content"
    ],
    resource is File
);

// System agents execute Temper-native PawFS workflows such as ArtifactBatch and
// Monty write helpers; their target workspace is carried in the entity fields.
permit(
    principal is Agent,
    action in [Action::"StreamUpdated", Action::"Create", Action::"create", Action::"update"],
    resource is File
) when {
    principal.agent_type == "system"
};

permit(
    principal is Agent,
    action == Action::"RecordVersion",
    resource is File
) when {
    principal.agent_type == "file-service"
};

// Locked files reject StreamUpdated (defense in depth — transition table also rejects)
forbid(
    principal,
    action == Action::"StreamUpdated",
    resource is File
) when {
    resource.status == "Locked"
};

// Sessions write files for the caller that started them (turn artifacts,
// agent temper.write via the directory tree): through a module's call
// (`context.module`) or a move the kernel makes for the caller (a module's
// answer, a reaction or a timer: `context.via` other than "request"). A
// caller's own direct requests do not pass this permit. The relay
// principal service:wasm-runtime keeps it until every deployment sends
// these moves as the caller. Without this permit every fresh tenant needs
// a runtime decision approval before any session can persist content.
permit(
    principal,
    action in [Action::"create", Action::"Create", Action::"StreamUpdated", Action::"update"],
    resource is File
) when {
    principal == Agent::"service:wasm-runtime" ||
    (context has via && context.via != "request") ||
    context has module
};
