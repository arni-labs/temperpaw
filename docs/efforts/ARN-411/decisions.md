# Decisions - enforced SDLC gates

## Evidence rides in PR comments, never committed
- **Decision:** proof/review records reach CI as a hidden JSON marker in a PR comment, not a committed file or Actions artifact.
- **Came up because:** the laptop can't make an Actions artifact; committing evidence scatters the repo (the .proofs mistake).
- **Options:** commit to docs/ (rejected - scatter); Actions artifact (laptop can't); PR comment (chosen).
- **Chose comment over the others because:** postable from laptop or cloud, human-visible, machine-parseable; nothing committed. Gave up native check-run status (deferred, ARN-419).
- **Where:** review/sdlc-review.yml, proof/sdlc-verification.yml, post-*-record.sh.

## Autonomy tiers live in one config
- **Decision:** auto-merge-vs-human logic lives only in autonomy.yaml; a repo may override stricter.
- **Came up because:** Rita wants one tweakable place for the fully-autonomous->human dial.
- **Options:** per-workflow hardcode (drifts); one global config (chosen).
- **Chose global config because:** change a threshold once, every repo follows. Gave up per-repo independence except stricter overrides.
- **Where:** autonomy.yaml, gates/sdlc-automerge.yml.
