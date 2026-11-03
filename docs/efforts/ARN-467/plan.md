# Implementation plan

The expected end state is the complete DSF software factory defined in spec.md. Each unit ends in a verifiable result; passing an early unit does not complete the effort.

1. Establish isolated current-main worktrees, private Foundry fork, committed contract/decisions and the Temper Intent/Effort chain. Inspect Genesis pins before modifying existing apps. Resolve provider/subscription access with actual probes and record any required human consent immediately.
2. Replace the generic resource/operation schema with the typed resource graph in spec.md. Define each resource's configuration, observations, supported actions and sequences. Use `Dsf`-prefixed type and entity-set names and check them against repository and installed-app ownership. Prove duplicate short type and entity-set names are rejected, including across CSDL namespaces. Add stable provider-scoped resource identities and seeded DSF bindings. Keep the readable contract, formal model and executable invariants aligned. Prove invalid transitions and stale/replayed observations fail before implementing integrations.
3. Implement narrow provider, Datadog and DSF operational collection adapters and the governed recurring sync. Prove real reads, redaction, coverage, freshness and drift-to-Ask behavior. Preserve prior observations when sources fail.
4. Implement provider-specific WASM integrations attached to the owning resource's deploy, configuration, rollback and observation sequences. Remove the universal provider-switching operation route. Adapt Effort deployment selection to invoke the named resource action and preserve existing TemperPaw release behavior. Prove idempotency, exact-revision verification, configuration changes and rollback/recovery under injected failures.
5. Add necessary DSF startup, operational telemetry and scoped repair corrections in its own worktree. Run migration/restart, route, DST and browser checks against disposable data, then use the resource operation to ship and verify the affected flow.
6. Adapt private Foundry hosting, machine auth, subscription connection, Temper MCP bootstrap, effort/run linkage, decision bridge and operational UI. Boot its real API/web/Postgres shape and run an agent inside Tensorlake. Exercise direct chat, decisions, files, suspension and recovery.
7. Connect observation-driven investigation/repair and isolated experiment lifecycles. Demonstrate one repair, two isolated variants, selection through delivery and cleanup. Verify automatic ongoing reconciliation after the interactive session ends.
8. Run independent reviews and required checks for every changed repository. Fix every finding, merge in dependency order, publish app deltas to Genesis, install pinned revisions, deploy private services and perform browser/provider/Datadog verification. Attach final evidence and costs to ARN-467 and close the verified Effort.

Keep operational notes current while working. Stop dependent production actions on a real denied authorization or provider consent requirement; continue independent implementation and tests. Never weaken gates to meet the morning target.

## Visible delivery checkpoints

Show each working result as soon as it is verified. Keep the complete expected
end state above; these checkpoints do not defer any part of it.

1. Open Foundry's resource graph backed by an actual Temper server. Show real DSF
   provider identities, dependencies, configuration Files and available actions.
   Label the environment and distinguish imported inventory from live collector
   observations and exercised operations.
2. Show a subscription agent reading a source observation and updating a model
   record through Temper, with the before/after record and recorded provenance.
   Demonstrate the resident worker triggering the same path separately.
3. Open one resource operation's execution and verification evidence, including
   a real isolated deployment/configuration change and recovery or rollback.
4. Open the deployed private Foundry workspace and exercise direct agent chat,
   outstanding decisions and the continuously maintained production model.

A checkpoint handover includes a phone-accessible HTTPS link, the interaction
actually verified, and the remaining limitation. Localhost alone is not a handover.
A hosted snapshot must identify its capture time and cannot stand in for live
operation or chat verification. An empty page, a passing unit test or a
prepared registration file is not a completed checkpoint.


## September 16 continuation: first executable DSF delivery

Rita clarified that the twin is the agent-maintained application, infrastructure and user representation through which agents act. The Foundry composer-link direction is withdrawn. The next live milestone is one real resource-owned delivery through the existing Effort, followed by provider, application and Datadog verification; a reusable twin-building skill follows proven DSF usage.

1. Restore governed reads of application/user model rows and authenticated Genesis discovery. Establish actual installed dsf-twin and paw-patrol pins and rollback references before publication.
2. Repair recurring application/user collection through ModelSync state timeouts, respecting source intervals, pause and callback sequences. Compare the candidate delta with canonical Genesis source before release.
3. Publish and install the verified twin and resource-delivery lifecycle modules. Bind the chosen DSF service to its real repository, application origin, verification flow and telemetry; the inspected staging resource currently declares no allowed operations.
4. Run one bounded change through proof, review, the declared resource operation and read-only Effort completion. Observe later automatic collection and an agent-maintained model update. Retain empirical application verification alongside twin evidence. Do not declare delivery from local tests or a registered resource alone.
