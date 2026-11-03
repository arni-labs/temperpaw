use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use temper_authz::{AuthzEngine, SecurityContext};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

fn worker_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root.join("crates/paw-codex-worker/src"))
        .expect("read paw-codex-worker src")
        .map(|entry| entry.expect("worker source entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn read_worker_sources(root: &Path) -> String {
    worker_source_files(root)
        .into_iter()
        .map(read)
        .collect::<Vec<_>>()
        .join("\n")
}

fn paw_patrol_wasm_source(root: &Path, module: &str) -> String {
    read(root.join(format!("os-apps/paw-patrol/wasm/{module}/src/lib.rs")))
}

/// The `[[action]]` table named `name` in a spec's TOML text.
fn spec_action(spec: &str, name: &str) -> toml::value::Table {
    let parsed: toml::Value = spec.parse().expect("spec should parse as TOML");
    parsed
        .get("action")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
        .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("spec should define action {name}"))
}

/// The action's effect statements, e.g. `["run_count += 1"]`.
fn effect_statements(action: &toml::value::Table) -> Vec<String> {
    action
        .get("effect")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// Names of the `[[action.triggers]]` the action fires, in declaration order.
fn trigger_names(action: &toml::value::Table) -> Vec<String> {
    action
        .get("triggers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

/// Asserts `action` has exactly these effect statements and fires exactly
/// these triggers, in order.
fn assert_action_effects(
    spec: &str,
    label: &str,
    action: &str,
    effects: &[&str],
    triggers: &[&str],
) {
    let table = spec_action(spec, action);
    assert_eq!(
        effect_statements(&table),
        effects,
        "{label}.{action} should have effects {effects:?}"
    );
    assert_eq!(
        trigger_names(&table),
        triggers,
        "{label}.{action} should fire triggers {triggers:?}"
    );
}

fn agent_context(id: &str, agent_type: &str) -> SecurityContext {
    SecurityContext::from_resolved_identity(id, agent_type, None)
}

fn resource_attrs(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

#[test]
fn patrol_cedar_human_gate_approvals_are_not_available_to_system_agents() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-patrol/policies/patrol.cedar"));
    let engine = AuthzEngine::new(&policy).expect("patrol.cedar should parse");
    let attrs = resource_attrs(&[
        ("id", serde_json::json!("wc-approval")),
        ("risk_lane", serde_json::json!("L3")),
    ]);

    let system_agent = agent_context("patrol-wasm-system", "system");
    for action in ["ApproveHumanStart", "ApproveHumanCompletion"] {
        let decision = engine.authorize(&system_agent, action, "WorkCycle", &attrs);
        assert!(
            !decision.is_allowed(),
            "system agent must not be able to dispatch human gate {action}: {decision:?}"
        );
    }

    let human = agent_context("sesh", "human");
    for action in ["ApproveHumanStart", "ApproveHumanCompletion"] {
        let decision = engine.authorize(&human, action, "WorkCycle", &attrs);
        assert!(
            decision.is_allowed(),
            "human agent should be able to dispatch human gate {action}: {decision:?}"
        );
    }
}

#[test]
fn patrol_cedar_binds_evaluation_pass_fail_to_claimed_evaluator() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-patrol/policies/patrol.cedar"));
    let engine = AuthzEngine::new(&policy).expect("patrol.cedar should parse");
    let attrs = resource_attrs(&[
        ("id", serde_json::json!("eval-1")),
        ("evaluator_id", serde_json::json!("mac-mini-codex-prod")),
    ]);

    let unrelated_agent = agent_context("generic-agent", "agent");
    for action in ["Start", "Pass", "Fail"] {
        let decision = engine.authorize(&unrelated_agent, action, "EvaluationRun", &attrs);
        assert!(
            !decision.is_allowed(),
            "unclaimed generic agent must not be able to dispatch EvaluationRun.{action}: {decision:?}"
        );
    }

    let wrong_worker = agent_context("other-worker", "worker");
    for action in ["Start", "Pass", "Fail"] {
        let decision = engine.authorize(&wrong_worker, action, "EvaluationRun", &attrs);
        assert!(
            !decision.is_allowed(),
            "wrong worker must not be able to dispatch EvaluationRun.{action}: {decision:?}"
        );
    }

    let claimed_worker = agent_context("mac-mini-codex-prod", "worker");
    for action in ["Start", "Pass", "Fail"] {
        let decision = engine.authorize(&claimed_worker, action, "EvaluationRun", &attrs);
        assert!(
            decision.is_allowed(),
            "claimed evaluator should be able to dispatch EvaluationRun.{action}: {decision:?}"
        );
    }
}

#[test]
fn production_docker_image_builds_patrol_wasm_modules() {
    let root = repo_root();
    let dockerfile = read(root.join("Dockerfile"));

    for needle in [
        "os-apps/paw-ingest/wasm && bash build.sh",
        "os-apps/paw-managed-agents/wasm && bash build.sh",
        "os-apps/paw-patrol/wasm && bash build.sh",
    ] {
        assert!(
            dockerfile.contains(needle),
            "production Docker image should build required app WASM modules: {needle}"
        );
    }
}

#[test]
fn paw_patrol_is_discoverable_by_the_os_app_catalog() {
    let root = repo_root();
    temper_platform::os_apps::set_os_apps_dir(root.join("os-apps"));

    let startup_apps = temper_platform::os_apps::list_startup_os_apps();
    assert!(
        startup_apps.iter().any(|app| app == "paw-patrol"),
        "paw-patrol should be part of the startup OS app surface: {startup_apps:?}"
    );

    let bundle = temper_platform::os_apps::get_os_app("paw-patrol")
        .expect("paw-patrol should load as an OS app bundle");
    assert_eq!(
        bundle.specs.len(),
        28,
        "paw-patrol should expose all Patrol entity specs (incl. Ask, ReleaseRun, DsfDeploy, TemperDeploy, ContractProbe, StandingDecision/ShadowVerdict, and the Intent/Effort loop)"
    );
    for entity in [
        "Intent",
        "Effort",
        "Ask",
        "DsfDeploy",
        "TemperDeploy",
        "ContractProbe",
        "ReleaseRun",
    ] {
        assert!(
            bundle
                .specs
                .iter()
                .any(|(entity_type, _)| entity_type == entity),
            "paw-patrol bundle should include the {entity} spec: {:?}",
            bundle.specs.iter().map(|(t, _)| t).collect::<Vec<_>>()
        );
    }
    assert!(
        bundle
            .csdl
            .as_deref()
            .unwrap_or_default()
            .contains("TemperPaw.Patrol"),
        "paw-patrol bundle should include the Patrol CSDL namespace"
    );
    assert!(
        !bundle.cedar_policies.is_empty(),
        "paw-patrol should include Cedar policies"
    );
    assert!(
        bundle.seed_instances.len() >= 10,
        "paw-patrol should seed RiskRule floors and Patrol webhook intake routes"
    );
}

#[test]
fn paw_agent_is_discoverable_by_the_os_app_catalog() {
    let root = repo_root();
    temper_platform::os_apps::set_os_apps_dir(root.join("os-apps"));

    let startup_apps = temper_platform::os_apps::list_startup_os_apps();
    assert!(
        startup_apps.iter().any(|app| app == "paw-agent"),
        "paw-agent should be part of the startup OS app surface: {startup_apps:?}"
    );

    let specs_dir = root.join("os-apps/paw-agent/specs");
    let mut parse_errors = Vec::new();
    for entry in
        std::fs::read_dir(&specs_dir).expect("paw-agent specs directory should be readable")
    {
        let path = entry
            .expect("paw-agent spec entry should be readable")
            .path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("paw-agent IOA spec should be readable");
        if let Err(error) = temper_jit::TransitionTable::try_from_ioa_source(&source) {
            parse_errors.push(format!("{}: {error}", path.display()));
        }
    }
    assert!(
        parse_errors.is_empty(),
        "paw-agent IOA specs should parse individually:\n{}",
        parse_errors.join("\n")
    );

    let bundle = temper_platform::os_apps::get_os_app("paw-agent")
        .expect("paw-agent should load as an OS app bundle");
    assert_eq!(
        bundle.specs.len(),
        13,
        "paw-agent should expose all active Agent entity specs"
    );
    assert!(
        bundle
            .csdl
            .as_deref()
            .unwrap_or_default()
            .contains("TemperPaw"),
        "paw-agent bundle should include the TemperPaw CSDL namespace"
    );
    assert!(
        bundle
            .specs
            .iter()
            .any(|(entity_type, _)| entity_type == "Session"),
        "paw-agent bundle should include the Session spec"
    );
}

#[test]
fn patrol_work_cycles_have_revisable_codex_plan_mode_plans() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let work_cycle = read(patrol.join("specs/work_cycle.ioa.toml"));
    for needle in [
        "name = \"plan_revision_count\"",
        "name = \"RevisePlan\"",
        "from = [\"Planned\", \"AwaitingHumanStartApproval\", \"InProgress\"]",
        "effect = [",
    ] {
        assert!(
            work_cycle.contains(needle),
            "WorkCycle spec should support revising visible plans before and during implementation: {needle}"
        );
    }
    assert!(
        effect_statements(&spec_action(&work_cycle, "RevisePlan"))
            .contains(&"plan_revision_count += 1".to_string()),
        "WorkCycle spec should support revising visible plans before and during implementation: RevisePlan increments plan_revision_count"
    );

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    assert!(
        csdl.contains("<Property Name=\"PlanRevisionCount\" Type=\"Edm.Int32\"/>"),
        "WorkCycle CSDL should expose the plan revision counter"
    );

    let policy = read(patrol.join("policies/patrol.cedar"));
    assert!(
        policy.contains("Action::\"RevisePlan\""),
        "patrol.cedar should authorize Patrol plan revision actions"
    );

    for module in [
        "patrol_request_router",
        "signal_router",
        "finding_lifecycle",
        "patrol_run_lifecycle",
        "repo_sweep_lifecycle",
        "daily_brief_lifecycle",
    ] {
        let source = paw_patrol_wasm_source(&root, module);
        for needle in [
            "Codex Plan Mode",
            "## Context",
            "## Approach",
            "## Verification Plan",
            "## Risks",
        ] {
            assert!(
                source.contains(needle),
                "{module} should write structured WorkCycle plans with plan-mode sections: {needle}"
            );
        }
    }

    let worker_sources = read_worker_sources(&root);
    for needle in [
        "run_codex_plan_mode",
        "codex_plan_args",
        "--sandbox",
        "read-only",
        "RevisePlan",
        "<active_workcycle_plan>",
    ] {
        assert!(
            worker_sources.contains(needle),
            "paw-codex-worker should run a read-only Codex planning pass and attach the plan before implementation: {needle}"
        );
    }
}

#[test]
fn work_cycle_completion_requires_recorded_live_e2e_evidence() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");
    let work_cycle = read(patrol.join("specs/work_cycle.ioa.toml"));

    for needle in [
        "name = \"ReportE2e\"",
        "name = \"e2e_summary\"",
        "from = [\"Reviewing\"]",
        "to = \"Reviewing\"",
        "params = [\"e2e_summary\"]",
        "Complete only when review, evaluation, proof, and live/E2E evidence are all attached.",
    ] {
        assert!(
            work_cycle.contains(needle),
            "WorkCycle should require explicit live/E2E evidence before completion: {needle}"
        );
    }
    assert_action_effects(
        &work_cycle,
        "WorkCycle",
        "ReportE2e",
        &["e2e_ok = true"],
        &[],
    );
    let complete_guard = spec_action(&work_cycle, "Complete")
        .get("guard")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .expect("WorkCycle.Complete should declare a guard expression");
    assert!(
        complete_guard
            .split("&&")
            .any(|term| term.trim() == "e2e_ok"),
        "WorkCycle should require explicit live/E2E evidence before completion: Complete guard `{complete_guard}` must require e2e_ok"
    );

    let review_gate = read(patrol.join("wasm/review_gate_lifecycle/src/lib.rs"));
    for needle in [
        "TemperPaw.Patrol.ReportE2e",
        "record_e2e_if_present",
        "e2e_summary",
        "EvaluationRun",
        "live_e2e_summary",
    ] {
        assert!(
            review_gate.contains(needle),
            "review_gate_lifecycle should record E2E evidence before final gates: {needle}"
        );
    }

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    assert!(
        csdl.contains("<Property Name=\"E2eSummary\" Type=\"Edm.String\"/>"),
        "WorkCycle CSDL should expose durable live/E2E evidence"
    );
}

