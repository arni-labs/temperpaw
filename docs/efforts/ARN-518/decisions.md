# Decisions and tradeoffs

## D1 — Use the existing TemperPaw dashboard

**Decision**: Add Foresight to the authenticated TemperPaw dashboard.

**Came up because:** The user asked for a working interface as soon as possible, and the historical Temper-based Deep Sci-Fi frontend is in a closed, unmerged 117-file rewrite.

**Options:** Extend the current dashboard; revive the separate Next.js rewrite; build another standalone frontend.

**Chose the current dashboard because:** It already provides authenticated OData reads/actions and an event stream, keeping backend and UI in the same repository. This gives up the historical frontend's presentation work, which remains reference material.

**Where:** dashboard/src/routes/foresight; PR https://github.com/nerdsane/temperpaw/pull/526.

## D2 — Use an isolated worktree on the functioning governed computer

**Decision**: Work in a dedicated git worktree on Computers('arni-big') while its requested copy remains unresolved.

**Came up because:** Computer.Copy created 01a0a7c4-ad8d-7e20-b5d9-54b05ab62f08 in CopyUnknown, and reconciliation returned HTTP 404, while governed Exec on arni-big succeeded.

**Options:** Wait for the unresolved copy; use an isolated git worktree on arni-big; move implementation to the laptop.

**Chose the isolated worktree on arni-big because:** It retains the prescribed governed execution path and keeps other checkouts untouched while allowing progress. The shared machine requires disjoint worktrees and scoped build directories.

**Where:** /home/tl-user/worktrees/arn-518-foresight, branch codex/arn-518-learning-foresight, GitHub remote nerdsane/temperpaw.

## D3 — Fit a bounded probability calibrator

**Decision**: Fit logistic calibration as the first trainable predictive component while retaining the scenario engine.

**Came up because:** The user prioritized a complete, observable learning loop quickly.

**Options:** Train a larger dynamics model; add memory only; fit a small predictor with held-out evaluation.

**Chose calibration because:** It changes real parameters and later probabilities, exposes its coefficients, and needs no GPU service. This component does not learn causal world dynamics.

**Where:** os-apps/paw-foresight/adrs/010-observable-learning.md; backend commit 77539d5.

## D4 — Let the authenticated server select the tenant

**Decision**: Remove the dashboard's hardcoded default-tenant header.

**Came up because:** Dedicated Foresight runs in tenant deep-sci-fi; authentication already injects its configured tenant.

**Options:** Add a second frontend configuration endpoint; duplicate transport; use existing authenticated context.

**Chose existing context because:** It preserves a single tenant authority and avoids sending Foresight requests to the wrong data.

**Where:** dashboard/src/lib/api.ts and dashboard/tests/foresight-transport.test.mjs.

## D5 — Preserve Genesis changes during publication

**Decision**: Reconcile existing Genesis fixes before applying the learner delta.

**Came up because:** Genesis commit 7c5b6e750b215d64e3d3178e7c9eb4abefaf1909 contains scenario, prompt, timeout and policy fixes absent from the GitHub mirror.

**Options:** Replace Genesis from GitHub; preserve the authoritative source and retain compatible build configuration.

**Chose preservation because:** A learning feature must not remove existing runtime behavior. One inherited prompt failed its existing contract test; restoring a concrete EventNode payload preserves the new execution guidance and makes the test pass.

**Where:** os-apps/paw-foresight agents, world specification, policy and reconciled existing WASM modules.

## D6 — Reuse the published runtime with identical server source

**Decision**: Diagnose the app and UI integration against the verified published f1b3f892ce652f1355daeabe68ae49cd3de3fa85 runtime before rebuilding the required session adapter.

**Came up because:** A fresh native build consumed the computer's remaining disk capacity.

**Options:** Continue until disk exhaustion; alter shared caches; use the published binary after checking source identity.

**Chose the published binary because:** The server, transport and Cargo lockfile diff against base 07cbb1d is empty. The binary came from image digest sha256:8ba6cb3b887ee7d7a15aaaf587711b1c9507bcce8af5012e3a917c61d68e24e3; its SHA-256 is c9e507322df4657bef5152004bf67fc17d88038a3bbae06d866cbc50ff57b276. Only this effort's incomplete build cache was removed. A missing packaged file-storage module was rebuilt from its own source.

**Where:** Local proof on governed arni-big, isolated PostgreSQL database arn518, with evidence retained outside the repository.


## D7 — Authenticate browser requests through the supported inner API contract

**Decision**: Translate a verified dashboard session into the server's existing single-use credential scoped to its identity, tenant, method and URI.

**Came up because:** A real signed-in browser passed /auth/me but received an empty401 from the inner platform bearer middleware. That middleware does not accept the outer dashboard's typed context by itself.

**Options:** Change the kernel; bypass its credential check; use its existing internal invocation credential mechanism from the authenticated application adapter.

**Chose the existing credential mechanism because:** It retains both authentication layers and the user's permissions while fixing the necessary TemperPaw integration. Untrusted headers cannot mint a credential.

**Where:** crates/temperpaw/src/auth.rs and crates/temperpaw/src/startup.rs; composed-router regression test and live browser acceptance flow.


## D8 — Continue on the approved replacement computer

**Decision**: Restore the integration branch on an isolated copy of arni-big-2.

**Came up because:** arni-big stopped accepting Exec calls during the native build, and its documented Wake action returned HTTP404 Sandbox not found. The user explicitly approved the replacement computer.

**Options:** Wait for the missing sandbox; move to the laptop; use an isolated copy of another governed computer.

**Chose the governed copy because:** It preserves the prescribed execution layer and has enough disk capacity for a fresh build. Interrupted tests and scratch artifacts are recreated and rerun rather than treated as completed evidence.

**Where:** Computer 01a0a805-fcd7-7252-94fa-8100d049e845; integration worktree /home/tl-user/worktrees/arn-518-foresight.

## D9 — Initialize forecasts through declared transitions

**Decision**: Register forecasts through a declared one-at-a-time World → Forecast → World transition loop.

**Came up because:** The first authenticated browser registration returned HTTP400 StrictActionContract; strict entities accept only identity during creation.

**Options:** Disable strictness; dispatch Register inside WASM; add kernel initializer support; use existing entity triggers.

**Chose declared entity triggers because:** They preserve strict initialization, immutable revisions, deterministic retry identities and the one-concern WASM rule without a kernel change. This adds one transient world state and persisted pending-registration fields.

**Where:** os-apps/paw-foresight/specs/world.ioa.toml, specs/forecast.ioa.toml, and wasm/register_forecasts/src/lib.rs.

## D10 — Keep learning callbacks outside operator authority

**Decision**: Restrict learning, model-adoption, forecast-registration and grading callbacks to their declared system and WASM principals, including against existing broad Admin permits.

**Came up because:** The first review and a failing Cedar test proved that a signed-in dashboard admin could submit forged LearningRun.Prepared data. The same broad permits cover World and Forecast, whose callbacks supply the new learner's model and scored examples.

**Options:** Narrow only LearningRun's Admin permit; enforce the boundary across every callback that produces this learner's model or prediction record.

**Chose the complete callback boundary because:** Cedar permits are additive. Explicit denies prevent broader dashboard permissions from bypassing the new integrity contract while retaining create/start, replay, prediction-update and reviewed outcome-entry controls.

**Where:** os-apps/paw-foresight/policies/foresight.cedar; crates/temperpaw/tests/corridor_cedar_matrix.rs.

## D11 — Recover registration and preserve its model snapshot

**Decision**: Return failed forecast registration to an operable world with a visible error, and retain one model snapshot throughout each registration pass.

**Came up because:** Review found that a bad replay date or transient lookup failure permanently failed the world, while concurrent adoption could interrupt or change a registration pass.

