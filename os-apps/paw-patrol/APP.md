# paw-patrol

Operational control plane for TemperPaw maintaining itself.

Ordinary agent sessions create a lightweight Effort directly. The autonomous
Patrol factory is a separate, optional workflow for scheduled or machine-routed
work; its WorkCycle protocol is not a prerequisite for ordinary tasks.

## Entity Types

### WorkRequest
WorkRequest means human or manager-agent intent for the optional factory: "do this work", "clean this
up", or "investigate this thing." OpenClaw, Discord, or a human dashboard
submits here instead of writing directly to paw-pm.

### Intent
Optional intake for requests that need triage. `Accept` creates an Effort,
without checking for committed documents. Ordinary sessions create Efforts
directly and do not walk the intake sequence.

### PatrolRequest
Legacy name for request intake. New intake must use WorkRequest; PatrolRequest
stays readable only for old entity history and compatibility tests.

### Signal
Signal means observed evidence: a machine-observed event from Discord, Datadog,
GitHub, schedules, or repo sweeps. `Signal.Ingest` routes actionable failures
through `signal_router`; obvious noise is archived visibly.

### PatrolRun
PatrolRun means active investigation. Risk Patrol creates a PatrolRun for
agent-driven sweeps such as `datadog_observability` and `github_repository`;
the run queues a capable WorkerRun, records evidence, opens durable
findings/cases/work, and completes or escalates through Temper actions.

### FactoryCase
The operational case that groups one request or signal, its risk floor, linked
paw-pm Issues, WorkCycle, worker runs, reviews, evaluations, and proofs.

### WorkCycle
The Patrol-owned implementation contract. It replaces the need for a separate
paw-harness app for this factory slice and tracks planning, implementation,
testing, review, proof, and completion. L3 work pauses in
`AwaitingHumanStartApproval` before any WorkerRun is queued, then pauses again
in `AwaitingHumanCompletionApproval` after reviewer, evaluator, and ProofPacket
gates pass.

Plans are visible WorkCycle state, not private worker scratch. Intake WASM
modules write a structured plan-mode markdown plan, and the local Codex worker
runs a read-only Codex Plan Mode pass before implementation. That pass revises
the WorkCycle plan through `WorkCycle.RevisePlan`, increments
`plan_revision_count`, and injects the approved plan into the mutating Codex
run.

### Effort
A simple task record: **Open → Done or Cancelled**. Create one for a new task,
reuse it when resuming, and keep the objective in `task_summary` and context in
`task_detail`. `intent_ref` can link a useful document; it is optional.

- `Update`: edit the objective and context; resumes legacy records as Open.
- `AttachPullRequest(pull_request_urls)`: append one URL. Attach all related
  PRs, including several PRs from the same repository.
- `RecordEvidence(evidence)`: append a plain result or evidence link.
- `Complete(result_summary)`: record the delivered result as Done.
- `Cancel(result_summary)` and `Reopen`: stop or resume the same task.

No review quorum, proof packet, GitHub file check, deployment, or lifecycle
ceremony is required. Completion never merges code or deploys an application.
Use the existing repository delivery route for authorized operations. Legacy
states remain readable and can move directly to Open, Done, or Cancelled.

### DsfDeploy
Deep Sci-Fi deploy tool. Merge the PR on the named computer, watch
`health_url` for the merge sha, git-revert if it never becomes healthy.
Reuses `release_run_lifecycle`. Live `ReleaseRun` rows are not renamed.

### TemperDeploy
TemperPaw image deploy tool. `Request` waits until GHCR has `image_tag`,
then upserts Railway `IMAGE_TAG`, redeploys, polls `/paw/version` until
the live sha matches, and restores the previous tag on rollback. Railway
project/service ids are pinned in the trigger config. `CheckHealthy`
fires `ContractProbe.RunScan` on `probe_id` (default `mcp-455-lists`).

### ContractProbe
LatencyDiag-class HTTP probe. `RunScan` takes no parameters. Path,
filter, and `max_ms` are set at create. The default seeded row
`mcp-455-lists` is empty-equality `GET /tdata/DesignLanguages` with
`max_ms` 800. Ready and Failed re-run every 6h.

