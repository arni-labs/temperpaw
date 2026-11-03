// TemperFS FileVersion Authorization Policies

permit(
    principal is Agent,
    action in [Action::"Create", Action::"Supersede"],
    resource is FileVersion
) when {
    principal.agent_type == "file-service"
};

// The platform operator finds the versions File deletes left behind before
// FileVersion.File cascaded (#590), and removes them. Cedar sees a version's
// own fields alone, so it cannot ask whether the File still exists: the
// version's Orphan move checks that (specs/file_version.ioa.toml), and only a
// version it moved to Orphaned may be deleted.
permit(
    principal,
    action in [Action::"read", Action::"list", Action::"Orphan"],
    resource is FileVersion
) when {
    principal has agent_type && principal.agent_type == "operator" &&
    principal has agentTypeVerified && principal.agentTypeVerified == true
};

permit(
    principal,
    action == Action::"delete",
    resource is FileVersion
) when {
    principal has agent_type && principal.agent_type == "operator" &&
    principal has agentTypeVerified && principal.agentTypeVerified == true &&
    resource has status && resource.status == "Orphaned"
};
