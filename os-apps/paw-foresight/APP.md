# paw-foresight

Foresight explores possible futures for an ordinary question. The question-driven
UI uses the `SemanticRun` lifecycle. New runs with `world_search_contract: 1`
imagine distinct worlds, record their original commitments, and search backward
for paths connecting them to the sourced present.

Jev assesses the proposed pieces and paths. Exactly unchanged assessments can be
reused across routes; changed inputs invalidate them. An unresolved or rejected
route prompts an alternative search within the run's resource budget. Changes to
a world's commitments remain explicit, alongside the original idea. Final worlds
retain their selected causal paths and receive separate whole-world estimates.
These estimates are uncalibrated model judgments. A connected proposed path,
repeated score or agreement between models does not establish that a future will
happen. Missing evidence and unfinished search remain visible.

See [the worlds-first contract](adrs/018-worlds-before-routes.md). Existing runs
retain their recorded contract; resuming them does not silently convert their
history. The separate `ForesightModel` / `Projection` interface is documented below.

## Existing projection interface

The Foresight Engine is a **substrate, not a pipeline**. Probes are the simulation — they project what would happen, and their convergent projections become the next step's reality. Quality emerges from structural gates: independent observation, semantic convergence detection, and direction versioning.

```
                    ┌────────────────┐
                    │ ForesightModel │ (knowledge graph JSON — schema-free, domain-specific)
                    └──────┬─────────┘
                           │ Seed → spawn_seed_agent WASM (spawns agent session)
                           ▼
STEP 0              ┌─────────────┐
(day 1)             │  Projection │ → Start → spawn_probes
                    └──────┬──────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Probe-1      Probe-2      Probe-3    (independent, 1 Direction each)
         Observations  Observations  Observations
         1 Direction   1 Direction   1 Direction
              │            │            │
              └──── ProbeStepDone ──────┘  (self-report to Projection)
                           │
                    handle_probe_done WASM
                    (checks: all reported?)
                           │
                           ▼
                 ┌───────────────────┐
                 │ Convergence       │  (LLM agent: confirms/contradicts)
                 │ Analyst           │
                 └─────────┬─────────┘
                           │ ConvergenceComplete
                           ▼
                 ┌───────────────────┐
                 │ Model Projector   │  (LLM agent: evolves projected state)
                 └─────────┬─────────┘
                           │ ProjectionUpdated
                           ▼
STEP 1          handle_projection_updated WASM
(day 3)         (respawns probes with projected state + episodic memory)
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         Probe-1      Probe-2      Probe-3
         sees evolved  sees evolved  sees evolved
         world         world         world
         revises or    revises or    revises or
         doubles down  doubles down  doubles down
              │            │            │
              └──── ProbeStepDone ──────┘
                           │
                    ... repeat until max_steps ...
                           │
                           ▼
                    Projection.Complete
```

**Key properties:**
- Probes work independently — they MUST NOT read each other's observations
- 1 Direction per Probe — want more perspectives, run more Probes
- Directions are versioned: each step archives the old Direction, creates a revision with `parent_direction_id`
- The model evolves through simulated time via convergent projections
- Convergence is detected by a separate LLM analyst, not string matching

## Entity Types

### ForesightModel
Living knowledge graph for any domain. The `model_type` field determines the domain (software_product, knowledge_domain, business, etc.). The JSON structure is schema-free — whatever the seed agent produces.

- **States**: Created → Seeding → Active ↔ Stale
- **Key fields**: `name`, `model_type`, `signal_source_config` (domain-specific JSON), `seed_model`, `seed_provider`, `seed_soul_id`
- **Key actions**: `Seed`, `SeedComplete`, `RefreshSignals`, `MarkStale`, `Reactivate`
- **WASM**: `spawn_seed_agent` (spawns an agent session to build the knowledge graph — bitter lesson applied)

### Projection
Temporal simulation that advances through adaptive time steps. Event-driven — no polling.

- **States**: Created → Running → Complete / Branched / Failed
- **Key actions**: `Configure`, `Start`, `ProbesReady`, `ProbeStepDone`, `ConvergenceComplete`, `ProjectionUpdated`, `AdvanceStep`, `Complete`
- **WASM**: `spawn_probes`, `handle_probe_done`, `handle_convergence`, `handle_projection_updated`
- **Flow**: Start → spawn_probes → ProbesReady → (probes run) → ProbeStepDone × N → handle_probe_done → Convergence Analyst → ConvergenceComplete → handle_convergence → Model Projector → ProjectionUpdated → handle_projection_updated → AdvanceStep → (repeat)

### Observation
Probe agents record what they see in the projected state.

- **States**: Created → Confirmed → Escalated / Faded
- **Key actions**: `Record`, `Confirm` (by Convergence Analyst), `Escalate`, `Fade`

### Direction
Direction proposed by a Probe. Versioned across steps.

- **States**: Proposed → UnderReview → Implementing → Implemented → Selected / Archived
- **Key actions**: `Propose`, `Archive` (when superseded by revision), `Select` (human)
- **Fields**: `parent_direction_id` (links to previous version), `step_at`

### DirectionFeedback
Blind review feedback on Directions.

- **States**: Open → Addressed → Resolved / Reopened

## Setup

Depends on `paw-agent` for probe sessions and `paw-fs` for knowledge graph storage.

Model and provider are configured per-probe in `probe_config` (array of `{name, model, provider}`). The seed agent's model/provider are fields on the ForesightModel entity itself (`seed_model`, `seed_provider`).