**Options:** Keep terminal failure and require a new world; hide invalid dates by adjusting them; preserve the supplied clock, expose the error and allow a retry.

**Chose recovery with a frozen pass model because:** Invalid or unavailable inputs should not destroy the world, and forecasts in one pass need an auditable model identity. Observed registration uses the host event time; replay and hindcast retain explicit logical time. A matching authoritative forecast read advances the loop even if its list projection lags.

**Where:** os-apps/paw-foresight/specs/world.ioa.toml and wasm/register_forecasts/src/lib.rs.

## D12 — Start learning once after a grading batch

**Decision**: Use a separate system-only batch scoring transition and one declared learning trigger after hindcast grading completes.

**Came up because:** The new per-Score trigger would start a competing learning run for every revision in a hindcast batch.

**Options:** Keep every per-revision run; add a background scheduler; preserve existing batch grading and trigger learning once from its completion.

**Chose the batch completion trigger because:** It retains all scores without a new orchestration service or repeated fitting over the same batch. Standalone scoring and reviewed outcome entry retain their automatic feedback paths.

**Where:** os-apps/paw-foresight/specs/forecast.ioa.toml, specs/hindcast.ioa.toml, and wasm/grade_hindcast/src/lib.rs.

## D13 — Use configured research models and the existing settings page

**Decision**: Research creation uses the server's configured provider/model pair and links missing setup to the authenticated settings page.

**Came up because:** The first review found that the new form omitted both fields required by seed_world. The existing welcome flow cannot configure every supported provider or the model.

**Options:** Guess model defaults; build another provider form; reuse non-secret setup metadata and the existing settings controls.

**Chose the existing setup contract because:** It gives the UI a real configured pair without exposing credentials or duplicating model configuration. Settings must remain reachable before initial setup is complete; authentication still applies. Replay remains available independently of provider setup.

**Where:** dashboard/src/routes/foresight/+page.svelte and dashboard setup routing.

## D14 — Package the required file-storage module

**Decision**: Use one aggregate paw-fs WASM build entry in Docker and CI so all existing child module builders run.

**Came up because:** The immutable release image for d758711 failed its isolated fresh startup: paw-fs declares artifact_batch_apply as app-required, but Docker builds only blob_adapter and workspace_fs. The missing artifact predates this effort, but prevents the complete release image from booting for this app's required verification.

**Options:** Add another per-module entry to both callers; give paw-fs one aggregate build entry like the other apps; trim the core app or copy a module into the extracted image.

**Chose the aggregate entry because:** It removes the duplicated per-module selection that caused the omission, reuses the existing builders, and keeps the tested image intact. The change is limited to packaging and its existing regression coverage; file-storage behavior and the platform are unchanged.

**Where:** Dockerfile; .github/workflows/ci.yml; os-apps/paw-fs/wasm/build.sh; existing WASM packaging tests. Failed immutable image sha256:d221367b10339a427e2be4c7fe9aac73a285a8728313a243adcc5792ed85b2cb; raw evidence /tmp/arn518-image-verification/image-verification-d758711.json.

## D15 — Permit dashboard research to create its workspace

**Decision:** Grant authenticated admins create-only access to Workspace in the Foresight app policy.

**Came up because:** On 16 September, the real-provider browser flow at 33e74b7 created and configured a world, then failed at seed_world's Workspace POST with HTTP 403. The kernel preserves the initiating admin identity for transition-triggered internal HTTP; the filesystem policy permits only agents to create workspaces.

**Options:** Add the required create-only app permission; impersonate an agent or change kernel identity propagation; pre-create workspaces outside the user flow.

**Chose the create-only permission because:** It preserves verified caller identity and makes the normal dashboard flow work without kernel changes or special test setup. It grants no workspace read, update, delete, freeze, or archive access. Rita explicitly approved this permission change after the denial was reported.

**Where:** os-apps/paw-foresight/policies/foresight.cedar; crates/temperpaw/tests/corridor_cedar_matrix.rs; PR #526. The regression failed against 33e74b7 before the permission was added.

## D16 — Authorize the session runtime's transcript operations

**Decision:** Permit only Agent::"service:wasm-runtime" to create, read and list SessionEntry records in the owning paw-agent policy.

**Came up because:** After the approved workspace fix, browser research created its workspace and session, but the callback runtime failed to materialize the first transcript entry with AuthorizationDenied (PD-01a0aaa4-7ad0-75f0-8821-339e0e501e58). Existing permits cover admins and named agent types but omit the authenticated callback runtime.

**Options:** Grant the exact runtime identity its append/read operations; broaden access by agent type; bypass transcript materialization or change kernel identities.

**Chose the exact identity because:** It repairs the existing session pipeline while preserving authentication and denying unrelated identities, transcript updates and deletes. Rita explicitly approved this narrow permission after the reproduced denial. The change belongs to paw-agent and must be published with that dependency before the Foresight app is installed.

**Where:** os-apps/paw-agent/policies/session_entry.cedar; crates/temperpaw/tests/session_entry_runtime_policy.rs; PR #526. The new regression failed on transcript creation before the permission was added.

## D17 — Record the world's research session explicitly

**Decision:** Store the last reported research session ID on World through a declared seed_world callback and show that exact Session's durable status and error in Foresight Activity.

**Came up because:** A real provider run left World in Seeding after its surveyor Session failed, while Activity said no failures were recorded. seed_world discarded the Session ID and returned the Agent ID from its spawn helper. No durable World-to-Session association existed; matching agent names or prompt text would make the display unreliable.

**Options:** Infer a session from names or prompts; add a general session-monitoring service; record the exact ID returned by Session creation through the existing WASM callback path.

**Chose the explicit callback because:** It adds one association field, one attempt counter and one trusted callback without changing the kernel or the world's research lifecycle. Only the named WASM runtime can report the association, and callers cannot forge it during World creation. The callback preserves every post-seeding state, including a completed or archived world, so a late callback cannot undo progress. Seed and ResumeSeed increment the attempt counter. The callback carries the attempt captured in its WASM context and must equal the current counter, so an older callback cannot overwrite a newer session after ResumeSeed. The current kernel does not supply an originating-invocation guard; the existing app constraint provides it atomically. The reverse-order regression uses the app's pinned runtime evaluator, including world completion before the stale callback, and fails when the equality constraint is removed. Existing worlds without a pointer remain explicitly unverified; no names or prompts are treated as identity. The current IOA has no string-clear effect, so a retry keeps the last recorded session visible until the new callback arrives; the UI labels that limit explicitly. Authenticated Session detail routes are reachable before first-run setup completes, so the diagnostic link can open; the login guard and other agent setup routes are unchanged.

**Where:** os-apps/paw-foresight/wasm/seed_world/src/lib.rs; os-apps/paw-foresight/specs/world.ioa.toml and model.csdl.xml; os-apps/paw-foresight/policies/foresight.cedar; dashboard/src/lib/foresight.ts; dashboard/src/routes/foresight/+page.svelte; focused dashboard, Cedar, transition-contract and real-WASM tests; PR #526.

## D18 — Use bounded background continuations from the kernel

**Decision:** Pin this app release to the reviewed Temper kernel fix that separates inline callback depth from total background callback hops.

**Came up because:** The real-provider preview completed two OpenAI responses and an Exa search, then stopped at the kernel's eight-callback depth limit despite a forty-turn Session budget. A detached task retained the previous inline depth. Rita explicitly approved the scoped kernel fix after this failure was reported.

**Options:** Reduce research to fewer callbacks; raise or remove the existing recursion limit; retain the inline limit and separately bound background continuation.

