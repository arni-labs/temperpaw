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
fn paw_patrol_owns_the_dark_factory_entities_without_extra_factory_apps() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    assert!(patrol.is_dir(), "paw-patrol app should exist");
    assert!(
        !root.join("os-apps/paw-factory").exists(),
        "factory should be a paw-patrol capability, not a separate app"
    );
    assert!(
        !root.join("os-apps/paw-quality").exists(),
        "quality should be a paw-patrol capability, not a separate app"
    );

    let manifest = read(patrol.join("app.toml"));
    assert!(manifest.contains("name = \"paw-patrol\""));
    assert!(manifest.contains("paw-ingest"));
    assert!(manifest.contains("paw-pm"));
    assert!(manifest.contains("paw-agent"));

    let app_doc = read(patrol.join("APP.md"));
    for needle in [
        "PatrolRequest",
        "Signal",
        "FactoryCase",
        "WorkCycle",
        "WorkerRun",
        "ReviewRun",
        "EvaluationRun",
        "ProofPacket",
        "RiskRule",
        "RepoGraphSnapshot",
        "QualityFinding",
        "SecurityFinding",
        "DailyBrief",
        "PatrolSchedule",
        "paw-pm Issues",
        "Mac mini",
        "resource-bound ownership",
        "DsfDeploy",
        "TemperDeploy",
    ] {
        assert!(app_doc.contains(needle), "APP.md should mention {needle}");
    }

    for spec in [
        "patrol_request.ioa.toml",
        "signal.ioa.toml",
        "factory_case.ioa.toml",
        "work_cycle.ioa.toml",
        "intent.ioa.toml",
        "effort.ioa.toml",
        "dsf_deploy.ioa.toml",
        "temper_deploy.ioa.toml",
        "worker_run.ioa.toml",
        "review_run.ioa.toml",
        "evaluation_run.ioa.toml",
        "proof_packet.ioa.toml",
        "risk_rule.ioa.toml",
        "repo_graph_snapshot.ioa.toml",
        "quality_finding.ioa.toml",
        "security_finding.ioa.toml",
        "daily_brief.ioa.toml",
        "patrol_schedule.ioa.toml",
    ] {
        assert!(
            patrol.join("specs").join(spec).is_file(),
            "paw-patrol should define specs/{spec}"
        );
    }

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    for entity in [
        "PatrolRequest",
        "Signal",
        "FactoryCase",
        "WorkCycle",
        "Intent",
        "Effort",
        "DsfDeploy",
        "TemperDeploy",
        "WorkerRun",
        "ReviewRun",
        "EvaluationRun",
        "ProofPacket",
        "RiskRule",
        "RepoGraphSnapshot",
        "QualityFinding",
        "SecurityFinding",
        "DailyBrief",
        "PatrolSchedule",
    ] {
        assert!(
            csdl.contains(&format!("<EntityType Name=\"{entity}\">")),
            "CSDL should expose {entity}"
        );
        assert!(
            csdl.contains(&format!("<EntitySet Name=\"{entity}s\"")),
            "CSDL should expose the {entity}s entity set"
        );
    }
}

#[test]
fn paw_patrol_renames_human_intake_to_work_request_and_adds_risk_patrol_entities() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    for spec in [
        "work_request.ioa.toml",
        "patrol_run.ioa.toml",
        "observability_finding.ioa.toml",
        "worker_provider.ioa.toml",
        "worker_agent.ioa.toml",
    ] {
        assert!(
            patrol.join("specs").join(spec).is_file(),
            "paw-patrol should define specs/{spec}"
        );
    }

    let work_request = read(patrol.join("specs/work_request.ioa.toml"));
    for needle in [
        "name = \"WorkRequest\"",
        "hint = \"Submit human or manager-agent intent into Patrol as work.\"",
        "module = \"patrol_request_router\"",
        "request_text",
        "requester_id",
    ] {
        assert!(
            work_request.contains(needle),
            "WorkRequest spec should contain {needle}"
        );
    }
    assert_action_effects(
        &work_request,
        "WorkRequest",
        "Submit",
        &[],
        &["route_work_request"],
    );

    let patrol_run = read(patrol.join("specs/patrol_run.ioa.toml"));
    for needle in [
        "name = \"PatrolRun\"",
        "datadog_observability",
        "name = \"patrol_kind\"",
        "name = \"required_capabilities\"",
        "name = \"worker_run_id\"",
        "name = \"evidence_json\"",
        "name = \"observability_finding_ids\"",
        "name = \"factory_case_ids\"",
        "name = \"Start\"",
        "module = \"patrol_run_lifecycle\"",
        "name = \"RecordEvidence\"",
        "name = \"Complete\"",
        "name = \"Escalate\"",
    ] {
        assert!(
            patrol_run.contains(needle),
            "PatrolRun spec should contain {needle}"
        );
    }

    let observability = read(patrol.join("specs/observability_finding.ioa.toml"));
    for needle in [
        "name = \"ObservabilityFinding\"",
        "source",
        "datadog_monitor_id",
        "evidence_json",
        "risk_lane",
        "patrol_run_id",
        "work_cycle_id",
        "name = \"OpenFinding\"",
        "module = \"finding_lifecycle\"",
    ] {
        assert!(
            observability.contains(needle),
            "ObservabilityFinding spec should contain {needle}"
        );
    }

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    for entity in [
        "WorkRequest",
        "PatrolRun",
        "ObservabilityFinding",
        "WorkerProvider",
        "WorkerAgent",
    ] {
        assert!(
            csdl.contains(&format!("<EntityType Name=\"{entity}\">")),
            "CSDL should expose {entity}"
        );
        assert!(
            csdl.contains(&format!("<EntitySet Name=\"{entity}s\"")),
            "CSDL should expose {entity}s"
        );
    }

    let routes = read(patrol.join("seed-data/webhook_routes.toml"));
    assert!(
        routes.contains("target_entity_type = \"WorkRequest\""),
        "patrol-request webhook should create WorkRequest, not PatrolRequest"
    );

    let app_doc = read(patrol.join("APP.md"));
    for needle in [
        "WorkRequest means human or manager-agent intent",
        "Signal means observed evidence",
        "PatrolRun means active investigation",
        "Risk Patrol",
    ] {
        assert!(app_doc.contains(needle), "APP.md should explain {needle}");
    }
}

#[test]
fn paw_patrol_project_deploy_tools_remain_available_independently() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");
    let dsf = read(patrol.join("specs/dsf_deploy.ioa.toml"));
    let release_run = read(patrol.join("specs/release_run.ioa.toml"));
    let temper = read(patrol.join("specs/temper_deploy.ioa.toml"));
    let cedar = read(patrol.join("policies/patrol.cedar"));
    let csdl = read(patrol.join("specs/model.csdl.xml"));
    let manifest = read(patrol.join("app.toml"));

    for needle in [
        "name = \"DsfDeploy\"",
        "module = \"release_run_lifecycle\"",
        "github_token = \"{secret:github_token}\"",
        "github_app_id = \"{secret:github_app_id}\"",
        "github_app_private_key = \"{secret:github_app_private_key}\"",
        "concurrent_entity_set = \"DsfDeploys\"",
    ] {
        assert!(
            dsf.contains(needle),
            "dsf_deploy.ioa.toml should carry `{needle}`"
        );
    }
    for needle in [
        "github_app_id = \"{secret:github_app_id}\"",
        "github_app_private_key = \"{secret:github_app_private_key}\"",
    ] {
        assert!(
            release_run.contains(needle),
            "release_run.ioa.toml should carry `{needle}`"
        );
    }
    for needle in [
        "name = \"TemperDeploy\"",
        "module = \"temper_deploy_lifecycle\"",
        "WaitingForImage",
        "name = \"CheckImage\"",
        "name = \"ImageReady\"",
        "target_entity = \"ContractProbe\"",
        "github_token = \"{secret:github_token}\"",
    ] {
        assert!(
            temper.contains(needle),
            "temper_deploy.ioa.toml should carry `{needle}`"
        );
    }
    for needle in [
        "resource is DsfDeploy",
        "resource is TemperDeploy",
        "principal.agent_type == \"wasm-runtime\"",
    ] {
        assert!(
            cedar.contains(needle),
            "patrol.cedar should carry `{needle}`"
        );
    }
    for needle in [
        "<EntityType Name=\"DsfDeploy\">",
        "<EntityType Name=\"TemperDeploy\">",
        "<EntityType Name=\"ContractProbe\">",
        "<EntitySet Name=\"DsfDeploys\"",
        "<EntitySet Name=\"TemperDeploys\"",
        "<EntitySet Name=\"ContractProbes\"",
        "<Property Name=\"ArtifactFileReady\"",
    ] {
        assert!(csdl.contains(needle), "CSDL should carry `{needle}`");
    }
    assert!(
        manifest.contains("name = \"temper_deploy_lifecycle\""),
        "app.toml should register temper_deploy_lifecycle"
    );
    assert!(
        read(patrol.join("wasm/build.sh")).contains("temper_deploy_lifecycle"),
        "build.sh should build temper_deploy_lifecycle"
    );
    assert!(
        manifest.contains("name = \"contract_probe\"")
            && read(patrol.join("wasm/build.sh")).contains("contract_probe")
            && read(patrol.join("specs/contract_probe.ioa.toml"))
                .contains("name = \"ContractProbe\"")
            && read(patrol.join("specs/contract_probe.ioa.toml")).contains("state = \"Failed\"")
            && read(patrol.join("seed-data/contract_probes.toml")).contains("mcp-455-lists"),
        "ContractProbe spec, wasm, seed, and app.toml must travel together"
    );
    assert!(
        read(patrol.join("wasm/temper_deploy_lifecycle/src/lib.rs")).contains("ghcr.io/token"),
        "TemperDeploy must exchange a GHCR pull token, not send the GitHub PAT as the registry bearer"
    );
    let deploy = read(patrol.join("specs/temper_deploy.ioa.toml"));
    assert!(
        deploy.contains("ready_path = \"/healthz\""),
        "TemperDeploy must poll /healthz (ARN-432); /readyz is 503 when Discord is down"
    );
    assert!(
        !deploy.contains("ready_path = \"/readyz\""),
        "TemperDeploy must not poll /readyz"
    );
    let app_doc = read(patrol.join("APP.md"));
    assert!(
        !app_doc.contains("Deploy` creates the existing `ReleaseRun"),
        "APP.md must not say Effort.Deploy creates ReleaseRun"
    );
    let merge_src = read(patrol.join("wasm/release_run_lifecycle/src/lib.rs"));
    let app_src = read(patrol.join("wasm/release_run_lifecycle/src/github_app.rs"));
    assert!(
        app_src.contains("fn installation_token") && app_src.contains("github_app_jwt"),
        "release_run_lifecycle must mint a GitHub App installation token"
    );
    assert!(
        merge_src.contains("github_app::github_bearer"),
        "merge/rollback must use the App bearer, not only github_token"
    );
    let wasm = fs::read(patrol.join("wasm/release_run_lifecycle/release_run_lifecycle.wasm"))
        .expect("committed release_run_lifecycle.wasm");
    let wasm_text = String::from_utf8_lossy(&wasm);
    assert!(
        wasm_text.contains("github_app_id") && wasm_text.contains("installation token"),
        "committed wasm must mint an App installation token"
    );
    assert!(
        !wasm_text.contains(".git-credentials"),
        "committed wasm must not read ~/.git-credentials"
    );
}

