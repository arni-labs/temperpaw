# ADR-0067: Retire paw-heal, paw-wiki, paw-consilium and paw-autoreason

## Status

Accepted. Removes the apps that ADR-0001 (paw-heal), ADR-0002 (paw-heal) and ADR-0004 (Monitor, MonitorScan and AlertCycle) introduced. Those ADRs stay as history.

## Context

When the old-syntax TemperPaw versions on Genesis were converted (31 Oct to 1 Nov 2026), these four apps were held back. Their specs trigger 13 WASM modules that were never built or declared, so none of them has ever worked from an install. Before building the modules, we checked whether anything used the apps (read only, 1 Nov 2026):

- Records: their entity sets (Monitors, AlertCycles, WikiSources, WikiPages, WikiJobs, Deliberations, Perspectives, Briefs, Tournaments, Rounds, Versions, Judgments) have no records on production or staging, tenant `default`. The same production key reads 285 Agents and 133 WorkCycles, and an unknown set returns 404, so the zeros are real.
- MonitorScans answers 403 to every key, including staging's operator key, because no Cedar permit reaches MonitorScan.
- Datadog logs over 14 days have no hit for any of the apps' names or type names. The control term "Agent" has 741.
- No other app depends on any of the four, and Katagami doesn't name them.

## Decision

The four apps leave TemperPaw main, along with the code that only existed for them:

- the `os-apps/paw-heal`, `os-apps/paw-wiki`, `os-apps/paw-consilium` and `os-apps/paw-autoreason` directories
- the paw-wiki checks in `crates/temperpaw/tests/session_lifecycle_and_config.rs` (SessionLink's own checks stay)
- `AlertCycle` from the Discord observer's watched types
- paw-heal from the dashboard's fallback app list

Their Genesis App records stay installable at their current pins until they are deprecated. Deprecating them goes through `App.Deprecate` on Genesis, not Delete.

## Consequences

- Building the 13 modules is no longer part of t-paw-module-one-source.
- Self-healing monitoring has no app now. If it comes back, it starts from a new design, and any module it triggers has to be built and declared before it ships. The git history keeps the old specs.
- Some text still describes the AlertCycle loop: the paw-agent lead and SRE agent instructions, the AGENTS.md/CLAUDE.md self-reporting example, `docs/datadog-webhook-setup.md`, the dsf reference project's webhook route and the dashboard mock data. Changing paw-agent's text means republishing paw-agent on Genesis, so that cleanup is a separate change.
- Reverting this commit brings the apps back exactly as they were, still without modules.
