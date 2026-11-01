# paw-agent

Core agent lifecycle management. Defines agent identities, sessions, souls, skills, memory, teams, and cron jobs.

## Entity Types

### Agent
Persistent agent identity (team member). Sessions are spawned from agents.

- **States**: Created -> Active -> Archived
- **Key actions**: `Configure` (name, role, soul_id, team_id, model, tools), `Update`, `Archive`

### Session
Ephemeral agent run. Created per task, transitions through execution phases.

- **States**: Created -> Configuring -> Running -> Complete/Failed/TimedOut
- **Key actions**: `Configure`, `Start`, `Heartbeat`, `Complete`, `Fail`, `TimeoutFail`
- **WASM**: `sandbox_provisioner` provisions compute on session start

### Soul
Agent personality — identity and communication style. Optional; task agents skip this.

- **States**: Created -> Published -> Archived
- **Key actions**: `Publish` (with content_file_id), `Archive`

### Skill
Reusable knowledge injected into agent prompts. Scoped by agent/soul name.

- **States**: Created -> Published -> Archived
- **Key actions**: `Publish` (content, scope, skill_type), `Archive`

### Memory
Cross-session persistent knowledge, scoped to an agent_id.

- **States**: Active -> Archived
- **Key actions**: `Save` (key, content, memory_type, agent_id), `Update`, `Recall`, `Archive`

### Team
Group of agents working together.

- **States**: Active -> Archived
- **Key actions**: `Configure`, `AddMember`, `RemoveMember`, `Archive`

### Project
A coordinated effort. Teams, sessions, skills and memories point at it by `project_id`, and so do paw-pm's issues and cycles (ADR-0018).

- **States**: Active <-> Paused, Active -> Completed, then Archived
- **Key actions**: `Configure` (name, description, owner_agent_id, app_ids), `Update`, `Pause`, `Resume`, `Complete`, `Archive`

### CronJob
Scheduled agent runs using cron expressions. Self-scheduling via `schedule_at` effects.

- **States**: Created -> Active <-> Paused -> Expired
- **Key actions**: `Configure`, `Activate`, `Trigger` (auto-spawns Session), `Pause`, `Resume`; `RecordLastSession` is an operator-only repair
- **WASM**: `cron_compute_next` parses cron schedule and substitutes message templates; a job with no template sends its own `user_message`

### Plan
Plan entity with review workflow. Worker creates, lead reviews, human escalation optional.

- **States**: Draft -> UnderReview -> Active -> Completed/Failed (+ Escalated, Paused)
- **Key actions**: `SubmitForReview`, `Approve`, `RequestChanges`, `Escalate`, `AddTask`, `Complete`

## Setup

Depends on `paw-fs` for file storage (soul content, agent instructions). Install from Genesis with a pinned ref such as `temper.install_app({"app_ref":"temperpaw/paw-agent@HASH","follow_policy":"pinned"})`.
