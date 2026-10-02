//! Contract tests for the corridor world engine (paw-foresight 0.2).
//!
//! See os-apps/paw-foresight/adrs/002-corridor-world-engine.md and
//! docs/corridor-world-engine-rfc.md. These tests pin the entity surface
//! that WASM modules, souls, and Deep Sci-Fi 2.0 depend on.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

fn parse_spec(path: &Path) -> toml::Value {
    read(path)
        .parse::<toml::Value>()
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
}

fn automaton<'a>(spec: &'a toml::Value, path: &Path) -> &'a toml::value::Table {
    spec.get("automaton")
        .and_then(|v| v.as_table())
        .unwrap_or_else(|| panic!("{} missing [automaton]", path.display()))
}

fn states(spec: &toml::Value, path: &Path) -> BTreeSet<String> {
    automaton(spec, path)
        .get("states")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} missing automaton.states", path.display()))
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn terminal_states(spec: &toml::Value, path: &Path) -> BTreeSet<String> {
    automaton(spec, path)
        .get("terminal")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn actions(spec: &toml::Value) -> Vec<&toml::value::Table> {
    spec.get("action")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_table()).collect())
        .unwrap_or_default()
}

fn action<'a>(spec: &'a toml::Value, name: &str, path: &Path) -> &'a toml::value::Table {
    actions(spec)
        .into_iter()
        .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(name))
        .unwrap_or_else(|| panic!("{} missing action {name}", path.display()))
}