#[test]
fn paw_patrol_app_doc_documents_agent_submission_api() {
    let root = repo_root();
    let app_doc = read(root.join("os-apps/paw-patrol/APP.md"));

    for needle in [
        "## Agent Submission API",
        "Do not submit new work directly to paw-pm",
        "Human or manager-agent task",
        "WorkRequests('<id>')/TemperPaw.Patrol.Submit",
        "\"request_text\"",
        "Observed evidence or error",
        "Signals('<id>')/TemperPaw.Patrol.Ingest",
        "\"severity\"",
        "Active patrol run",
        "PatrolRuns('<id>')/TemperPaw.Patrol.Configure",
        "PatrolRuns('<id>')/TemperPaw.Patrol.Start",
        "datadog_observability",
        "datadog_query",
        "github_repository",
        "github_query",
        "ReviewRun.RequestChanges",
        "new implementer WorkerRun",
        "same branch/worktree",
        "ReviewRun.Escalate",
        "reviewer output is invalid",
    ] {
        assert!(
            app_doc.contains(needle),
            "APP.md should document agent submission API detail: {needle}"
        );
    }
}

#[test]
fn worker_provider_registry_and_capabilities_gate_datadog_patrol() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let worker_provider = read(patrol.join("specs/worker_provider.ioa.toml"));
    for needle in [
        "name = \"WorkerProvider\"",
        "local_codex",
        "codex_cloud",
        "openclaw",
        "anthropic_managed",
        "temperpaw_agent",
        "capabilities",
        "enabled",
    ] {
        assert!(
            worker_provider.contains(needle),
            "WorkerProvider spec should contain {needle}"
        );
    }

    let worker_agent = read(patrol.join("specs/worker_agent.ioa.toml"));
    for needle in [
        "name = \"WorkerAgent\"",
        "worker_id",
        "provider_id",
        "capabilities",
        "local_codex,repo_write,review,evaluation,datadog_query",
        "ReportHeartbeat",
    ] {
        assert!(
            worker_agent.contains(needle),
            "WorkerAgent spec should contain {needle}"
        );
    }

    let worker_run = read(patrol.join("specs/worker_run.ioa.toml"));
    for needle in [
        "name = \"provider_id\"",
        "name = \"required_capabilities\"",
        "params = [\"work_cycle_id\", \"factory_case_id\", \"risk_lane\", \"task\", \"branch_name\", \"worktree_path\", \"runner_kind\", \"allowed_worker_id\", \"provider_id\", \"required_capabilities\"]",
        "datadog_query",
    ] {
        assert!(
            worker_run.contains(needle),
            "WorkerRun spec should encode provider/capability requirements: {needle}"
        );
    }

    let worker_sources = read_worker_sources(&root);
    for needle in [
        "worker_capabilities",
        "PAW_CODEX_WORKER_CAPABILITIES",
        "required_capabilities",
        "datadog_query",
        "worker_run_required_capabilities_satisfied",
        "WorkerRun requires capabilities this worker does not advertise",
    ] {
        assert!(
            worker_sources.contains(needle),
            "paw-codex-worker should enforce capability gates: {needle}"
        );
    }

    let seed = read(patrol.join("seed-data/worker_providers.toml"));
    for needle in [
        "id = \"local-codex\"",
        "id = \"mac-mini-codex-prod\"",
        "capabilities = \"local_codex,repo_write,review,evaluation,datadog_query,github_query\"",
        "id = \"codex-cloud\"",
        "enabled = false",
    ] {
        assert!(
            seed.contains(needle),
            "worker_providers.toml should seed provider registry: {needle}"
        );
    }

    let bootstrap = read(root.join("crates/paw-codex-worker/scripts/mac-mini-bootstrap.sh"));
    for needle in [
        "railway variables --json",
        "DD_API_KEY",
        "DD_APP_KEY",
        "DD_SITE",
        "PATROL_DATADOG_WEBHOOK_SECRET",
        "chmod 600",
        "PAW_CODEX_WORKER_CAPABILITIES",
        "datadog_query",
        "launchctl",
        "paw-codex-worker doctor",
    ] {
        assert!(
            bootstrap.contains(needle),
            "Mac mini bootstrap script should contain {needle}"
        );
    }
}

#[test]
fn datadog_observability_patrol_run_uses_temper_state_and_creates_work() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"patrol_run_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should install patrol_run_lifecycle: {needle}"
        );
    }

    let lifecycle = read(patrol.join("wasm/patrol_run_lifecycle/src/lib.rs"));
    for needle in [
        "/tdata/WorkerAgents",
        "/tdata/WorkerRuns",
        "/tdata/WorkCycles",
        "datadog_observability",
        "datadog_query",
        "TemperPaw.Patrol.AttachWorkerRun",
        "TemperPaw.Patrol.AttachEvidenceLinks",
        "TemperPaw.Patrol.Escalate",
        "ObservabilityFindings",
        "OpenFinding",
        "ProofPackets",
        "AttachDraft",
        "MarkReady",
        "visual_summary_url",
        "state_diagram_mermaid",
        "FactoryCases",
        "WorkCycles",
        "Signals",
        "create_entity_with_body",
        "\"source\": \"datadog_mcp\"",
        "\"payload\": finding_evidence.to_string()",
        "\"source_url\": string_value(finding, \"source_url\", \"\")",
        "StartWork",
        "local Codex Datadog MCP Patrol",
        "DATADOG_PATROL_RESULT_JSON_BEGIN",
        "monitors, logs, traces, metrics, incidents, and dashboards",
        "required_capabilities",
        "sensitive_followup_surface",
        "\"paw-agent\"",
        "\"paw-channels\"",
        "\"discord\"",
        "\"production\"",
        "\"deploy\"",
    ] {
        assert!(
            lifecycle.contains(needle),
            "patrol_run_lifecycle should queue or escalate Datadog Patrol: {needle}"
        );
    }

    let worker_sources = read_worker_sources(&root);
    for needle in [
        "run_datadog_patrol",
        "investigate_datadog_with_codex",
        "Datadog MCP",
        "DATADOG_PATROL_RESULT_JSON_BEGIN",
        "DATADOG_PATROL_RESULT_JSON_END",
        "codex_datadog_mcp_agent",
        "PatrolRun:",
        "datadog_observability",
        "PatrolRuns",
        "monitors",
        "logs",
        "traces",
        "metrics",
        "incidents",
        "dashboards",
        "TemperPaw.Patrol.RecordEvidence",
    ] {
        assert!(
            worker_sources.contains(needle),
            "paw-codex-worker should run Datadog Patrol and report through Temper: {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "low-risk Datadog follow-up WorkCycles",
        "Action::\"StartWork\"",
        "Action::\"AttachWorkerRun\"",
        "Action::\"AttachEvidenceLinks\"",
        "resource is WorkerRun",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should let the Mac mini worker queue low-risk Datadog follow-up work: {needle}"
        );
    }
}

#[test]
fn github_repository_patrol_uses_agentic_github_tools_and_creates_work() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let lifecycle = read(patrol.join("wasm/patrol_run_lifecycle/src/lib.rs"));
    for needle in [
        "github_repository",
        "github_query",
        "local Codex GitHub Patrol agent",
        "open issues, open pull requests, checks, reviews",
        "GITHUB_PATROL_RESULT_JSON_BEGIN",
        "codex_github_agent",
        "\"source\": \"github_agent\"",
        "github_patrol:agent_investigation",
        "local_codex,repo_write,github_query",
        "GitHub repository Patrol",
        "GitHub issue and PR investigation",
        "risk-gated WorkCycles",
        "requires_human_approval",
    ] {
        assert!(
            lifecycle.contains(needle),
            "patrol_run_lifecycle should keep GitHub Patrol agentic and Temper-native: {needle}"
        );
    }

    let worker_sources = read_worker_sources(&root);
    for needle in [
        "run_github_patrol",
        "GITHUB_PATROL_RESULT_JSON_BEGIN",
        "GITHUB_PATROL_RESULT_JSON_END",
        "open issues",
        "open pull requests",
        "checks",
        "reviews",
        "anomalies",
        "TemperPaw.Patrol.RecordEvidence",
    ] {
        assert!(
            worker_sources.contains(needle),
            "paw-codex-worker should run GitHub Patrol as a Codex/GitHub agent and report evidence: {needle}"
        );
    }

    let schedule = read(patrol.join("wasm/patrol_schedule_lifecycle/src/lib.rs"));
    for needle in [
        "enable_github_patrol",
        "github_repository",
        "required_capabilities\": \"github_query",
        "last_github_patrol_run_id",
    ] {
        assert!(
            schedule.contains(needle),
            "PatrolSchedule should include GitHub issue/PR patrol in recurring runs: {needle}"
        );
    }

    let daily_brief = read(patrol.join("wasm/daily_brief_lifecycle/src/lib.rs"));
    for needle in [
        "PatrolRuns",
        "Signals",
        "GitHub issue/PR patrol",
        "datadog_observability or github_repository",
    ] {
        assert!(
            daily_brief.contains(needle),
            "DailyBrief should roll GitHub Patrol results into the visual report: {needle}"
        );
    }

    let dashboard = read(root.join("dashboard/src/lib/app-views/paw-patrol.ts"));
    for needle in ["Run GitHub Patrol", "github_repository", "github_query"] {
        assert!(
            dashboard.contains(needle),
            "Paw Patrol dashboard should expose GitHub Patrol: {needle}"
        );
    }
}