#[test]
fn review_request_changes_requeues_a_revision_worker_run() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let work_cycle = read(patrol.join("specs/work_cycle.ioa.toml"));
    for needle in [
        "name = \"RequestChanges\"",
        "from = [\"Reviewing\"]",
        "to = \"InProgress\"",
        "name = \"work_cycle_changes_requested\"",
        "module = \"work_cycle_lifecycle\"",
        "effect = [",
    ] {
        assert!(
            work_cycle.contains(needle),
            "WorkCycle.RequestChanges should reset gates and trigger rework: {needle}"
        );
    }
    let request_changes = spec_action(&work_cycle, "RequestChanges");
    let request_changes_effects = effect_statements(&request_changes);
    for statement in [
        "review_passed = false",
        "worker_done = false",
        "evaluation_passed = false",
    ] {
        assert!(
            request_changes_effects.contains(&statement.to_string()),
            "WorkCycle.RequestChanges should reset gates and trigger rework: {statement}"
        );
    }
    assert!(
        trigger_names(&request_changes).contains(&"work_cycle_changes_requested".to_string()),
        "WorkCycle.RequestChanges should reset gates and trigger rework: work_cycle_changes_requested"
    );

    let lifecycle = read(patrol.join("wasm/work_cycle_lifecycle/src/lib.rs"));
    for needle in [
        "\"RequestChanges\" => handle_review_changes_requested",
        "reviewer-requested rework",
        "revision_worker_task",
        "previous_worker_assignment",
        "reuse the existing assigned git worktree and branch",
        "create_entity(ctx, base_url, headers, WORKER_RUNS_PATH)",
        "TemperPaw.Patrol.AttachWorkerRun",
        "required_capabilities_for_task(&task)",
        "work_cycle_lifecycle: queued reviewer-requested rework",
    ] {
        assert!(
            lifecycle.contains(needle),
            "work_cycle_lifecycle should queue a revision WorkerRun after review feedback: {needle}"
        );
    }
    assert!(
        !lifecycle.contains("codex/paw-rework-"),
        "review-requested rework should update the existing branch/PR instead of creating a side PR"
    );
}

