# ADR-0018: Project as First-Class Entity

## Status

Accepted. Step 3 finished 1 Nov 2026; see the amendment at the end.

## Context

OpenPaw has no first-class "Project" concept. The idea of agents working together on a coordinated effort is scattered across three disconnected entities:

- **Team** (paw-agent) — groups agents via `team_id`, has a loose `harness_id` link
- **ProjectHarness** (paw-harness) — governs a repo's development workflow, owns WorkCycles
- **Project** (paw-pm) — tracks issues and cycles, but has no link to Team or Harness

These three don't reference each other. The dashboard has to guess what a "project" is by querying Teams. Skills are scoped by soul name (fragile and confusing). There's no way to declare which apps a project uses.

A project is any coordinated effort — code, research, content, operational. Not every project has a repo or a harness. Some agents (Paw) are cross-project. Apps are tenant-level capabilities, but a human needs to see which ones a specific project uses.

## Decision

### 1. New Project entity in paw-agent

Project is a simple entity in the core `paw-agent` app (always installed):

- **States:** Active → Paused → Archived
- **Fields:** `name`, `description`, `owner_agent_id`, `app_ids`
- **`owner_agent_id`:** The lead agent (e.g., Ren for Deep Sci-Fi). Cross-project agents like Paw can own multiple projects.
- **`app_ids`:** Comma-separated list of which apps this project uses (e.g., "paw-harness,paw-heal,paw-pm"). This is a declaration — not enforcement. Apps are still installed tenant-wide. This field tells the dashboard and the LLM caller what's relevant.

Project does NOT own harness_id, team_id, or skill_ids. Those entities point TO the project via their own `project_id` field.

### 2. Existing entities gain `project_id`

| Entity | App | Change |
|---|---|---|
| Team | paw-agent | Add `project_id`. A team belongs to a project. |
| Session | paw-agent | Add `project_id`. Work happens in the context of a project. |
| Skill | paw-agent | Add `project_id`. If set, the skill is project-scoped. If empty, it's tenant-wide. |
| Memory | paw-agent | Add `project_id`. If set, the memory is shared across all agents in the project. |
| Issue | paw-pm | Add `project_id`. Issues are tracked under a project. |

### 3. PM's Project entity is removed

paw-pm's `Project` entity is replaced by the paw-agent Project. Issues, Cycles, Labels, and Comments reference the paw-agent Project via `project_id`. If paw-pm isn't installed, Projects still exist — they just don't have issue tracking. PM becomes an add-on to the project, not the definition of it.

### 4. Skill scoping replaces soul-name matching

Current: LLM caller queries `Scope eq 'global' or Scope eq '{soul_name}'`
New: LLM caller queries `(project_id eq '' or project_id eq '{session_project_id}') and Status eq 'Active'` plus the agent's explicit `skill_ids`.

Three-tier scoping:
- **Tenant-wide:** `project_id` empty, available to all agents in all projects
- **Project-scoped:** `project_id` set, available to all agents in that project
- **Agent-attached:** In the agent's `skill_ids` field, only that specific agent

### 5. Cross-project agents

Agents like Paw have no `team_id` — they don't belong to any project team. They can:
- Own multiple projects (be `owner_agent_id` on multiple Projects)
- Spawn sessions with any `project_id`
- Access all projects (Cedar policies permit cross-project actions)

### 6. Temper does not change

Project is a standard IOA entity spec. All existing Temper mechanisms (OData queries, Cedar authorization, WASM integrations, SSE streaming) work unchanged.

## Consequences

### Positive

- Clear, queryable project scope — dashboard queries `Projects` instead of guessing from Teams
- Skills scoped by project instead of soul name — eliminates the confusing `scope: "soul"` pattern
- Project-shared memories — agents in the same project can share knowledge
- Apps per-project declared explicitly — dashboard shows what's relevant, LLM caller focuses context
- PM becomes optional — projects exist without issue tracking

### Negative

- Migration: existing seed scripts and data need `project_id` populated on Teams, Skills, Sessions
- The `scope` field on Skills becomes legacy — transition period where both `scope` and `project_id` coexist
- paw-pm's Project entity deletion is a breaking change for any code that references it

### Risks

- If `app_ids` is not maintained (agents don't update it when they start using a new app), it becomes stale. Mitigation: agents can be instructed to update `app_ids` when they use a new app, or the dashboard can supplement with derived data.

## Non-Goals

- Multi-tenant project sharing (projects exist within one tenant only)
- App installation scoping (apps remain tenant-wide)
- Enforcing app_ids as a hard permission boundary (it's advisory, not enforced by Cedar)

## Amendment, 1 Nov 2026: step 3 finished

Step 3 was never done. paw-agent's Project spec shipped without an entry in
paw-agent's CSDL, and paw-pm kept `Paw.PM.Project` in its CSDL with no spec.
In a tenant with both apps, such as production's `default`, the kernel served
paw-agent's state machine under paw-pm's schema with the `Projects` set, whose
states disagree (Planning and Completed had no place to be in; Active was
the initial place, not Planning). Production and staging held no Project
records on 1 Nov 2026.

- paw-agent's CSDL declares `TemperPaw.Project` and its `Projects` set.
- paw-pm's CSDL drops `Paw.PM.Project`, its status enum and its moves.
  `Issue.ProjectId` and `Cycle.ProjectId` stay, as strings, since a
  TemperPaw.Project id is not a Guid. The navigations to Project go: a
  navigation to `TemperPaw.Project` would make paw-pm depend on paw-agent,
  which adds a paw-agent pin to paw-pm's closure that must move with every
  paw-agent version, for a link no code follows.
- Project takes what paw-pm's half had and it lacked: a `Completed` place,
  a `Complete` move, `Archive` from Completed, and `Update` while Paused.
  Pause and Resume were already here. Planning is not added: a Project starts
  Active.
