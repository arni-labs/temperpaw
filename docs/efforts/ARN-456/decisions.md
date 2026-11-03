# ARN-456 decisions

**Decision:** One Ask type, three kinds (decide / do / fyi). Not three entities.
**Came up because:** Rita wanted an inbox for decide, do, and agent-only FYI.
**Options:** (1) three entities; (2) Effort fields only; (3) one Ask.
**Chose (3) because:** decide and fyi are the same noun; do is the same inbox. Three types would duplicate Stall and decisions.md.
**Where:** ask.ioa.toml.

---

**Decision:** Raise does not Stall. Answer does not Resume. The agent Stalls when idle. Resume names the stall Ask.
**Came up because:** A partial block must not freeze the Effort. Entity triggers cannot be conditional.
**Options:** (1) trigger Stall on every Raise; (2) agent sets Stall/Resume.
**Chose (2) because:** (1) would Stall on the first open do.
**Where:** effort.ioa.toml Stall/Resume.

---

**Decision:** Delete Adjudication. Resume is answering the stall Ask.
**Came up because:** Adjudication was the S0 name for an owner ruling. Ask.Answer is that fact.
**Options:** (1) keep both; (2) delete Adjudication.
**Chose (2) because:** two ruling types is the mess we were leaving.
**Where:** removed adjudication.ioa.toml.

---

**Decision:** Fix-it rubrics fail review. Risk rubrics only choose auto-merge vs a do Ask.
**Came up because:** Unused code and intent drift are not human merge questions.
**Options:** (1) all rubrics feed human-merge; (2) split fix-it vs risk.
**Chose (2) because:** a PR that failed fix-it is not mergeable.
**Where:** stack REVIEW.md; rubrics.py.

---

**Decision:** Cedar is a project risk flag, not a global one.
**Came up because:** Cedar is TemperPaw.
**Options:** (1) global REVIEW.md; (2) repo REVIEW.md.
**Chose (2) because:** Cedar only exists in TemperPaw. A global flag would fail every other repo.
**Where:** os-apps is not the home; temperpaw REVIEW.md.

---

**Decision:** The stuck-loop prompt is ASSESS-REVIEW-SPIRAL.md. Always an agent. Harness list in panel.json, Fable 5.1 first.
**Came up because:** "Arbiter" hid the job. File-count fallback was a no-agent path.
**Options:** (1) no-agent file-count fallback; (2) always run an agent from panel.json spiral, Fable 5.1 first.
**Chose (2) because:** a spiral with no agent is another empty review. The prompt name is the job.
**Where:** stack ASSESS-REVIEW-SPIRAL.md; review/panel.json.

---

**Decision:** `chain_github_ready` fails if tenant `github_token` is missing, then probes the repo before contents. temper-agent does not teach the SDLC or this door.
**Came up because:** Rita asked what I had encoded in temper-agent, and whether the file-ready miss was a missing file or a missing Temper token. Production vault has no `github_token`. Railway `openpaw` has no `GITHUB_TOKEN`. The check was anonymous GitHub. `arni-labs/aya` is private (404). `nerdsane/temperpaw` is public (works). The agent's `gh` is a different token.
**Options:** (1) teach the door in temper-agent; (2) fail with an honest WASM error and put a token in the Temper vault.
**Chose (2) because:** temper-agent is how to use Temper. The factory order stays in AGENTS.md. Agents stumble when the error lies.
**Where:** `os-apps/paw-patrol/wasm/chain_github_ready/src/lib.rs`; stack `skills/temper-agent/SKILL.md`.

---