### WorkerRun
One execution attempt by a registered worker. The first worker type is the Mac
mini local Codex worker. It claims queued work from Railway Temper over SSE,
starts local Codex with ChatGPT auth, and self-reports results.

### ReviewRun
A review record used by the optional autonomous WorkCycle factory. It is not
required for an Effort. For ordinary tasks, apply the shared review guidance:
read changed code, independently exercise a meaningful changed feature, and
report demonstrated defects within the accepted scope. Plain findings and
verification results can be recorded as Effort evidence or on the PR.

### EvaluationRun
Automated gate execution and result capture for tests, proof requirements,
policy gates, architecture checks, and targeted live verification evidence. If
an evaluation fails while the WorkCycle is in review, Patrol treats it like
requested rework and queues the implementer again with the failing evidence.

### ProofPacket
Structured evidence used by the optional autonomous WorkCycle factory. Ordinary
Efforts use plain evidence and do not need a packet, schema, or hosted report.

### RiskRule
Explicit rule that sets a minimum risk lane from concrete evidence. Agents may
raise risk but cannot silently lower the rule-derived floor.

### RepoGraphSnapshot
Recurring codebase/dependency graph snapshot for routing, quality cleanup,
security sweeps, and agent orientation. The local Codex worker performs an
agent-led investigation of the repo/dependency graph, giant modules, duplicate
logic, specs, Cedar policies, WASM modules, dependencies, tests, proofs,
security drift, and readability. Patrol validates the structured findings,
opens QualityFinding/SecurityFinding entities, records the visual summary, and
dispatches `AssessmentComplete` from the agent evidence unless a configured real
assessment Session is available.

### QualityFinding
Readable code health finding: giant modules, duplicated logic, TODO/HACK
band-aids, missing proofs, hidden orchestration, polling loops, and related
cleanup debt.

### SecurityFinding
Security or authorization finding: Cedar drift, secret handling, dangerous tool
surface, dependency risk, or risky deploy/billing/provider change.

### DailyBrief
Daily visual and textual summary of completed work, open risks, new findings,
proof packets, and escalations. It is an agent-driven DailyBrief Session plus
local Codex WorkerRun: `daily_brief_lifecycle` gathers source facts, creates the
Session record, queues the local Codex WorkerRun, and attaches both. Codex writes
the visual daily summary through `DailyBrief.Render`, then self-reports through
the normal reviewer/evaluator/proof gates so the brief can include judgment,
diagrams, and readable prioritization.

### PatrolSchedule
Recurring Patrol job that schedules repo sweeps and daily briefs from Temper
state transitions. It uses `schedule_at` to fire `Trigger`, then
`patrol_schedule_lifecycle` creates RepoGraphSnapshot and DailyBrief entities.
Patrol seeds `patrol-default-daily-maintenance` as an active daily schedule so a
fresh install has the recurring maintenance loop visible in Temper immediately.

## PatrolSchedule And CronJob

PatrolSchedule intentionally does not reuse the paw-agent CronJob entity.
Both entities use Temper's schedule_at timer effect, but they are different
business state machines.

CronJob is for scheduled agent Session creation: each trigger computes the next
run and declaratively spawns a `Session`.
PatrolSchedule is for scheduled Patrol maintenance: each trigger creates
Patrol-native `RepoGraphSnapshot` and `DailyBrief` entities so repo health and
briefs stay in the Patrol audit graph.

If the cadence parsing or schedule conventions drift, factor shared helpers or
platform conventions. Do not route Patrol maintenance through CronJob unless
CronJob stops meaning "spawn a scheduled agent Session."

## Quality Cleanup Status

Detection is not cleanup. Patrol's repo sweep opens `QualityFinding` and
`SecurityFinding` entities for cleanup debt, and accepted findings become
Patrol `WorkCycle`s. The scanner makes giant WASM modules visible and
reviewable; it does not mark them fixed.

As of this implementation, giant WASM modules remain work to be done. Known
examples include Monty REPL, provider_caller, context_preparer, and
route_message. Each should become a scoped cleanup `WorkCycle` with reviewer,
evaluation, and proof gates before the cleaned area is ratcheted.

## Intake

Submit everything to Patrol first:

```text
You / OpenClaw / Discord / Datadog / GitHub / schedule
        |
        v
WorkRequest / legacy PatrolRequest / Signal / PatrolRun
        |
        v
FactoryCase + risk floor
        |
        +--> optional paw-pm Issue
        |
        v
WorkCycle -> WorkerRun -> ReviewRun -> EvaluationRun -> ProofPacket
        |
        v
Review + evaluation + proof gates close WorkCycle before human escalation
```

Use the three intake shapes this way:

| Shape | Use it for | What Patrol creates |
| --- | --- | --- |
| WorkRequest | A human or manager-agent says "do this work" | FactoryCase, optional paw-pm Issue, WorkCycle, and a risk-gated WorkerRun |
| Signal | Observed evidence or error from Datadog, Discord, GitHub, a webhook, or another agent | Normalized/triaged Signal, then a FactoryCase and WorkCycle if actionable |
| PatrolRun | Active investigation by an agent, such as Datadog or GitHub patrol | WorkerRun for the patrol agent, evidence, Signals, findings, cases, work, and ProofPackets |

For the optional factory: Do not submit new work directly to paw-pm. Paw-pm is durable project memory;
Patrol owns intake, triage, risk, worker assignment, review, evaluation, and
proof. Patrol creates or links paw-pm Issues only after it decides the input is
real work.

High-risk work follows the same visible state graph, but with human approval
gates:

```text
L3 WorkCycle.Planned
        |
        v
AwaitingHumanStartApproval
        |
        | ApproveHumanStart
        v
WorkerRun -> ReviewRun -> EvaluationRun -> ProofPacket
        |
        v
AwaitingHumanCompletionApproval
        |
        | ApproveHumanCompletion
        v
Complete
```

## Agent Submission API

Use these OData action paths when an agent, OpenClaw, Discord bridge, script, or
human operator submits work directly to Temper. The examples omit auth headers;
callers still need a valid bearer token and principal headers that Cedar allows.

### Ordinary task

Create an empty record (or supply only `Id`), then set its context with `Update`.
Strict entity creation rejects task fields; they belong to the declared action.

```http
POST /tdata/Efforts
{}

POST /tdata/Efforts('<id>')/TemperPaw.Patrol.Update
{"task_summary": "Fix gallery loading", "task_detail": "Show loaded images after navigation", "repo": "arni-labs/foundry", "branch": "fix-gallery", "intent_ref": ""}

POST /tdata/Efforts('<id>')/TemperPaw.Patrol.AttachPullRequest
{"pull_request_urls": "https://github.com/arni-labs/foundry/pull/123"}

POST /tdata/Efforts('<id>')/TemperPaw.Patrol.RecordEvidence
{"evidence": "Opened the gallery, navigated away and back, and verified images loaded."}

POST /tdata/Efforts('<id>')/TemperPaw.Patrol.Complete
{"result_summary": "Gallery loading fixed and verified."}
```

### Optional autonomous factory

The following WorkRequest protocol is for the resident Patrol worker. It is not
the default for interactive Codex, Claude, or Foundry task sessions.

### Human or manager-agent task

Create a WorkRequest, then submit it:

```http
POST /tdata/WorkRequests
{}

POST /tdata/WorkRequests('<id>')/TemperPaw.Patrol.Submit
{
  "source": "openclaw",
  "request_text": "Fix the broken dashboard detail view and prove it live.",
  "requester_id": "openclaw"
}
```

Expected result: WorkRequest moves through Patrol routing, then links to a
FactoryCase. If the request is accepted, Patrol creates or links a paw-pm Issue,
creates a WorkCycle, applies RiskRule floors, and either queues a WorkerRun or
pauses in `AwaitingHumanStartApproval` for L3 work.

### Observed evidence or error

Create a Signal, then ingest the raw evidence:

```http
POST /tdata/Signals
{}

POST /tdata/Signals('<id>')/TemperPaw.Patrol.Ingest
{
  "source": "datadog",
  "payload": "{...raw alert, trace, log, webhook, or agent evidence...}",
  "source_url": "https://app.datadoghq.com/...",
  "severity": "error"
}
```

Expected result: Signal is normalized and triaged. Noise is archived visibly.
Actionable evidence links to a FactoryCase, WorkCycle, WorkerRun, and ProofPacket
through `signal_router`.