#[test]
fn accepted_findings_queue_cleanup_work_cycles() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let quality_spec = read(patrol.join("specs/quality_finding.ioa.toml"));
    let security_spec = read(patrol.join("specs/security_finding.ioa.toml"));
    for (label, spec) in [
        ("QualityFinding", quality_spec.as_str()),
        ("SecurityFinding", security_spec.as_str()),
    ] {
        let accept = spec_action(spec, "Accept");
        assert!(
            effect_statements(&accept).is_empty() && !trigger_names(&accept).is_empty(),
            "{label} should trigger cleanup work on Accept: Accept fires a trigger"
        );
        for needle in [
            "module = \"finding_lifecycle\"",
            "name = \"Accept\"",
            "name = \"LinkPmIssue\"",
            "name = \"StartWork\"",
            "name = \"fingerprint\"",
            "name = \"repo_graph_snapshot_id\"",
        ] {
            assert!(
                spec.contains(needle),
                "{label} should trigger cleanup work on Accept: {needle}"
            );
        }
    }

    let manifest = read(patrol.join("app.toml"));
    assert!(
        manifest.contains("name = \"finding_lifecycle\""),
        "paw-patrol should install finding_lifecycle"
    );

    let lifecycle = read(patrol.join("wasm/finding_lifecycle/src/lib.rs"));
    for needle in [
        "/tdata/Issues",
        "/tdata/WorkCycles",
        "/tdata/WorkerRuns",
        "QualityFinding",
        "SecurityFinding",
        "TemperPaw.PM.SetDescription",
        "TemperPaw.PM.MoveToTriage",
        "TemperPaw.Patrol.LinkPmIssue",
        "TemperPaw.Patrol.StartWork",
        "TemperPaw.Patrol.RequestHumanStartApproval",
        "cleanup WorkCycle for accepted finding",
        "requires_human_start_approval",
        "local_codex",
    ] {
        assert!(
            lifecycle.contains(needle),
            "finding_lifecycle should turn accepted findings into actionable work: {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "finding_lifecycle",
        "Action::\"Accept\"",
        "Action::\"LinkPmIssue\"",
        "Action::\"StartWork\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize finding cleanup actions: {needle}"
        );
    }
}

