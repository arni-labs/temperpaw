# Decisions

## Pin the complete dependency graph

Decision: Replace the two unpinned app dependencies with full canonical Genesis revisions.
Came up because: The pinned Katagami curation installer rejected mixed paw-fs versions, while bare dependency names resolved stale registry versions.
Options: Update the dependency pins; modify the kernel resolver; bypass bundle validation.
Chose explicit pins because they preserve the installer boundary and make the app closure reproducible without a kernel change.
Where: os-apps/paw-agent/app.toml and os-apps/paw-research/app.toml.