**Chose separate bounds because:** The existing inline limit of eight still stops recursion, while a finite 512-hop internal context-lineage budget allows ordinary research to progress. Detaching resets only inline depth. The kernel change preserves authentication, principal propagation, reaction limits and application turn and spending bounds. It does not claim aggregate fanout accounting or persistence of this budget across restarts.

**Where:** nerdsane/temper PR #473 and docs/efforts/ARN-518 in that repository; the two Temper dependency manifests and Cargo.lock in this PR. Delivery requires the reviewed kernel revision, a rebuilt app, and a successful real-provider preview before release.


## D19 — Preserve the published agent package across divergent Git source

**Decision:** Base the agent release package on published paw-agent b72f5d6e9b9c6e00c4b887f0dd0892b767be45dd, preserving its source and WASM bytes except the required SessionEntry permission and an exact paw-fs dependency pin.

**Came up because:** Genesis Git main c2e99e5e5bae0dbfc667fee929100cef32ecebc6 differs from the published dependency package. The published package preserves streamed tool calls at response completion and contains compaction, tool-completion and sandbox-cleanup behavior absent from Git main. Git main separately contains OpenRouter work, so branch names do not establish chronology or compatibility.

**Options:** Publish the Git main candidate; combine the divergent implementations; preserve the currently published package with the two release-required changes.

**Chose the published package because:** Genesis publication is the installed application contract. A wholesale source replacement would regress working provider behavior; merging independent provider changes is outside this delivery. Publication must retain the divergent Git main and use a separate source branch if supported. The old production manifests also float dependencies, so rollback preparation must pin the complete captured package closure rather than assume that reinstalling old root refs restores old dependencies.

**Where:** Genesis paw-agent release preparation for ARN-518; dedicated Foresight installed metadata captured in tenant_installed_apps; app PR526 and kernel PR473. Candidate packages remain unpublished until their exact bytes pass isolated verification.


## D20 — Reconstruct published package trees without inventing Git ancestry

**Decision:** Prepare independent Genesis release branches from exact published bundle bytes, recording their source version in a reconstruction baseline commit before applying the intended delta.

**Came up because:** The supported Genesis Git server refuses fetches for unadvertised historical objects, including the currently published agent version. The supported bundle API exposes the exact published file bytes but no commit ancestry. The divergent main branch must remain intact.

**Options:** Overwrite main; pretend the unavailable object is a verified Git parent; reconstruct the published tree on a new branch with explicit provenance.

**Chose explicit reconstruction because:** It preserves published source and WASM bytes and retains the real two-file release delta without claiming unavailable history or bypassing the Git server restriction. These commits have new hashes; bundle and file hashes prove their source provenance. The same rule applies to rollback snapshots, whose manifests alone receive closed dependency pins.

**Where:** Local Genesis release and rollback branches under ARN-518; registry publication uses each exact new branch ref after isolated verification. Existing remote main refs are unchanged.

The preserved Foresight tree includes Genesis-only repair/adversary workspace reuse and inline file-context handling absent from the mirror. Their complete original module source and published WASM bytes are retained; mirror build inputs for those modules are aligned to that source.


## D21 — Require the research session to finish through its tool contract

**Decision:** Configure seeded research sessions with the published agent runtime's existing tool_choice="required" setting.

**Came up because:** The real provider-backed surveyor created three events and completed nine tool turns, then ended with a plain-text narration of prior tool calls without issuing World.SeedComplete. Its world remained Seeding. The session did not exhaust its40-turn bound; the new kernel successfully continued past the original callback limit.

**Options:** Accept the incomplete text result; repeatedly retry the same unconstrained session; add a new completion coordinator; use the published runtime's existing required-tool configuration.

**Chose the existing configuration because:** The surveyor's contract already requires SeedComplete followed by temper.done. Requiring a tool response prevents ordinary text-only completion for the configured Responses provider and lets the existing explicit done path terminate the session. It does not guarantee evidence quality or replace checking that the world actually became Active. Turn limits remain finite and the full provider flow must be rerun.

**Where:** seed_world Session.Configure request and its actual-WASM regression; published paw-agent b72f5d6 provider caller's OpenAI Codex Responses request; isolated observed-world test for ARN-518.

## D22 — Expose first-pass future exploration without inventing resampling

**Decision:** Add an Active-world control for the existing SampleEndpoints action, disable it once endpoints exist, and display each endpoint's actual progress.

**Came up because:** The live surveyor completed SeedComplete and recorded eight observed events, but the UI exposed only forecast registration. Registration does not create alternative futures. The existing endpoint sampler reuses slots without resetting settled endpoint states, so presenting it as a general retry would be misleading.

**Options:** Leave the action available only through generic entity tools; add a full resampling protocol; expose the supported first pass with honest progress.

**Chose the first pass because:** It makes the requested worlds flow usable with the existing application contract while avoiding duplicate writer launches and an unrelated lifecycle redesign. Existing endpoints remain inspectable, including failure states.

**Where:** Foresight dashboard world and activity controls; World.SampleEndpoints contract; isolated observed world en-01a0ab41-48cc-7001-a581-84b89bbdbeff.

## D23 — Apply explicit completion to tool-dependent Foresight workers

**Decision:** Require tool responses in every audited Foresight worker configuration whose completion contract requires an entity action or temper.done.

**Came up because:** After the seed fix completed research in sixteen turns, one of three endpoint writers ended with plain text narrating BundleWritten and left its endpoint Sampled. The other two wrote their bundles. The endpoint and downstream worker launchers omitted the same existing tool_choice setting.

**Options:** Repair only the failed test entity; repeat unconstrained sessions; apply the seed's tested configuration to the affected worker launchers.

**Chose the shared configuration rule because:** The observed failure is the same premature text-only completion class. Each worker already has a bounded tool-based completion contract. This changes request configuration only, preserves the existing published application behavior, and does not introduce a runtime coordinator or guarantee that a worker's content is correct.

**Where:** Foresight Session.Configure calls in sample_endpoints, spawn_repairers, spawn_adversaries, decompose_endpoint, consistency_gate, adjudicate_nodes, render_artifacts and animate_dwellers. Each has an explicit tool-based completion contract. Actual outgoing-request tests and rebuilt module hashes provide the verification boundary.

## D24 — Open outcome forms at the selected world's clock

**Decision:** Initialize each new outcome form from the selected replay clock for historical and simulated worlds, or fresh current UTC for observed worlds, and clear the prior draft's answer and sources.

**Came up because:** Browser verification opened a simulated outcome at a 2025 replay clock, but the form retained the computer's 2026 page-load time. The backend accepts explicitly dated replay outcomes and uses their resolution time as the automatic learning cutoff, so submitting that default would advance the outcome and learning run beyond the selected replay point.

**Options:** Leave the user to correct the computer-time default; force every outcome to the replay clock even in observed worlds; select the appropriate clock whenever the form opens.

**Chose opening-time defaults because:** It preserves editable historical evidence dates, avoids carrying another forecast's draft across openings, and uses current time for real observed outcomes. The input keeps seconds so opening an observed outcome does not round its timestamp down before a recently registered prediction. Backend chronology and evidence validation remain authoritative.

**Where:** Foresight dashboard outcome form, outcomeFormDefaults helper, and focused replay/observed/default-reset tests.


## D25 — Enter deterministic corridor work through declared system reactions

**Decision:** Keep session self-reports separate from deterministic corridor entry actions, and enter those actions through declared system entity triggers.

**Came up because:** The real endpoint writer completed its bundle, but the all-written barrier inherited `service:wasm-runtime` and was denied `World.GateDiversity`. The same identity loss affects later deterministic integrations entered from session results.

**Options:** Grant the shared session relay deterministic actions; rewrite the legacy corridor into returned composite batches; add trusted entry actions using the existing entity-trigger principal contract.