#[test]
fn dashboard_has_generic_app_console_and_paw_patrol_view_manifest() {
    let root = repo_root();

    for path in [
        "dashboard/src/routes/apps/[name]/+page.svelte",
        "dashboard/src/routes/sessions/+page.svelte",
        "dashboard/src/routes/sessions/[id]/+page.svelte",
        "dashboard/src/routes/entities/[type]/[id]/+page.svelte",
        "dashboard/src/lib/dashboard-format.ts",
        "dashboard/src/lib/components/app-console/EntityBoard.svelte",
        "dashboard/src/lib/components/app-console/RelationTimeline.svelte",
        "dashboard/src/lib/components/app-console/ProofViewer.svelte",
        "dashboard/src/lib/app-views/paw-patrol.ts",
    ] {
        assert!(
            root.join(path).is_file(),
            "dashboard should provide generic app console file {path}"
        );
    }

    let api = read(root.join("dashboard/src/lib/api.ts"));
    for needle in [
        "fetchAppViewManifest",
        "fetchEntityHistory",
        "queryEntities",
        "getEntity",
        "postEntityAction",
        "entityResource",
        "'@odata.actions'",
        "namespace = 'Temper'",
        "'x-temper-principal-kind': 'human'",
        "'x-temper-principal-id': 'dashboard'",
    ] {
        assert!(api.contains(needle), "dashboard API should expose {needle}");
    }

    let layout = read(root.join("dashboard/src/routes/+layout.svelte"));
    assert!(
        layout.contains("appHref('/apps/paw-patrol')") && layout.contains("Patrol"),
        "dashboard navigation should include Paw Patrol"
    );

    let patrol_view = read(root.join("dashboard/src/lib/app-views/paw-patrol.ts"));
    for needle in [
        "WorkRequests",
        "Signals",
        "PatrolRuns",
        "ObservabilityFindings",
        "FactoryCases",
        "WorkCycles",
        "WorkerRuns",
        "ReviewRuns",
        "EvaluationRuns",
        "ProofPackets",
        "PatrolRequests",
        "WorkerProviders",
        "RiskRules",
        "WorkerAgents",
        "Run Datadog Patrol",
        "Run GitHub Patrol",
        "Submit Work",
        "work-request",
    ] {
        assert!(
            patrol_view.contains(needle),
            "Paw Patrol view manifest should contain {needle}"
        );
    }

    let entity_detail = read(root.join("dashboard/src/routes/entities/[type]/[id]/+page.svelte"));
    assert!(
        entity_detail.contains("entitySetParam") && !entity_detail.contains("entityType + 's'"),
        "entity detail routes should treat /entities/<set>/<id> as an entity set, not append another s"
    );

    let entity_board =
        read(root.join("dashboard/src/lib/components/app-console/EntityBoard.svelte"));
    assert!(
        entity_board.contains("readField(row, column)"),
        "dashboard app tables should resolve snake_case Temper fields for readable Patrol columns"
    );

    let app_page = read(root.join("dashboard/src/routes/apps/[name]/+page.svelte"));
    for needle in [
        "newestFirst",
        "lastActivityMs(right) - lastActivityMs(left)",
        "loadErrors",
        "console-ledger",
        "submitWorkRequest",
        "postEntityAction('WorkRequests', id, 'Submit'",
    ] {
        assert!(
            app_page.contains(needle),
            "dashboard app console should keep newest Patrol entities first: {needle}"
        );
    }

    let overview =
        read(root.join("dashboard/src/lib/components/app-console/PatrolOverview.svelte"));
    for needle in [
        "isDatadogMcpProof",
        "codex_datadog_mcp_agent",
        "createdFindingIds",
        "openedFindings",
    ] {
        assert!(
            overview.contains(needle),
            "Paw Patrol dashboard should prefer latest Datadog MCP proof evidence: {needle}"
        );
    }

    let proof_viewer =
        read(root.join("dashboard/src/lib/components/app-console/ProofViewer.svelte"));
    assert!(
        proof_viewer.contains("readyLatest"),
        "Paw Patrol proof viewer should show the newest Ready proof instead of a rejected draft"
    );

    for needle in [
        "visualSummaryUrl",
        "visual-proof",
        "stateDiagram",
        "changedFiles",
    ] {
        assert!(
            proof_viewer.contains(needle),
            "Paw Patrol proof viewer should expose visual proof detail: {needle}"
        );
    }

    for needle in [
        "fetchEntityHistory",
        "entity-summary",
        "history-table",
        "WorkCycles",
        "ProofPackets",
        "Timeline events",
        "No history or entity event rows were returned",
    ] {
        assert!(
            entity_detail.contains(needle),
            "entity detail dashboard should show comprehensive Patrol details: {needle}"
        );
    }

    let sessions = read(root.join("dashboard/src/routes/sessions/+page.svelte"));
    for needle in [
        "Temporal session ledger",
        "queryEntities('Sessions', undefined, 'Id desc', 200)",
        "lastActivityMs(right) - lastActivityMs(left)",
        "Total loaded",
        "Tokens / cost",
        "Relations",
        "entities/Sessions",
    ] {
        assert!(
            sessions.contains(needle),
            "sessions dashboard should show chronological comprehensive rows: {needle}"
        );
    }

    let session_detail = read(root.join("dashboard/src/routes/sessions/[id]/+page.svelte"));
    assert!(
        session_detail.contains("/entities/Sessions/")
            && session_detail.contains("/entities/WorkCycles/"),
        "session detail should link to raw Session and plural WorkCycles entity detail routes"
    );
}

#[test]
fn worker_run_encodes_local_codex_claiming_and_manual_cloud_overflow() {
    let root = repo_root();
    let spec = read(root.join("os-apps/paw-patrol/specs/worker_run.ioa.toml"));

    for needle in [
        "name = \"WorkerRun\"",
        "states = [\"Queued\", \"Claimed\", \"Running\", \"WaitingForCloudApproval\", \"CloudApproved\", \"Done\", \"Failed\", \"TimedOut\"]",
        "name = \"worker_id\"",
        "name = \"runner_kind\"",
        "name = \"work_cycle_id\"",
        "name = \"risk_lane\"",
        "name = \"proof_packet_id\"",
        "name = \"Claim\"",
        "name = \"StartLocal\"",
        "name = \"RequestCloudOverflow\"",
        "name = \"ApproveCloudOverflow\"",
        "name = \"DispatchCloud\"",
        "name = \"ReportDone\"",
        "name = \"ReportFailed\"",
        "on_timeout = \"Timeout\"",
    ] {
        assert!(
            spec.contains(needle),
            "WorkerRun spec should contain {needle}"
        );
    }
}

#[test]
fn paw_patrol_dark_factory_architecture_is_recorded_in_app_adr() {
    let root = repo_root();
    let adr = root
        .join("os-apps/paw-patrol/adrs")
        .join("0001-patrol-controlled-dark-factory.md");

    assert!(
        adr.is_file(),
        "material paw-patrol architecture changes should be recorded in an app-scoped ADR"
    );

    let text = read(adr);
    for needle in [
        "Patrol-Controlled Dark Factory",
        "Status: Accepted",
        "Temper-native",
        "PatrolRequest",
        "Signal",
        "FactoryCase",
        "WorkCycle",
        "WorkerRun",
        "ReviewRun",
        "EvaluationRun",
        "ProofPacket",
        "RiskRule",
        "RepoGraphSnapshot",
        "QualityFinding",
        "SecurityFinding",
        "DailyBrief",
        "paw-pm",
        "Mac mini",
        "Cedar",
        "principal.id == resource.worker_id",
        "principal.id == resource.reviewer_id",
        "human-gated",
    ] {
        assert!(
            text.contains(needle),
            "paw-patrol Dark Factory ADR should contain {needle}"
        );
    }
}

#[test]
fn paw_patrol_evaluation_timeout_classification_is_recorded_in_app_adr() {
    let root = repo_root();
    let adr = root
        .join("os-apps/paw-patrol/adrs")
        .join("0002-evaluation-timeout-classification.md");

    assert!(
        adr.is_file(),
        "EvaluationRun timeout classification should be recorded in an app-scoped ADR"
    );

    let text = read(adr);
    for needle in [
        "Evaluation Timeout Classification",
        "Status: Accepted",
        "EvaluationRun.Fail",
        "failure_classification",
        "evaluator_timeout",
        "evaluation_entity_timeout",
        "Temper-native",
    ] {
        assert!(
            text.contains(needle),
            "paw-patrol evaluation timeout ADR should contain {needle}"
        );
    }
}

#[test]
fn paw_patrol_queued_evaluation_terminal_cleanup_is_recorded_in_app_adr() {
    let root = repo_root();
    let adr = root
        .join("os-apps/paw-patrol/adrs")
        .join("0003-queued-evaluation-terminal-cleanup.md");

    assert!(
        adr.is_file(),
        "queued EvaluationRun terminal cleanup should be recorded in an app-scoped ADR"
    );

    let text = read(adr);
    for needle in [
        "Queued Evaluation Terminal Cleanup",
        "Status: Accepted",
        "ReviewRun",
        "EvaluationRun.Fail",
        "review_terminal_without_approval",
        "parent_work_cycle_terminal",
        "Temper-native",
    ] {
        assert!(
            text.contains(needle),
            "paw-patrol queued evaluation cleanup ADR should contain {needle}"
        );
    }
}

#[test]
fn paw_patrol_docs_explain_worker_scripts_tests_and_wasms() {
    let root = repo_root();
    let app_doc = read(root.join("os-apps/paw-patrol/APP.md"));
    for needle in [
        "## WASM Modules",
        "patrol_request_router",
        "signal_router",
        "repo_sweep_lifecycle",
        "worker_run_lifecycle",
        "review_gate_lifecycle",
        "finding_lifecycle",
        "patrol_schedule_lifecycle",
        "daily_brief_lifecycle",
    ] {
        assert!(app_doc.contains(needle), "APP.md should document {needle}");
    }

    let worker_readme = read(root.join("crates/paw-codex-worker/README.md"));
    for needle in [
        "## Worker Responsibilities",
        "## Scripts Versus Rust Tests",
        "not one-off migration scripts",
        "acceptance and operations harnesses",
        "deterministic-smoke.sh",
        "webhook-intake-smoke.sh",
        "repo-sweep-brief-smoke.sh",
        "production-preflight.sh",
        "production-readiness.sh",
        "production-observe-only.sh",
        "paw-patrol-acceptance.sh",
        "Rust tests",
    ] {
        assert!(
            worker_readme.contains(needle),
            "paw-codex-worker README should explain {needle}"
        );
    }
}

#[test]
fn paw_patrol_docs_explain_schedule_boundary_and_cleanup_status() {
    let root = repo_root();
    let app_doc = read(root.join("os-apps/paw-patrol/APP.md"));
    for needle in [
        "## PatrolSchedule And CronJob",
        "PatrolSchedule intentionally does not reuse the paw-agent CronJob entity",
        "Both entities use Temper's schedule_at timer effect",
        "CronJob is for scheduled agent Session creation",
        "PatrolSchedule is for scheduled Patrol maintenance",
        "RepoGraphSnapshot and DailyBrief",
        "## Quality Cleanup Status",
        "Detection is not cleanup",
        "giant WASM modules remain work to be done",
        "Monty REPL",
        "provider_caller",
        "context_preparer",
        "route_message",
        "QualityFinding",
        "WorkCycle",
    ] {
        assert!(
            app_doc.contains(needle),
            "APP.md should explain schedule boundary and cleanup status: {needle}"
        );
    }

    let adr = read(
        root.join("os-apps/paw-patrol/adrs")
            .join("0001-patrol-controlled-dark-factory.md"),
    );
    for needle in [
        "PatrolSchedule intentionally remains a Patrol entity",
        "CronJob remains the paw-agent scheduled Session entity",
        "share the platform schedule_at effect",
        "RepoGraphSnapshot",
        "DailyBrief",
        "Detection is not the same as cleanup",
    ] {
        assert!(
            adr.contains(needle),
            "Dark Factory ADR should explain schedule boundary and cleanup status: {needle}"
        );
    }
}

#[test]
fn worker_claims_are_bound_to_the_configured_local_worker() {
    let root = repo_root();
    let spec = read(root.join("os-apps/paw-patrol/specs/worker_run.ioa.toml"));
    let csdl = read(root.join("os-apps/paw-patrol/specs/model.csdl.xml"));
    let policy = read(root.join("os-apps/paw-patrol/policies/patrol.cedar"));

    for needle in [
        "name = \"allowed_worker_id\"",
        "params = [\"work_cycle_id\", \"factory_case_id\", \"risk_lane\", \"task\", \"branch_name\", \"worktree_path\", \"runner_kind\", \"allowed_worker_id\", \"provider_id\", \"required_capabilities\"]",
    ] {
        assert!(
            spec.contains(needle),
            "WorkerRun should encode the allowed local worker identity: {needle}"
        );
    }

    assert!(
        csdl.contains("<Property Name=\"AllowedWorkerId\" Type=\"Edm.String\"/>"),
        "CSDL should expose WorkerRun.AllowedWorkerId for Cedar ABAC and human audit"
    );

    for needle in [
        "principal.id == resource.AllowedWorkerId",
        "principal.id == resource.allowed_worker_id",
        "Action::\"Claim\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should bind WorkerRun.Claim to the configured worker: {needle}"
        );
    }

    for module in [
        "patrol_request_router",
        "signal_router",
        "finding_lifecycle",
        "repo_sweep_lifecycle",
        "work_cycle_lifecycle",
    ] {
        let source = read(root.join(format!("os-apps/paw-patrol/wasm/{module}/src/lib.rs")));
        assert!(
            source.contains("configured_local_worker_id"),
            "{module} should configure WorkerRuns with the registered local worker id"
        );
        assert!(
            source.contains("configured_local_worktree_root"),
            "{module} should configure WorkerRun worktree paths with the registered local worktree root"
        );
        assert!(
            source.contains("\"allowed_worker_id\":"),
            "{module} should pass allowed_worker_id to WorkerRun.Configure"
        );
        assert!(
            !source.contains("/Users/seshendranalla/Development/temperpaw-worktrees"),
            "{module} should not hard-code a laptop-specific WorkerRun worktree root"
        );
    }

    for spec in [
        "patrol_request.ioa.toml",
        "signal.ioa.toml",
        "quality_finding.ioa.toml",
        "security_finding.ioa.toml",
        "repo_graph_snapshot.ioa.toml",
        "work_cycle.ioa.toml",
    ] {
        let source = read(root.join("os-apps/paw-patrol/specs").join(spec));
        assert!(
            source.contains("local_codex_worker_id = \"{secret:local_codex_worker_id}\""),
            "{spec} should pass the registered local Codex worker id into its WASM config"
        );
        assert!(
            source.contains("local_codex_worktree_root = \"{secret:local_codex_worktree_root}\""),
            "{spec} should pass the registered local Codex worktree root into its WASM config"
        );
    }
}