**Decision:** Production Cedar is patched in place for Ask. GitHub access is an arni-labs GitHub App, not a personal PAT.
**Came up because:** Live GET Ask was authorization_denied. Vault had no github_token. Rita asked for the durable credential that can see every factory repo, and forbade putting her login in the vault.
**Options:** (1) replace the whole 447k tenant policy with patrol.cedar; (2) add Ask to the live patrol blankets and Effort actions; (3) put the rita-aga gh session token in the vault; (4) a classic/fine-grained PAT (one owner, or her identity); (5) a GitHub App owned by arni-labs that mints an installation token per repo owner.
**Chose (2) and (5) because:** (1) would wipe every other app's permits. (3) is the login she refused. (4) is either one-org or still a person. The App is the factory. WASM mints; public repos with no install stay anonymous.
**Where:** PUT /api/tenants/default/policies; vault `github_app_id` + `github_app_private_key`; `chain_github_ready`.

---

**Decision:** Intent.Reopen takes Rejected back to Triaged.
**Came up because:** `intent-arn-455-aya-ui-redesign` sat in Triaged 24h waiting for a production GitHub credential and the state_timeout Rejected it. There was no way back.
**Options:** (1) create a new Intent id; (2) Reopen to Triaged on the same row.
**Chose (2) because:** the work is the same Intent. A timeout is not a human reject.
**Where:** intent.ioa.toml Reopen; patrol.cedar.

---

**Decision:** AttachReviewRun clears review_fix_it_clear, merge_risk_clear, and review_passed. Resume still goes to Building.
**Came up because:** Greptile said an old clearance can authorize Merge after a later failing run, and Resume from Proving or Deploying always returns to Building.
**Options:** (1) leave both; (2) reset clearance on AttachReviewRun and keep Resume to Building; (3) store the stall source stage and Resume there.
**Chose (2) because:** (1) lets PassReview stay true after a new failing ReviewRun. (3) fights the lock: Resume names the stall Ask and returns to Building so the implementer walks the doors again.
**Where:** effort.ioa.toml AttachReviewRun; effort.ioa.toml Resume.

---

**Decision:** Resume clears the same review and proof gates as RequestChanges.
**Came up because:** Fable showed Stall from Proving, then Resume to Building, then SubmitForReview / PassEvaluation / Merge with every guard still true and no new ReviewRun.
**Options:** (1) keep Resume as a state change only; (2) reset the RequestChanges bools on Resume; (3) Resume to the stall source stage.
**Chose (2) because:** the lock is Resume to Building. Walking the doors again only happens if the bools are false.
**Where:** effort.ioa.toml Resume.

---

**Decision:** RaiseBlocking sets stalls. RecordPanelFixItFailed sets fix_it_failed. Bool params do not write.
**Came up because:** Grok and Codex showed Raise(stalls=true) and RecordPanel(fix_it_failed=true) leave the bool store false. ShadowVerdict already documents this: booleans are set_bool, not params.
**Options:** (1) keep bool params and hope the kernel learns them; (2) one action per bool value, like MarkAgree/MarkDisagree.
**Chose (2) because:** that is the house pattern and it works today.
**Where:** ask.ioa.toml RaiseBlocking; review_run.ioa.toml RecordPanelFixItFailed.

---

**Decision:** The ReviewRun write permit keeps the agent_type when clause. Ask has its own copy.
**Came up because:** Fable showed the when clause moved from ReviewRun onto Ask, so any Agent type could RecordPanel.
**Options:** (1) leave ReviewRun open; (2) restore the same when on ReviewRun and keep it on Ask.
**Chose (2) because:** RecordPanel is the row PassReview reads. Widening it was not a recorded call.
**Where:** patrol.cedar ReviewRun permit.

---

**Decision:** RequestChanges clears panel_started, same as Resume.
**Came up because:** Fable and Codex showed RequestChanges resets the review bools but leaves panel_started true, so PassReview can succeed on the rework path with no new ReviewRun.
**Options:** (1) leave it; (2) reset panel_started on RequestChanges; (3) require a new ReviewRun count in L1.
**Chose (2) because:** Resume already does this for the stall path. (3) fights the lock that L1 cannot require three ReviewRuns.
**Where:** effort.ioa.toml RequestChanges.