**Chose the declared entry actions because:** The kernel assigns their system principal and preserves it across the existing internal HTTP path. Direct session and dashboard access stays denied. Composite batches do not run target integrations on PostgreSQL, so that rewrite would stop the pipeline. The existing legacy modules already dispatch cross-entity actions; this repair retains those calls and adds no new direct HTTP write. The user prioritized completing the full vertical slice and authorized required repairs, so a whole-engine migration is outside this repair. This is an explicit tradeoff against applying the no-direct-WASM-dispatch guidance retroactively to the legacy corridor.

**Where:** `os-apps/paw-foresight/specs/endpoint.ioa.toml`, `specs/path.ioa.toml`, `policies/foresight.cedar`, and `crates/temperpaw/tests/foresight_gate_identity.rs`; PR #526.

The protected pruned-route report preserves `Pruned`, so the old `PrunedIsFinal` assertion (`no_further_transitions`) no longer expresses the intended contract: it forbids reporting as well as reopening. Remove that assertion and test that the sole action enabled from `Pruned` is the protected, state-preserving report. A pruned route remains terminal; no repair or challenge can restart it.

## D26 — Publish a bounded first pass, then deepen it

**Decision:** Extract at most three ranked claims per future, evaluate one repaired and challenged route per claim, publish accepted paths' predictions as they finish, and start bounded deeper exploration after the first pass and its queued prediction batch complete.

**Came up because:** The observed run extracted 19 claims and withheld all predictions behind full-world settlement. Revision rounds and alternate routes amplified work, while completed paths were already useful. The user explicitly approved immediate publication, three claims per future, background alternatives, and faster recovery.

**Options:** Keep the whole-world barrier; remove challenges to return faster; retain challenges but separate the first result from deeper search.

**Chose a challenged first pass because:** It reduces initial work without presenting unchallenged paths as evaluated. Three futures produce at most nine initial claim workers. The first pass uses one route and no extra revision rounds; background exploration retains the existing three-route/two-revision ceiling. High-cost first-pass findings remain visibly strained or unreachable. Predictions use only Canonical/Tail path requirements and keep frozen inputs, model, and registration time per batch. Concurrent completion requests coalesce, and attempt/cursor guards reject stale callbacks. Prior paths and immutable forecasts remain available as exploration continues.

**Where:** Foresight World/Claim/Path specs; aggregate_costs, decompose_endpoint, spawn_repairers, and register_forecasts modules; registration actor and actual-WASM tests; PR #526.

## D27 — Recover provider streams without losing executable tool history

**Decision:** Preserve matched Responses function calls and outputs as structured input, stop reading on explicit terminal response events, and enter provider work through declared system reactions with a bounded Foresight first-pass profile.

**Came up because:** Thirteen failed repairer streams copied the client's historical `Tool call …` prose rather than emitting executable tool calls. Partial text arrived within a few minutes, but the client kept reading until its 600-second outer limit. Progress callbacks and timer-driven repair restarts also lost their trusted entry identity and were denied.

**Options:** Increase concurrency; shorten every provider deadline; execute tool-like prose; repair structured history and terminal handling while scoping the faster budget to first-pass Foresight sessions.

**Chose the scoped repair because:** Tool execution must come from actual provider tool events. Explicit completion/failure events should not wait for transport EOF. Standard sessions retain their existing budget; Foresight's explicit first-pass profile gets a 180-second hard call limit and at most one sequential retry per session. This is a total-call deadline, not an idle timeout. Trusted declared entries restore progress reporting and recovery without granting the shared WASM relay broad permissions. Existing published Session capabilities and current token tracking remain preserved.

**Where:** paw-agent provider_caller, Session spec/CSDL/policy and provider runtime tests; Foresight launcher configuration and recovery entries; PR #526.

## D28 — Keep recovery and workspace reuse within trusted boundaries

**Decision:** Permit tenant-scoped Workspace read/list only for declared system workers, normalize a zero sampling budget to one, and leave the world usable when a stale phase-fenced cascade report is rejected.

**Came up because:** Review confirmed that the spawners' workspace lookup had no matching permit in the complete installed closure, so it fell back to duplicate creation. A zero endpoint budget created no future and left the UI waiting. The new phase guard could reject an old first-pass report after deepening began; compensating that rejection with World.Fail would incorrectly stop newer work.

**Options:** Broaden agent/operator workspace access; preserve denied lookups; accept an empty exploration; fail the world on every cascade error; grant only trusted lookup access and retain bounded retryable progress.

**Chose the narrow changes because:** System workers can reuse workspaces without giving model sessions or operators browse/management rights. Every accepted sampling request starts at least one future. A rejected stale cascade stays recorded as an integration failure and cannot kill newer exploration; the world's existing timeout can retry a genuine failed cascade.

**Where:** Foresight Cedar, sample_endpoints, World cascade trigger, and focused negative/positive budget and policy tests; PR #526.


## D29 — Finish the first pass only after every future has attached its claims

**Decision:** Require all sampled futures to have a durable, fully listed claim set before marking the first pass complete; failed or discarded futures remain explicit exceptions.

**Came up because:** A real-WASM regression showed that one future's settled claim could trigger PathsScored while a second future was still decomposing and had no attached claims. The new background pass would then capture an incomplete claim set.

**Options:** Rely on decomposers usually being faster than path evaluation; hold every prediction until all futures finish; check endpoint claim attachments only at the pass-completion boundary.

**Chose the completion barrier because:** Individual accepted paths still publish immediately. The world cannot declare a complete first pass or start background exploration until every active future's attached claims are included in the terminal claim set. Collection omissions defer completion instead of silently excluding work.

**Where:** aggregate_costs world cascade and actual-WASM progressive exploration regression; PR #526.

## D30 — Keep changed agent modules’ build inputs separate from published consumers

**Decision:** Package private build-library copies for `provider_caller` and `context_preparer`, while preserving the published agent’s other libraries, module sources, and WASMs byte for byte.

**Came up because:** The tested provider and context changes use newer library APIs than published `paw-agent@b72f5d6`. Replacing the shared libraries globally made the unchanged published sandbox consumer fail compilation with eight errors, including missing sandbox helpers and incompatible SDK context types.

**Options:** Replace the shared libraries and migrate unrelated consumers; ship changed WASMs without their complete source inputs; or include private library copies used only by the two changed modules.

**Chose private copies over global replacement because:** It supplies the actual inputs needed to rebuild these two modules without removing published sandbox behavior or expanding this effort into an agent-wide migration. The copies match the owning worktree’s library bytes; the receipt records each source path, hash, dependency edge, and Cargo path change. The tradeoff is four duplicated library inputs confined to these modules. The resulting candidate preserves 170 original files, including every unchanged WASM. Verification passed 40 provider unit tests, 18 context unit tests, five tests against the rebuilt provider WASM, and compilation of the unchanged sandbox consumer. The rebuilt context WASM still requires the final live closure acceptance; these checks do not establish external-provider success.

**Where:** Candidate `paw-agent/wasm/provider-context-libs/`; the two modules’ `Cargo.toml` and lockfiles; source mapping and evidence in `/private/tmp/arn518-agent-isolated-proof/source-closure.json` and `verification.json`. This is an unpublished candidate pending final source reconciliation and live acceptance.

## D31 — Preserve provider limits and stop futile retries

**Decision:** Treat an OpenAI Codex `usage_limit_reached` HTTP 429 as terminal for the current provider invocation, and retain only its bounded error type, code, message, and numeric reset times.

**Came up because:** A real Foresight seed received `usage_limit_reached` from an exhausted preview account. The provider module discarded the useful body, sent five immediate requests, then incorrectly reported that no HTTP response had arrived. The provider account was corrected separately under the existing temporary-credential authorization; retrying did not restore quota.