#[test]
fn risk_rules_set_a_floor_that_agents_cannot_silently_lower() {
    let root = repo_root();
    let risk_rule = read(root.join("os-apps/paw-patrol/specs/risk_rule.ioa.toml"));
    let factory_case = read(root.join("os-apps/paw-patrol/specs/factory_case.ioa.toml"));

    for needle in [
        "name = \"RiskRule\"",
        "name = \"minimum_risk_lane\"",
        "name = \"evidence_selector\"",
        "name = \"required_checks\"",
        "name = \"Define\"",
        "name = \"Activate\"",
        "name = \"Archive\"",
    ] {
        assert!(
            risk_rule.contains(needle),
            "RiskRule should contain {needle}"
        );
    }

    for needle in [
        "name = \"minimum_risk_lane\"",
        "name = \"risk_floor_source\"",
        "name = \"SetRiskFloor\"",
        "name = \"RaiseRisk\"",
        "name = \"ApproveRiskOverride\"",
    ] {
        assert!(
            factory_case.contains(needle),
            "FactoryCase should contain {needle}"
        );
    }

    assert!(
        !factory_case.contains("LowerRisk"),
        "agents should not have a silent LowerRisk action"
    );
}

#[test]
fn local_codex_worker_is_a_real_daemon_scaffold() {
    let root = repo_root();

    let workspace = read(root.join("Cargo.toml"));
    assert!(
        workspace.contains("\"crates/paw-codex-worker\""),
        "paw-codex-worker should be a workspace member"
    );

    let manifest = read(root.join("crates/paw-codex-worker/Cargo.toml"));
    assert!(manifest.contains("name = \"paw-codex-worker\""));

    let worker_src = read_worker_sources(&root);
    for needle in [
        "TEMPER_URL",
        "TEMPER_TENANT",
        "WORKER_ID",
        "REPO_ROOT",
        "/tdata/$events",
        "WorkerRun.Claim",
        "WorkerRun.ReportDone",
        "RepoGraphSnapshot",
        "ScanComplete",
        "repo_health_agent_prompt",
        "parse_repo_health_agent_output",
        "REPO_HEALTH_PATROL_RESULT_JSON_BEGIN",
        "git",
        "worktree",
        "doctor",
        "check_odata",
        "check_event_stream",
        "check_codex_binary",
        "check_codex_exec_smoke",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "LaunchdPlist",
        "render_launchd_plist",
        "match watch_events(&client, &config).await",
        "tokio::select!",
        "event_stream_queue_poll_interval()",
        "claim_event_stream_backlog(client, config).await?",
        "Temper event stream is open; polling queued Patrol work",
        "x-temper-principal-kind",
        "codex",
    ] {
        assert!(
            worker_src.contains(needle),
            "worker source should contain {needle}"
        );
    }

    let launchd =
        read(root.join("crates/paw-codex-worker/launchd/com.temperpaw.paw-codex-worker.plist"));
    assert!(launchd.contains("com.temperpaw.paw-codex-worker"));
    assert!(launchd.contains("paw-codex-worker"));
    for needle in [
        "WORKER_TOKEN",
        "REPO_ROOT",
        "CODEX_BIN",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE",
        "PAW_CODEX_POLL_ON_START",
        "RUST_LOG",
    ] {
        assert!(
            launchd.contains(needle),
            "launchd worker template should include {needle}"
        );
    }

    let readme = read(root.join("crates/paw-codex-worker/README.md"));
    for needle in [
        "launchctl bootstrap",
        "PAW_CODEX_ENABLE_EXECUTION",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE",
        "RepoGraphSnapshot.StartScan",
        "ChatGPT/Codex auth",
        "paw-codex-worker doctor",
        "launchd-plist",
        "proof bundle",
        "PROOF_DIR",
    ] {
        assert!(
            readme.contains(needle),
            "worker README should document {needle}"
        );
    }
}

#[test]
fn paw_codex_worker_sources_stay_under_giant_module_budget() {
    let root = repo_root();

    for path in worker_source_files(&root) {
        let source = read(&path);
        let line_count = source.lines().count();
        assert!(
            line_count < 900,
            "{} has {line_count} lines; paw-codex-worker source files should stay below the repo-health giant-module threshold",
            path.display()
        );
    }
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
fn deterministic_smoke_exports_a_visual_proof_bundle() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/deterministic-smoke.sh"));

    for needle in [
        "PROOF_DIR",
        "summary.json",
        "proof.md",
        "proof.svg",
        "visual_summary_url",
        "state_diagram_mermaid",
        "changed_files_map",
        "reviewer_verdict",
        "residual_risks",
        "## OData Links",
        "## Trace And Log Evidence",
        "os-apps/paw-patrol/wasm/build.sh",
        "proof bundle:",
    ] {
        assert!(
            script.contains(needle),
            "deterministic smoke should export human-reviewable proof evidence: {needle}"
        );
    }
}

#[test]
fn live_smoke_scripts_choose_non_colliding_odata_and_webhook_ports() {
    let root = repo_root();

    for script_name in [
        "deterministic-smoke.sh",
        "webhook-intake-smoke.sh",
        "repo-sweep-brief-smoke.sh",
        "datadog-patrol-smoke.sh",
        "production-readiness-smoke.sh",
        "production-observe-only-smoke.sh",
    ] {
        let script = read(
            root.join("crates/paw-codex-worker/scripts")
                .join(script_name),
        );
        for needle in [
            "pick_available_port",
            "port_is_free",
            "candidate + 12",
            "implicit webhook trigger port",
        ] {
            assert!(
                script.contains(needle),
                "{script_name} should avoid OData/webhook port collisions with {needle}"
            );
        }
    }
}

#[test]
fn datadog_patrol_smoke_proves_mcp_agent_fanout() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/datadog-patrol-smoke.sh"));

    for needle in [
        "PatrolRuns",
        "datadog_observability",
        "TemperPaw.Patrol.Start",
        "PAW_CODEX_ENABLE_EXECUTION=1",
        "PAW_CODEX_WORKER_CAPABILITIES=local_codex,repo_write,review,evaluation,datadog_query",
        "fixtures/fake-codex.sh",
        "DATADOG_PATROL_RESULT_JSON_BEGIN",
        "ObservabilityFindings",
        "FactoryCases",
        "Signals",
        "ProofPackets",
        "visual_summary_url",
        "datadog-patrol.svg",
        "datadog patrol smoke passed",
    ] {
        assert!(
            script.contains(needle),
            "Datadog Patrol smoke should prove MCP agent fanout: {needle}"
        );
    }
}

#[test]
fn repo_sweep_brief_smoke_exports_visual_proof_bundle() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/repo-sweep-brief-smoke.sh"));

    for needle in [
        "PatrolSchedules",
        "patrol-default-daily-maintenance",
        "patrol-schedule.json",
        "default_schedule",
        "RepoGraphSnapshots",
        "TemperPaw.Patrol.StartScan",
        "assessment_session_id",
        "assessment_status",
        "assessment_summary_markdown",
        "complete_from_repo_health_agent",
        "Assessment Session",
        "DailyBriefs",
        "TemperPaw.Patrol.Start",
        "repo-graph.json",
        "daily-brief.svg",
        "proof.svg",
        "LOCAL_CODEX_WORKER_ID=\"$WORKER_ID\"",
        "LOCAL_CODEX_WORKTREE_ROOT=\"$WORKSPACE_ROOT\"",
        "PAW_CODEX_ENABLE_EXECUTION=1",
        "norm_key",
        "top_level",
        "## Daily Brief",
        "## OData Links",
        "## Trace And Log Evidence",
        "proof bundle:",
    ] {
        assert!(
            script.contains(needle),
            "repo sweep/brief smoke should export maintenance proof evidence: {needle}"
        );
    }

    let startup = read(root.join("crates/temperpaw/src/startup.rs"));
    for needle in [
        "LOCAL_CODEX_WORKER_ID",
        "LOCAL_CODEX_WORKTREE_ROOT",
        "local_codex_worker_id",
        "local_codex_worktree_root",
    ] {
        assert!(
            startup.contains(needle),
            "startup should seed local Codex platform secrets from env for portable smoke tests: {needle}"
        );
    }
}

#[test]
fn webhook_intake_smoke_exercises_the_trigger_boundary() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/webhook-intake-smoke.sh"));

    for needle in [
        "WEBHOOK_URL",
        "/triggers/webhook/${route_key}",
        "WebhookEvents",
        "TemperPaw.Ingest.Received",
        "TemperPaw.Ingest.Register",
        "TemperPaw.Patrol.Submit",
        "TemperPaw.Patrol.Ingest",
        "PatrolRequests",
        "Signals",
        "FactoryCases",
        "WorkCycles",
        "patrol-discord",
        "summary.json",
        "webhook-intake.svg",
        "request-webhook-event.json",
        "datadog-webhook-event.json",
        "discord-webhook-event.json",
        "discord-signal.json",
        "## State Diagram",
        "## OData Links",
        "proof bundle:",
    ] {
        assert!(
            script.contains(needle),
            "webhook intake smoke should prove live trigger-boundary intake: {needle}"
        );
    }
}

