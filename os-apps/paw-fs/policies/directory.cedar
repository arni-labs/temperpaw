// Directory policies for paw-fs. Moved from workspace.cedar so each entity has
// its own policy file, as `temper verify` requires; the text is unchanged.

// Agents can manage directories (addchild, create, rename)
permit(
    principal is Agent,
    action in [Action::"read", Action::"list", Action::"Create", Action::"create", Action::"AddChild", Action::"RemoveChild", Action::"DirectoryRename", Action::"DirectoryArchive"],
    resource is Directory
);