**Options:** Keep the generic retry/error path; change all provider retry policies; or classify this terminal response and preserve the observed response boundary.

**Chose the narrow classification over a retry redesign because:** It reports an actionable HTTP 429 and ends a request sequence that cannot restore account quota, while preserving the existing five-attempt budget for transient rate limits. Stream failures also retain a received HTTP status, so final errors distinguish a failed response from failure before a response. Messages are limited to 512 characters, identifiers to 64, and only the declared fields from a bounded JSON body are retained. Session-level recovery is unchanged: a Foresight session may still perform its one sequential outer retry, giving two terminal requests across two invocations rather than five per invocation. No model, provider, credential, or account reset behavior changes in this module.

**Where:** `os-apps/paw-agent/wasm/provider_caller/src/lib.rs`; `crates/temperpaw/tests/foresight_provider_profile/mod.rs`. The same final test binary exercised frozen old and new WASMs: four new cases failed on the old module and passed on the new module; the five existing profile cases passed on both. All 41 module unit tests passed. Packaging and source mappings are in `/private/tmp/arn518-provider-429-proof/verification.json` and `source-closure.json`.

## D32 — Make failed research retryable from its visible session

**Decision:** Show “Retry research” only for a Seeding world whose linked research Session has been read successfully and is Failed or Cancelled, and invoke the existing World.ResumeSeed action.

**Came up because:** The fresh observed-world run encountered a provider quota rejection before its first research turn. Activity correctly displayed the failed Session, but the user had no recovery control: the alternatives were an operator API call or the existing 20-minute Seeding timeout.

**Options:** Keep recovery API-only; retry automatically from the dashboard; expose an unrestricted research restart; or provide a button guarded by the selected world and its verified linked Session.

**Chose the guarded button because:** It makes the failure recoverable through the same UI that explains it, without starting duplicate research automatically or treating an unavailable Session read as a confirmed failure. The page marks the failed Session ID before dispatch and blocks further requests for that ID until a replacement Session is recorded, including during delayed refreshes and world switching. A definite HTTP client rejection releases the guard; a transport failure or timeout retains it while the user checks the world. HTTP status is carried as structured error metadata rather than parsed from prose. The guard is per-page, avoiding a persistent browser lock that could strand recovery after an unsent request. Existing authentication, World action rules, and the ResearchSessionStarted attempt fence remain the backend authority.

**Where:** `dashboard/src/lib/foresight.ts:77` (eligibility and duplicate guard), `dashboard/src/routes/foresight/+page.svelte:179` and `:489` (action and visible control), `dashboard/src/lib/api.ts:126` (HTTP status metadata), `dashboard/tests/foresight-session.test.mjs` and `dashboard/tests/foresight-transport.test.mjs` (eligibility, concurrent requests, old-session reuse, transport uncertainty, and real HTTP rejection).


## D33 — Drain queued registration requests after a batch fails

**Decision:** After registration failure or timeout, start a new batch only when a request arrived after the previous batch began.

**Came up because:** Review identified that a pending request survived either recovery transition but had no continuation. That stranded later accepted paths and blocked background exploration even though the world had returned to Active. A new actor regression reproduced the missing continuation before the change.

**Options:** Retry every failed batch automatically; discard pending work on failure; or drain only the existing guarded queue.

**Chose the guarded queue because:** It preserves later work without introducing an unbounded retry loop. Starting a batch consumes the pending flag, and attempt guards still reject stale callbacks. A second failure without another request stops with the error visible. The same rule applies to explicit failure and the sequence-checked timeout.

**Where:** `os-apps/paw-foresight/specs/world.ioa.toml`, `crates/temperpaw/tests/foresight_registration_contract.rs`; PR #526. All 11 registration contract tests pass; the new regression failed on the original missing trigger.


## D34 — Validate existing route attachments before starting workers

**Decision:** Treat an absent or blank initial claim route list as empty, reject malformed nonempty input, and validate the list before creating any path or worker.

**Came up because:** The observed first pass saved three futures and scored nine paths, but all nine claims failed when the new route-retention code parsed an unmaterialized initial `path_ids` value as JSON after creating the workers. The declared IOA default was already `[]`; repeating that declaration would not fix the actual module input.

**Options:** Repeat the state default; parse after creating work; coerce every malformed value to empty; or distinguish an absent initial value from malformed data before side effects.

**Chose validation before side effects because:** It preserves prior route IDs during background exploration and prevents invalid input from leaving unattached workers. Blank initial state is legitimate; malformed existing state must remain an explicit failure. The already failed claims are final by their declared contract. Their scored paths remain preserved as evidence, and verification uses a new normal run instead of adding a test-only resurrection action or patching database state. The dashboard also includes failed claims in its existing failure list so this stage cannot appear healthy while attachment has failed.

**Where:** `os-apps/paw-foresight/wasm/spawn_repairers/src/lib.rs`; actual-WASM regression; `dashboard/src/lib/foresight.ts` and `dashboard/src/routes/foresight/+page.svelte`; PR #526.


## D35 — Package the tested app and UI over the matching immutable server image

**Decision:** Build a delivery image from the existing immutable `681969c` server image and overlay the exact tested app closure and dashboard, subject to native-input equivalence and resulting-image verification.

**Came up because:** The live preview uses a development binary with the required kernel callback fix, while the dedicated production image predates it. A successful existing Linux release build at `681969c` already contains that native source. Rebuilding the whole server and every unrelated WASM would add avoidable delivery time.

**Options:** Deploy the older production server; rebuild all native and WASM components; or retain the matching immutable release server and package only the verified app and UI files.

**Chose the immutable base because:** It preserves the compiled server and unrelated packaged applications while allowing the final app closure and UI to be checked byte for byte. The base is `ghcr.io/nerdsane/temperpaw@sha256:5846ef5906e86bb260f411af30c165abdbb8aa243b958886622585a4f328f831`, with OCI revision `681969cc909ea9cfcb82a90d3d64f9eb91c2a775`. This is a packaging choice, not a verification exemption: native build inputs must match, the resulting image must run before deployment, and production must report the pinned application versions. The latest fetched GitHub main is `07cbb1d`, an ancestor of this branch. No shared CI or deployment workflow is changed.

**Where:** The bounded release recipe and receipts under `/private/tmp/arn518-genesis-final-02944f0/`; the final tested 360-file Genesis closure and 73-file dashboard build. Publication and production activation remain separate steps after verification and review.


## D36 — Keep Foresight record links usable before unrelated setup is complete

**Decision:** Generate Foresight detail URLs with their OData collection names and permit authenticated access to those seven detail routes before agent setup is complete.

**Came up because:** Live verification followed a published prediction from a canonical path. The singular `/entities/Forecast/<id>` URL returned 404; the correct `/entities/Forecasts/<id>` URL then redirected to Welcome because the setup guard did not exempt Foresight record details.

**Options:** Keep the broken links; broaden the setup exemption to every entity route; or correct the shared Foresight link builder and add an explicit seven-collection detail-route exemption.

**Chose the scoped correction because:** Users can inspect worlds, events, futures, claims, paths, forecasts, and learning runs through the existing generic detail page. Authentication, tenant authorization, collection pages, unrelated entities, and nested routes remain subject to their existing checks. No native or generic detail-page change is required.

**Where:** `dashboard/src/routes/foresight/+page.svelte` recordHref; `dashboard/src/lib/setup-routing.ts` requiresAgentSetup; focused link and setup-route tests. Live failure and verification evidence: `/private/tmp/arn518-record-links/`.


## D37 — Keep retrospective prerequisites out of forward predictions

**Decision:** Register a prediction only if its resolution deadline is later than the frozen batch registration clock; retain earlier prerequisites as path context.