#[test]
fn production_readiness_script_keeps_mac_mini_activation_checkable() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/production-readiness.sh"));
    let preflight = read(root.join("crates/paw-codex-worker/scripts/production-preflight.sh"));
    let preflight_diff =
        read(root.join("crates/paw-codex-worker/scripts/production-preflight-diff.sh"));
    let smoke = read(root.join("crates/paw-codex-worker/scripts/production-readiness-smoke.sh"));
    let readme = read(root.join("crates/paw-codex-worker/README.md"));
    let env_example = read(root.join(".env.example"));

    for needle in [
        "TEMPER_URL",
        "WORKER_TOKEN",
        "WORKER_ID",
        "mac-mini-codex-prod",
        "cargo build -p paw-codex-worker --release",
        "paw-codex-worker doctor",
        "launchd-plist",
        "WRITE_LAUNCHD_PLIST",
        "INSTALL_LAUNCHD=1",
        "PAW_CODEX_ENABLE_EXECUTION=0",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE",
    ] {
        assert!(
            script.contains(needle),
            "production readiness script should contain {needle}"
        );
    }

    for needle in [
        "PROOF_DIR",
        "summary.json",
        "proof.md",
        "preflight.svg",
        "operator-handoff.md",
        "git_head",
        "git_branch",
        "git_status_short",
        "git_clean",
        "Git head:",
        "Git clean:",
        "Human Blocker Decisions",
        "Secret And Approval Inputs",
        "human_blockers",
        "TEMPER_URL",
        "WORKER_TOKEN",
        "PATROL_OPERATOR_TOKEN",
        "launchctl print",
        "railway status",
        "railway project list --json",
        "railway-candidates.json",
        "railway:candidate_projects",
        "railway link <project_id>",
        "CHECK_RAILWAY",
        "STRICT=1",
        "CONFIRM_TEMPER_PIN_OK",
        "CONFIRM_TEMPERPAW_PR_OK",
        "statusCheckRollup",
        "TemperPaw PR #218 is clean and green but unmerged",
        "temperpaw-pr-220.json",
        "github:temperpaw_pr_220",
        "TemperPaw PR #220 is merged",
        "temperpaw-pr-221.json",
        "github:temperpaw_pr_221",
        "TemperPaw PR #221 is merged",
        "does not mutate Railway, launchd, or Temper",
    ] {
        assert!(
            preflight.contains(needle),
            "production preflight should report non-mutating activation readiness: {needle}"
        );
    }

    for needle in [
        "baseline-summary.json",
        "current-summary.json",
        "preflight-diff.svg",
        "Resolved Blockers",
        "New Blockers",
        "Railway Candidate Drift",
        "does not mutate Railway, launchd, Temper, or git",
    ] {
        assert!(
            preflight_diff.contains(needle),
            "production preflight diff should report activation drift: {needle}"
        );
    }

    for needle in [
        "production-readiness.sh",
        "production-preflight.sh",
        "paw-codex-worker doctor",
        "WRITE_LAUNCHD_PLIST=1",
        "INSTALL_LAUNCHD=0",
        "PAW_CODEX_ENABLE_EXECUTION=0",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE=1",
        "token_not_printed_to_readiness_log",
        "launchd_not_loaded",
        "production readiness smoke passed",
    ] {
        assert!(
            smoke.contains(needle),
            "production readiness smoke should prove guarded cutover readiness: {needle}"
        );
    }

    for needle in [
        "scripts/production-preflight.sh",
        "scripts/production-readiness.sh",
        "scripts/production-readiness-smoke.sh",
        "WRITE_LAUNCHD_PLIST=1",
        "INSTALL_LAUNCHD=1",
        "does not print `WORKER_TOKEN`",
    ] {
        assert!(
            readme.contains(needle),
            "worker README should document production readiness: {needle}"
        );
    }

    for needle in [
        "Paw Patrol local Codex worker",
        "TEMPER_URL=",
        "WORKER_TOKEN=",
        "WORKER_ID=mac-mini-codex-prod",
        "PAW_CODEX_ENABLE_EXECUTION=0",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE=1",
        "PAW_CODEX_POLL_ON_START=1",
    ] {
        assert!(
            env_example.contains(needle),
            ".env.example should document Patrol worker activation: {needle}"
        );
    }
}

#[test]
fn production_observe_only_script_turns_cutover_gate_into_a_guarded_proof() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/production-observe-only.sh"));
    let smoke = read(root.join("crates/paw-codex-worker/scripts/production-observe-only-smoke.sh"));
    let acceptance = read(root.join("crates/paw-codex-worker/scripts/paw-patrol-acceptance.sh"));
    let ci = read(root.join(".github/workflows/ci.yml"));
    let readme = read(root.join("crates/paw-codex-worker/README.md"));
    let runbook = read(root.join("docs/runbooks/paw-patrol-production-cutover.md"));

    for needle in [
        "ALLOW_PRODUCTION_WRITE=1",
        "PATROL_OPERATOR_TOKEN",
        "RepoGraphSnapshots",
        "TemperPaw.Patrol.StartScan",
        "WorkerRuns",
        "WorkCycles",
        "ReviewRuns",
        "EvaluationRuns",
        "ProofPackets",
        "DailyBriefs",
        "summary.json",
        "proof.md",
        "observe-only.svg",
        "allowed_worker_id",
        "PAW_CODEX_ENABLE_EXECUTION=0",
    ] {
        assert!(
            script.contains(needle),
            "production observe-only script should make Gate 6 executable and evidenced: {needle}"
        );
    }

    for needle in [
        "production-observe-only.sh",
        "ALLOW_PRODUCTION_WRITE=1",
        "PAW_CODEX_ENABLE_EXECUTION=0",
        "fixtures/fake-codex.sh",
        "production observe-only smoke passed",
        "observe-only.svg",
    ] {
        assert!(
            smoke.contains(needle),
            "production observe-only smoke should prove the guarded script locally: {needle}"
        );
    }

    for needle in [
        "production-observe-only.sh",
        "production-observe-only-smoke.sh",
        "production-observe-only/summary.json",
        "production-observe-only/observe-only.svg",
    ] {
        assert!(
            acceptance.contains(needle),
            "acceptance harness should collect observe-only proof evidence: {needle}"
        );
    }

    let needle = "for f in crates/paw-codex-worker/scripts/*.sh; do bash -n \"$f\"; done";
    assert!(ci.contains(needle), "CI should syntax-check {needle}");

    for needle in [
        "production-observe-only.sh",
        "production-observe-only-smoke.sh",
        "ALLOW_PRODUCTION_WRITE=1",
        "PATROL_OPERATOR_TOKEN",
    ] {
        assert!(
            readme.contains(needle),
            "worker README should document observe-only production proof: {needle}"
        );
        assert!(
            runbook.contains(needle),
            "production runbook should document observe-only production proof: {needle}"
        );
    }
}

#[test]
fn production_cutover_runbook_maps_every_human_blocker_to_a_gate() {
    let root = repo_root();
    let runbook = read(root.join("docs/runbooks/paw-patrol-production-cutover.md"));
    let readme = read(root.join("crates/paw-codex-worker/README.md"));

    for needle in [
        "```mermaid",
        "Railway TemperPaw URL",
        "WORKER_TOKEN",
        "production-preflight.sh",
        "local_codex_worker_id",
        "production-readiness-smoke.sh",
        "production-readiness.sh",
        "WRITE_LAUNCHD_PLIST=1",
        "INSTALL_LAUNCHD=1",
        "PAW_CODEX_ENABLE_EXECUTION=0",
        "PAW_CODEX_DOCTOR_EXEC_SMOKE=1",
        "Datadog",
        "Discord",
        "GitHub",
        "launchctl bootout",
        "Temper Cedar",
        "rollback",
        "human approval",
        "evidence to capture",
    ] {
        assert!(
            runbook.contains(needle),
            "production cutover runbook should map blocker or gate: {needle}"
        );
    }

    assert!(
        readme.contains("docs/runbooks/paw-patrol-production-cutover.md"),
        "worker README should point operators to the production cutover runbook"
    );
}

#[test]
fn production_preflight_exports_standalone_human_blockers_artifact() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/production-preflight.sh"));
    let runbook = read(root.join("docs/runbooks/paw-patrol-production-cutover.md"));
    let readme = read(root.join("crates/paw-codex-worker/README.md"));

    for needle in [
        "HUMAN_BLOCKERS_JSON",
        "human-blockers.json",
        ".human_blockers",
    ] {
        assert!(
            script.contains(needle),
            "production preflight should export standalone blocker artifact: {needle}"
        );
    }

    for needle in ["human-blockers.json", "standalone blocker list"] {
        assert!(
            runbook.contains(needle),
            "production cutover runbook should mention blocker artifact: {needle}"
        );
        assert!(
            readme.contains(needle),
            "worker README should mention blocker artifact: {needle}"
        );
    }
}

