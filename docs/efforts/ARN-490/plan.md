# Implementation plan

1. Reproduce stale packaging and false success with a small isolated fixture around the existing builders.
2. Add one shared build-and-locate helper. Migrate all affected builders, preserve targets and destinations, remove duplicated guesses, and make required build failures nonzero.
3. Run the fixture against every caller and compile/run a real packaged module against the deployed-compatible kernel.
4. Complete the fixed review panel and Greptile; triage, fix, and confirm on the resulting head. Merge only after the required gates and authorization.
5. Verify Foundry's current computer source, checkout revision and build command. Install the merged repository version through an isolated clean checkout; demonstrate builds on Tensorlake. Preserve dirty worktrees and record how existing sessions refresh and what new copies inherit.
