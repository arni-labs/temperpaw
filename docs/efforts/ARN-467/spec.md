# DSF software factory contract

## Outcome

A user asks an agent to change, investigate or explore DSF. The agent can inspect its live operational model, edit and test in a computer, raise a decision when required, request an operation on the correct resource, and verify the result. The user can join that same agent directly in the private Foundry workspace. After a run ends or a process restarts, the model and work records remain available and scheduled observation continues.

The scope includes delivery, operations and isolated exploration. DSF product features are changed only where needed to make those flows observable and safely operable. Building the future scientific prediction engine or migrating the product domain is outside this effort.

## Sources of state

Temper owns operational resources, observations, operations, experiments and the existing Intent/Effort/Ask/review/proof records. Foundry owns its computer-run lifecycle, command mailbox and transcript. A stable effort/run association connects them. Provider identifiers and observed revisions identify external resources. Source files and Datadog links support interpretations; neither a human label nor a successful HTTP transport response substitutes for evidence.

The new application belongs in `os-apps/dsf-twin`, published to Genesis with a pinned installed revision. Existing SDLC changes belong in `paw-patrol`. Foundry integration stays in the private `arni-labs/foundry` fork. Necessary DSF application instrumentation and startup corrections stay in `arni-labs/deep-sci-fi`. Kernel changes require an actual missing primitive and a separately recorded architectural decision.

## Operational records

Resource types own their configuration, observations, dependencies and supported actions. The generic `DsfResource` and provider-switching `DsfOperation` design in the current branch must be replaced before installation. The following types define the target contract and are not yet installed.

| Record | Meaning and required linkage |
|---|---|
| DsfRailwayServiceInstance | A Railway service in one environment, identified by project, service and environment IDs. Owns deploy, configuration, rollback and observation sequences for that target. Production API, staging APIs and the Datadog collector are distinct instances. |
| DsfVercelProject | A Vercel project identified within its provider account. Owns deployment, alias, configuration and rollback sequences with explicit production or preview targets. Aliases of the same project do not create duplicate project records. |
| DsfSupabaseProject | A Supabase project and its managed Postgres database, identified by project reference. Records database version, schema revision, connection bindings and database-specific maintenance capabilities. |
| DsfCloudflareR2Bucket | An R2 storage bucket identified by Cloudflare account and bucket name. Records configuration, public domains, application bindings and supported storage operations. |
| DsfDatadogMonitor | A real Datadog monitor identified by site, organization and monitor ID. Owns monitor configuration and links its observations to affected resources and flows. Arbitrary telemetry queries remain observations on their subjects. |
| DsfFlow | A real application flow, its source revision, entry points, dependent resource IDs and outcome definitions. |
| DsfParticipant | An identified DSF API participant or an aggregate anonymous-reader cohort, with identity provenance and activity window. No API keys, content or invented individual browser identities. |
| DsfObservation | Immutable evidence summary with subject, source, query/window, observed time, coverage, outcome and evidence reference. Measured, absent, inaccessible and stale data are distinct. |
| DsfModelSync | Bounded observation/reconciliation execution, source cursor/revision, last successful observation and next due time. Failure preserves prior facts and makes staleness visible. |

DsfModelSync checks due time every minute from Idle and Ready. A scheduled check before next_due_at returns to Ready without reading a provider or emitting an observation; explicit Refresh collects immediately. Pause cancels collection scheduling. Failed collections use the existing bounded retry path. A deferred callback must match the current sync sequence.
| DsfExperiment | An isolated variant with its Effort, branch, computer, actual data/media bindings, results and cleanup record. Selection returns to delivery; it never directly replaces production. |

The first resource graph includes the observed DSF deployment: Vercel production and staging frontends, Railway production API and Datadog collector, both distinct staging APIs, Supabase production and staging databases, and R2 media storage. The discovered foresight database and sleeping Bun service remain observed candidates with unconfirmed ownership. Their presence in the graph does not establish that DSF owns them or permit operations on them. Flow records cover agent registration/participation, story submission and media completion, story reading, feed browsing, world exploration, action processing, notification delivery and video generation.

Application and environment membership are explicit relationships or fields. Resources with the same contract share a type across environments. Flow and participant records describe operational behavior and activity. Dependencies use actual IDs and explicit provenance. Observers preserve the uncertainty of unconfirmed relationships.

### Names and identities

Temper currently indexes entity types by short name within a tenant, and entity-set names share that tenant's registry. CSDL namespaces and app boundaries do not isolate those names. Prefix every type and entity set introduced by this app with `Dsf`, and name the actual modeled object. For example, `DsfRailwayServiceInstance` uses the entity set `DsfRailwayServiceInstances`. WASM module names use the `dsf_` prefix and identify the provider and concern, such as `dsf_railway_deploy`.

