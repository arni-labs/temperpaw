# repo-contract
The foundation tests pin the repo's own contracts: CI coverage of the worker
script surface, doc/runbook invariants, dependency-pin coherence.
Drive: cargo test --test paw_patrol_foundation ; pass = all tests green.
A change to these tests IS a change to this surface - the drive is the suite.