#[test]
fn paw_patrol_acceptance_harness_collects_quick_and_live_proofs() {
    let root = repo_root();
    let script = read(root.join("crates/paw-codex-worker/scripts/paw-patrol-acceptance.sh"));
    let readme = read(root.join("crates/paw-codex-worker/README.md"));
    let ci = read(root.join(".github/workflows/ci.yml"));

    for needle in [
        "quick",
        "live",
        "summary.json",
        "git_head",
        "git_branch",
        "git_status_short",
        "Git head:",
        "proof.md",
        "index.html",
        "acceptance.log",
        "cargo fmt --check --all",
        "cargo test --locked -p temperpaw --test paw_patrol_foundation -- --nocapture",
        "cargo test --locked -p paw-codex-worker --quiet",
        "deterministic-smoke.sh",
        "webhook-intake-smoke.sh",
        "repo-sweep-brief-smoke.sh",
        "production-preflight.sh",
        "production-preflight/summary.json",
        "production-preflight/proof.md",
        "production-preflight/operator-handoff.md",
        "production-preflight/preflight.svg",
        "production-preflight-railway-discovery-smoke.sh",
        "production-preflight-railway-discovery-smoke/railway-candidates.json",
        "production-preflight-railway-discovery-smoke/operator-handoff.md",
        "Railway Discovery Preflight",
        "production-preflight-diff-smoke.sh",
        "production-preflight-diff-smoke/preflight-diff.svg",
        "Preflight Diff",
        "production-preflight-github-smoke.sh",
        "production-preflight-github-smoke/summary-without-confirm.json",
        "production-preflight-github-smoke/summary-with-confirm.json",
        "production-readiness-smoke.sh",
        "deterministic-smoke/proof.svg",
        "webhook-intake-smoke/webhook-intake.svg",
        "repo-sweep-brief-smoke/patrol-schedule.json",
        "Default PatrolSchedule",
        "patrol-github",
        "github-webhook-event.json",
        "github-signal.json",
        "GitHub Signal",
        "datadog-patrol-smoke/datadog-patrol.svg",
        "Datadog MCP Patrol",
        "repo-sweep-brief-smoke/daily-brief.svg",
        "write_artifact_link",
        "not generated in",
        "```mermaid",
        "proof bundle:",
    ] {
        assert!(
            script.contains(needle),
            "acceptance harness should collect quick/live proof evidence: {needle}"
        );
    }

    for needle in [
        "paw-patrol-acceptance.sh quick",
        "paw-patrol-acceptance.sh live",
        "/tmp/paw-patrol-acceptance-",
        "index.html",
    ] {
        assert!(
            readme.contains(needle),
            "worker README should document the acceptance harness: {needle}"
        );
    }

    assert!(
        ci.contains("for f in crates/paw-codex-worker/scripts/*.sh; do bash -n \"$f\"; done"),
        "CI should syntax-check the acceptance harness"
    );
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
fn mac_mini_bootstrap_uses_railway_token_handoff_without_printing_secrets() {
    let root = repo_root();
    let script_path = root.join("crates/paw-codex-worker/scripts/mac-mini-production-bootstrap.sh");

    assert!(
        script_path.is_file(),
        "Mac mini production bootstrap script should exist"
    );

    let script = read(script_path);
    for needle in [
        "railway link --project",
        "ad7f8977-cf48-43ef-b129-ba1e17896ae4",
        "railway run --service temperpaw --environment production",
        "printf %s \"$TEMPER_API_KEY\"",
        "PAW_CODEX_ENABLE_EXECUTION=\"${PAW_CODEX_ENABLE_EXECUTION:-0}\"",
        "crates/paw-codex-worker/scripts/production-readiness.sh",
        "WRITE_LAUNCHD_PLIST=1",
        "INSTALL_LAUNCHD=\"${INSTALL_LAUNCHD:-0}\"",
        "crates/paw-codex-worker/scripts/production-observe-only.sh",
    ] {
        assert!(
            script.contains(needle),
            "Mac mini bootstrap should contain {needle}"
        );
    }

    for forbidden in [
        "echo \"$WORKER_TOKEN\"",
        "echo \"$TEMPER_API_KEY\"",
        "set -x",
    ] {
        assert!(
            !script.contains(forbidden),
            "Mac mini bootstrap must not print secrets via {forbidden}"
        );
    }

    let readme = read(root.join("crates/paw-codex-worker/README.md"));
    for needle in [
        "mac-mini-production-bootstrap.sh",
        "without printing it",
        "INSTALL_LAUNCHD=1",
        "RUN_OBSERVE_ONLY=1",
    ] {
        assert!(
            readme.contains(needle),
            "worker README should document Mac mini bootstrap usage: {needle}"
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
fn startup_rehydrates_os_app_verification_for_unchanged_apps() {
    let root = repo_root();
    let startup = read(root.join("crates/temperpaw/src/startup.rs"));
    let skipped_start = startup
        .find("Ok(OsAppReconcileResult::Skipped")
        .expect("startup should handle skipped OS app reconciliation");
    let installed_start = startup
        .find("Ok(OsAppReconcileResult::Installed")
        .expect("startup should handle installed OS app reconciliation");
    let skipped_branch = &startup[skipped_start..installed_start];

    assert!(
        skipped_branch.contains("persist_os_app_verification"),
        "unchanged OS apps must repopulate the in-memory verification registry after deploy/restart"
    );
}

#[test]
fn startup_rehydrates_persisted_verification_after_spec_restore() {
    let root = repo_root();
    let startup = read(root.join("crates/temperpaw/src/startup.rs"));

    assert!(
        startup.contains(
            "restore_persisted_spec_verification_statuses(&state, platform_store).await?"
        ),
        "startup must rehydrate persisted verification statuses after restoring specs from storage"
    );
    assert!(
        startup.contains("DurableVerificationCache"),
        "rehydrated verification statuses should be identifiable in the registry"
    );
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
fn paw_patrol_has_webhook_intake_routes_through_paw_ingest() {
    let root = repo_root();

    let routes = read(root.join("os-apps/paw-patrol/seed-data/webhook_routes.toml"));
    for needle in [
        "id = \"patrol-request-route\"",
        "route_key = \"patrol-request\"",
        "target_entity_type = \"WorkRequest\"",
        "target_action = \"TemperPaw.Patrol.Submit\"",
        "id = \"patrol-signal-route\"",
        "route_key = \"patrol-signal\"",
        "target_entity_type = \"Signal\"",
        "target_action = \"TemperPaw.Patrol.Ingest\"",
        "route_key = \"patrol-datadog\"",
        "route_key = \"patrol-github\"",
        "route_key = \"patrol-discord\"",
    ] {
        assert!(
            routes.contains(needle),
            "Patrol webhook route seed data should contain {needle}"
        );
    }

    let process_webhook = read(root.join("os-apps/paw-ingest/wasm/process_webhook/src/lib.rs"));
    for needle in [
        "build_patrol_request_submit_params",
        "\"request_text\"",
        "\"requester_id\"",
        "build_signal_ingest_params",
        "\"source_url\"",
        "\"severity\"",
        "\"payload\"",
        "fallback_source",
    ] {
        assert!(
            process_webhook.contains(needle),
            "process_webhook should translate payloads into Patrol params: {needle}"
        );
    }

    let ingest_manifest = read(root.join("os-apps/paw-ingest/app.toml"));
    for needle in [
        "name = \"validate_webhook\"",
        "name = \"route_webhook\"",
        "name = \"process_webhook\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            ingest_manifest.contains(needle),
            "paw-ingest app.toml should explicitly install webhook WASM module {needle}"
        );
    }

    let ingest_build = read(root.join("os-apps/paw-ingest/wasm/build.sh"));
    for needle in ["validate_webhook", "route_webhook", "process_webhook"] {
        assert!(
            ingest_build.contains(needle),
            "paw-ingest build.sh should build {needle}"
        );
    }

    let app_doc = read(root.join("os-apps/paw-patrol/APP.md"));
    for needle in [
        "/triggers/webhook/patrol-request",
        "/triggers/webhook/patrol-signal",
        "/triggers/webhook/patrol-datadog",
        "/triggers/webhook/patrol-github",
        "WebhookEvent -> WorkRequest.Submit",
        "WebhookEvent -> Signal.Ingest",
    ] {
        assert!(app_doc.contains(needle), "APP.md should document {needle}");
    }

    let startup = read(root.join("crates/temperpaw/src/startup.rs"));
    assert!(
        startup.contains("paw_transport::webhook::router"),
        "production runtime router should expose /triggers/webhook/{{route_key}} on the main Railway port"
    );
}

#[test]
fn patrol_schedule_recurs_sweeps_and_daily_briefs_inside_patrol() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"patrol_schedule_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/patrol_schedule.ioa.toml"));
    for needle in [
        "name = \"PatrolSchedule\"",
        "\"schedule_at('Trigger', next_run_at)\"",
        "module = \"patrol_schedule_lifecycle\"",
        "name = \"TriggerComplete\"",
        "name = \"Recover\"",
        "from = [\"Failed\"]",
        "patrol_schedule_recover",
        "last_repo_graph_snapshot_id",
        "last_daily_brief_id",
    ] {
        assert!(
            spec.contains(needle),
            "PatrolSchedule spec should contain {needle}"
        );
    }
    for action in ["Activate", "Resume"] {
        assert_action_effects(
            &spec,
            "PatrolSchedule",
            action,
            &[],
            &["patrol_schedule_activate"],
        );
    }
    assert_action_effects(
        &spec,
        "PatrolSchedule",
        "Trigger",
        &["run_count += 1"],
        &["patrol_schedule_trigger"],
    );

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    for needle in [
        "<EntityType Name=\"PatrolSchedule\">",
        "<EntitySet Name=\"PatrolSchedules\" EntityType=\"TemperPaw.Patrol.PatrolSchedule\"/>",
        "<Property Name=\"NextRunAt\" Type=\"Edm.String\"/>",
    ] {
        assert!(csdl.contains(needle), "CSDL should expose {needle}");
    }

    let wasm_root = patrol.join("wasm/patrol_schedule_lifecycle");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "patrol_schedule_lifecycle should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "patrol_schedule_lifecycle should have a WASM entry point"
    );

    let lifecycle = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/RepoGraphSnapshots",
        "/tdata/DailyBriefs",
        "TemperPaw.Patrol.StartScan",
        "TemperPaw.Patrol.Start",
        "TriggerComplete",
        "ActivateComplete",
        "parse_patrol_interval",
    ] {
        assert!(
            lifecycle.contains(needle),
            "patrol_schedule_lifecycle should contain {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "patrol_schedule_lifecycle",
        "PatrolSchedule",
        "Action::\"Configure\"",
        "Action::\"Activate\"",
        "Action::\"Trigger\"",
        "Action::\"TriggerComplete\"",
        "Action::\"Recover\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize PatrolSchedule with {needle}"
        );
    }
    for needle in [
        "principal.id == resource.WorkerId",
        "principal.id == resource.ReviewerId",
        "principal.id == resource.worker_id",
        "principal.id == resource.reviewer_id",
        "principal.agent_type == \"worker\"",
        "principal.agent_type == \"system\"",
        "[\"supervisor\", \"human\"].contains(principal.agent_type)",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should bind lifecycle actions to claimed identities: {needle}"
        );
    }

    let build_script = read(patrol.join("wasm/build.sh"));
    assert!(
        build_script.contains("patrol_schedule_lifecycle"),
        "build.sh should build patrol_schedule_lifecycle"
    );

    let default_schedule = read(patrol.join("seed-data/default_schedules.toml"));
    for needle in [
        "id = \"patrol-default-daily-maintenance\"",
        "type = \"PatrolSchedule\"",
        "name = \"Default daily Patrol maintenance\"",
        "cadence = \"daily\"",
        "enable_repo_sweep = true",
        "enable_daily_brief = true",
        "[[instance.actions]]",
        "name = \"Configure\"",
        "name = \"Activate\"",
    ] {
        assert!(
            default_schedule.contains(needle),
            "default_schedules.toml should seed an active daily PatrolSchedule: {needle}"
        );
    }
}

#[test]
fn patrol_request_submit_is_temper_native_intake_routing() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "[[wasm_modules]]",
        "name = \"patrol_request_router\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/patrol_request.ioa.toml"));
    for needle in [
        "[[action.triggers]]",
        "name = \"route_patrol_request\"",
        "kind = \"wasm\"",
        "module = \"patrol_request_router\"",
        "on_failure = \"RouteFailed\"",
        "temper_api_url = \"{secret:temper_api_url}\"",
        "name = \"RouteFailed\"",
        "params = [\"error_message\", \"integration\"]",
    ] {
        assert!(
            spec.contains(needle),
            "PatrolRequest spec should contain {needle}"
        );
    }
    assert_action_effects(
        &spec,
        "PatrolRequest",
        "Submit",
        &[],
        &["route_patrol_request"],
    );

    let wasm_root = patrol.join("wasm/patrol_request_router");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "patrol_request_router should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "patrol_request_router should have a WASM entry point"
    );

    let router = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/Issues",
        "/tdata/FactoryCases",
        "/tdata/WorkCycles",
        "/tdata/WorkerRuns",
        "TemperPaw.PM.SetDescription",
        "TemperPaw.PM.SetPriority",
        "TemperPaw.PM.MoveToTriage",
        "TemperPaw.Patrol.Triage",
        "TemperPaw.Patrol.AcceptAsCase",
        "TemperPaw.Patrol.LinkPmIssue",
        "TemperPaw.Patrol.Open",
        "TemperPaw.Patrol.SetRiskFloor",
        "TemperPaw.Patrol.LinkPmIssue",
        "TemperPaw.Patrol.OpenWorkCycle",
        "TemperPaw.Patrol.QueueWork",
        "TemperPaw.Patrol.Configure",
        "TemperPaw.Patrol.WritePlan",
        "TemperPaw.Patrol.StartWork",
        "TemperPaw.Patrol.AttachWorkerRun",
        "runner_kind",
        "allowed_worker_id",
        "local_codex",
        "paw-codex-worker",
    ] {
        assert!(
            router.contains(needle),
            "patrol_request_router should contain {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "action == Action::\"http_call\"",
        "resource is HttpEndpoint",
        "patrol_request_router",
        "worker_run_lifecycle",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize WASM host HTTP calls with {needle}"
        );
    }
}

#[test]
fn signal_ingest_routes_observable_failures_into_patrol_work() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"signal_router\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/signal.ioa.toml"));
    for needle in [
        "[[action.triggers]]",
        "name = \"route_signal\"",
        "kind = \"wasm\"",
        "module = \"signal_router\"",
        "on_failure = \"Archive\"",
        "temper_api_url = \"{secret:temper_api_url}\"",
    ] {
        assert!(spec.contains(needle), "Signal spec should contain {needle}");
    }
    assert_action_effects(&spec, "Signal", "Ingest", &[], &["route_signal"]);

    let wasm_root = patrol.join("wasm/signal_router");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "signal_router should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "signal_router should have a WASM entry point"
    );

    let router = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/Issues",
        "/tdata/FactoryCases",
        "/tdata/WorkCycles",
        "/tdata/WorkerRuns",
        "TemperPaw.PM.SetDescription",
        "TemperPaw.PM.SetPriority",
        "TemperPaw.PM.MoveToTriage",
        "TemperPaw.Patrol.Normalize",
        "TemperPaw.Patrol.Triage",
        "TemperPaw.Patrol.AttachCase",
        "TemperPaw.Patrol.Archive",
        "TemperPaw.Patrol.Open",
        "TemperPaw.Patrol.SetRiskFloor",
        "TemperPaw.Patrol.LinkPmIssue",
        "TemperPaw.Patrol.OpenWorkCycle",
        "TemperPaw.Patrol.QueueWork",
        "TemperPaw.Patrol.Configure",
        "TemperPaw.Patrol.WritePlan",
        "TemperPaw.Patrol.StartWork",
        "TemperPaw.Patrol.AttachWorkerRun",
        "Datadog",
        "Discord",
        "local_codex",
    ] {
        assert!(
            router.contains(needle),
            "signal_router should contain {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    assert!(
        policy.contains("signal_router"),
        "patrol.cedar should authorize signal_router host HTTP calls"
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
fn worker_run_done_fans_out_to_review_evaluation_and_proof() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"worker_run_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/worker_run.ioa.toml"));
    for needle in [
        "module = \"worker_run_lifecycle\"",
        "on_failure = \"ReportFailed\"",
    ] {
        assert!(
            spec.contains(needle),
            "WorkerRun spec should contain {needle}"
        );
    }
    for (action, trigger) in [
        ("StartLocal", "worker_run_started"),
        ("ReportDone", "worker_run_finished"),
        ("ReportFailed", "worker_run_failed"),
    ] {
        assert_action_effects(&spec, "WorkerRun", action, &[], &[trigger]);
    }

    let wasm_root = patrol.join("wasm/worker_run_lifecycle");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "worker_run_lifecycle should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "worker_run_lifecycle should have a WASM entry point"
    );

    let lifecycle = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/FactoryCases",
        "/tdata/WorkCycles",
        "/tdata/ReviewRuns",
        "/tdata/EvaluationRuns",
        "/tdata/ProofPackets",
        "TemperPaw.Patrol.BeginWork",
        "TemperPaw.Patrol.BeginReview",
        "TemperPaw.Patrol.WorkerDone",
        "TemperPaw.Patrol.SubmitForReview",
        "TemperPaw.Patrol.AttachReviewRun",
        "TemperPaw.Patrol.AttachEvaluationRun",
        "TemperPaw.Patrol.Request",
        "TemperPaw.Patrol.Queue",
        "TemperPaw.Patrol.AttachDraft",
        "independent reviewer",
        "visual ProofPacket",
        "data:image/svg+xml",
        "visual_summary_svg",
        "wait_for_status",
        "worker_run_lifecycle: WorkCycle",
    ] {
        assert!(
            lifecycle.contains(needle),
            "worker_run_lifecycle should contain {needle}"
        );
    }
}

