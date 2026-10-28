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
fn local_worker_can_review_and_evaluate_repo_sweep_runs() {
    let root = repo_root();
    let worker_src = read_worker_sources(&root);

    for needle in [
        "ReviewRun.Claim",
        "ReviewDecisionAction::Approve",
        "EvaluationRun.Start",
        "EvaluationRun.Pass",
        "handle_requested_review_run",
        "handle_queued_evaluation_run",
        "claim_boot_requested_review_runs",
        "claim_boot_queued_evaluation_runs",
        "worker_run_is_repo_sweep",
        "run_codex_review",
        "parse_codex_review_verdict",
        "run_code_change_evaluation",
        "EvaluationRun.Claim",
        "PAW_CODEX_EVAL_COMMANDS",
    ] {
        assert!(
            worker_src.contains(needle),
            "local worker should include repo-sweep review/evaluation support: {needle}"
        );
    }
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
fn ci_covers_paw_patrol_worker_and_wasm_gates() {
    let root = repo_root();
    let ci = read(root.join(".github/workflows/ci.yml"));

    for needle in [
        "workflow_dispatch:",
        "cargo clippy --locked -p temperpaw -p paw-codex-worker --all-targets -- -D warnings",
        "cargo check --locked -p temperpaw -p paw-codex-worker",
        "for f in crates/paw-codex-worker/scripts/*.sh; do bash -n \"$f\"; done",
        "os-apps/paw-ingest/wasm/build.sh",
        "os-apps/paw-patrol/wasm/build.sh",
        "cargo test --locked -p paw-codex-worker --quiet",
        "cargo test --locked -p temperpaw --quiet",
        "cargo test --manifest-path os-apps/paw-patrol/wasm/review_gate_lifecycle/Cargo.toml --quiet",
    ] {
        assert!(
            ci.contains(needle),
            "CI should keep the Patrol worker/wasm surface covered: {needle}"
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