#[test]
fn local_codex_worker_prompts_leave_worker_run_reporting_to_daemon() {
    let root = repo_root();
    for module in [
        "patrol_request_router",
        "signal_router",
        "finding_lifecycle",
        "work_cycle_lifecycle",
        "repo_sweep_lifecycle",
    ] {
        let source = paw_patrol_wasm_source(&root, module);
        assert!(
            !source.contains("Self-report WorkerRun.ReportDone")
                && !source.contains("self-report WorkerRun.ReportDone")
                && !source.contains("self-report WorkerRun.ReportFailed"),
            "{module} prompt must not ask inner Codex to self-report WorkerRun actions"
        );
        assert!(
            source.contains(
                "paw-codex-worker will report WorkerRun.ReportDone or WorkerRun.ReportFailed"
            ),
            "{module} prompt should make the wrapper-owned reporting contract explicit"
        );
    }
}

// --- Stage 3 S0: record entities + shadow ingest (ARN-430) ---

// --- ARN-434: narrow Cedar permits for the shadow-sweep writes ---

#[test]
fn verified_operator_may_write_shadow_records_narrowly() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-patrol/policies/patrol.cedar"));
    let engine = AuthzEngine::new(&policy).expect("patrol.cedar should parse");
    let attrs = resource_attrs(&[("id", serde_json::json!("x"))]);
    // The sweep's key resolves to the verified operator (agent_type "operator",
    // agentTypeVerified == true - byte-identical to the prod operator credential).
    let operator = agent_context("operator", "operator");

    // ALLOWED: exactly the S1-denied action x resource pairs.
    for (action, resource) in [
        ("Ingest", "ReviewRun"),
        ("IngestRecord", "ReviewRun"),
        ("Ingest", "ProofPacket"),
        ("IngestProof", "ProofPacket"),
        ("Record", "ShadowVerdict"),
        ("MarkAgree", "ShadowVerdict"),
        ("MarkDisagree", "ShadowVerdict"),
    ] {
        let d = engine.authorize(&operator, action, resource, &attrs);
        assert!(
            d.is_allowed(),
            "verified operator should be allowed {resource}.{action}: {d:?}"
        );
    }

    // SCOPED OUT: a non-operator agent gets none of it.
    let worker = agent_context("some-worker", "worker");
    assert!(
        !engine
            .authorize(&worker, "Ingest", "ReviewRun", &attrs)
            .is_allowed(),
        "a non-operator agent must not Ingest ReviewRun"
    );

    // SCOPED OUT: the operator permit covers ONLY the listed actions/resources -
    // including a nonexistent action on a granted resource (ShadowVerdict.Ingest,
    // which the intent explicitly disclaims).
    for (action, resource) in [
        ("Supersede", "ReviewRun"),        // real action, not granted
        ("SupersedeProof", "ProofPacket"), // real action, not granted
        ("Record", "ReviewRun"),           // Record is ShadowVerdict-only here
        ("Ingest", "ShadowVerdict"),       // Ingest is not a ShadowVerdict action
    ] {
        let d = engine.authorize(&operator, action, resource, &attrs);
        assert!(
            !d.is_allowed(),
            "operator permit must NOT cover {resource}.{action}: {d:?}"
        );
    }

    // SCOPED OUT: a SELF-DECLARED operator (agent_type == "operator" but
    // agentTypeVerified == false - the header path, not a resolved credential)
    // is denied. This is the exact attack the verified flag exists for.
    let self_declared = temper_authz::SecurityContext::anonymous().with_agent_context(
        Some("operator"),
        None,
        Some("operator"),
    );
    for (action, resource) in [
        ("Ingest", "ReviewRun"),
        ("IngestRecord", "ReviewRun"),
        ("Ingest", "ProofPacket"),
        ("IngestProof", "ProofPacket"),
        ("Record", "ShadowVerdict"),
        ("MarkAgree", "ShadowVerdict"),
        ("MarkDisagree", "ShadowVerdict"),
    ] {
        let d = engine.authorize(&self_declared, action, resource, &attrs);
        assert!(
            !d.is_allowed(),
            "a self-declared (unverified) operator must NOT write {resource}.{action}: {d:?}"
        );
    }
}

