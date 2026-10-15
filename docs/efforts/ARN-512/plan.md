# ARN-512 plan (temperpaw side)

1. Bump both kernel pins to the fix (`90548bd1` for the twin marker, then
   `bd15e892` for the edges); move only the kernel crates in `Cargo.lock`.
2. Build temperpaw; run the dsf-twin contract and runtime tests.
3. Records in Temper for the head (ReviewRuns, ProofPacket), gates, merge.
4. Docker image for the merge commit; `railway-redeploy` with that tag;
   `/healthz` 200.
5. Production `$metadata` has `Temper.Twin` and the 25 targeted blocks (the
   kernel re-parses the stored schema at boot; no reload needed); open the
   Twins page and see "Deep Sci-Fi" drawn as a graph with edges.