Keep the existing `Intent`, `Effort`, `Ask`, `File` and `Computer` contracts as dependencies. Do not redeclare them. The production tenant inventory read on 2026-09-06 includes `DsfDeploy`; this app must not replace or reuse that name. Ownership checks are required even when a name has the expected prefix.

Publication checks must reject duplicate short type names or entity-set names within the submitted app and against other installed apps. An update may reuse names only when the previous installed manifest establishes that the same app owns them. Check both the repository definitions and a fresh target-tenant inventory before installation. Repeat the check for every later model extension; do not add a namespace subsystem to the kernel for this effort.

Resource instance identity derives from the complete provider scope, never its display label. For Railway that scope is project ID, service ID and environment ID. Production and staging therefore have distinct IDs even when they use the same service ID. Renaming a label preserves identity, and a recreated provider resource receives a new identity. UI labels include the application, environment, role and provider (for example, `DSF / production / API / Railway`). The UI also exposes the provider identity to distinguish resources with the same label.

## Observation and reconciliation

Provider adapters collect deployed revisions, aliases, resource configuration and bindings. Source inspection updates flow and dependency descriptions. Datadog queries collect service/monitor status, request outcomes and bounded trace/metric summaries. DSF exposes authenticated operational aggregates where provider/telemetry data cannot establish queue, job or participant state.

Each adapter has a narrow typed result and redacts secrets at its boundary. It retains observation windows and coverage. Sampled spans are labeled sampled; metric counts, jobs, users and span counts are never conflated. An empty query result does not imply health. Missing permissions are reported as inaccessible rather than silently ignored.

DSF and resource-delivery guests read Temper records through the invocation's native HTTP authority. The host supplies the internal API origin and preserves the initiating identity's Cedar permissions. Their integration declarations contain no shared tenant bearer key or shared-key secret template. Provider requests use only their module's named provider credentials.

Provider and monitor events request reconciliation where available. A bounded recurring reconciliation recovers missed events and marks stale observations. The state machine owns scheduling, retries and failure transitions. Agents interpret changed code and ambiguous operational findings using the same recorded work/Ask flow. Adapter code performs one collection or provider operation, not a hidden SDLC loop.

Repeated observation of the same source event is idempotent. Older data cannot overwrite a newer observation or intended configuration. Reconciliation never changes intended configuration. Drift creates or updates a single outstanding Ask for the same resource and drift fingerprint.

## Resource operations and delivery

An Effort retains specification, plan, decisions, review, proof and completion. `ConfigureResourceDelivery` records up to eight exact final resource operations, each with its type, resource ID, action, operation key and sequence, revision, configuration digest and proof packet. The plan bytes, tested head and delivery sequence fence every asynchronous result. `MergeResourceDelivery` runs the existing review, proof and merge gates. The agent then invokes the declared resource actions. `VerifyResourceDelivery` only reads their durable results: every expected operation must match, carry its verification flag, and provide provider, affected-flow and Datadog evidence before the Effort becomes Verified. The existing `ConfigureDeploy` → `Merge` → `TemperDeploy` path remains available for TemperPaw releases. Neither path can use the other path’s completion callback.

A resource's own spec declares its supported actions and sequences request, validation, execution, observation and verification. For example, `DsfRailwayServiceInstance.Deploy`, `ApplyConfiguration`, `Rollback` and `RefreshObservations` operate on the exact service/environment instance. Each external concern is one provider-specific integration. The same integration can serve multiple instances of its resource type. A WASM module never dispatches another transition itself.

Do not route every resource through a generic operation entity whose executable switches on provider or operation kind. If an operation needs a separate durable history record, give it a precise, app-specific type and link it to its owning resource and Effort. Such a record preserves the request and result; it does not move the resource's behavior into a universal executor. Resource pages derive supported actions and current availability from the resource contract.

Before an external write, deterministic validation requires the exact resource, allowed operation, linked Effort and evidence for the exact intended source revision or configuration. Retries reuse the operation key. On uncertain provider responses, observe the provider execution before issuing another write. An unrelated successful provider deployment cannot satisfy this operation.

Verification requires provider state and application behavior. For the backend this includes exact revision, health body, schema and the changed flow; for the frontend it includes active alias/revision, compiled API target and browser behavior. Datadog verification checks the affected service/flow and distinguishes unavailable telemetry from a passing result. A rollback is a recorded operation that also needs observed verification.

