# Spec: stage-3 S1 - shadow verdicts (ARN-431)

The S1 "shadow verdicts" row of the shadowing plan in `stack/docs/stage3-spec.md`.
Three parts, in order: publish S0 to Genesis + install on prod; the sweep; the
acceptance run. No entity or module changes (S0 shipped them); shadow only -
nothing blocks a merge, nothing is written to GitHub.

## Part 1 - Genesis publish of S0 (closes ARN-430's source-of-truth leg)

Publish paw-patrol at temperpaw `main` (the S0 merge `7fcfae791`) to Genesis and
hot-install it on openpaw-production, so the pinned ref serves the S0 specs.
Recipe: the `genesis-temperpaw-deploy` skill. paw-patrol ONLY; nothing else on
prod is touched. Delta push (not the rsync snapshot); unscoped `http.extraHeader`
pair (the URL-scoped form drops Authorization); install via
`POST /paw/apps/install-from-genesis` with the prod `TEMPER_API_KEY`. Verify:
`ShadowVerdict`, `Adjudication`, `StandingDecision` exist on prod (boot log or an
OData read of the pinned ref).

## Part 2 - the sweep

`stack/shadow/shadow-sweep.py` (+ a `shadow/README.md`) and a temperpaw workflow
`.github/workflows/shadow-sweep.yml` (nightly cron + `workflow_dispatch` with a
`pr_numbers` input; clones stack for the script like the gates do). Per PR in the
window:

1. **CI verdicts** - read the four gate check-run conclusions on the PR head from
   the GitHub Checks API: `sdlc-planning`, `sdlc-decisions`, `sdlc-review`,
   `sdlc-verification`. Map `success`→pass, `failure`/`timed_out`/`cancelled`→fail,
   `skipped`/`neutral`/missing→na.
2. **Read the PR's b64 record comments** - the `sdlc-review-record-b64` and
   `sdlc-proof-record-b64` markers (same regex the gates use), newest per head.
3. **Ingest into prod Temper (pull-mode, no webhook)** - create the record entity
   (id keyed by pr+head+kind) and dispatch its `Ingest` action with
   `comment_body`, so `record_ingest` parses and the state machine writes
   `IngestRecord`/`IngestProof`. tenant `default`, `TEMPER_API_KEY`.
4. **Temper verdict per gate**, computed as:
   - `sdlc-review` - from entity state: the ReviewRun for the head is `Recorded`
     AND `open_act_on_count == 0` → pass; `Recorded` with open act-ons → fail; no
     Recorded run → na.
   - `sdlc-verification` (proof) - from entity state: the ProofPacket for the head
     is `Recorded` → pass (S0 `parse_ok` already refuses a failing verdict / empty
     changed_surface, so a Recorded packet is a passing proof); not Recorded → na.
     If no app code changed (the gate's `needs=false` rule) → na.
   - `sdlc-planning` - **not entity-derived** (S0 has no planning entity): a
     presence check mirroring the gate, computed from the PR content by running
     the SAME gate script `stack/gates/check-effort-artifacts.py` on the PR's
     branch/base/head + title/body. Documented as a mirror, so it agrees with CI
     by construction (a divergence would mean the script is non-deterministic).
   - `sdlc-decisions` - same: run `stack/gates/check-decision-log.py` on the PR
     body. Mirror, not entity-derived.
5. **Write** `ShadowVerdict{pr, gate, temper_verdict, ci_verdict}` via `Record`
   then `MarkAgree`/`MarkDisagree` (agree = temper_verdict == ci_verdict, computed
   by the sweep - the entity cannot compare params, ARN-430 residual).
6. **Print a disagreement summary table.** Never block, never write to GitHub.

Precisely what is mirrored vs entity-derived: review and proof verdicts are read
from Temper entity state (the real S0 machine output); planning and decisions are
recomputed from PR content with the gates' own scripts (S0 has no entities for
them). This is stated in the sweep README and here so a reader knows which two
columns are a true shadow of the state machine and which two are a consistency
check on the gate scripts.

## Part 3 - acceptance

Run the sweep via `workflow_dispatch` for PRs 477 480 481 482 484 476 against
prod. Report the ShadowVerdict rows and the agreement table verbatim. Honest
disagreements are FINDINGS, not failures.

## What this is not
No CI change to the four gates, no Cedar, no rename, no gate flip (that is S2).
The sweep is a script, not a WASM module or a Temper reaction (that is later).
ShadowVerdict is written only by the sweep.

## Test plan (red-green)
`stack/shadow/` unit tests for the pure pieces: the CI-conclusion→verdict mapping;
the b64 marker extraction (reuse the ARN-430 fixtures); the per-gate Temper-verdict
computation given a stub entity-state JSON (Recorded + open_act_on_count cases,
proof not-required case); the agree computation. The Temper/GitHub I/O is behind
thin functions tested with recorded fixtures; the live run is the acceptance drive.
