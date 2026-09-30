# Patrol (dark-factory work loop)

## Sub-features
20 entities. Three PARALLEL intake shapes (WorkRequest, Signal, PatrolRun) converge on FactoryCase -> WorkCycle -> WorkerRun -> ReviewRun -> EvaluationRun -> ProofPacket. ReleaseRun is a separate governed-release machine, not a step in the work loop. PatrolRequest is a LEGACY intake alias, readable-only.

## How to get to it (user POV)
Signals (webhooks, schedules) trigger patrols; workers pick up cycles; results surface as briefs and findings.

## Driving it
Create a WorkRequest (body {}), then dispatch TemperPaw.Patrol.Submit with source/request_text/requester_id. Read the WorkRequest back (Submitted -> Accepted/Linked once the router runs), then read FactoryCases/WorkCycles/WorkerRuns to see the WASM fan-out. Worker self-report is on WorkerRun (ReportDone/ReportFailed), not WorkCycle; there is no PassTests action.

## What proves it
Pass: a FactoryCase in Open with work_request_id pointing at the WorkRequest, and a linked WorkCycle. The trigger BOUNDARY is one-entity-one-action; post-trigger WASM legitimately fans out to several entities (signal_router creates four) - that is not a violation.

## Gotchas
L3 work stalls at WorkCycle AwaitingHumanStartApproval BY DESIGN (dispatch ApproveHumanStart to continue) - not a failure. Intake needs secrets temper_api_url / local_codex_worker_id / local_codex_worktree_root, or it RouteFaileds before any WorkerRun. There is no live 'Claude token' path (anthropic_managed is disabled); Codex auth is Mac-mini subscription auth. A WorkerRun stuck in Queued is the real provider boundary - mark Claimed onward verified-unreachable.
