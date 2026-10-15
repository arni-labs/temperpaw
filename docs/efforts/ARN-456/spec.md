# ARN-456 spec

## Ask

One child of Effort. States: `Open → Answered` and `Withdrawn`.

| kind | Waiting | Close |
|---|---|---|
| decide | Rita picks an option | Answer |
| do | Rita does one act | Answer |
| fyi | no | RecordFyi, born Answered |

Same fields either way: `need`, `options` or `act`, `chose`, `why`, `who`, `stalls`, `pr_url`, `effort_id`.

`stalls` is true only when this Ask is why the Effort is Stalled (nothing left the agent can do). Partial blocks stay Open while the Effort stays Building. "Want to continue?" is not an Ask.

Raise does not Stall. The agent calls Effort.Stall when it is idle and waiting. Answer does not Resume. The implementer calls Effort.Resume after answering the stall Ask, passing that `ask_id`.

Adjudication is deleted. Resume no longer takes `adjudication_ids`.

## Review doors

Stack `REVIEW.md` is the only global reviewer prompt. Repo `REVIEW.md` is only what is true in that project. Reviewers return one JSON object of rubrics (yes/no and scores). Code applies the thresholds in stack `REVIEW.md`.

Fix-it failures fail PassReview / send RequestChanges. They are not a human Ask.

Any risk flag true → raise a do Ask with `pr_url`, then `ConfirmMerge` (sets `merge_risk_clear`). No risk flags → `MarkRiskClear`. Merge requires `merge_risk_clear` (Cedar L0/L1 still applies).

L1 cannot require three ReviewRuns. PassReview still requires `panel_started` and `review_fix_it_clear`. Three Recorded runs remain the rule the implementer follows. GitHub `validate.py` still fails open fix-it.

## Assess review spiral

After three unresolved panel rounds the implementer runs the spiral agent (stack `ASSESS-REVIEW-SPIRAL.md`, harness list in `review/panel.json`, Fable 5.1 first). Always an agent. It writes a stall Ask. PassReview and Merge refuse until that Ask is Answered and the Effort is Resumed.

## PR

Body starts with what this solves, why it exists, how, and links to the proof/review report. Then decisions. Every agent GitHub text goes through unspeak first.