**Came up because:** The first complete observed pass produced 42 records, six of which described prerequisites dated before registration. They were legitimately part of backward paths but were incorrectly published as forward predictions. The existing selection checked the world's farthest forecast date, not the lower time boundary.

**Options:** Treat past-dated requirements as forecasts; delete or rewrite already registered records; or enforce the lower boundary before each new registration using the immutable batch clock.

**Chose the temporal boundary because:** The system preserves honest historical context and immutable audit history while preventing newly published predictions about already elapsed deadlines. Observed mode uses the host-recorded snapshot clock; historical and simulated modes use their explicit logical clocks, so replay remains useful without waiting for real time. Date-only deadlines include their UTC day; exact UTC timestamp deadlines must be strictly later than registration. Invalid deadlines fail visibly through the existing recoverable registration callback. Earlier test records remain preserved and disclosed rather than silently rewritten.

**Where:** `os-apps/paw-foresight/wasm/register_forecasts/src/lib.rs`; actual-WASM clock-boundary tests in `crates/temperpaw/tests/foresight_learning_wasm.rs`; the observed first-pass receipt and its six retrospective records in `/private/tmp/arn518-visible-proof/fresh-background-state.json`.


## D38 — Let failure override an old exploration phase

**Decision:** Display the world's Failed status before any saved first-pass or deepening progress message.

**Came up because:** The background registration crash left exploration_phase=deepening, so Activity incorrectly said the engine was still exploring.

**Options:** Clear historical phase data; hide the failure; or make the visible failure status take precedence.

**Chose status precedence because:** It preserves saved progress and predictions while explaining why no further work is running. The same input fails the previous phase-only text test and passes the corrected helper.

**Where:** dashboard/src/lib/foresight.ts, dashboard/src/routes/foresight/+page.svelte and dashboard/tests/foresight-research.test.mjs.

## D39 — Reconnect the original UI and preserve recovered history separately

**Decision:** Reuse the existing Temper-backed Deep Sci-Fi frontend, read current runs from persistent storage, and show recovered preview exports as labeled read-only snapshots.

**Came up because:** The user explicitly requested the original presentation and an always-available interface. Its existing Vercel branch codex/dsf-2 still reads the dedicated Foresight service, while the recent preview database was destroyed and only partial exports survived. This supersedes D1's presentation choice.

**Options:** Keep the temporary dashboard alone; merge the unrelated fiction-to-Temper rewrite into fiction production; fabricate live entities from partial exports; or extend the existing Foresight deployment and use a separate historical read model.

**Chose the existing deployment and historical read model because:** It preserves the user's chosen interface and actual observations without changing fiction production or pretending missing stories and events were recovered. Snapshot files and their manifest use existing persistent file storage; the UI does not offer engine mutations for recovered history. New unattended acceptance runs use a separate persistent Railway environment and remain browsable after testing.

**Where:** original UI branch codex/dsf-2 in arni-labs/deep-sci-fi; its focused ARN-518 follow-up; dedicated Foresight Railway project; docs/efforts/ARN-518/plan.md.


## D40 — Pin the kernel actually verified in persistent acceptance

**Decision:** Pin the application dependencies to Temper `a40d3795bb5361e645d9a5ffda01cc30c5239518`, the reviewed context-allocation repair used by the acceptance image.

**Came up because:** Large forecast-registration contexts overwrote guest static memory in the earlier runtime. Temper PR #479 merged the repair, and the persistent acceptance run now registers predictions without that crash, but this app branch still referenced the older runtime.

**Options:** Leave the old dependency and rely on an image-only fix; adopt an unverified newer runtime; or pin the exact repaired source used by acceptance.

**Chose the verified source because:** The release source must reproduce the repair rather than silently reverting it during a rebuild. The kernel change was independently reviewed and tested in its owning repository; this app change aligns the two manifests and lockfile with the native image already under live verification. Kernel PR #479 merged as `42e6223c460eb22f24248a98754d3487a7a97808`.

**Where:** `crates/temperpaw/Cargo.toml`, `crates/paw-codex-worker/Cargo.toml`, `Cargo.lock`; acceptance image native source `9836b4bc21b67b02a575fc241050e179ab2022c1`.

## D41 — Measure Jev in shadow mode before changing routing
- **Decision:** Add a pinned Jev semantic evaluator without allowing its judgments to change criticism, costing or forecasts.
- **Came up because:** Rita approved implementing the Jev proposal and comparing the rearchitecture with the current full pipeline.
- **Options:** Replace the critic immediately; shadow the existing critic with recorded evaluator inputs.
- **Chose shadow over replacement because:** No Foresight-held-out error rates or routing threshold have been demonstrated. This preserves a valid comparator while measuring coverage, disagreement, latency and usage.
- **Where:** os-apps/paw-foresight/wasm/evaluate_semantics; docs/efforts/ARN-518/jev-shadow.md.

## D42 — Store asynchronous judgments independently of path progress
- **Decision:** Each path challenge spawns a SemanticEvaluation whose terminal record is immutable.
- **Came up because:** A path can revise or finish before a shadow response returns; storing its result on that path would reject late callbacks or overwrite earlier rounds.
- **Options:** Store the latest result on Path; use a separate bounded evaluation lifecycle.
- **Chose the evaluation lifecycle because:** It records success, skipped, failed and timed-out evaluations without changing the path's state or losing earlier observations.
- **Where:** os-apps/paw-foresight/specs/semantic_evaluation.ioa.toml.

## D43 — Freeze identical complete context before starting either evaluator

**Decision:** Prepare one hashed packet containing full endpoint and repair narratives, observed evidence, required nodes, repair flags and a shared four-question rubric. Persist it before launching the critic and shadow evaluator. In matched shadow mode neither consumer independently retrieves more context.

**Came up because:** The earlier three-question Jev implementation omitted the endpoint narrative and the broader critic checks, making the initial comparison unfair. The user explicitly requested equal context and a full comparison.

**Options:** Independently retrieve documents in each consumer; send a reduced summary only to Jev; or freeze and share one packet.

**Chose the shared packet because:** It prevents temporal drift, lost context and differing question scope. Hash verification fails closed on changed packets. A stale preparation is rejected by repair-log identity. Complete packets above the size bound fail instead of being silently truncated.

**Where:** `prepare_challenge`, `evaluation_contract.json`, `evaluation_packet.rs`, Path.ChallengePrepared/LaunchChallenge and both consumers.

## D44 — Distinguish paired evaluator evidence from pipeline and forecasting claims

**Decision:** Compare fresh GPT-5.5 and Jev judgments on all 22 completed run-C paths, holding the full packets fixed; retain the incumbent critic while disagreements lack independent labels.

**Came up because:** The user required a fair comparison. Identical-input live runs agree on 63 of 88 judgments, with most disagreement on unexplained discontinuities.

**Options:** Treat the incumbent as truth; treat more Jev flags as better accuracy; or report agreement and limits without inventing labels.

**Chose explicit limits because:** More flags can mean sensitivity or false alarms. The reasoning-session and Jev invocation clocks differ, reasoning usage counters are unusable, and future outcomes are not observed. A critic-stage replay cannot establish a whole-pipeline speedup.

**Where:** `scripts/compare_foresight_evaluators.py`, `docs/efforts/ARN-518/jev-shadow.md` and the Foundry matched-run evidence.

## D45 — Keep the source dependency aligned with the native release package

**Decision:** Pin Foresight to the prepared agent package `temperpaw/paw-agent@1fe41a7a5efd4b71bb72c349cab94b28b211f4ff` and publish that dependency before installing Foresight.

**Came up because:** The native Foresight release package already references this rebuilt agent, while the source manifest still names the older unpublished `801e2fdf7e626dd0d0285b370f626f6b94e80609` version. A later package build would restore that stale dependency.