The current backend startup migration-reset defect must be corrected and exercised against a disposable migrated database before routine backend deployment. The newer staging API currently points to production data/media and cannot serve as an isolated test target. Recovery endpoints need authentication and atomic claiming before they become permitted routine repair operations.

## Ongoing operations

Fresh observations can produce an investigation Effort. The agent establishes cause and a proposed repair, invokes an already permitted resource operation or raises an Ask for a new capability, then records the result and affected flow verification. Historical failed media is not automatically regenerated. Configuration drift remains a decision until accepted.

Provider API failures, stale credentials, missing telemetry, abandoned runs and unsuccessful repairs remain visible. A restart resumes from durable records. It must not replay an already completed deploy or silently mark interrupted work successful.

## Isolated exploration

An experiment records exact branch/revision, computer identity, database identity, media namespace and permitted external calls. Validation refuses production database/media bindings. A local disposable Postgres with pgvector and a separate experiment bucket are sufficient; no staging name is treated as isolation evidence.

An agent can create and compare at least two variants, run the required application checks, record results, choose a candidate through the existing decision mechanism and clean up unused resources. Promoting a candidate creates ordinary delivery work and respects its review/proof gates.

## Foundry and agent access

Preserve Foundry's existing authenticated membership and durable mailbox. Admit workspace members with the agreed full control. Configure our own public origins, repository installation and provider credentials. Remove upstream deployment targets before any fork deployment. Keep images and source private.

An explicit machine-authenticated integration lets an outside agent start, steer and inspect runs in the configured factory organization. Do not copy a user's short-lived browser JWT or expose broad credentials to the browser. The resident harness receives a scoped Temper connection after Foundry writes its configuration. The initial factory harness is Codex using the existing ChatGPT subscription; no API-key fallback.

The workspace shows the live resource graph, observations, operations, linked work, run transcripts and outstanding Temper Asks. Answering invokes the existing Temper action. Synchronous MCP elicitation must reach the human and return the actual response; it cannot be unconditionally accepted, declined or answered with an empty object. Resume uses the existing mailbox and rechecks the authoritative Temper state.

## Contract invariants

The readable contract, formal state model and executable fault-injection tests express the same invariants:

1. Observing state cannot change intended configuration or authorize an operation.
2. Operation resource, kind, effort and idempotency key cannot change after acceptance.
3. A retry or restart cannot cause a duplicate external write after its provider execution is known.
4. Verified requires the intended revision/resource, successful affected-flow proof and available, checked telemetry required for that flow. Unrelated coverage gaps remain documented.
5. An unresolved blocking Ask prevents its dependent action; resolving an unrelated Ask cannot release it.
6. Experiment bindings exclude production data and media; promotion uses delivery.
7. Stale, missing or inaccessible evidence cannot become healthy by default.
8. Run completion is distinct from verified Effort completion; interrupted work resumes or fails explicitly.
9. Agent credentials, provider keys and private upstream source never enter public artifacts or browser responses.
10. Metered agent fallback is disabled. Additional hosting, computer and product verification costs, including committed resource costs, stay within the $200 implementation cap. Before starting another bounded run, reserve its maximum cost; do not assume delayed billing is zero. Recurring runs use the same remaining cap until Rita changes it.
11. Each introduced entity type and entity set has one owning app within the tenant. App installation and extension refuse collisions, including collisions hidden by different CSDL namespaces. Resource labels cannot determine provider identity.
12. A resource contract declares and sequences every supported mutation on that resource. Shared integrations do not replace resource-specific action contracts with a provider switch.

## Required proof

- Verify specs and policies, then exercise the same invariants under duplicate events, out-of-order observations, concurrent operations, lost responses and process restarts.
- Boot the production shape locally or on the governed computer using real configuration and disposable data before any production deploy.
- Refresh the real DSF graph from providers, code, Datadog and operational aggregates; prove a later observation updates the same identities without adopting drift.
- Start a subscription-authenticated agent inside a real computer; edit and test a scratch change; join and steer that same run through Foundry.
- Raise an actual decision, answer it through the UI, and prove the dependent agent continues exactly once. Exercise MCP elicitation separately from ordinary planning Asks.
- Complete a real resource-owned deployment with exact revision and affected-flow/Datadog evidence attached to the Effort.
- Complete an observation-driven repair and verify the outcome, then create two isolated variants and prove cleanup.
- Restart services and recover interrupted work without duplicate deployment.
- Run the fixed three-harness review panel, Greptile and required CI checks; fix all findings; merge, publish to Genesis, deploy and verify the installed revisions and browser flow.

No core part is deferred by calling the initial end-to-end proof a completed phase.
