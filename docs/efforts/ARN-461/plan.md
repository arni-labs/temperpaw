# ARN-461 plan

What we are addressing: Effort.Merge believed implementer bools. CI believed a hidden PR comment. Those were two books, and neither was the rows.

Expected end state: PassReview / AttachProofPacket / Merge retract unless Temper rows pass the same rules as validate.py. CI asks those rows by commit. No hidden comment.

1. Add `chain_review_ready`, `chain_proof_ready`, `chain_merge_ready` (host-tested pure checks + GET). Register in `app.toml`, `build.sh`, Cedar `http_call`.
2. Wire the three Effort actions and retract callbacks.
3. Let `RecordPanel` write `commit` / `reviewers_ran` / `risk` so one write can land a real record.
4. Point CI at Temper by commit. Drop the hidden `<!-- sdlc-review -->` comment.
5. Foundation tests for the spec/WASM/CI contract. Compile the new wasm blobs.
