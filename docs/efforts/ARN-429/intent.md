# ARN-429: CI fast lane
## Problem
One monolithic CI job (fmt+clippy+check+17 scripts+11 WASM app builds+tests+dashboard) runs on every PR - slow feedback, and PR iteration pays for merge-grade validation.
## Proposed outcome
PRs wait on a <5-min fast lane (fmt, clippy, check, script syntax, rust-cache); the heavy job runs on push to main/release + nightly + dispatch.
## Affected users and systems
.github/workflows/ci.yml only; `checks` context name unchanged.
## Constraints
Nothing is deleted - every heavy step still runs, gating merges instead of iteration.
## Open questions
None.