**Options:** Leave the replacement only in the packaging index; or record the same exact dependency in the owning source manifest.

**Chose the source manifest because:** Source and package must describe the same dependency closure. This pin records the intended release; it does not claim that Genesis publication or live installation has succeeded. Those remain required verification steps.

**Where:** `os-apps/paw-foresight/app.toml`; prepared Foresight package `3aff81d797d20ee8de817c32fd2ebc861080693b`.

## D46 — Deploy bundled Foresight apps while Genesis publication is deferred

**Decision:** Use TemperPaw's existing image-bundled app reconciliation to release the prepared native Foresight packages, verifying acceptance before production; reconcile Genesis publication afterward.

**Came up because:** On 2026-09-19 Rita explicitly requested bypassing the Genesis dependency because Foresight delivery is time-sensitive. Genesis PR #52 remains unpublished while its final review authorization is unresolved.

**Options:** Keep Foresight waiting for Genesis; build a replacement orchestrator; or bundle the exact prepared app packages into the already-verified native runtime image.

**Chose the bundled image because:** The runtime already loads and reconciles bundled specs, policies, and WASM by content digest. This removes the registry repair from the delivery path while retaining Temper-owned orchestration and the existing persistent acceptance service. It does not bypass the denied Genesis review operation or claim registry publication. Preserve the previous deployment for rollback and verify the installed modules, real forecast flow, and restart recovery before production promotion.

**Where:** TemperPaw startup Phase 6b; native source `9836b4bc21b67b02a575fc241050e179ab2022c1`; app source `350ff8435102cc422eda45c2c8766abc5f05d554`; prepared agent `1fe41a7a5efd4b71bb72c349cab94b28b211f4ff` and Foresight `3aff81d797d20ee8de817c32fd2ebc861080693b`.


## D47 — Recognize the verified deployment operator for question starts

**Decision:** Permit the verified tenant operator to ConfigureLearning, Configure, RequestSemanticExploration and Seed a World.

**Came up because:** The first real browser question on the direct acceptance deployment created its World but ConfigureLearning was denied: the pinned runtime resolves the existing server credential as a verified operator, not an Admin.

**Options:** Correct the app's explicit input permission; change identity headers; grant broad operator mutation. **Chose the narrow app permission** because it uses the runtime's credential verification and preserves machine-only callbacks. Caller identity headers cannot confer authority. No provider/result actions are added.

**Where:** `os-apps/paw-foresight/policies/foresight.cedar`; regression in `crates/temperpaw/tests/foresight_registration_contract.rs`; PR #526.


## D48 — Hand requested research to the declared system flow

**Decision:** World.Seed enters a same-World StartSeed entity trigger with the declared system principal; StartSeed alone invokes seed_world.

**Came up because:** The first operator-started World passed configuration but its inherited operator could not create the researcher Session. The previous harness-driven run masked this difference.

**Options:** Give the UI operator Session management; let Seed explicitly hand off to a system action. **Chose the system handoff** because the browser only requests a World, while Temper owns creating and running researchers. StartSeed rejects direct non-system calls. Existing ResumeSeed remains system-only.

**Where:** World IOA/CSDL and Foresight Cedar; contract tests in `foresight_registration_contract.rs`; PR #526.


## D49 — Persist reasoning prompts before spawning the Session

**Decision:** LaunchReasoning saves the prompt and schedules SpawnReasoning; the latter copies the persisted fields into its Session.

**Came up because:** The first automatic SemanticRun completed its evidence snapshot but the child Session received empty prompts. The pinned actor evaluates spawn effects before synchronizing the same action's parameters, so the old action copied its prior empty values.

**Options:** Change kernel effect ordering; use two existing app transitions. **Chose two app transitions** because this expresses the persistence boundary without a kernel change and applies to seed, deepening and synthesis alike. SpawnReasoning is system-only.

**Where:** `semantic_run.ioa.toml`, Foresight CSDL/policy; real actor regression in `foresight_registration_contract.rs`; PR #526.


## D50 — Declare the native payload sizes and normalize empty evidence edges

**Decision:** Keep bounded semantic snapshots/programs inline up to 1 MiB and traces up to 16 MiB using the existing per-field storage contract; normalize EventNode's empty edge default to an empty list. Synthesis includes full option text once and scenario dependency references instead of duplicating all option statements 81 times.

**Came up because:** The first live four-axis response produced an 81-world snapshot of 190,847 bytes. The default 128 KiB field policy replaced it with a blob reference, which string consumers rejected. The UI also rejected valid evidence nodes whose default edges value is empty. These are app contract mismatches, not provider or Genesis failures.

**Options:** Introduce new blob-fetch plumbing across every consumer; declare the existing bounded native payload sizes and avoid duplicated synthesis input. **Chose explicit app field bounds** to retain one durable trace and keep the existing replay/API contract. Existing 256-node, 320-call and provider response bounds remain enforced. Malformed nonempty edge data still fails validation.

**Where:** SemanticRun spec; semantic_prepare and semantic_reasoning modules; corresponding actor/TypeScript regressions; PRs #526 and deep-sci-fi #121.


### D51 — Bound transient event context and size semantic WASM memory

**Decision:** Run the dedicated Foresight service with `TEMPER_RECENT_EVENTS_BUDGET=5` and give SemanticRun WASM integrations a 256 MiB memory ceiling.

**Came up because:** The real 81-world run recorded 81 Jev decisions, then failed while the SDK cloned invocation context containing repeated cumulative traces from the default 50 recent events.

**Options:** Keep the 50-event transient window and only raise memory; change the kernel or remove trace evidence; use the existing runtime event-tail setting with a scoped module memory bound.

**Chose the existing settings because:** Five transient events avoid multiplying the cumulative trace fifty times, while durable events and the complete current trace remain stored. The semantic modules receive extra bounded headroom without changing unrelated modules or waiting on a kernel release.

**Where:** `os-apps/paw-foresight/specs/semantic_run.ioa.toml`; dedicated Foresight Railway service configuration; PR #526.


## D52 — Measure intrinsic dependency depth and share assessment definitions

**Decision:** Compute each node's longest prerequisite path before scheduling Jev calls, and pass the same gap definitions to Jev and synthesis.

**Came up because:** Independent verification found that first-visit DFS depth depended on sorted IDs, allowing prerequisite-first chains to bypass the eight-level limit. A completed synthesis also interpreted the `evidence` gap label as supporting evidence.

**Options:** Relabel the displayed traversal depth and add another prose caveat; compute intrinsic graph depth and supply the existing criterion dictionary at the synthesis boundary.

**Chose intrinsic depth and a shared dictionary because:** They correct the causal errors without changing the graph's evidence or manufacturing results. Node-count bounds cap traversal at 256 nodes; dependencies deeper than eight are explicitly excluded. Missing references and cycles remain recorded. Earlier incorrect runs stay preserved.

**Where:** `os-apps/paw-foresight/wasm/semantic_core.rs`, `semantic_reasoning/src/lib.rs`; PR #526; independent red/green tests in both identifier orders, including 256-node chains.

## D53 — Enforce cancellation and machine-owned result boundaries

**Decision:** Cancel the spawned reasoning Session when its SemanticRun is cancelled, restrict admin actions to starting/cancelling runs, and accept deepening only for unique Jev-selected repair parents.

**Came up because:** Final round-two review found that cancellation left the child running, a blanket admin permit allowed forged failure callbacks, and generated revisions could target an unselected scenario. Review also identified duplicate unchanged trace writes and missing URL setup validation.

**Options:** Leave these as documented operational limitations; enforce the existing declared contract at the state-machine, authorization and generated-output boundaries.