### Active patrol run

Create a PatrolRun, configure the patrol kind and worker capability, then start
it:

```http
POST /tdata/PatrolRuns
{}

POST /tdata/PatrolRuns('<id>')/TemperPaw.Patrol.Configure
{
  "patrol_kind": "datadog_observability",
  "summary": "Investigate current TemperPaw/TemperPaw runtime health.",
  "requested_by": "risk-patrol",
  "required_capabilities": "datadog_query"
}

POST /tdata/PatrolRuns('<id>')/TemperPaw.Patrol.Start
{}
```

For GitHub issue and PR patrols, use:

```json
{
  "patrol_kind": "github_repository",
  "summary": "Triage open issues, PRs, checks, reviews, and repository anomalies.",
  "requested_by": "risk-patrol",
  "required_capabilities": "github_query"
}
```

Expected result: `patrol_run_lifecycle` queues a WorkerRun for a worker that
advertises the required capability. The patrol agent does read-only
investigation through its tools, returns structured evidence, and Patrol records
the evidence through `PatrolRun.RecordEvidence`. Datadog patrol fans out to
Signals, ObservabilityFindings, FactoryCases, risk-gated WorkCycles, and visual
ProofPackets. GitHub patrol fans out to Signals, FactoryCases, WorkCycles, and
ProofPackets for actionable repository issues or PR anomalies.

## Review And Rework Loop

Every implementation WorkerRun is followed by an independent ReviewRun,
EvaluationRun, and ProofPacket. You should not be the first reviewer of normal
agent work.

```text
WorkerRun.ReportDone
        |
        v
ReviewRun + EvaluationRun + ProofPacket draft
        |
        +--> ReviewRun.Approve        -> evaluation/proof can complete
        +--> ReviewRun.RequestChanges -> WorkCycle.RequestChanges
        +--> ReviewRun.Escalate       -> WorkCycle.Fail + FactoryCase escalation
        +--> ReviewRun.Fail           -> WorkCycle.Fail + FactoryCase escalation
```

`ReviewRun.RequestChanges` is the ordinary unhappy path. Patrol dispatches
`WorkCycle.RequestChanges`, creates a new implementer WorkerRun, reuses the
same branch/worktree when the previous WorkerRun had one, and prompts the
implementer with the reviewer feedback. This can repeat until review and
evaluation pass.

`ReviewRun.Escalate` means the reviewer cannot safely decide. Typical reasons:
the change is security-sensitive, policy-sensitive, production-impacting,
deployment/secrets/billing/data-migration related, user-facing in a risky way,
or the available proof is not enough for an agent to approve.

`ReviewRun.Fail` means the reviewer machinery failed rather than judged the
implementation. Examples: the reviewer timed out, crashed, could not write back
to Temper, or the reviewer output is invalid because it did not include a
recognized verdict marker.

Webhook intake is provided by `paw-ingest`, with routes seeded by Patrol:

```text
POST /triggers/webhook/patrol-request
  -> WebhookEvent -> WorkRequest.Submit

POST /triggers/webhook/patrol-signal
POST /triggers/webhook/patrol-datadog
POST /triggers/webhook/patrol-github
POST /triggers/webhook/patrol-discord
  -> WebhookEvent -> Signal.Ingest
```

Use `patrol-request` for human or manager-agent asks. Use the signal routes for
observed failures, alerts, traces, GitHub events, and Discord incidents.
PatrolRequest remains a legacy entity set, but new human or manager-agent work
flows through `WebhookEvent -> WorkRequest.Submit`.

## WASM Modules

Patrol's business logic lives in WASM integrations on entity actions. The Rust
server hosts Temper and triggers, but these modules own the workflow decisions:

- `patrol_request_router`: turns an accepted `WorkRequest` or legacy
  `PatrolRequest` into a
  `FactoryCase`, optional paw-pm Issue linkage, `WorkCycle`, and queued
  `WorkerRun`.
- `patrol_run_lifecycle`: starts Risk Patrol investigations such as Datadog
  Observability Patrol, picks a registered worker with the required
  capabilities, and escalates visibly if no capable worker is available.
