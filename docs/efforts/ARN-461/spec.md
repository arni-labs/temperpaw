# ARN-461 spec

## Door

`PassReview`, `AttachProofPacket`, and `Merge` stay the verbs. The implementer still fires them. The machine does not believe the declaration.

| Verb | WASM | Looks at | On failure |
|---|---|---|---|
| PassReview | `chain_review_ready` | Effort `review_run_ids` | RetractReviewPassed |
| AttachProofPacket | `chain_proof_ready` | the attached ProofPacket | RetractProofAttached |
| Merge | `chain_merge_ready` | both, plus `head_sha` | RetractMerge |

Each module GETs sibling rows over `/tdata`. It does not dispatch. It does not scrape GitHub.

## Review rules (same as `stack/review/validate.py`)

Across the attached ReviewRuns that are `Recorded` with `record_present`:

- `commit` is a 40-char lowercase hex sha, and every run shares it
- mandatory model reviewers from `review/panel.json` (`grok`, `codex`, `fable`) — at least two of three present on `reviewer_id` or `reviewers_ran`
- no `fix_it_failed`
- no unresolved `act-on` finding (recomputed from `findings`, not from `open_act_on_count`)

`RecordPanel` may write `commit`, `reviewers_ran`, `findings`, and `risk` so one action is enough. `IngestRecord` stays for the comment shadow.

## Proof rules (same as `stack/proof/validate.py`)

The attached ProofPacket is `Recorded` with `record_present`. Its `commit` / `changed_surface` / `blast_radius` / `features` / `tests` / `independent_verifier` pass the ingest proof shape: changed+blast rerun, verifier agrees, no failing feature, tests pass.

## Merge

Guards stay the existing bools. After the transition, `chain_merge_ready` checks the rows again and pins `commit` to Merge `head_sha`. Failure returns Proving and clears `review_passed` and `proof_attached`. Merge still does not call GitHub.

## CI

`sdlc-review.yml` and `sdlc-verification.yml` ask production Temper for ReviewRuns / ProofPackets whose `commit` equals the PR head. They do not read a hidden PR comment. A walked Effort with Recorded rows is enough. No record on Temper fails closed.

## Out of scope

`review_gate_lifecycle` is unchanged. Genesis publish is a later walk.
