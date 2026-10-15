# Implementation

1. Reproduce the missing permit with the real AuthzEngine and canonical Cedar.
2. Verify installed Genesis source and pin; add only the verified operator Ask.Answer permit there and in the GitHub mirror.
3. Verify positive and negative cases, including original policy red and new policy green.
4. Review the full correction, merge the mirror, publish the exact Genesis delta, install the pinned app, and verify through Foundry in Arc.

The user explicitly authorized the scoped publication, evidence upload and installation. Both catalog and installed-provenance reads still return403 under the existing operator. The configured paw-patrol bootstrap pin and canonical Genesis main both resolve to d11b760c8ab5fdb7bd858de5e2a74a25492af4a9. Installed-state access remains under investigation with the Genesis access owner; no raw policy overwrite or credential substitution is part of this repair.