fn action_params(action: &toml::value::Table) -> BTreeSet<String> {
    action
        .get("params")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn action_from(action: &toml::value::Table) -> BTreeSet<String> {
    action
        .get("from")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn field_names(spec: &toml::Value) -> BTreeSet<String> {
    spec.get("state")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_table())
                .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn spec_path(file: &str) -> PathBuf {
    repo_root().join(format!("os-apps/paw-foresight/specs/{file}"))
}

const CORRIDOR_SPECS: &[(&str, &str)] = &[
    ("world.ioa.toml", "World"),
    ("event_node.ioa.toml", "EventNode"),
    ("endpoint.ioa.toml", "Endpoint"),
    ("claim.ioa.toml", "Claim"),
    ("path.ioa.toml", "Path"),
    ("artifact.ioa.toml", "Artifact"),
    ("dweller.ioa.toml", "Dweller"),
    ("forecast.ioa.toml", "Forecast"),
    ("hindcast.ioa.toml", "Hindcast"),
    ("lens.ioa.toml", "Lens"),
];

const CORRIDOR_WASM_MODULES: &[&str] = &[
    "seed_world",
    "sample_endpoints",
    "decompose_endpoint",
    "spawn_repairers",
    "spawn_adversaries",
    "aggregate_costs",
    "evidence_ingest",
    "register_forecasts",
    "render_artifacts",
    "consistency_gate",
    "grade_hindcast",
    "animate_dwellers",
    "adjudicate_nodes",
];

#[test]
fn corridor_specs_exist_and_declare_expected_automatons() {
    for (file, expected_name) in CORRIDOR_SPECS {
        let path = spec_path(file);
        assert!(path.is_file(), "missing corridor spec {}", path.display());
        let spec = parse_spec(&path);
        let auto = automaton(&spec, &path);
        let name = auto.get("name").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(name, *expected_name, "{} automaton name", path.display());
        let initial = auto.get("initial").and_then(|v| v.as_str()).unwrap_or("");
        let all_states = states(&spec, &path);
        assert!(
            all_states.contains(initial),
            "{} initial state {initial} not in states",
            path.display()
        );
        // Every action's from/to must reference declared states.
        for act in actions(&spec) {
            for from in action_from(act) {
                assert!(
                    all_states.contains(&from),
                    "{}: action {:?} from undeclared state {from}",
                    path.display(),
                    act.get("name")
                );
            }
            if let Some(to) = act.get("to").and_then(|v| v.as_str()) {
                assert!(
                    all_states.contains(to),
                    "{}: action {:?} targets undeclared state {to}",
                    path.display(),
                    act.get("name")
                );
            }
        }
    }
}

#[test]
fn entity_type_names_are_unique_across_apps() {
    // The tenant CSDL is merged across all installed apps and install_os_app
    // upserts specs by entity-type name: a duplicate name in two apps means
    // whichever installs second silently overwrites the other's automaton.
    //
    // Pre-existing collision allowlist — tracked for removal, do NOT add to it:
    //   WorkCycle: paw-harness vs paw-patrol (predates the corridor engine).
    let allowlist: BTreeSet<&str> = ["WorkCycle"].into_iter().collect();

    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let apps_dir = repo_root().join("os-apps");
    for app in fs::read_dir(&apps_dir).expect("read os-apps") {
        let app = app.expect("os-app entry").path();
        let specs = app.join("specs");
        if !specs.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&specs).expect("read specs dir") {
            let path = entry.expect("spec entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".ioa.toml"))
                .unwrap_or(false)
            {
                continue;
            }
            let spec = parse_spec(&path);
            if let Some(name) = spec
                .get("automaton")
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
            {
                owners
                    .entry(name.to_string())
                    .or_default()
                    .push(path.display().to_string());
            }
        }
    }

    let collisions: Vec<_> = owners
        .iter()
        .filter(|(name, paths)| paths.len() > 1 && !allowlist.contains(name.as_str()))
        .collect();
    assert!(
        collisions.is_empty(),
        "duplicate entity-type names across apps (merged-CSDL overwrite hazard): {collisions:?}"
    );
}

#[test]
fn forecast_is_immutable_once_registered() {
    let path = spec_path("forecast.ioa.toml");
    let spec = parse_spec(&path);

    // The only legal transitions are resolution and scoring. No action may
    // alter the registered question or probability, and nothing leaves Scored.
    let allowed: BTreeSet<&str> = ["Resolve", "Score", "Void"].into_iter().collect();
    for act in actions(&spec) {
        let name = act.get("name").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            allowed.contains(name),
            "Forecast declares unexpected action {name}; registration is immutable"
        );
        let params = action_params(act);
        for frozen in ["probability", "question", "resolve_by"] {
            assert!(
                !params.contains(frozen),
                "Forecast action {name} must not accept {frozen} (immutable after registration)"
            );
        }
        assert!(
            !action_from(act).contains("Scored"),
            "Forecast action {name} transitions out of Scored (must be final)"
        );
    }

    assert!(
        terminal_states(&spec, &path).contains("Scored"),
        "Forecast spec must pin Scored as terminal"
    );
}

#[test]
fn artifact_publication_is_gated() {
    let path = spec_path("artifact.ioa.toml");
    let spec = parse_spec(&path);

    let publish = action(&spec, "Publish", &path);
    assert_eq!(
        action_from(publish),
        ["ConsistencyChecked".to_string()].into_iter().collect(),
        "Publish must only be reachable from ConsistencyChecked"
    );

    let retcon = action(&spec, "Retcon", &path);
    assert!(
        action_params(retcon).contains("retcon_cause_node_id"),
        "Retcon must cite the resolved EventNode that forced it"
    );

    // Drafted content lives in paw-fs, not entity fields (32KB discipline).
    let fields = field_names(&spec);
    assert!(fields.contains("content_file_id"));
    assert!(
        !fields.contains("content"),
        "Artifact must not carry document content inline"
    );
}

#[test]
fn event_node_confirmation_is_peer_gated_and_resolution_is_final() {
    let path = spec_path("event_node.ioa.toml");
    let spec = parse_spec(&path);

    let confirm = action(&spec, "Confirm", &path);
    assert!(
        action_params(confirm).contains("confirmer_agent_id"),
        "Confirm must record the confirming principal for Cedar's author/confirmer separation"
    );

    let fields = field_names(&spec);
    for required in [
        "world_id",
        "statement",
        "layer",
        "probability",
        "provenance",
        "resolve_by",
        "edges",
        "author_agent_id",
    ] {
        assert!(
            fields.contains(required),
            "EventNode missing field {required}"
        );
    }

    let terminal = terminal_states(&spec, &path);
    assert!(
        terminal.contains("Resolved") && terminal.contains("Retired"),
        "EventNode Resolved/Retired must be terminal"
    );
}

#[test]
fn ingest_evidence_carries_the_as_of_date() {
    // WASM has no clock: the evidence-ingest as-of date is data the caller
    // supplies, persisted on the world so registered_at / resolved_at are
    // auditable. Both the IOA action and the CSDL bound action must carry it.
    let path = spec_path("world.ioa.toml");
    let spec = parse_spec(&path);

    let ingest = action(&spec, "IngestEvidence", &path);
    assert!(
        action_params(ingest).contains("last_ingest_date"),
        "World.IngestEvidence must accept last_ingest_date (the caller-supplied as-of date)"
    );
    assert!(
        field_names(&spec).contains("last_ingest_date"),
        "World must declare the last_ingest_date state var"
    );

    let csdl = read(repo_root().join("os-apps/paw-foresight/specs/model.csdl.xml"));
    assert!(
        csdl.contains("<Property Name=\"LastIngestDate\" Type=\"Edm.String\"/>"),
        "model.csdl.xml must serve World.LastIngestDate"
    );
    let ingest_action = csdl
        .split("<Action Name=\"IngestEvidence\"")
        .nth(1)
        .and_then(|rest| rest.split("</Action>").next())
        .unwrap_or("");
    assert!(
        ingest_action.contains("<Parameter Name=\"last_ingest_date\""),
        "CSDL IngestEvidence bound action must declare the last_ingest_date parameter"
    );
}

#[test]
fn corridor_documents_are_paw_fs_refs() {
    for (file, field) in [
        ("endpoint.ioa.toml", "bundle_file_id"),
        ("path.ioa.toml", "repair_log_file_id"),
        ("world.ioa.toml", "graph_snapshot_file_id"),
        ("hindcast.ioa.toml", "frozen_corpus_file_id"),
    ] {
        let path = spec_path(file);
        let fields = field_names(&parse_spec(&path));
        assert!(
            fields.contains(field),
            "{file} must store documents as paw-fs refs ({field})"
        );
    }
}

#[test]
fn path_scoring_is_deterministic_wasm_territory() {
    let path = spec_path("path.ioa.toml");
    let spec = parse_spec(&path);

    // Repair and challenge self-reports carry flags; Score carries the cost
    // computed by aggregate_costs. Sessions never pass a cost of their own.
    let repair = action(&spec, "RepairComplete", &path);
    assert!(action_params(repair).contains("cost_flags"));
    assert!(
        !action_params(repair).contains("repair_cost"),
        "repairers flag costs, they do not compute them"
    );
    // Adversary flags land in a separate field so neither side can overwrite
    // the other (action params write fields directly).
    let challenge = action(&spec, "ChallengeComplete", &path);
    assert!(action_params(challenge).contains("challenge_flags"));
    assert!(!action_params(challenge).contains("cost_flags"));
    let score = action(&spec, "Score", &path);
    assert!(action_params(score).contains("repair_cost"));
}

#[test]
fn endpoint_decomposition_bridges_bundles_to_claims() {
    // ADR-004: SubmitForRepair no longer spawns repairers directly — it
    // spawns the decomposer, which splits the bundle into Claims; each
    // claim bridges independently. Chunked claim fan-out runs through a
    // bounded self-loop, never a Rust loop over sessions.
    let path = spec_path("endpoint.ioa.toml");
    let spec = parse_spec(&path);

    let submit = action(&spec, "SubmitForRepair", &path);
    let trigger_module = submit
        .get("triggers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|t| t.as_table())
        .and_then(|t| t.get("module"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        trigger_module, "decompose_endpoint",
        "SubmitForRepair must trigger the decomposer, not spawn repairers directly"
    );

    let done = action(&spec, "DecompositionComplete", &path);
    assert!(
        action_params(done).contains("claims_json"),
        "the decomposer self-reports the extracted claims"
    );

    let next = action(&spec, "SpawnNextBridge", &path);
    assert!(
        action_params(next).contains("check_count"),
        "chunked bridge fan-out is a bounded self-loop (ADR-0005: no Rust loops over sessions)"
    );
}

#[test]
fn claim_amendment_is_explicit_and_settlement_is_single_writer() {
    // ADR-004 commitment 4: the only way a claim's text changes is the
    // diff-carrying AmendText action; original_text is a frozen field so
    // drift is auditable. Settle carries the costing module's verdict.
    let path = spec_path("claim.ioa.toml");
    let spec = parse_spec(&path);

    let fields = field_names(&spec);
    for required in [
        "original_text",
        "current_text",
        "amendment_log",
        "settled_route_id",
    ] {
        assert!(fields.contains(required), "Claim missing field {required}");
    }

    let amend = action(&spec, "AmendText", &path);
    for p in ["old_text", "new_text", "justification", "agent_id"] {
        assert!(
            action_params(amend).contains(p),
            "AmendText must carry the diff ({p})"
        );
    }

    let settle = action(&spec, "Settle", &path);
    for p in ["settled_route_id", "best_route_cost", "classification"] {
        assert!(action_params(settle).contains(p), "Settle missing {p}");
    }
    // Sessions never settle claims: RouteSettled re-enters costing, and
    // costing alone dispatches Settle. The spec encodes this by routing
    // RouteSettled through the aggregate_costs trigger.
    let route_settled = action(&spec, "RouteSettled", &path);
    let trigger_module = route_settled
        .get("triggers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|t| t.as_table())
        .and_then(|t| t.get("module"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(trigger_module, "aggregate_costs");
}

#[test]
fn csdl_serves_all_corridor_entity_sets() {
    // The OData layer serves entity sets from specs/model.csdl.xml, not from
    // the .ioa.toml automatons — a spec without a CSDL entry registers but
    // never serves (found the hard way during the A1 boot gate).
    let csdl = read(repo_root().join("os-apps/paw-foresight/specs/model.csdl.xml"));
    for set in [
        "Worlds",
        "EventNodes",
        "Endpoints",
        "Claims",
        "Paths",
        "Artifacts",
        "Dwellers",
        "Forecasts",
        "Hindcasts",
        "Lenses",
        // Legacy sets remain served until the ADR-003 removal commit.
        "Projections",
        "ForesightModels",
        "Observations",
        "Directions",
    ] {
        assert!(
            csdl.contains(&format!("EntitySet Name=\"{set}\"")),
            "model.csdl.xml must declare EntitySet {set}"
        );
    }
}

#[test]
fn app_manifest_declares_corridor_wasm_modules() {
    let manifest_path = repo_root().join("os-apps/paw-foresight/app.toml");
    let manifest = parse_spec(&manifest_path);
    let declared: BTreeSet<String> = manifest
        .get("wasm_modules")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_table())
                .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    for module in CORRIDOR_WASM_MODULES {
        assert!(
            declared.contains(*module),
            "app.toml must declare wasm module {module}"
        );
        let crate_dir = repo_root().join(format!(
            "os-apps/paw-foresight/wasm/{}",
            module.replace('_', "-")
        ));
        let alt_dir = repo_root().join(format!("os-apps/paw-foresight/wasm/{module}"));
        assert!(
            crate_dir.is_dir() || alt_dir.is_dir(),
            "wasm crate for {module} must exist"
        );
    }
}

#[test]
fn corridor_wasm_modules_are_packaged_for_core_startup() {
    let root = repo_root();
    let dockerfile = read(root.join("Dockerfile"));
    let ci = read(root.join(".github/workflows/ci.yml"));
    let identity_contract =
        read(root.join("crates/temperpaw/tests/temperpaw_identity_contract.rs"));
    let build_script = read(root.join("os-apps/paw-foresight/wasm/build.sh"));

    assert!(
        dockerfile.contains("cd /app/os-apps/paw-foresight/wasm && bash build.sh"),
        "Dockerfile must build paw-foresight WASM before copying os-apps into the runtime image"
    );
    assert!(
        ci.contains("os-apps/paw-foresight/wasm/build.sh"),
        "CI must build paw-foresight WASM so fresh core startup images do not miss corridor modules"
    );
    assert!(
        identity_contract.contains("\"os-apps/paw-foresight/wasm/build.sh\""),
        "identity contract should keep paw-foresight in the audited WASM build-script set"
    );

    for module in CORRIDOR_WASM_MODULES {
        assert!(
            build_script.contains(module),
            "paw-foresight build.sh should build {module}"
        );
    }

    // Drive packaging: a loop need not spell every output filename literally.
    let output = std::process::Command::new("bash")
        .arg(root.join("scripts/test-wasm-build-artifacts.sh"))
        .output()
        .expect("WASM artifact regression should run");
    assert!(
        output.status.success(),
        "WASM artifact packaging failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn endpoint_sampled_self_heals_so_a_dead_writer_re_spawns() {
    // ADR-0050/ADR-007: the writer phase (Endpoint.Sampled) used to sit in
    // allow_indefinite_states with no state_timeout. A writer session that died
    // — or a server restart that orphaned its in-flight provider call — wedged
    // the endpoint forever, and the diversity-gate barrier never fired because
    // one sibling bundle was never written (observed live, prod world
    // en-019ed392, 2026-06-16). Sampled must self-heal like Path's
    // Solving/Repaired/Challenged do.
    let path = spec_path("endpoint.ioa.toml");
    let spec = parse_spec(&path);

    // Sampled is no longer treated as indefinitely parked.
    let indefinite: BTreeSet<String> = automaton(&spec, &path)
        .get("allow_indefinite_states")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !indefinite.contains("Sampled"),
        "Endpoint.Sampled must not be indefinite — a dead writer would wedge the world"
    );

    // A state_timeout re-drives the writer.
    let timeouts = spec
        .get("state_timeout")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} missing [[state_timeout]]", path.display()));
    let sampled = timeouts
        .iter()
        .filter_map(|v| v.as_table())
        .find(|t| t.get("state").and_then(|v| v.as_str()) == Some("Sampled"))
        .unwrap_or_else(|| panic!("{} missing state_timeout for Sampled", path.display()));
    assert_eq!(
        sampled.get("on_timeout").and_then(|v| v.as_str()),
        Some("ResumeWriter"),
        "Sampled timeout must re-spawn the writer via ResumeWriter"
    );

    // ResumeWriter is a Sampled self-loop (re-spawn the writer, stay in Sampled).
    let resume = action(&spec, "ResumeWriter", &path);
    assert!(
        action_from(resume).contains("Sampled"),
        "ResumeWriter must fire from Sampled"
    );
    assert_eq!(
        resume.get("to").and_then(|v| v.as_str()),
        Some("Sampled"),
        "ResumeWriter is a self-loop"
    );
}

#[test]
fn world_active_self_heals_when_claims_terminal_but_canonical_unset() {
    // Gap #4 (en-019ed392): world aggregate only fired off Bridging claim
    // RouteSettled; operator MarkUnreachable bypassed it. Active must re-run the
    // cascade when canonical_path_id is still empty.
    let path = spec_path("world.ioa.toml");
    let spec = parse_spec(&path);
    let timeouts = spec
        .get("state_timeout")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} missing [[state_timeout]]", path.display()));
    let active = timeouts
        .iter()
        .filter_map(|v| v.as_table())
        .find(|t| t.get("state").and_then(|v| v.as_str()) == Some("Active"))
        .unwrap_or_else(|| panic!("{} missing state_timeout for Active", path.display()));
    assert_eq!(
        active.get("on_timeout").and_then(|v| v.as_str()),
        Some("ResumeWorldCascade"),
        "Active timeout must re-run world aggregate via ResumeWorldCascade"
    );
    let resume = action(&spec, "ResumeWorldCascade", &path);
    assert!(
        action_from(resume).contains("Active"),
        "ResumeWorldCascade must fire from Active"
    );
    assert_eq!(
        resume.get("to").and_then(|v| v.as_str()),
        Some("Active"),
        "ResumeWorldCascade is an Active self-loop"
    );
    let trigger = resume
        .get("triggers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("name").and_then(|v| v.as_str()));
    assert_eq!(
        trigger,
        Some("aggregate_costs_world_cascade"),
        "ResumeWorldCascade must trigger aggregate_costs"
    );
}

#[test]
fn world_seeding_self_heals_when_surveyor_never_reports_seed_complete() {
    // ARN-65 / ARN-64: a surveyor session can complete or stall without
    // dispatching SeedComplete. Seeding must re-enter seed_world instead of
    // parking forever with no active work.
    let path = spec_path("world.ioa.toml");
    let spec = parse_spec(&path);

    let indefinite: BTreeSet<String> = automaton(&spec, &path)
        .get("allow_indefinite_states")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !indefinite.contains("Seeding"),
        "World.Seeding must not be indefinite — a surveyor missing SeedComplete would wedge the world"
    );

    let timeouts = spec
        .get("state_timeout")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} missing [[state_timeout]]", path.display()));
    let seeding = timeouts
        .iter()
        .filter_map(|v| v.as_table())
        .find(|t| t.get("state").and_then(|v| v.as_str()) == Some("Seeding"))
        .unwrap_or_else(|| panic!("{} missing state_timeout for Seeding", path.display()));
    assert_eq!(
        seeding.get("on_timeout").and_then(|v| v.as_str()),
        Some("ResumeSeed"),
        "Seeding timeout must re-spawn the surveyor via ResumeSeed"
    );

    let resume = action(&spec, "ResumeSeed", &path);
    assert!(
        action_from(resume).contains("Seeding"),
        "ResumeSeed must fire from Seeding"
    );
    assert_eq!(
        resume.get("to").and_then(|v| v.as_str()),
        Some("Seeding"),
        "ResumeSeed is a Seeding self-loop"
    );
    let trigger = resume
        .get("triggers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("name").and_then(|v| v.as_str()));
    assert_eq!(
        trigger,
        Some("seed_world"),
        "ResumeSeed must trigger seed_world"
    );

    let csdl = read(repo_root().join("os-apps/paw-foresight/specs/model.csdl.xml"));
    assert!(
        csdl.contains("<Action Name=\"ResumeSeed\" IsBound=\"true\">"),
        "model.csdl.xml must serve World.ResumeSeed"
    );

    let cedar = read(repo_root().join("os-apps/paw-foresight/policies/foresight.cedar"));
    assert!(
        cedar.contains("Action::\"ResumeSeed\""),
        "foresight Cedar policy must permit system-dispatched ResumeSeed"
    );
}

#[test]
fn endpoint_under_repair_self_heals_when_claims_terminal_but_unscored() {
    // Gap #3: Endpoint.UnderRepair with all claims terminal but ScoreComplete
    // never fired (world aggregate missed).
    let path = spec_path("endpoint.ioa.toml");
    let spec = parse_spec(&path);
    let timeouts = spec
        .get("state_timeout")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} missing [[state_timeout]]", path.display()));
    let under_repair = timeouts
        .iter()
        .filter_map(|v| v.as_table())
        .find(|t| t.get("state").and_then(|v| v.as_str()) == Some("UnderRepair"))
        .unwrap_or_else(|| panic!("{} missing state_timeout for UnderRepair", path.display()));
    assert_eq!(
        under_repair.get("on_timeout").and_then(|v| v.as_str()),
        Some("ResumeEndpointScoring"),
        "UnderRepair timeout must re-score via ResumeEndpointScoring"
    );
    let resume = action(&spec, "ResumeEndpointScoring", &path);
    assert!(
        action_from(resume).contains("UnderRepair"),
        "ResumeEndpointScoring must fire from UnderRepair"
    );
    assert_eq!(
        resume.get("to").and_then(|v| v.as_str()),
        Some("UnderRepair"),
        "ResumeEndpointScoring is an UnderRepair self-loop"
    );
    let trigger = resume
        .get("triggers")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("name").and_then(|v| v.as_str()));
    assert_eq!(
        trigger,
        Some("aggregate_costs_endpoint_scoring"),
        "ResumeEndpointScoring must trigger aggregate_costs"
    );
}
