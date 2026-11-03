# Decisions and tradeoffs

## D1: Keep the operational model in a separate DSF factory app

**Decision:** Add `os-apps/dsf-factory` and extend existing SDLC deployment linkage where required.

**Came up because:** DSF resources and observations outlive individual Efforts, while the user explicitly excluded migrating DSF product logic into Temper.

**Options:** Put operational records on each Effort. migrate DSF product entities. use a separate operational app linked to existing Efforts.

**Chose the operational app because:** It preserves stable resource identity and existing product implementation. It adds one published app and explicit cross-app dependencies.

**Where:** docs/efforts/ARN-467/spec.md, Operational records and Resource operations and delivery.

## D2: Preserve Foundry's run system and Temper's work records

**Decision:** Link Foundry runs to Temper Efforts and render existing Asks through a narrow integration.

**Came up because:** Foundry already provides direct computer chat, durable commands and transcripts, but currently declines MCP elicitation and overwrites harness configuration during bootstrap.

**Options:** Replace Foundry orchestration. duplicate Effort/Ask records in Foundry. retain each system's existing records and add explicit linkage and request/reply handling.

**Chose explicit linkage because:** It reuses working computer/chat behavior and gives both interfaces one authoritative decision record. The integration must handle synchronous elicitation and recovery deliberately.

**Where:** docs/efforts/ARN-467/spec.md, Foundry and agent access. private fork arni-labs/foundry at 6ca87a793df711c79e609111560ee0c7491b0c1b.

## D3: Use subscriptions for agents and bound additional spend

**Decision:** Use existing agent subscriptions without automatic API-key fallback and cap additional overnight costs at $200.

**Came up because:** Rita approved $200 and clarified that agents should run on subscriptions. DSF product verification can independently call metered APIs.

**Options:** Allow metered agent fallback. stop all paid product verification. keep agent auth subscription-only and account for necessary product API calls plus hosting/compute under the cap.

**Chose the third option because:** It matches the user's authorization while permitting real application proof. Unavailable subscription consent remains a concrete blocker. It does not authorize a different billing mode.

**Where:** docs/efforts/ARN-467/intent.md and ARN-467 authorization record.

## D4: Use separate storage for experiments

**Decision:** Isolated experiments use a disposable database and a separate media bucket.

**Came up because:** The current staging service uses production data and media, and a prefix in that bucket would not meet the declared isolation contract.

**Options:** Reuse staging. restrict a prefix in the production bucket. provision separate disposable data and storage.

**Chose separate storage because:** Provider identity gives a direct isolation check and cleanup target. It requires a small additional resource whose cost counts against the cap.

**Where:** docs/efforts/ARN-467/spec.md, Isolated exploration.

## D5: Start with Codex subscription auth in Foundry

**Decision:** Use Codex with the existing ChatGPT subscription for the first factory harness.

**Came up because:** Current Foundry supports ChatGPT subscription credentials, while its Claude adapter currently requires a metered API key. The existing arni-big computer has a Codex auth file whose validity still needs a real probe.

**Options:** Add Claude subscription support first. use metered model keys. verify and connect existing Codex subscription auth.

**Chose Codex because:** It uses Foundry's implemented subscription path and the user's intended billing mode. Unavailable auth remains visible and cannot trigger a paid fallback.

**Where:** docs/efforts/ARN-467/spec.md, Foundry and agent access.

## D6: Fix the shared action boundary before installing the factory

**Decision:** Add strict IOA parameter contracts in the Temper kernel and pin that version before installing DSF factory specifications.

**Came up because:** Real actor and HTTP tests showed undeclared action inputs and generic writes could change protected operational fields.

**Options:** Depend on agent instructions; add DSF-specific endpoint checks; enforce the specification in the shared actor boundary.

**Chose the shared boundary because:** It protects the same contract through MCP, HTTP, reactions and simulation. It adds a kernel dependency that must be reviewed and deployed before app installation.

**Where:** nerdsane/temper branch codex/dsf-factory-boundaries, docs/efforts/ARN-467/spec.md.

## D7: Put behavior on typed resources

**Decision:** Replace the generic resource and operation dispatcher with resource-specific Temper contracts whose actions own configuration, deployment, observation and rollback sequences.

**Came up because:** The user clarified that the production Railway API must be a resource with its own configuration, telemetry and deploy/rollback behavior. The current branch's generic provider switch did not express that model.

**Options:** Keep `DsfResource` and `DsfOperation` with a shared executor. Split only the executor WASMs. Define resource-specific contracts and attach narrow provider integrations to their actions.

**Chose resource-specific contracts because:** The graph exposes what each resource can do and how it changes. Production and staging can reuse a type and its integrations without sharing identity. This requires replacing the uninstalled generic contracts and updating their callers and tests.

**Where:** docs/efforts/ARN-467/spec.md, Operational records and Resource operations and delivery; docs/efforts/ARN-467/plan.md, steps 2 and 4.

## D8: Use unique app-specific names and provider-scoped identities

**Decision:** Prefix new DSF entity types and sets with `Dsf`, validate their ownership before publication, and derive resource instance IDs from full provider scope.

**Came up because:** The user reminded us that Temper cannot distinguish different entity types with the same name. The registry strips CSDL namespaces, and the live default tenant already contains `DsfDeploy`.

**Options:** Rely on app or CSDL namespace separation. Add kernel namespace support. Use accurate app-specific names with collision checks against existing definitions and installed ownership.

**Chose explicit names and checks because:** They work with the current registry without a kernel namespace change. Names become longer, but the modeled object and owning app remain clear. A matching prefix alone never authorizes replacing an installed type.

**Where:** docs/efforts/ARN-467/spec.md, Names and identities and invariants 11–12; docs/efforts/ARN-467/plan.md, step 2. Evidence: Temper registry entity-set mapping and relation target lookup, plus production `temper.specs('default')` on 2026-09-06.

## D9: Compile each provider action into its own integration

**Decision:** Give each resource action statically typed target and change inputs, with separate validation, execution, observation and verification modules.

**Came up because:** The previous common executor selected a provider at runtime and its operation records required a Git revision even for configuration changes.

**Options:** Keep a universal provider switch behind renamed modules; duplicate all proof and transport code; share proof and transport checks while binding each module to one typed action at compilation.

**Chose compilation binding because:** The module cannot deploy another provider, shared checks stay consistent, and the resource keeps its own operation sequence. A configuration change is bound to the exact requested configuration in the proof artifact; its proof commit identifies the tested source, not an invented deployment revision.

**Where:** os-apps/dsf-factory/wasm/dsf_resource_common and the provider action modules.

## D10: Resume exhausted reads without repeating the provider write

