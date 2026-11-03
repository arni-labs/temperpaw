# paw-pm

Project management for agent teams. Models issues with a governed lifecycle: backlog triage, planning with approval gates, execution, code review, and archival.

## Entity Types

### Issue
Core work item with separated planning and execution phases.

- **States**: Backlog -> Triage -> Todo -> Planning -> Planned -> InProgress -> InReview -> Done -> Archived (+ Cancelled)
- **Key actions**:
  - Triage: `MoveToTriage`, `SetPriority`, `MoveToTodo`
  - Planning: `AssignPlanner`, `BeginPlanning`, `WritePlan`, `ApprovePlan`, `RejectPlan`
  - Execution: `Assign`, `StartWork` (requires assignee + approved plan), `SubmitForReview`
  - Review: `ApproveReview`, `RequestChanges`
  - Hierarchy: `SetParent`, `AddSubIssue`
- **Invariants**: Priority required from Todo onward, planner required for Planning, plan required from Planned onward, assignee required from InProgress onward

### Cycle
Time-boxed sprint within a project.

- **States**: Planning -> Active -> Completed
- **Key actions**: `SetProject`, `AddIssueToCycle`, `Start` (requires at least one issue), `MarkIssueComplete`, `Complete`

### Label
Categorization tag for issues.

- **States**: Active -> Archived
- **Key actions**: `Create` (name, color, description), `IncrementUsage`, `Archive`

### Comment
Discussion comment on an issue, traceable to agent session.

- **States**: Active -> Edited -> Deleted
- **Key actions**: `Create` (issue_id, body, author_id, agent_type, session_id), `Edit`, `Delete`, `React`

## Setup

No dependencies. Projects are paw-agent's (`TemperPaw.Project`, ADR-0018): an Issue or Cycle names one by `ProjectId`. Add Issues and organize them into Cycles. Cedar policies enforce role separation between planners, approvers, and implementers.
