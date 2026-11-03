# DSF Harness — Development Governance for Deep-Sci-Fi

## What

The DSF Harness is a reference app that enforces a 3-level gate system for the Deep-Sci-Fi project through a single merged entity type: `DsfWorkCycle`. It combines repository configuration (harness setup) with a governed implementation loop (work cycle) into one entity, so agents cannot skip verification steps — the platform rejects state transitions when boolean gate fields are not satisfied.

## Entity Types

### DsfWorkCycle

One unit of work through the harness gates. The repository contract (repo URL, tech stack, conventions) lives on the project harness, which a work cycle references by `project_harness_id`; a new work cycle starts in `Planning` and is configured there.

**Work cycle fields:** `project_harness_id`, `task_summary`, `planner_id`, `plan_summary`, `test_summary`, `pr_url`, `approver_id`, `error_message`, `sandbox_url`, `review_notes`

**Gate booleans:** `HasPlan`, `MigrationsOk`, `TypecheckOk`, `UnitTestsOk`, `DstOk`, `PolicyGatesOk`, `E2eOk`, `TestsPassed`

## States

```
Planning → Planned → InProgress → Testing → Reviewing → Complete
    ↓          ↓          ↓           ↓          ↓
  Failed     Failed     Failed      Failed     Failed
```

- **Planning** — A work cycle has been started and configured; waiting for a plan.
- **Planned** — Plan written; ready to begin implementation.
- **InProgress** — Implementation underway; agents report gate results via `Report*` actions.
- **Testing** — Level-1 gates passed (migrations, typecheck, unit tests); running level-2 checks.
- **Reviewing** — All required tests passed; awaiting human or agent approval.
- **Complete** — Approved and merged.
- **Failed** — Work cycle failed at any stage.

## Gate Verification

The `gate_verifier` WASM integration runs verification commands in a sandbox and dispatches `Report*` actions for each passing check. It is triggered as an effect on `BeginTesting` (level-1 gates) and `PassTests` (level-2 gates).

**Level 1 (required for BeginTesting):**
- `migrations_ok` — Alembic migration check
- `typecheck_ok` — TypeScript/Python type check
- `unit_tests_ok` — pytest + vitest unit tests

**Level 2 (required for PassTests):**
- `dst_ok` — Hypothesis DST simulation tests
- `policy_gates_ok` — All policy gate scripts

**Optional:**
- `e2e_ok` — Playwright E2E tests

## Usage

### 1. Start and configure a work cycle

```
POST /tdata/DsfWorkCycles
  → creates the work cycle in "Planning"

POST /tdata/DsfWorkCycles('{id}')/DSF.Harness.Configure
  { "project_harness_id": "{harness id}",
    "planner_id": "agent-ren",
    "task_summary": "Add user profile page",
    "sandbox_url": "https://sandbox.example.com" }
  → stays in "Planning"
```

### 2. Plan and implement

```
POST .../WritePlan    { "plan_summary": "..." }  → Planned
POST .../StartWork                               → InProgress
```

### 3. Report gates and advance

Agents report gate results as self-loop actions in InProgress:

```
POST .../ReportMigrations    → sets migrations_ok = true
POST .../ReportTypecheck     → sets typecheck_ok = true
POST .../ReportUnitTests     → sets unit_tests_ok = true
POST .../BeginTesting        → Testing (requires migrations_ok, typecheck_ok, unit_tests_ok)
POST .../ReportDst           → sets dst_ok = true
POST .../ReportPolicyGates   → sets policy_gates_ok = true
POST .../PassTests           → Reviewing (requires dst_ok, policy_gates_ok)
```

### 4. Review and complete

```
POST .../Approve  { "approver_id": "human-sesh", "pr_url": "https://..." }  → Complete
  — or —
POST .../RequestChanges { "review_notes": "Fix the migration" }              → Planning (resets tests_passed)
```

### 5. Failure

Any stage can fail:

```
POST .../Fail { "error_message": "Sandbox crashed" }  → Failed
```
