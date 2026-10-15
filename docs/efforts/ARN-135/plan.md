# Dependency pin repair

1. Preserve the current canonical Genesis app trees.
2. Pin paw-agent to paw-fs Git head737c0ac23d95b8d412cb4f632c2926b720835eef; pin paw-research to the resulting agent commit.
3. Update the matching GitHub manifest files and Katagami dependency pins.
4. Verify the resolved bundle and local installation, review the delta, merge, install the exact app refs, and verify the live contribution flow.