**Decision:** Add explicit resource actions to resume reconciliation or verification for the current operation key and sequence.

**Came up because:** Bounded automatic reads must stop, but exhausted counters would otherwise strand a resource after credentials or provider availability recover.

**Options:** Poll indefinitely; start a new operation and risk another write; reset only the current operation's read counter through a governed action.

**Chose explicit read resumption because:** It preserves the accepted request, provider execution and write count while allowing recovery. Resumption requires another recorded action and remains bounded.

**Where:** os-apps/dsf-factory/specs/generate.py and crates/temperpaw/tests/dsf_factory_contract.rs.

## D11: Preserve uncertainty when a media receipt is unavailable

**Decision:** A selected media job that already carries this resource operation's derived attempt UUID remains uncertain when its durable receipt is unavailable.

**Came up because:** A receipt 404 alone does not prove that no attempt was claimed; replaying recovery after a lost or removed receipt could schedule another attempt or unlock while prior work remains active.

**Options:** Treat 404 as sufficient evidence for another POST; read the selected job states and retain uncertainty when this attempt UUID is present.

**Chose the state check because:** It prevents duplicate work without creating another recovery mechanism. Reconciliation requires the receipt or an explicit investigation when receipt and job state disagree.

**Where:** os-apps/dsf-factory/wasm/dsf_media_actions/src/lib.rs and src/tests.rs.

## D12: Check live short names before installation

**Decision:** Refuse new DSF names that overlap the target tenant, and permit upgrade reuse only with the previous pinned installed bundle's ownership record and matching model digest.

**Came up because:** Production already has DsfDeploy, while its metadata also repeats unrelated dependency names across namespaces. A Dsf prefix alone does not establish ownership.

**Options:** Trust the namespace or prefix; reject all pre-existing metadata duplication; check the candidate's complete type and entity-set names against live metadata and explicit prior ownership.

**Chose candidate ownership checks because:** They protect existing apps without making this installation repair unrelated metadata. A duplicate live candidate name remains ambiguous even when a prior bundle claims it. The publication operator must export ownership from the running installation; repository state cannot substitute for that export.

**Where:** os-apps/dsf-factory/check_names.py and test_names.py. The first-install check found no overlap for the eleven introduced type and entity-set names.

## D13: Generate deployable modules from the executable contracts

**Decision:** Generate the resource entry points and app module manifest from the IOA module contract, and package each binary in the directory Temper actually loads.

**Came up because:** The app still declared the deleted generic operation modules, and the first generated source layout did not match Temper's module loader.

**Options:** Maintain the manifest, source wrappers and packaging list separately; generate them from the executable declarations.

**Chose generation because:** A declared action cannot silently lose its binary or keep a retired dispatcher. The build checks generated drift, requires every declared module's source, and verifies the resulting WASM header before packaging.

**Where:** os-apps/dsf-factory/wasm/generate_modules.py; os-apps/dsf-factory/wasm/build.sh; Dockerfile; crates/temperpaw/tests/dsf_resource_wasm.rs.

## D14: Keep recurring observations on the resource

**Decision:** Each active resource refreshes its own observations every five minutes through a native state timeout.

**Came up because:** The new typed ModelSync proxies scheduled only one refresh and added another row for behavior the resource already owns.

**Options:** Add recurring scheduling to each proxy; rearm the resource's existing observation action directly.

**Chose the resource timer because:** Returning to Active rearms observation after collection failure or operation completion. Operations and retirement suspend the timer naturally. ModelSync remains responsible for code, flow and participant sources. The five-minute reconciliation recovers missed external events without creating a second resource scheduler.

**Where:** os-apps/dsf-factory/specs/generate.py; crates/temperpaw/tests/dsf_resource_scheduling.rs; crates/temperpaw/tests/dsf_factory_contract.rs. The missing recurrence was reproduced before the change, then native timer tests proved repeated collection after failure and cancellation on retirement.

## D15: Run experiments through native Exec with disposable local services

**Decision:** Use the existing governed Computer and Exec entities for experiment phases, with a pinned runner archive, disposable PostgreSQL and MinIO data, and a fresh network namespace for every command.

**Came up because:** The staging API points at production data and media. Experiment validation, execution, selection and cleanup had declared transitions but no working integrations. The existing computer supports sudo and Linux network namespaces.

**Options:** Reuse staging. Provision another hosted database and media account. Start a separate orchestration service. Run bounded local services through native Exec and retain only owned data and receipts.

**Chose local services through Exec because:** Actual cluster and bucket identities can be checked without a new paid service or provider credentials. Native reactions and timers retain the lifecycle in Temper. Each immutable variant has a deterministic Exec identity per phase; resume reads existing execution before another start. A file lock and fsynced receipt preserve completed results across process failure. This first runner supports the agreed CORS variants and full product HTTP checks with all external calls disabled; it does not evaluate generated media quality.

**Where:** os-apps/dsf-factory/specs/experiment.ioa.toml; os-apps/dsf-factory/experiments/; os-apps/dsf-factory/wasm/dsf_experiment_common/; crates/temperpaw/tests/dsf_experiment_runtime.rs. Selection validates an Answered same-Effort Ask and an ordinary delivery Effort; the experiment never deploys directly.

## D16: Keep Effort delivery on exact resource results

**Decision:** Keep resource delivery as a separate Effort path that aggregates exact durable resource results after the agent invokes resource actions.

**Came up because:** One Effort must deliver API and frontend changes without requiring a TemperPaw image, while the existing ConfigureDeploy path must remain available.

**Options:** Reuse a generic deployment dispatcher; overload ConfigureDeploy; add separate resource configuration, merge and verification gates to Effort.

**Chose separate gates because:** They preserve the existing delivery path and keep provider writes on their owning resources. Captured plan bytes, head and sequence fence every asynchronous result. A third merge wrapper reuses the existing review/proof validators and adds correlation to failure callbacks; it does not copy those gates.

**Where:** `os-apps/paw-patrol/specs/effort.ioa.toml`; `os-apps/paw-patrol/wasm/effort_resource_delivery/`; `crates/temperpaw/tests/effort_resource_delivery.rs`.

## D17: Audit strict Effort producers

**Decision:** Enforce the Effort action boundary with strict parameters and audit every existing declared producer in the same change.

**Came up because:** An aggregate checker cannot protect delivery state if generic create/PATCH/PUT/DELETE can fabricate its fields.

**Options:** Protect only the new callbacks; opt Effort into strict parameters and verify existing callers.

**Chose strict parameters because:** The existing named actions already describe its writes. The caller audit and native Intent-to-Effort-to-TemperDeploy fixture preserve the working legacy path. HTTP tests demonstrate both policy rejection and strict rejection even after an explicit generic-write grant.