#[test]
fn reviewer_and_evaluator_results_gate_completion_before_human_review() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"review_gate_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let review_spec = read(patrol.join("specs/review_run.ioa.toml"));
    assert!(
        review_spec.contains("module = \"review_gate_lifecycle\""),
        "ReviewRun spec should contain module = \"review_gate_lifecycle\""
    );
    for (action, trigger) in [
        ("Approve", "review_run_approved"),
        ("RequestChanges", "review_run_changes_requested"),
        ("Escalate", "review_run_escalated"),
        ("Fail", "review_run_failed"),
    ] {
        assert_action_effects(&review_spec, "ReviewRun", action, &[], &[trigger]);
    }

    let evaluation_spec = read(patrol.join("specs/evaluation_run.ioa.toml"));
    for needle in [
        "name = \"evaluator_id\"",
        "name = \"failure_classification\"",
        "name = \"Claim\"",
        "params = [\"evaluator_id\"]",
        "params = [\"results_json\", \"error_message\", \"failure_classification\"]",
        "module = \"review_gate_lifecycle\"",
    ] {
        assert!(
            evaluation_spec.contains(needle),
            "EvaluationRun spec should contain {needle}"
        );
    }
    for (action, trigger) in [
        ("Pass", "evaluation_run_passed"),
        ("Fail", "evaluation_run_failed"),
    ] {
        assert_action_effects(&evaluation_spec, "EvaluationRun", action, &[], &[trigger]);
    }

    let wasm_root = patrol.join("wasm/review_gate_lifecycle");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "review_gate_lifecycle should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "review_gate_lifecycle should have a WASM entry point"
    );

    let lifecycle = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/WorkCycles",
        "/tdata/FactoryCases",
        "/tdata/ProofPackets",
        "TemperPaw.Patrol.PassReview",
        "TemperPaw.Patrol.RequestChanges",
        "TemperPaw.Patrol.ReportE2e",
        "TemperPaw.Patrol.PassEvaluation",
        "TemperPaw.Patrol.AttachProofPacket",
        "TemperPaw.Patrol.Complete",
        "TemperPaw.Patrol.BeginProof",
        "TemperPaw.Patrol.MarkReady",
        "TemperPaw.Patrol.Reject",
        "TemperPaw.Patrol.Escalate",
        "record_e2e_if_present",
        "known_passed_evaluation_run_id",
        "reviewer approved before human review",
        "evaluation gates passed before proof readiness",
    ] {
        assert!(
            lifecycle.contains(needle),
            "review_gate_lifecycle should contain {needle}"
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
fn evaluation_failures_requeue_rework_instead_of_dead_ending_the_cycle() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");
    let lifecycle = read(patrol.join("wasm/review_gate_lifecycle/src/lib.rs"));

    for needle in [
        "\"Fail\" if is_entity_type(&ctx, \"EvaluationRun\")",
        "handle_evaluation_failed",
        "Automated evaluation requested rework",
        "TemperPaw.Patrol.RequestChanges",
        "status == \"Reviewing\"",
        "fail_work_cycle_and_escalate_case(ctx, base_url, headers, &work_cycle_id, &message)",
    ] {
        assert!(
            lifecycle.contains(needle),
            "evaluation failures should requeue ordinary Reviewing cycles and only fail unexpected states: {needle}"
        );
    }
}

#[test]
fn terminal_reviews_cleanup_obsolete_queued_evaluation_runs() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");
    let lifecycle = read(patrol.join("wasm/review_gate_lifecycle/src/lib.rs"));

    for needle in [
        "fail_obsolete_evaluation_run_if_needed",
        "review_terminal_without_approval",
        "parent_work_cycle_terminal",
        "is_obsolete_evaluation_failure(&failure_classification)",
        "obsolete queued EvaluationRun",
    ] {
        assert!(
            lifecycle.contains(needle),
            "review_gate_lifecycle should terminalize obsolete queued evaluations without reopening parent cycles: {needle}"
        );
    }
}

#[test]
fn high_risk_work_requires_human_start_and_completion_approval() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let work_cycle = read(patrol.join("specs/work_cycle.ioa.toml"));
    for needle in [
        "AwaitingHumanStartApproval",
        "AwaitingHumanCompletionApproval",
        "name = \"human_start_approval_required\"",
        "name = \"human_start_approved\"",
        "name = \"human_completion_approval_required\"",
        "name = \"human_completion_approved\"",
        "name = \"task_detail\"",
        "name = \"RequestHumanStartApproval\"",
        "name = \"ApproveHumanStart\"",
        "name = \"RequestHumanCompletionApproval\"",
        "name = \"ApproveHumanCompletion\"",
        "module = \"work_cycle_lifecycle\"",
    ] {
        assert!(
            work_cycle.contains(needle),
            "WorkCycle spec should encode L3 human approval gates: {needle}"
        );
    }

    let manifest = read(patrol.join("app.toml"));
    assert!(
        manifest.contains("name = \"work_cycle_lifecycle\""),
        "paw-patrol should install work_cycle_lifecycle for approved L3 dispatch"
    );

    let request_router = read(patrol.join("wasm/patrol_request_router/src/lib.rs"));
    for needle in [
        "requires_human_start_approval",
        "PATROL_REQUEST_HUMAN_START_APPROVAL",
        "if requires_human_start_approval(risk.lane)",
        "queued after human start approval",
        "sensitive_initial_intake",
        "sensitive_intake:production",
        "ordinary maintenance request",
    ] {
        assert!(
            request_router.contains(needle),
            "patrol_request_router should pause L3 requests before WorkerRun: {needle}"
        );
    }

    let signal_router = read(patrol.join("wasm/signal_router/src/lib.rs"));
    for needle in [
        "requires_human_start_approval",
        "PATROL_REQUEST_HUMAN_START_APPROVAL",
        "if requires_human_start_approval(risk.lane)",
        "queued after human start approval",
        "sensitive_initial_signal_intake",
        "sensitive_intake:production",
        "ordinary maintenance signal",
    ] {
        assert!(
            signal_router.contains(needle),
            "signal_router should pause L3 signals before WorkerRun: {needle}"
        );
    }

    let work_cycle_lifecycle = read(patrol.join("wasm/work_cycle_lifecycle/src/lib.rs"));
    for needle in [
        "/tdata/WorkerRuns",
        "ApproveHumanStart",
        "TemperPaw.Patrol.StartWork",
        "TemperPaw.Patrol.AttachWorkerRun",
        "TemperPaw.Patrol.QueueWork",
        "human-approved L3 work",
        "local_codex",
    ] {
        assert!(
            work_cycle_lifecycle.contains(needle),
            "work_cycle_lifecycle should dispatch approved high-risk work: {needle}"
        );
    }

    let review_gate = read(patrol.join("wasm/review_gate_lifecycle/src/lib.rs"));
    for needle in [
        "requires_human_completion_approval",
        "TemperPaw.Patrol.RequestHumanCompletionApproval",
        "human completion approval required before WorkCycle.Complete",
        "ApproveHumanCompletion",
    ] {
        assert!(
            review_gate.contains(needle),
            "review_gate_lifecycle should pause L3 completion after proof gates: {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "Action::\"RequestHumanStartApproval\"",
        "Action::\"ApproveHumanStart\"",
        "Action::\"RequestHumanCompletionApproval\"",
        "Action::\"ApproveHumanCompletion\"",
        "work_cycle_lifecycle",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize L3 approval transitions: {needle}"
        );
    }
}

#[test]
fn repo_graph_snapshot_queues_sweep_and_fans_out_findings() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"repo_sweep_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/repo_graph_snapshot.ioa.toml"));
    for needle in [
        "module = \"repo_sweep_lifecycle\"",
        "on_failure = \"ScanFailed\"",
        "name = \"work_cycle_id\"",
        "name = \"worker_run_id\"",
        "name = \"assessment_session_id\"",
        "name = \"assessment_status\"",
        "name = \"assessment_summary_markdown\"",
        "name = \"AttachWorkerRun\"",
        "name = \"AttachAssessmentSession\"",
        "name = \"AssessmentComplete\"",
        "params = [\"error_message\", \"integration\"]",
    ] {
        assert!(
            spec.contains(needle),
            "RepoGraphSnapshot spec should contain {needle}"
        );
    }
    for (action, trigger) in [
        ("StartScan", "repo_sweep_started"),
        ("ScanComplete", "repo_sweep_completed"),
    ] {
        assert_action_effects(&spec, "RepoGraphSnapshot", action, &[], &[trigger]);
    }

    let wasm_root = patrol.join("wasm/repo_sweep_lifecycle");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "repo_sweep_lifecycle should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "repo_sweep_lifecycle should have a WASM entry point"
    );

    let lifecycle = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/WorkCycles",
        "/tdata/WorkerRuns",
        "/tdata/Sessions",
        "/tdata/QualityFindings",
        "/tdata/SecurityFindings",
        "TemperPaw.Patrol.Configure",
        "TemperPaw.Patrol.WritePlan",
        "TemperPaw.Patrol.StartWork",
        "TemperPaw.Patrol.AttachWorkerRun",
        "TemperPaw.Configure",
        "TemperPaw.Patrol.AttachAssessmentSession",
        "TemperPaw.Patrol.AssessmentComplete",
        "TemperPaw.Patrol.OpenFinding",
        "agent-led repo health patrol",
        "agent-authored sweep output",
        "repo_graph_snapshot_id",
        "complete_from_repo_health_agent",
        "repo-health Patrol agent",
        "giant modules",
        "duplicate logic",
        "fingerprint",
        "TODO/HACK",
        "Cedar drift",
        "local_codex",
    ] {
        assert!(
            lifecycle.contains(needle),
            "repo_sweep_lifecycle should contain {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "repo_sweep_lifecycle",
        "RepoGraphSnapshot",
        "QualityFinding",
        "SecurityFinding",
        "Action::\"StartScan\"",
        "Action::\"ScanComplete\"",
        "Action::\"AttachAssessmentSession\"",
        "Action::\"AssessmentComplete\"",
        "Action::\"OpenFinding\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize repo sweep lifecycle with {needle}"
        );
    }

    let app_doc = read(patrol.join("APP.md"));
    for needle in [
        "agent-led investigation",
        "structured findings",
        "AssessmentComplete",
    ] {
        assert!(
            app_doc.contains(needle),
            "APP.md should explain RepoGraphSnapshot session assessment: {needle}"
        );
    }
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
fn accepted_finding_work_cycles_resolve_source_findings_on_completion() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let work_cycle = read(patrol.join("specs/work_cycle.ioa.toml"));
    for needle in [
        "name = \"source_entity_type\"",
        "name = \"source_entity_id\"",
        "name = \"LinkSource\"",
        "params = [\"source_entity_type\", \"source_entity_id\"]",
        "name = \"work_cycle_completed\"",
        "module = \"work_cycle_lifecycle\"",
    ] {
        assert!(
            work_cycle.contains(needle),
            "WorkCycle should carry and trigger source finding closure: {needle}"
        );
    }

    let csdl = read(patrol.join("specs/model.csdl.xml"));
    for needle in [
        "<Property Name=\"SourceEntityType\" Type=\"Edm.String\"/>",
        "<Property Name=\"SourceEntityId\" Type=\"Edm.String\"/>",
    ] {
        assert!(csdl.contains(needle), "CSDL should expose {needle}");
    }

    let finding_lifecycle = read(patrol.join("wasm/finding_lifecycle/src/lib.rs"));
    for needle in [
        "TemperPaw.Patrol.LinkSource",
        "\"source_entity_type\"",
        "\"source_entity_id\"",
    ] {
        assert!(
            finding_lifecycle.contains(needle),
            "finding_lifecycle should link WorkCycle back to source finding: {needle}"
        );
    }

    let work_cycle_lifecycle = read(patrol.join("wasm/work_cycle_lifecycle/src/lib.rs"));
    for needle in [
        "handle_complete",
        "/tdata/QualityFindings",
        "/tdata/SecurityFindings",
        "TemperPaw.Patrol.Resolve",
        "source_entity_type",
        "source_entity_id",
        "ProofPacket",
    ] {
        assert!(
            work_cycle_lifecycle.contains(needle),
            "work_cycle_lifecycle should resolve source findings with proof: {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in ["Action::\"LinkSource\"", "Action::\"Resolve\""] {
        assert!(
            policy.contains(needle),
            "Cedar should authorize WorkCycle source closure action: {needle}"
        );
    }
}