#[test]
fn effort_records_are_writable_without_authorizing_deployment() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-patrol/policies/patrol.cedar"));
    let engine = AuthzEngine::new(&policy).expect("patrol.cedar should parse");
    let agent = agent_context("cursor", "agent");

    for action in [
        "create",
        "Seed",
        "Update",
        "AttachPullRequest",
        "RecordEvidence",
        "Complete",
        "Cancel",
        "Reopen",
    ] {
        assert!(
            engine
                .authorize(&agent, action, "Effort", &HashMap::new())
                .is_allowed(),
            "{action}"
        );
    }
    let l2 = resource_attrs(&[("risk_lane", serde_json::json!("L2"))]);
    assert!(
        !engine
            .authorize(&agent, "Merge", "Effort", &l2)
            .is_allowed()
    );

    let deploy_attrs = resource_attrs(&[("id", serde_json::json!("d1"))]);
    assert!(
        engine
            .authorize(&agent, "create", "DsfDeploy", &deploy_attrs)
            .is_allowed(),
        "implementer must be able to create DsfDeploy"
    );
    assert!(
        engine
            .authorize(&agent, "create", "TemperDeploy", &deploy_attrs)
            .is_allowed(),
        "implementer must be able to create TemperDeploy"
    );
    assert!(
        !engine
            .authorize(&agent, "MarkDeployVerified", "Effort", &l2)
            .is_allowed(),
        "agents must not mark deploy verified by hand"
    );

    for action in [
        "Request",
        "MergeSucceeded",
        "Check",
        "CheckPending",
        "CheckHealthy",
        "CheckUnhealthy",
        "Fail",
        "RollbackPushed",
    ] {
        assert!(
            engine
                .authorize(&agent, action, "DsfDeploy", &deploy_attrs)
                .is_allowed(),
            "implementer Agent must complete DsfDeploy.{action}: {:?}",
            engine.authorize(&agent, action, "DsfDeploy", &deploy_attrs)
        );
    }
    assert!(
        engine
            .authorize(&agent, "SwapSucceeded", "TemperDeploy", &deploy_attrs)
            .is_allowed(),
        "implementer Agent must complete TemperDeploy.SwapSucceeded"
    );
    let wasm_runtime = agent_context("service:wasm-runtime", "wasm-runtime");
    assert!(
        engine
            .authorize(&wasm_runtime, "MergeSucceeded", "DsfDeploy", &deploy_attrs)
            .is_allowed(),
        "background WASM callback principal must land MergeSucceeded"
    );
    let timeout = agent_context("service:timeout-scheduler", "timeout-scheduler");
    assert!(
        engine
            .authorize(&timeout, "Check", "DsfDeploy", &deploy_attrs)
            .is_allowed(),
        "timer principal must fire DsfDeploy.Check"
    );
    assert!(
        !engine
            .authorize(&agent, "MergeSucceeded", "ReleaseRun", &deploy_attrs)
            .is_allowed(),
        "ordinary Agent must not drive ReleaseRun callbacks"
    );
}