**Where:** `os-apps/paw-patrol/wasm/effort_resource_delivery/audit_callers.py`; `crates/temperpaw/tests/effort_resource_delivery.rs`.

## D18: Generate scoped factory permissions

**Decision:** Generate DSF command and runtime permissions from the declared contracts, and restrict module secret access to exact named credentials.

**Came up because:** The app had no Cedar policy, and allowing ordinary agents to report operation success would defeat its evidence checks.

**Options:** Require approvals for every action; permit all app actions; separate registered member commands from native callbacks.

**Chose separate permissions because:** Registered factory agents and humans can operate resources and maintain Flow/Participant models, while only kernel service principals can publish observations or operation results. Explicit callback forbids survive unrelated permits. Each compiled module receives only its required Temper/provider credentials; dynamic source config cannot request unrelated tenant secrets.

**Where:** `os-apps/dsf-factory/policies/`; `crates/temperpaw/tests/dsf_factory_policy.rs`.

## D19: Investigate material model changes through existing workers

**Decision:** Queue material model changes through the existing PatrolRun and WorkerRun types, using native reactions and a subscription agent in paw-codex-worker.

**Came up because:** Resource and ModelSync refreshes preserved observations but did not wake an agent to maintain the application/user model or investigate drift. The old PatrolRun WASM dispatches WorkCycle transitions imperatively.

**Options:** Reuse the WorkCycle dispatcher; keep the setup Effort open indefinitely; add an operations entity; or use native PatrolRun/WorkerRun actions with ordinary Intent/Effort/Ask for resulting repairs.

**Chose native PatrolRun/WorkerRun actions because:** They preserve worker ownership without another entity or a perpetual setup Effort. A material fingerprint includes the subject, desired configuration/revision and observed source facts. It excludes collection timestamps and derived age noise but retains participant activity, job identities and eligibility. Startup reconciliation reuses deterministic run IDs and restores interrupted child assignment or terminal evidence. Completed investigations do not rerun; failures remain visible for recovery.

**Where:** `os-apps/paw-patrol/specs/{patrol_run,worker_run}.ioa.toml`; their CSDL declarations; `crates/paw-codex-worker/src/dsf_model_{patrol,investigation}.rs`; `crates/temperpaw/tests/dsf_model_patrol.rs`.

## D20: Separate resident worker and reasoning-agent credentials

**Decision:** Give the daemon and reasoning agent separate registered Temper credentials, and launch Temper through its actual stdio MCP transport.

**Came up because:** The existing worker invocation ignores user configuration and exposes only Datadog MCP. An OData base URL is not an HTTP MCP endpoint. The current standalone temper-mcp binary reads TEMPER_API_KEY and resolves the registered identity.

**Options:** Assume an HTTP MCP route; inherit all user configuration; or configure the existing stdio binary explicitly.

**Chose explicit stdio configuration because:** It makes the resident binding inspectable. The child uses ChatGPT subscription authentication and the OpenAI provider. API billing keys and the daemon WORKER_TOKEN are removed; DSF_FACTORY_AGENT_TOKEN becomes the MCP child's TEMPER_API_KEY. The daemon rereads returned model/work references before recording completion. Cedar reserves runtime reactions and assigned-worker results even when another policy permits them. This requires a separately provisioned agent credential and current binary.

**Where:** `crates/paw-codex-worker/src/{dsf_model_investigation,execution}.rs`; `os-apps/dsf-factory/policies/model_investigation.cedar`.

## D21: Exercise the actual application catalog contract

**Decision:** Add APP.md so the platform's app catalog can load dsf-factory.

**Came up because:** A real bundle-loading test returned no dsf-factory bundle: the loader skips directories without APP.md. README.md does not satisfy that contract.

**Options:** Change the shared catalog's loading rules; provide the app guide it already requires.

