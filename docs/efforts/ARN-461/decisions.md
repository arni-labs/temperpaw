# Decision log — ARN-461

**Decision:** (2026-09-03) Three chain_* modules, not an extension of `review_gate_lifecycle`.
**Came up because:** Rita said WASM inspects rows and retracts a bool. `review_gate_lifecycle` already dispatches PassReview / AttachProofPacket.
**Options:** (1) add checks inside `review_gate_lifecycle`; (2) one mega `chain_sdlc_ready`; (3) one module per verb.
**Chose (3) over (1) and (2) because:** (1) hides more dispatch. (2) mixes review, proof, and commit pin. What we gave up: three Cargo.toml files instead of one.
**Where:** `os-apps/paw-patrol/wasm/chain_review_ready`, `chain_proof_ready`, `chain_merge_ready`.

---

**Decision:** (2026-09-03) Merge WASM on_failure returns Proving. The agent still fires Merge.
**Came up because:** Merge is Proving → Merged. A self-loop bool cannot pin `head_sha` until Merge names it.
**Options:** (1) new CheckRecords verb before Merge; (2) Merge stays Merged on a failed check; (3) RetractMerge back to Proving and clear the bools.
**Chose (3) over (1) and (2) because:** (1) is another agent step. (2) leaves a false Merged. Merge does not call GitHub, so retracting the state is safe. What we gave up: a brief Merged flicker if the rows fail.
**Where:** `effort.ioa.toml` Merge / RetractMerge.

---

**Decision:** (2026-09-03) CI lists Temper rows by commit. It does not read PR comments.
**Came up because:** The vendored gate scraped `sdlc-review-record-b64` comments and folded a hidden `<!-- sdlc-review -->` comment.
**Options:** (1) keep comments as fallback; (2) Temper only, fail closed.
**Chose (2) because:** Rita said one book and no hidden comment. What we gave up: a PR with only a comment record and no Temper rows fails until the implementer writes the rows.
**Where:** `.github/workflows/sdlc-review.yml`, `sdlc-verification.yml`.
