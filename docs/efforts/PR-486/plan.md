# Delivery

Update both kernel manifests and lockfile. Check uniformity and Cargo metadata with --locked, run the normal daemon CI against that revision, and review the bounded pin diff. The upstream real HTTP/Turso, PostgreSQL CAS and native Foundry flow provide behavioral proof; this pin must also pass consumer compilation/tests. Merge the kernel first, then this pin through normal gates. Use the immutable image deployment workflow and verify the actual OpenPaw deployment before enabling live proposals.