**Chose the existing catalog contract because:** It fixes this app without changing other applications. The loader already enumerates every top-level policies/*.cedar file. A catalog test proves the generated provider policy and investigation policy are both loaded.

**Where:** `os-apps/dsf-factory/APP.md`; `crates/temperpaw/tests/dsf_model_patrol.rs`.

## D22: Revise resource models without changing provider identity

**Decision:** Add an Active-only ReviseModel action with a model sequence check and provenance reference, while retaining immutable provider identity and delivery history.

**Came up because:** Register was the only way to set configuration bindings, dependencies and operation availability. Strict generic writes correctly refused subsequent changes, so agents could not keep the resource graph current or bind an observation-only discovery later.

**Options:** Permit generic updates; replace the row and lose its history; or declare the exact model-edit action on each typed resource.

**Chose the named action because:** It allows agents to maintain labels, source repository, dependencies, pinned configuration and available operations without changing provider identity, environment, observed facts or desired delivery state. A sequence check refuses stale edits. Revision clears current operation verification flags so earlier completion evidence cannot satisfy delivery against a changed binding. Only Active resources can be rebound; no provider integration runs during this action.

**Where:** `os-apps/dsf-factory/specs/generate.py`; generated resource contracts and policy; `crates/temperpaw/tests/dsf_resource_model_revision.rs`.

## D23: Verify the application owned by the resource

**Decision:** Require ResourceConfig version 3 to name the registered application and provider-owned origin used for verification; observation-only discoveries may be explicitly unbound.

**Came up because:** Railway staging could verify a matching production revision, infrastructure configuration always probed production, and Vercel verification did not prove that the apex belonged to the target deployment.

**Options:** Retain global production probes; retain version 2 as a fallback; or bind verification to the actual application and environment.

**Chose the explicit binding because:** A matching revision does not identify the environment. Railway deploy and rollback verify their own resource; related infrastructure may verify its consuming Railway application. The verifier rereads the application row, pinned configuration bytes and provider identity before proving domain ownership. Vercel proves project, alias and deployment identity. Datadog must match the successful probe URL as well as revision, request ID, service and environment. Production administrative credentials are refused outside the production API. The uninstalled version 2 contract is removed, and unbound configurations cannot authorize provider operations.

**Where:** `os-apps/dsf-factory/wasm/dsf_resource_common/`; Railway and Vercel adapters; `crates/temperpaw/tests/dsf_resource_wasm.rs`.

## D24: Resolve generated Railway domains through their owning service

**Decision:** Allow a generated Railway domain's projectId to be null only when its queried parent service belongs to the exact registered project and the domain matches the exact service and environment.

**Came up because:** Real production and staging GraphQL responses returned null projectId for generated domains. Requiring that field unconditionally refused the actual staging application.

**Options:** Ignore project identity for every domain; refuse legitimate generated domains; or verify their owning service and retain stricter custom-domain checks.

**Chose the owning-service check because:** It matches the provider's actual representation without accepting foreign ownership. Custom domains still require an explicit matching projectId, and any explicit foreign projectId is refused. The media adapter reuses the same domain reader. A regression reproduced the refusal before correction, and the compiled helper then proved both actual production and staging domains through the real provider API.

**Where:** `os-apps/dsf-factory/wasm/dsf_resource_common/src/application.rs`; its domain regressions; `os-apps/dsf-factory/wasm/dsf_media_actions/src/links.rs`.


## D25: Scope model reads and configuration File access

**Decision:** Express Paw reads as individual resource scopes and give verified DSF factory agents a capability limited to canonical resource configuration Files.

**Came up because:** The long read-policy expression exhausted Cedar's remaining stack in the actual OData path. Separately, a new File's native content PUT has no workspace attribute, so the ordinary workspace rule cannot authorize configuration creation.

**Options:** Add overlapping role-wide grants; keep the long expression and enlarge only the fixture stack; or preserve the same read scopes and add an explicit configuration File capability.

**Chose explicit scopes because:** They preserve existing read/list authority and permit candidate filtering. The configuration rule permits create, update and read only for verified dsf-factory agents and nonempty dsf-resource-config- IDs. It grants no deletion or arbitrary File access. Provider operations still verify the exact pinned configuration hash. The kernel separately handles the general Cedar evaluation-stack failure.

**Where:** os-apps/paw-patrol/policies/patrol.cedar; os-apps/dsf-factory/policies/model_investigation.cedar; crates/temperpaw/tests/dsf_model_file_policy.rs.

## D26: Use native invocation authority for internal reads

**Decision:** DSF and resource-delivery WASMs read Temper records through the initiating invocation's native HTTP authority without a shared tenant bearer credential.

**Came up because:** The collector guests required temper_api_key, which the host reserves and refuses to expose. Their generated declarations also retained the older shared-key substitution path.

**Options:** Supply a shared key, preserve an empty-key fallback, or remove the key contract and migrate all callers to existing internal HTTP authority.

**Chose native authority because:** It preserves the initiating identity's Cedar permissions without a second credential. Provider requests retain their own module-scoped credentials. The host supplies the internal API origin; it is no longer a secret template. All DSF and new resource-delivery callers drop the Runtime key field and shared-secret grants. Unrelated existing chain gates remain unchanged.

**Where:** os-apps/dsf-factory/wasm/dsf_resource_common/src/transport.rs; collector, model, experiment and resource-delivery guests; DSF specification/policy generators; os-apps/paw-patrol/specs/effort.ioa.toml.

## D27: Reconcile uncertain copies by their complete recorded request

**Decision:** Preserve uncertain copies in CopyUnknown and reconcile the exact provider name derived from the full child and source IDs, without repeating the copy request.

**Came up because:** The old helper put unsupported timeout and count fields in a JSON body and never sent the intended name. Tensorlake used the source's default copy name, leaving an uncertain 502 followed by a name collision. CopyFailed marked the child Destroyed even though a provider copy might exist and its machine_id still identified the source.

**Options:** Retry the POST, truncate identifiers into a shorter name, adopt a loose name match, or retain one recorded request and reconcile it through GETs.

**Chose the recorded request because:** The full child and source IDs give an exact correlation key within Tensorlake's 63-character name limit; unsupported identifiers fail before any provider request. Recovery requires the exact name, a destination different from the source, and matching provider project namespaces. A received partial copy response must also report the exact source_sandbox_id and the same destination found by name. After a completely lost response, the name proves correlation with the recorded request; it does not independently prove provider-reported source lineage, which GET does not expose. ReconcileCopy only runs on an existing CopyUnknown child and never sends another POST. Destroy remains unavailable while the child still holds the source machine ID. This retains uncertain resources for investigation instead of claiming cleanup occurred.

The Docker and full CI build lists now both include paw-compute and dsf-factory before their native tests. Their WASMs are generated and ignored by Git, so omitting these builds would leave the new recovery module and its dependent runtime unavailable in a clean image.

**Where:** os-apps/paw-compute/specs/computer.ioa.toml; os-apps/paw-compute/wasm/computer_copy_start/src/lib.rs; os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs; crates/temperpaw/tests/computer_copy_reconciliation.rs; Dockerfile; .github/workflows/ci.yml.

## D28: Build the application against the verified kernel revision

**Decision:** Pin every Temper dependency in TemperPaw and its Codex worker to the same published kernel commit, fd5a1f31e31c8f006fb4ebe24283eb922b8f26f0.

**Came up because:** Local contract proofs used the updated kernel while the application manifests still referenced an older revision without the required resource constraints, native identity handling and persistence fixes.

**Options:** Keep the older application dependency while relying on a separate test harness, or update all kernel pins and verify the actual application build.

**Chose the application build because:** The shipped program must execute the kernel behavior that the resource specifications require. The kernel workspace tests, Clippy, formatting and readability checks passed before its commit was pushed. The dependency uses the published Git revision, with no local path override.

**Where:** crates/temperpaw/Cargo.toml; crates/paw-codex-worker/Cargo.toml; Cargo.lock.

## D29: Provide a protected mobile checkpoint before the hosted factory is ready

**Decision:** Publish a read-only snapshot of the verified local model to the existing protected Vercel report project, with explicit capture time and incomplete capabilities.

**Came up because:** The user was on a phone and could not open the local Foundry link.

**Options:** Keep handing over localhost, expose the local operator interface through a tunnel, or publish the verified model projection using the report project's existing team authentication.

**Chose the protected projection because:** The user can inspect the actual resource records, dependencies and configuration hashes now. The upload contains three static files, no credentials and no private application source. The snapshot cannot execute actions and does not satisfy the live factory checkpoint. The complete implementation and production verification remain required.

**Where:** docs/efforts/ARN-467/plan.md; the protected ARN-467 Vercel checkpoint linked from Linear.

## D30: Let the registered factory identity raise questions

**Decision:** Include explicit Ask creation, raising and withdrawal permissions for the registered `dsf-factory` identity in the DSF app, while forbidding that identity from answering Asks.

**Came up because:** A real Tensorlake resident agent reached Temper and edited a scratch file, but Ask creation returned HTTP 403. The existing patrol policy only recognizes older agent types for those actions.

**Options:** Reuse another agent's identity, broaden the patrol policy, or declare the new identity's required Ask actions in its own app.

**Chose the app policy because:** The dedicated identity remains distinct and the installed app defines its required capability. Browser answers still use the separate server-held human credential. A proposed operator answer permission was rejected by automatic approval review and was not added; approval for a single local verification Ask was initially pending. Rita authorized the exact local rule on 2026-09-08; it was installed only in the local proof instance and is excluded from the app policy.

**Where:** os-apps/dsf-factory/policies/resident_asks.cedar and crates/temperpaw/tests/dsf_factory_policy.rs.

## D31: Report the failed observation check

**Decision:** Keep static contract failures and HTTP status/source details in observation errors while omitting dynamic provider bodies and field values.

**Came up because:** The real local Railway refresh returned only a generic binding-or-evidence failure, even after the configuration File and its hash were independently verified. The missing detail prevented diagnosis from the operation record.

**Options:** Retain the generic message, copy every error payload, or expose only fixed contract descriptions and status codes.

**Chose fixed descriptions because:** Operators can identify the failed check without leaking provider credentials or returned content. A regression verifies that dynamic error payloads remain absent.

**Where:** os-apps/dsf-factory/wasm/dsf_resource_collect/src/{lib,guest,tests}.rs.

## D32: Collect against the committed invocation state

**Decision:** Bind observation reads to the state captured by the successful RefreshObservations transition, without rereading the same resource's query projection.

**Came up because:** A real Railway refresh failed before its provider read: the collector expected Refreshing, but the HTTP resource projection still held Active. The kernel enqueues that projection after inline integrations return, so rereading it inside the integration cannot establish newer authoritative state.

**Options:** Wait for the projection, move projection writes into the kernel's transition path, or use the committed invocation and existing callback sequence guards.

**Chose the committed invocation because:** The kernel already supplies the accepted transition's configuration reference, digest and counters. The collector verifies the immutable configuration File against that digest, performs only provider reads, and carries the captured refresh and observation sequences into the guarded callback. A later refresh or observation cannot be overwritten. The resource action modules already use this pattern; polling would add latency without authority.

Successful collection also carries an empty declared error_message, so a recovered resource no longer displays its previous collection failure. The same generated callback contract applies to every typed resource.

**Where:** os-apps/dsf-factory/wasm/dsf_resource_collect/src/{lib,tests}.rs; specs/generate.py; crates/temper-server/src/state/dispatch/effects.rs schedules query projection maintenance after integration dispatch.

## D33: Include verified native persistence and callback fixes in the application

**Decision:** Advance all application and worker kernel pins together to published commit 1eaeae43351bd9ae1ce729658436487328c4ff8f.

**Came up because:** The previous pin predates the embedded libSQL lifetime correction, complete inline callback reactions, and authorization-before-materialization fixes required by the live resource flow.

**Options:** Keep a locally tested path dependency or pin the application to the published revision after its normal release checks.

**Chose the published pin because:** All normal kernel push gates passed. Real native HTTP restart persistence and inline/background provider observation proofs passed. The dependency now carries the same implementation into the application build. Kernel D31 records Rita's native-runtime DST exception; ARN-179 remains open and PostgreSQL effect parity is not claimed.

**Where:** crates/temperpaw/Cargo.toml; crates/paw-codex-worker/Cargo.toml; Cargo.lock; kernel PR456.

## D34: Carry the final callback and concurrent creation fixes

**Decision:** Advance the application and worker together to published kernel commit 820c89a33accbf36c0275db7d0d801b1b86d1b5a.

**Came up because:** The final kernel panel reproduced six remaining authorization, callback and persistence defects on the previous pin.

**Options:** Deploy the earlier pin or use the revision that passed all normal kernel push gates and the new HTTP, PostgreSQL and callback regressions.

**Chose the verified revision because:** Inline adapters now dispatch their callback reactions, concurrent PostgreSQL creation retains the committed winner, and refused writes do not materialize entities or check stream state before authorization. Constrained asynchronous PostgreSQL actions return enqueue receipts. This preserves the full persistence scope already approved in Temper Ask arn467-kernel-review-scope; the subsequent assessment does not reopen that answered decision. The D31 native-runtime DST exception and ARN-179 remain unchanged.

**Where:** crates/temperpaw/Cargo.toml; crates/paw-codex-worker/Cargo.toml; Cargo.lock; kernel PR456 D32-D33.


## D34: Pin the factory to the reviewed kernel input and recovery fixes

**Decision:** Update the daemon and Codex worker to kernel revision a82410bd51915204406955d46d0f2bc5d09db8fa.

**Came up because:** Kernel reviews after the previous 820c pin corrected absent creation, static specification lookup, typed parameter validation, callback accounting, authorization recovery and failed-action telemetry.

**Options:** Keep the older dependency, patch the app around kernel failures, or build both consumers from the corrected kernel commit.

**Chose the shared pin because:** The factory and its worker use the same runtime contracts. Input validation and persistence remain kernel responsibilities. This dependency update does not change the accepted scope, the D31 native-runtime DST exception, its five findings, or ARN-179.

**Where:** crates/temperpaw/Cargo.toml; crates/paw-codex-worker/Cargo.toml; Cargo.lock; kernel PR456 D34-D43. The final correction rejects malformed typed declarations and unsupported uint64 state comparisons, and native actors compile initialization and transitions from the same parsed specification.

## D35: Rename the app to dsf-twin and merge it to main on Rita's direct authorization

**Decision:** The app `os-apps/dsf-factory` is renamed `os-apps/dsf-twin` (namespace `Dsf.Twin`), and PR #504 plus this rename merge to `main` through GitHub on Rita's explicit instruction of 2026-09-10, without the Effort `01a074e0-61c3-7e51-bc70-2c0130bb73b8` passing its Merge door.

**Came up because:** Rita and her partner settled the vocabulary: this app is a live model of Deep Sci-Fi (a twin), and the factory is the lifecycle around it (Intent, Effort, Ask, computers, Foundry). The app had only ever existed on the PR #504 branch, so nothing could be installed from `main`. Rita asked for fast, incremental, end-to-end results and directed the merge twice, knowing the review and proof gates on #504 had not passed.

**Options:** Drive the Effort through SubmitForReview, the three-reviewer panel, proof and Merge first (hours, and the twin stays unmerged meanwhile); merge on GitHub now and record the deviation; or install from the branch without merging.

**Chose the GitHub merge because:** Rita owns the merge decision and gave it explicitly. Installing from an unmerged branch was the shortcut she had already rejected. What is given up: the panel review and proof record for this head, which remain owed under ARN-467 before the twin is called delivered.

**Where:** temperpaw PR #504, PR #508; foundry PR #14; this file.


## D36: Emit graph annotations from the generator's own field knowledge

**Decision:** Extend `specs/generate.py` so the generated `model.csdl.xml` carries OData `Temper.Twin`, `Temper.Provider`, `Temper.Role`, `Temper.References` and `Temper.ReferenceShape` annotations, emitted from declarative tables in the generator; the hand-authored CSDL is never edited.

**Came up because:** Foundry needs to render the twin as a graph without hardcoding the twin's types or which fields hold ids. The schema is the natural place to declare what the twin already knows.

**Options:** Hand-edit `model.csdl.xml` (rejected: it is generated, and would drift on the next render); annotate every property that names an id including experiment infra ids and provider coordinates (rejected: invents references and points at entity types absent from the schema); or drive annotations from small declarative tables in the generator keyed by the resource manifest and state var names.

**Chose the generator tables because:** The annotations then follow the same source of truth as the types and stay in sync under `--check`. Provider values use the twin's vocabulary (`cloudflare`, not the internal module string `r2`) via an explicit map. References are limited to fields that truly hold entity ids: `ApplicationId`, `ConfigRef`, `DependencyIds` on every resource, plus `ApiResourceId`/`BucketResourceId` on the media pipeline, `SubjectId` on observations, `ResourceId` on model sync, and `ApplicationId`/`ResourceIds` on flow and participant. `DsfExperiment` is left unannotated: its `computer_id`, `database_id` and bucket fields point at Temper Computer, database and bucket entities that are not part of the `Dsf.Twin` schema vocabulary, so annotating them would create dangling references. The regenerated CSDL differs from the previous one only by the new annotations (proven by diff).

**Where:** os-apps/dsf-twin/specs/generate.py (annotation tables and `csdl`); os-apps/dsf-twin/specs/model.csdl.xml (regenerated); os-apps/dsf-twin/test_names.py (`TwinAnnotationsTest`).

## D37: Declare property references with external targeting, not nested in <Property>

**Decision:** The twin marker and per-entity `Temper.Provider`/`Temper.Role` are emitted as annotations inside their `<Schema>` and `<EntityType>` elements, but the per-property `Temper.References`/`Temper.ReferenceShape` are emitted as schema-level `<Annotations Target="Dsf.Twin.<Entity>/<Property>">` blocks rather than as children of each `<Property>`.

**Came up because:** Nesting annotations inside `<Property>` failed the existing kernel contract test `csdl_matches_every_declared_ioa_field_action_and_parameter`: the pinned kernel CSDL parser (temper-spec rev a82410bd, `parser/schema.rs::parse_entity_type`) reads a property only from a self-closing `Event::Empty` element and sends every other `Event::Start` child of an `EntityType` to `skip_element`. A `<Property>` given `<Annotation>` children therefore parses as a Start element and is dropped from the schema entirely, which also breaks live tenant registration, not just the test.

**Options:** Nest the annotations in `<Property>` as first drafted (rejected: drops the property in the deployed kernel, so the app cannot register); upgrade the kernel parser to read `<Property>` children and re-pin it (rejected: a kernel change in the separate temper repo, outside this effort's scope, and it does not help other consumers on the current pin); or declare the same edges with OData external targeting at schema level, keeping every `<Property>` self-closing.

**Chose external targeting because:** It is standard OData CSDL, keeps every property intact for the kernel parser and the contract test, and still declares each edge against the exact property target (`Dsf.Twin.DsfRailwayServiceInstance/DependencyIds`). The entity- and schema-level annotations are unaffected because the parser stores or harmlessly ignores them. Given up: the property-nested XML shape the intent illustrated. Foundry must read the reference edges from the `<Annotations Target=...>` blocks; this is flagged for confirmation in the completion report.

**Where:** os-apps/dsf-twin/specs/generate.py (`csdl`, the `references` block); os-apps/dsf-twin/specs/model.csdl.xml (25 `<Annotations Target=...>` blocks); os-apps/dsf-twin/test_names.py (`test_dependency_ids_carries_references`); temper-spec rev a82410bd `crates/temper-spec/src/csdl/parser/schema.rs`.

## D38: A counter the kernel never incremented reads as its declared initial

**Decision:** Reading a declared counter from entity state returns zero when the field is absent, and remains a binding failure when the field is present but is not an unsigned integer. The strict reads that survive are the ones with no zero branch of their own.

**Came up because:** The twin could never record a first observation for any resource. `collect()` required `observed_sequence` before building its callback, but the kernel materializes a counter only on its first increment, and `observed_sequence` is incremented only by the `Observe` and `ObserveUnavailable` callbacks that follow a successful collect. The production Railway resource exposed 19 of 60 declared state variables: `refresh_sequence` at 124 was present, `observed_sequence` was absent. Every collect reached Railway, got complete provider data in 515ms, then failed on the missing field. Zero observations existed across all resources. `Invocation::parse` had the same defect for `execution_attempts`, which is zero on every first execution attempt.

**Options:** Change the kernel to serialize counters at their declared initial; make every state read tolerant; or treat absence as the declared initial in the counter read only, keeping wrong types and missing string fields strict.

**Chose the counter read because:** The spec already declares `initial = "0"` for every counter in this app, so absence and zero are the same fact and the module was rejecting a value the model says exists. A kernel change is a larger blast radius for a defect that lives in the app's reader. Blanket tolerance was rejected: a present-but-wrong-typed counter is a real binding error and still fails, `required()` keeps rejecting absent and empty strings alike, and `operation_sequence` and `refresh_sequence` still refuse zero. What is given up: a genuinely missing counter no longer distinguishes itself from a never-incremented one, which is exactly the distinction the kernel does not serialize.

**Where:** `os-apps/dsf-twin/wasm/dsf_resource_common/src/transport.rs` (new `counter`), `dsf_resource_common/src/invocation.rs` (`operation_sequence`, `execution_attempts`), `dsf_resource_collect/src/lib.rs` (`refresh_sequence`, `observed_sequence`; the local strict `counter` removed). Left strict and why: `dsf_resource_collect/src/guest.rs` captures `refresh_sequence` only to fence a failure callback and has no zero branch, so absence there is an undecodable collector context rather than an unrefreshed resource; `dsf_model_collect` reads `sync_sequence`, which the transition that triggers the module increments in the same step, so it is always materialized; the `operation_sequence` comparisons in `authority.rs` and `Invocation::confirm_current` compare against a sequence already proved non-zero, so absence must fail there.

## D39: Give the selection phase its own fence instead of a third step of the shared one

**Decision:** `DsfExperiment` declares a second counter, `selection_sequence`. `Select` increments it instead of `operation_sequence`, and the three selection callbacks (`SelectionSucceeded`, `SelectionFailed`, `SelectionUncertain`) compare `expected_sequence` against it. `operation_sequence` keeps fencing the three Exec-bearing phases.

**Came up because:** The spec installed as `partial` in production because L1 reported six dead transitions — `Select` and everything downstream of `Selecting`. The cause is not in the Selecting flow at all: `temper-verify` bounds every counter at 2 (`crates/temper-verify/src/cascade.rs:172` sets `max_counter: 2`, `model/builder.rs:47` applies it to every declared counter) and `checker.rs:133-141` treats a transition whose `increment` effect would exceed that bound as *not enabled*. The shortest path to `Compared` spends both steps — `Validate` then `Run` — so at `Compared` the shared counter is saturated and `Select` can never fire, which strands the whole `Selecting` subgraph. The action's `param_nonempty` constraints are not involved: the L1 model carries only name, from-states, to-state, guard and effects (`model/types.rs:150-161`), and the string "constraints" appears nowhere in `temper-verify`.

**Options:** Delete `Select`'s increment (rejected: L1 goes green, but the fence goes with it — a superseded attempt's `SelectionSucceeded` then names the unchanged `operation_sequence` and promotes the experiment, proven by mutation below); restructure the four phases to branch from a hub state the way `railway_service_instance` does (rejected: a rewrite of a working lifecycle to satisfy an exploration bound); raise `max_counter` in the kernel (rejected: a temper-repo change, outside this effort, and it only moves the ceiling — a fifth phase would strand again); or give the phase that cannot fit under the ceiling its own counter.

**Chose the dedicated counter because:** It is the pattern the eleven clean sibling specs already use — `railway_service_instance` spends `operation_sequence` once per operation and gives each in-operation phase its own counter (`execution_attempts`, `reconciliation_attempts`, `verification_attempts`) — and it preserves the retry fence exactly: a selection attempt is still superseded by the next one, now by its own counter rather than a shared one that had no room left. What is given up is a single monotone number across all four phases; the phases remain distinguishable because each callback family fires only from its own state. Because the Selecting flow is genuinely reachable at runtime today, `dsf_experiment_common` had to follow: `Phase::sequence_field()` now names the counter each phase is fenced against, `Invocation::parse` takes the phase, and the concurrent-advance guard in `execute()` watches both counters.

**Verification:** The cascade fails on the pre-change file (`L1 FAILED: 25 states explored, 6 dead transition(s)`) and passes on this one (`L1 PASSED: 33 states explored`); the whole spec directory goes from `Error: IOA verification failed for entity 'Experiment'` to `IOA verification cascade: ALL PASSED` for all twelve entities. Each new check was run against a mutation first: removing `Select`'s increment, deleting `SelectionSucceeded`'s `param_equals_field` constraint, and pointing `sequence_field()` back at `operation_sequence` each fail the corresponding test. The composite cross-entity stage reports 236 dropped reactions and a PARTIAL `DsfExperiment` scope, both of which reproduce on `origin/main` with this spec removed or replaced by a counter-free variant — they were simply unreachable while the cascade aborted at Experiment.

**Where:** os-apps/dsf-twin/specs/experiment.ioa.toml (`selection_sequence`, `Select`, the three selection constraints); os-apps/dsf-twin/specs/model.csdl.xml (regenerated, one property); os-apps/dsf-twin/wasm/dsf_experiment_common/src/lib.rs (`Phase::sequence_field`, `Invocation::parse`, `execute`), src/guest.rs, src/tests.rs; crates/temperpaw/tests/dsf_experiment_runtime.rs (`a_retried_selection_refuses_the_previous_attempt_s_result`); crates/temperpaw/tests/dsf_factory_contract.rs (selection and cleanup sequence numbers); temper rev 2c6b9460 `crates/temper-verify/src/{cascade.rs,checker.rs,model/builder.rs,model/types.rs}`.

## D40: Keep application and participant observations recurring through the model state machine

**Decision:** DsfModelSync schedules a due check every minute from Idle and Ready. Its existing collector skips provider reads until the recorded next_due_at, while an explicit Refresh still collects immediately. Pause disables scheduling, and failures retain the existing bounded retry path.

**Came up because:** Rita clarified that agents must operate through and maintain the application, infrastructure and user layers. Infrastructure already schedules observations, but successful model collection enters Ready indefinitely; the investigation worker consumes existing observations without scheduling new ones.

**Options:** Add an external polling loop; ignore each source's interval and collect at a fixed cadence; or use declared state timeouts and a sequence-fenced deferred callback in the existing collector.

**Chose declared timeouts because:** Scheduling stays in the executable contract, source intervals remain meaningful, and a due check performs no external reads when the recorded deadline is in the future. Manual refresh remains available. The tradeoff is up to one minute of scheduling delay after a source becomes due. This is a candidate delta against GitHub source; publication still requires comparison with canonical Genesis source and verified installed pins.

**Where:** os-apps/dsf-twin/specs/model_sync.base.toml, generated IOA/CSDL and policies, dsf_model_collect, and dsf_factory_contract tests.

## D41: Bootstrap unused legacy resource counters through a guarded action

**Decision:** Each provider resource exposes BootstrapOperationSequence only in Active with operation_sequence below one. Its saturating decrement persists a missing zero and cannot reset a used sequence.

**Came up because:** The first staging Deploy request failed its expected_operation_sequence constraint before any provider write. The existing journal predates persisted defaults; current kernel recovery deliberately does not invent newly declared fields.

**Options:** Change kernel recovery or equality semantics; patch operational fields directly; or add an explicit, governed app migration for unused resource counters.

**Chose the guarded action because:** It preserves historical state and the deployment concurrency fence, is idempotent at zero, and leaves model and observation counters untouched. The same generated contract covers all six provider resource types. It does not authorize deployment or replace proof, review waiver, provider binding, or telemetry checks. Current canonical Genesis repairs must be retained when this delta is published.

**Where:** os-apps/dsf-twin/specs/generate.py and generated IOA/CSDL/module contracts/Cedar; legacy_resource_bootstrap_preserves_used_sequences in dsf_factory_contract. The regression reproduces the absent-counter failure and rejects a reset after the first operation.

## D42: Stop exhausted verification as an explicit failed operation

**Decision:** Each provider operation exposes StopExhaustedVerification from its Observed state only after all 40 verification attempts. The operator supplies the current operation key and sequence, a failure explanation, and an evidence reference. It transitions to Failed; the existing AcknowledgeFailure releases the resource for another operation.

**Came up because:** The first real staging deployment succeeded at Railway, but its old backend did not export the exact request/revision trace tags needed for verification. Retrying the same revision cannot fix its instrumentation, and the bounded verifier leaves the resource in DeployObserved after 40 attempts.

**Options:** Forge a runtime failure callback, add scheduler/kernel behavior, or expose a guarded operator failure decision in the existing resource contract.

**Chose the explicit decision because:** It preserves the provider execution identity and all attempt/sequence counters, records failure instead of claiming verification, and performs no provider write. ResumeVerification remains available when more read attempts can resolve a transient delay. Recovery rejects early use, stale operation identity, and empty evidence or explanation; runtime success/failure callbacks remain unavailable to operators.

**Where:** os-apps/dsf-twin/specs/generate.py and generated provider IOA/CSDL/module contracts/Cedar; exhausted_verification_can_be_stopped_without_claiming_success in dsf_factory_contract and exhausted_verification_recovery_is_an_operator_command_not_a_callback in dsf_factory_policy. The regression failed against the original contract before this change.

### D43: Reuse Datadog observations for participant cohorts

**Decision:** Allow participant cohorts to consume the existing bounded Datadog measurements.

**Came up because:** The deployed application lacks the operational snapshot endpoint, while the first working twin must maintain its user layer.

**Options:** Add a snapshot endpoint and related application machinery, keep the cohort static, or reuse existing Datadog collection.

**Chose existing Datadog collection over adding an endpoint because:** It maintains measured cohort activity through the existing immutable observation contract with no new API or credentials. Request counts are not unique people and may include synthetic probes; missing data stays absent rather than becoming zero.

**Where:** `os-apps/dsf-twin/wasm/dsf_model_collect/src/lib.rs`, participant binding regression tests and module README; PR527.

## D44: Scope provider receipts to the current operation

**Decision:** Decode a retained provider_execution_id as the current invocation's execution receipt only when provider_known is true. Keep the persisted field unchanged for audit history.

**Came up because:** A second staging Deploy correctly reset provider_known but inherited the previous deployment ID. Its adapter queried that old deployment and rejected the new requested revision before writing anything.

**Options:** Erase provider history when beginning an operation, special-case Railway, or honor the existing current-operation receipt flag at the shared invocation boundary.

**Chose the shared boundary because:** Every generated operation already resets provider_known and only correlated provider callbacks set it true. This preserves history and fixes sequential operations across providers. Uncertain operations still discover matching provider executions without writing; an empty listing remains pending because it does not prove absence under eventual consistency. The fix does not reset attempt budgets or authorize another ambiguous write.

**Where:** dsf_resource_common/src/invocation.rs; sequential receipt and Railway execute/reconcile regressions. The common regression failed against the original decoder before the fix. The packaged verification fixture now models the declared provider_known=true callback state and separately proves that an old receipt with provider_known=false cannot pass even with matching application and trace evidence; this negative case fails the original WASM.

## D45: Record abandoned reconciliation without claiming provider absence

**Decision:** Expose AbandonReconciliation from an operation's Unknown state to Failed, requiring its exact operation key/sequence and nonempty explanation/evidence. Existing AcknowledgeFailure releases the resource afterward; neither action retries or resets counters.

**Came up because:** The stale-receipt defect failed before the second deployment's provider mutation, but the conservative runtime reported an uncertain execution. Fixing receipt selection cannot turn an empty, potentially delayed provider listing into proof of absence.

**Options:** Assert absence from an empty listing, reset the write budget, or record the authorized operator decision to stop reconciliation separately from provider facts.

**Chose explicit abandonment because:** It preserves provider receipts, counters, and unverified status without inventing absence or success. For this incident, the source path and old-revision lookup establish a pre-write failure; a fresh deployment still requires a baseline recheck and its normal proof/authorization. A genuinely ambiguous write must be investigated before requesting another operation.

**Where:** generated provider contracts, human-action manifest, Cedar, and runtime contract/policy regressions. The new action regression failed against the original contract before the change.

## D46: Complete explicitly waived resource delivery with real resource proof

**Decision:** Let an exact-head owner-authorized Effort verify its configured twin operations through the existing resource verifier while preserving review_passed=false and resource_delivery_merged=false.

**Came up because:** Canonical owner authorization reaches Merged without pretending a model review or Git merge occurred, but the original resource delivery verifier only accepted the reviewed Git-merge path.

**Options:** Manufacture passing review/merge flags, leave the authorized lifecycle incomplete, or add an explicit waived entry to the same evidence verifier.

**Chose the explicit entry because:** It retains the recorded authorization, head, proof and readiness requirements, then requires current Active resource state, known provider identity, exact operation binding and application/telemetry evidence. It does not weaken the reviewed path. A guarded zero-only bootstrap handles existing records whose delivery counter predates the contract.

**Where:** paw-patrol Effort contract, chain_merge_ready and effort_resource_delivery modules; the actor plus actual compiled canonical module regression covers successful waived verification and wrong head, missing waiver/resource/telemetry, unknown provider and stale callback rejection. Canonical patrol f04211aa07be0d94b5dc88c7f40eaa0ca894f0da preserves the prior owner-waiver actions.

## D47: Match the verified external probe across TLS termination

**Decision:** Accept an observed HTTP span URL for an expected HTTPS probe only when every byte after the scheme is identical. Keep exact service, environment, revision, request ID, successful span status and HTTP 200 requirements.

**Came up because:** The real staging deployment is healthy and Datadog indexed its exact operation-sequence-three probe, but Railway terminates TLS before the application. That span reports `http://deep-sci-fi-staging.up.railway.app/api/health` while the provider-bound external probe successfully used HTTPS.

**Options:** Change application proxy handling, weaken origin matching generally, or recognize this one asymmetric scheme difference in the span comparison.

**Chose the narrow comparison because:** The external probe and provider-origin binding still require HTTPS. Host, path, explicit ports, credentials, query and fragment remain byte-exact; the comparison does not normalize URLs or accept a different service or deployment. The actual indexed span demonstrates the defect, so query time windows and retention settings remain unchanged.

**Where:** dsf_resource_common verification and real-span fixture; packaged configuration verification. The fixture records the exact twin request ID and revision, and its positive case fails against the original matcher. Negative cases retain the identity and URL boundaries.

## D48: Read owner waivers from the actual Temper entity envelope

**Decision:** Normalize an entity's `fields` envelope in `owner_waiver_holds`, while retaining support for flat fields and the existing exact-head, explicit-waiver and nonempty-reason checks.

**Came up because:** The live staging resource was Active with verified provider, application and trace evidence, but Effort completion returned pending. Replaying the captured rows passed as flat fixtures and failed with the real HTTP entity envelope before reading any resource: the waiver helper read the envelope as if it were the fields.

**Options:** Flatten each caller, alter the runtime response shape, or normalize at the shared waiver predicate as the other review/proof predicates already do.

**Chose the shared predicate because:** It fixes the representation boundary without changing authorization or evidence requirements. The compiled host regression now wraps Effort, resource, ProofPacket and File metadata responses, while file content remains the raw proof artifact.

**Where:** `chain_merge_ready::owner_waiver_holds` and the existing Effort resource delivery tests. Both the pure predicate and compiled installed-module regression fail before the fix; live completion remains a separate required verification.