- `signal_router`: routes Datadog, Discord, GitHub, and other machine signals
  into Patrol cases, work cycles, and worker assignments when the signal is
  real work.
- `repo_sweep_lifecycle`: starts repo graph scans, assigns the local worker,
  and fans scan results into quality and security findings.
- `worker_run_lifecycle`: reacts to worker success or failure, records proof
  evidence, and starts the independent review/evaluation gates.
- `review_gate_lifecycle`: applies reviewer verdicts and evaluation results to
  the `WorkCycle` and final `ProofPacket`.
- `finding_lifecycle`: turns accepted quality/security findings into cleanup
  `WorkCycle`s and links the source finding to the resulting work.
- `work_cycle_lifecycle`: handles high-risk approval transitions, completion,
  failure, and source-finding resolution.
- `patrol_schedule_lifecycle`: keeps recurring Patrol schedules inside Temper
  by creating repo sweeps and daily briefs from schedule transitions.
- `daily_brief_lifecycle`: queues the local Codex DailyBrief WorkerRun from
  finished proof packets, findings, and open risks.
- `release_run_lifecycle`: DsfDeploy / ReleaseRun merge, watch, and revert.
  Merge uses the configured GitHub App. `github_token` is fallback
  only. Do not put a PAT in the vault when the App is installed.
- `chain_file_ready`: GET a Temper File and require Ready/Locked. Fired by
  AttachReviewFile / AttachProofFile.
- `temper_deploy_lifecycle`: one side effect per TemperDeploy trigger
  (IMAGE_TAG swap, /paw/version poll, restore previous tag).

## Default Schedule

Patrol seeds a default daily maintenance schedule:

```text
PatrolSchedule: patrol-default-daily-maintenance
        |
        v
ActivateComplete schedules next Trigger
        |
        v
Trigger creates RepoGraphSnapshot + DailyBrief
        |
        v
TriggerComplete schedules the next daily Trigger
```

Pause or edit that PatrolSchedule in Temper if production should wait before
daily repo sweeps and briefs begin.

If a schedule fails because a required WASM module, policy, or secret was not
loaded yet, repair the missing dependency and dispatch `PatrolSchedule.Recover`.
Recovery recomputes `next_run_at` through the same `patrol_schedule_lifecycle`
activation path and keeps the repair visible in the entity history.

## Mac Mini Worker

The Mac mini runs `paw-codex-worker` under launchd as the `openclaw` user. The
worker connects outbound to Railway TemperPaw's `/tdata/$events`, watches for
`WorkerRun.Queued`, claims work through Cedar, starts local Codex, then reports
completion back to Temper. Codex subscription auth and Datadog MCP auth also
belong to `/Users/openclaw`, so worker worktrees, env files, launchd plist, and
doctor checks all stay under that user.
Patrol sets `WorkerRun.allowed_worker_id` from `local_codex_worker_id` so only
the registered local worker principal can claim the queued run. Patrol sets
`WorkerRun.worktree_path` from `local_codex_worktree_root` so queued runs point
at the worker host's worktree root rather than the machine that submitted the
request or signal.
Patrol also sets `WorkerRun.required_capabilities`; Datadog Patrol requires
`datadog_query`, so a generic local Codex worker cannot silently claim
observability work without the Datadog read capability.

This is resource-bound ownership. The principal ID identifies the caller; the
resource assignment field identifies which exact Temper entity the caller is
allowed to operate on. A worker principal with `id = "worker-a"` is not
authorized merely because it is a worker. It is authorized for a WorkerRun only
when that WorkerRun carries `allowed_worker_id = "worker-a"` for claim or
`worker_id = "worker-a"` for start/report actions. Review and evaluation use
the same pattern with `reviewer_id` and `evaluator_id`, so a valid reviewer can
review only the ReviewRun assigned to that reviewer.

For L3 requests, no `WorkerRun.Queued` event exists until a human or supervisor
approves `WorkCycle.ApproveHumanStart`. After proof gates pass, Patrol waits for
`WorkCycle.ApproveHumanCompletion` before marking the WorkCycle and FactoryCase
complete.

## OpenClaw

OpenClaw may act as a manager surface: summarize events, route approvals, and
submit WorkRequests, Signals, or PatrolRuns. It should not write code or mutate
repo files in v1.
