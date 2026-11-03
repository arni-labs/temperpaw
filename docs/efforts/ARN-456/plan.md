# ARN-456 plan

## TemperPaw

- Add `ask.ioa.toml`. Delete `adjudication.ioa.toml`.
- CSDL: Ask + Asks. Drop Adjudication. Effort gains AskIds and the review-outcome bools. ReviewRun gains Rubrics, FixItFailed, RiskFlags.
- Effort: AttachAsk, MarkFixItClear, MarkRiskClear, ConfirmMerge. Stall from Building/InReview/Proving/Deploying. Resume takes `ask_id`.
- Cedar: Ask on admin/read/agent write. Effort gains the new actions.
- Foundation needles for Ask, no Adjudication.
- TemperPaw `REVIEW.md`: project fix-it (entity-first, WASM dispatch) and project risk (Cedar).

## Stack

- `REVIEW.md` — global rules, return JSON, thresholds.
- `ASSESS-REVIEW-SPIRAL.md` — stall brief.
- `review/panel.json` — `spiral` harness list.
- `review/rubrics.py` — parse JSON, fix-it fail, risk flags.
- `run-panel.py` / `run-cloud-panel.py` / `validate.py` / `sdlc.yml` use those two files only.
- Delete reviewer-prompt.md, rubric.md, code-quality-review.md.
- `run-arbiter.py` reads the spiral prompt and the harness list. No empty-agent fallback.
- AGENTS.md, temper-agent, arni-mode: Ask inbox, PR shape, unspeak, spiral.

## Live

Load-inline Ask + Effort + ReviewRun + CSDL. Do not fire a ship Request.