#[test]
fn daily_brief_renders_visual_human_review_rollup() {
    let root = repo_root();
    let patrol = root.join("os-apps/paw-patrol");

    let manifest = read(patrol.join("app.toml"));
    for needle in [
        "name = \"daily_brief_lifecycle\"",
        "target = \"wasm32-unknown-unknown\"",
        "criticality = \"app-required\"",
        "startup_loading = \"lazy\"",
    ] {
        assert!(
            manifest.contains(needle),
            "app.toml should contain {needle}"
        );
    }

    let spec = read(patrol.join("specs/daily_brief.ioa.toml"));
    for needle in [
        "module = \"daily_brief_lifecycle\"",
        "on_failure = \"Fail\"",
        "name = \"session_id\"",
        "name = \"session_status\"",
        "name = \"work_cycle_id\"",
        "name = \"worker_run_id\"",
        "name = \"AttachSession\"",
        "name = \"AttachWorkerRun\"",
        "name = \"Render\"",
        "visual_summary_url",
    ] {
        assert!(
            spec.contains(needle),
            "DailyBrief spec should contain {needle}"
        );
    }
    assert_action_effects(&spec, "DailyBrief", "Start", &[], &["daily_brief_started"]);

    let wasm_root = patrol.join("wasm/daily_brief_lifecycle");
    assert!(
        wasm_root.join("Cargo.toml").is_file(),
        "daily_brief_lifecycle should have a standalone WASM Cargo manifest"
    );
    assert!(
        wasm_root.join("src/lib.rs").is_file(),
        "daily_brief_lifecycle should have a WASM entry point"
    );

    let lifecycle = read(wasm_root.join("src/lib.rs"));
    for needle in [
        "/tdata/ProofPackets",
        "/tdata/QualityFindings",
        "/tdata/SecurityFindings",
        "/tdata/WorkCycles",
        "/tdata/WorkerRuns",
        "/tdata/Sessions",
        "TemperPaw.Configure",
        "TemperPaw.Patrol.AttachSession",
        "TemperPaw.Patrol.AttachWorkerRun",
        "runner_kind\": \"local_codex",
        "DailyBrief:",
        "TemperPaw.Patrol.Render",
        "agent-driven DailyBrief Session",
        "visual_daily_brief_svg",
        "human-readable daily brief",
        "open risks",
        "done items",
    ] {
        assert!(
            lifecycle.contains(needle),
            "daily_brief_lifecycle should contain {needle}"
        );
    }

    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "daily_brief_lifecycle",
        "DailyBrief",
        "Action::\"Start\"",
        "Action::\"AttachSession\"",
        "Action::\"AttachWorkerRun\"",
        "Action::\"Render\"",
        "Action::\"Publish\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should authorize daily brief lifecycle with {needle}"
        );
    }

    assert!(
        !lifecycle.contains("mock_daily_brief_plan")
            && !lifecycle.contains("deterministic mock brief")
            && !lifecycle.contains("\"mock_plan\""),
        "DailyBrief must be rendered by an intelligent agent path, not a deterministic mock plan"
    );

    let app_doc = read(patrol.join("APP.md"));
    for needle in [
        "agent-driven DailyBrief Session",
        "DailyBrief.Render",
        "visual daily summary",
    ] {
        assert!(
            app_doc.contains(needle),
            "APP.md should explain DailyBrief session synthesis: {needle}"
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

#[test]
fn paw_patrol_wasm_modules_have_startup_build_script() {
    let root = repo_root();
    let build_script = root.join("os-apps/paw-patrol/wasm/build.sh");

    assert!(
        build_script.is_file(),
        "paw-patrol should have a wasm/build.sh so fresh boots can load bundled modules"
    );

    let script = read(build_script);
    for needle in [
        "patrol_request_router",
        "signal_router",
        "worker_run_lifecycle",
        "review_gate_lifecycle",
        "repo_sweep_lifecycle",
        "daily_brief_lifecycle",
    ] {
        assert!(script.contains(needle), "build.sh should contain {needle}");
    }
}

// --- Stage 3 S0: record entities + shadow ingest (ARN-430) ---

#[test]
fn paw_patrol_carries_the_stage3_s0_record_entities_and_ingest_module() {
    let patrol = repo_root().join("os-apps/paw-patrol");

    let ask = read(patrol.join("specs/ask.ioa.toml"));
    for needle in [
        "name = \"RaiseBlocking\"",
        "name = \"RecordFyi\"",
        "params = [\"effort_id\", \"kind\", \"need\", \"options\", \"chose\", \"why\", \"who\"]",
    ] {
        assert!(ask.contains(needle), "ask.ioa.toml should contain {needle}");
    }
    assert_action_effects(&ask, "Ask", "RaiseBlocking", &["stalls = true"], &[]);
    assert!(
        !ask.contains(
            "params = [\"effort_id\", \"kind\", \"need\", \"options\", \"act\", \"stalls\""
        ),
        "Ask.Raise must not take stalls as a param; booleans use set_bool"
    );

    // The three new entities have specs.
    for spec in [
        "ask.ioa.toml",
        "standing_decision.ioa.toml",
        "shadow_verdict.ioa.toml",
    ] {
        assert!(
            patrol.join("specs").join(spec).is_file(),
            "paw-patrol should define specs/{spec}"
        );
    }

    // ReviewRun/ProofPacket keep their existing lifecycle AND gain the record
    // fields and the Recorded/Superseded record sub-lifecycle (additive - S0
    // does not replace the automata; see docs/efforts/ARN-430).
    let review = read(patrol.join("specs/review_run.ioa.toml"));
    for needle in [
        "\"Recorded\"",
        "\"Superseded\"",
        "IngestRecord",
        "RecordPanel",
        "name = \"rubrics\"",
        "name = \"fix_it_failed\"",
        "name = \"risk_flags\"",
        "name = \"RecordPanelFixItFailed\"",
        "name = \"effort_id\"",
        "record_present",
        "RecordedHasRecord",
        "Requested",
        "AttachReviewFile",
    ] {
        assert!(
            review.contains(needle),
            "review_run.ioa.toml should contain {needle}"
        );
    }
    let proof = read(patrol.join("specs/proof_packet.ioa.toml"));
    for needle in [
        "\"Recorded\"",
        "IngestProof",
        "name = \"effort_id\"",
        "changed_surface",
        "independent_verifier",
        "ProofRecorded",
        "Drafting",
        "AttachProofFile",
    ] {
        assert!(
            proof.contains(needle),
            "proof_packet.ioa.toml should contain {needle}"
        );
    }

    // CSDL exposes the new entity types and the new properties.
    let csdl = read(patrol.join("specs/model.csdl.xml"));
    for entity in ["Ask", "StandingDecision", "ShadowVerdict"] {
        assert!(
            csdl.contains(&format!("<EntityType Name=\"{entity}\">")),
            "CSDL should expose {entity}"
        );
        assert!(
            csdl.contains(&format!("<EntitySet Name=\"{entity}s\"")),
            "CSDL should expose the {entity} set"
        );
    }
    for prop in [
        "\"ReviewersRan\"",
        "\"OpenActOnCount\"",
        "\"ChangedSurface\"",
        "\"IndependentVerifier\"",
        "\"RecordPresent\"",
    ] {
        assert!(
            csdl.contains(prop),
            "CSDL should carry the record property {prop}"
        );
    }

    // The ingest module is registered and its parser + committed binary exist.
    let manifest = read(patrol.join("app.toml"));
    assert!(
        manifest.contains("name = \"record_ingest\""),
        "app.toml should register record_ingest"
    );
    assert!(
        read(patrol.join("wasm/build.sh")).contains("record_ingest"),
        "build.sh should build record_ingest"
    );
    assert!(
        patrol
            .join("wasm/record_ingest/record_ingest.wasm")
            .is_file(),
        "record_ingest should have a committed .wasm binary"
    );
    let lib = read(patrol.join("wasm/record_ingest/src/lib.rs"));
    for needle in [
        "fn parse_record",
        "sdlc-review-record-b64",
        "sdlc-proof-record-b64",
        "parse_ok",
    ] {
        assert!(
            lib.contains(needle),
            "record_ingest lib.rs should contain {needle}"
        );
    }

    assert!(
        manifest.contains("name = \"chain_file_ready\""),
        "app.toml should register chain_file_ready"
    );
    assert!(
        read(patrol.join("wasm/build.sh")).contains("chain_file_ready"),
        "build.sh should build chain_file_ready"
    );
    let chain = read(patrol.join("wasm/chain_file_ready/src/lib.rs"));
    for needle in ["fn file_is_ready", "Files('", "Ready", "Locked"] {
        assert!(
            chain.contains(needle),
            "chain_file_ready lib.rs should contain {needle}"
        );
    }
    assert!(
        csdl.contains("<Property Name=\"ArtifactFileReady\""),
        "CSDL should carry ArtifactFileReady"
    );

    // The new entities are governed under the existing Admin permit.
    let policy = read(patrol.join("policies/patrol.cedar"));
    for needle in [
        "resource is Ask",
        "resource is StandingDecision",
        "resource is ShadowVerdict",
        "Action::\"RaiseBlocking\"",
        "Action::\"RecordPanelFixItFailed\"",
    ] {
        assert!(
            policy.contains(needle),
            "patrol.cedar should govern {needle}"
        );
    }
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
