// TemperFS Workspace Authorization Policies

// Agents can create workspaces
permit(
    principal is Agent,
    action in [Action::"Create", Action::"create"],
    resource is Workspace
);

permit(
    principal is Agent,
    action == Action::"IncrementUsage",
    resource is Workspace
) when {
    principal.agent_type == "file-service"
};

// Agents can perform filesystem operations (FUSE-mapped)
permit(
    principal is Agent,
    action in [Action::"MkDir", Action::"CreateFile", Action::"DirCreated", Action::"FileCreated",
               Action::"IncrementFileCount", Action::"DecrementFileCount",
               Action::"ResolvePath", Action::"PathResolved",
               Action::"ListDir", Action::"DirListed",
               Action::"DeleteFile", Action::"FileDeleted",
               Action::"Rename", Action::"FileRenamed",
               Action::"FsOperationFailed"],
    resource is Workspace
);