**Chose enforcement because:** These are violations of the accepted app behavior. System timeouts and the runtime's failure callback remain authorized. Evaluate now saves only its changed request, keeping the same persisted program and trace while avoiding redundant cumulative payloads. Empty or unresolved session-reader URLs fail with the existing setup message.

**Where:** `specs/semantic_run.ioa.toml`, `policies/foresight.cedar`, `wasm/semantic_expand`, `wasm/semantic_step`, `wasm/semantic_session`; PR #526.

## D54 — Restore published sandbox cleanup in the owning source

**Decision:** Restore the already-published sandbox release dispatch and destroy helper in the owning agent source and rebuild that module.

**Came up because:** Review identified that the preserved release-capable WASM and terminal Session triggers no longer matched the overlaid Git source, which only provisioned sandboxes. A future source rebuild would regress cleanup or create a new sandbox during a terminal transition.

**Options:** Keep relying on the frozen published binary; remove cleanup; restore only the published release behavior while preserving current provisioning features.

**Chose the narrow restoration because:** It makes source builds retain the deployed capability without a Genesis change or a broader merge of divergent agent implementations. No-handle and static sessions remain no-ops; deletion is idempotent on provider 404 responses and release failure is reported without undoing completion.

**Where:** `os-apps/paw-agent/wasm/sandbox_provisioner/src/lib.rs`, `os-apps/paw-agent/wasm/wasm-helpers/src/sandbox.rs`; source taken from the preserved package parent of `1fe41a7a5efd4b71bb72c349cab94b28b211f4ff`; PR #526.

## D55 — Deepen unsupported futures with bounded read-only research

**Decision:** Select research and uncertain recommendations alongside repairs for one bounded deepening round, using only the existing Session web-search and web-fetch tools on live worlds.

**Came up because:** The completed 81-world run recommended research rather than causal repair, and the repair-only selector therefore skipped deepening entirely. The user asked why all gaps remained and requested actual deeper exploration.

**Options:** Relabel unresolved nodes as resolved; generate cosmetic revisions without research; reuse supported read-only research and retain every unresolved premise.

**Chose bounded research because:** It can add relevant information without inventing evidence or replacing the original assessments. At most two searches, four source reports, eight revisions and eight Session turns are requested. Reports remain explicitly reported/unverified, link to the actual research Session, and are reassessed with revisions. Tool denial or lack of useful sources leaves an explicit research question. Hindcasts retain frozen evidence and never enable web tools. The total Jev call limit stays 320, with exactly one deepening round; each traversal gets its own ten-minute window so research waiting cannot consume the follow-up assessment window. The original run start time remains unchanged for elapsed-time display.

**Where:** `os-apps/paw-foresight/wasm/semantic_core.rs`, `semantic_step`, `semantic_reasoning`, `semantic_prepare`, `semantic_expand`, and `specs/semantic_run.ioa.toml`; red/green selection regression and source-report boundary tests; PR #526.

## D56 — Present a validated subjective outcome distribution

**Decision:** Persist a compact structured outlook with three to five mutually exclusive outcome buckets, including residual other, and explicitly subjective uncalibrated probability weights.

**Came up because:** The user asked for a clear final few outcomes and probability distributions; the former long Markdown answer and 81-cell causal-gap matrix did not answer that question directly.

**Options:** Reuse Jev classifier probabilities as forecasts; imply measured calibration; keep probability-free prose; produce a separate model-estimated distribution with explicit uncertainty and membership validation.

**Chose the separate distribution because:** Causal-gap classifications do not estimate outcome likelihood. The new `foresight-outlook-v1` answer labels its numbers `subjective_model_estimate` and `calibrated:false`, requires finite weights summing to one, assigns every modeled scenario exactly once to a substantive bucket, and reserves an explicit other bucket for futures outside that model. Text bounds support a readable overview. Evidence limitations and unanswered research questions remain visible. Partition membership is mechanically checked; the model's causal judgment and bucket definitions remain subjective and are not a calibration claim.

**Where:** `os-apps/paw-foresight/wasm/semantic_outlook.rs`, `semantic_expand`, `semantic_reasoning`; validation rejects overlapping/missing/invented memberships, invalid weights, false calibration claims and oversized display text; PR #526.

## D57 — Open hypothesis exploration with independent event estimates

**Decision:** Supersede D52's depth-eight exclusion, D53's selected-parent restriction, D55's single research round, and D56's exhaustive outcome partition for new runs. Research follows the question and preserves observations, contested reports, weak signals and hypotheses. Reasoning generates a variable collection of explicit event hypotheses, explores further after evaluation, and may change framing or research beyond the existing frontier. No fixed axes, Cartesian product, topic checklist or adoption/delay/failure poles remain.

**Why:** The previous verified run executed correctly but searched a narrow prescribed space. Twelve mostly policy/platform milestones became four predetermined axes and 81 combinations; synthesis grouped those combinations by one compliance axis. Execution verification did not establish research breadth or useful exploration.

**Evaluation:** Jev Choice diagnoses a causal gap and recommends further work; Noul estimates the explicitly stated event; Score evaluates comparative novelty and decision value. These are model judgments, not empirical calibration. Hypothesis generation and source retrieval belong to reasoning Sessions. Jev does not generate novel hypothesis text. Each final v2 outcome names an evaluated hypothesis, and the engine copies its actual Noul value. Futures may overlap; their probabilities are not normalized. Historical v1 answers remain readable.

**Bounds:** Capacity is 5,000 actual provider calls, 2,048 nodes, 64 rounds and one hour after semantic preparation. Generation batches are limited to 128 nodes for memory, not as a target. Eight sequential calls per callback preserve dependencies while reducing cumulative writes. Compact traces retain actual responses, request hashes and references into the append-only snapshot, including assessments as used. Provider, trace, time and call limits preserve partial progress and expose the stop reason; they must never be labelled convergence. These bounds protect execution and do not prescribe scenario content.

**Verification:** Tests distinguish repeated open rounds from one-shot deepening, reject invented references, preserve prior nodes and assessments, attach actual event probabilities, accept overlapping outcomes without unit probability mass, and plan 5,000 evaluations. Actual-WASM and deployed-flow evidence are recorded separately; passing structural tests alone does not establish novel or well-supported live output.

**Runtime capacity finding:** An actual-WASM fixture with 5,000 trace entries (8.4 MiB) exhausted default fuel and memory with five transient event copies. With 10 billion fuel and one transient event, the same guest completed at 256 MiB. The dedicated service uses `TEMPER_RECENT_EVENTS_BUDGET=1`; durable events remain persisted. The fixture performs 16 mocked HTTP calls around the initial and final batches, not 5,000 live provider calls.

**Input boundaries:** Live research receives an explicit as-of date distinct from its forecast horizon; hindcasts keep the frozen corpus vantage. Session polling projects only Status/result/error_message through existing OData support, avoiding rejection of a valid result because the Session also contains a multi-megabyte research prompt. An actual-WASM 3 MiB Session fixture fails with the full-row reader and passes with projection.

**Live research correction:** The first open-generation session returned eight hypotheses spanning maintenance, distribution, architecture, apprenticeship, domain experts and supply-chain exposure, but several large-page fetches failed. The existing web-fetch API has only a URL argument and reads through a 4 MiB SDK buffer before extraction. The prompt now permits the supported bounded Exa source-text fallback, labels indexed-excerpt/context/date limitations, and never treats titles or bare URLs as retrieved evidence. Initial research entries explicitly marked hypothesis retain that type in the semantic graph, so they can be evaluated and serve as revision parents. A verification-only manual steering request exposed a missing optional steering_checker module; normal production requests now carry the research date directly and do not require that optional path. Session failures preserve their actual error field.
