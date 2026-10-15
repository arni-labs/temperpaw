//! Computer copy uncertainty under the real actor evaluator; no provider calls.
use serde_json::json;
use std::{fs, path::PathBuf, sync::Arc};
use temper_jit::table::TransitionTable;
use temper_runtime::scheduler::{FaultConfig, SimActorSystem, SimActorSystemConfig};
use temper_server::entity_actor::sim_handler::EntityActorHandler;

fn child(seed: u64) -> SimActorSystem {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-compute/specs/computer.ioa.toml"),
    )
    .unwrap();
    let actor = EntityActorHandler::new(
        "Computer",
        "copy-child",
        Arc::new(TransitionTable::from_ioa_source(&source)),
    )
    .with_ioa_invariants(&source);
    let mut sim = SimActorSystem::new(SimActorSystemConfig {
        seed,
        faults: FaultConfig::none(),
        ..Default::default()
    });
    sim.register_actor("copy", Box::new(actor));
    sim.step("copy","ProvisionFromCopy",&json!({"machine_id":"source-machine","provider":"tensorlake","cpu_cores":"4","memory_gb":"16","storage_gb":"100","base_image":"","project_harness_id":"","description":"local test"}).to_string()).unwrap();
    sim
}

#[test]
fn failed_start_remains_reconcilable_without_source_termination_or_second_start() {
    for seed in 1..=30 {
        let mut sim = child(seed);
        assert!(sim.step("copy", "ReconcileCopy", "{}").is_err());
        sim.step(
            "copy",
            "CopyFailed",
            &json!({"error_message":"response lost"}).to_string(),
        )
        .unwrap();
        sim.assert_status("copy", "CopyUnknown");
        assert!(sim.step("copy", "Destroy", "{}").is_err());
        assert!(sim.step("copy", "ProvisionFromCopy", "{}").is_err());
        let state = sim.step("copy", "ReconcileCopy", "{}").unwrap();
        assert_eq!(state["fields"]["machine_id"], "source-machine");
        sim.assert_status("copy", "CopyUnknown");
        sim.step(
            "copy",
            "CopyFailed",
            &json!({"error_message":"still uncertain"}).to_string(),
        )
        .unwrap();
        sim.assert_status("copy", "CopyUnknown");
        assert!(!sim.has_violations());
    }
}

#[test]
fn copy_callback_must_keep_source_binding_and_use_a_different_destination() {
    for (destination, source) in [
        ("source-machine", "source-machine"),
        ("destination-machine", "other-source"),
        ("destination-machine", ""),
        ("", "source-machine"),
    ] {
        let mut sim = child(467);
        let bad = json!({"machine_id":destination,"source_machine_id":source,"sandbox_url":"https://copy.example","name":"copy-child","copy_deadline_at_ms":"300000"});
        assert!(sim.step("copy", "CopyStarted", &bad.to_string()).is_err());
    }
    let mut sim = child(468);
    let good = json!({"machine_id":"destination-machine","source_machine_id":"source-machine","sandbox_url":"https://copy.example","name":"copy-child","copy_deadline_at_ms":"300000"});
    sim.step(
        "copy",
        "CopyFailed",
        &json!({"error_message":"lost"}).to_string(),
    )
    .unwrap();
    sim.step("copy", "ReconcileCopy", "{}").unwrap();
    let state = sim.step("copy", "CopyStarted", &good.to_string()).unwrap();
    assert_eq!(state["fields"]["machine_id"], "destination-machine");
    assert_eq!(state["fields"]["source_machine_id"], "source-machine");
    sim.assert_status("copy", "Copying");
    assert!(sim.step("copy", "ReconcileCopy", "{}").is_err());
    sim.step("copy", "CopyComplete", "{}").unwrap();
    sim.assert_status("copy", "Leased");
}

#[test]
fn ordinary_computer_agents_can_reconcile_but_cannot_manufacture_copy_callbacks() {
    use std::collections::HashMap;
    use temper_authz::{AuthzDecision, AuthzEngine, SecurityContext};
    let policy = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-compute/policies/compute.cedar"),
    )
    .unwrap();
    let engine = AuthzEngine::new(&policy).unwrap();
    let context = SecurityContext::from_resolved_identity("local-copy-agent", "dsf-factory", None);
    let attributes = HashMap::from([
        ("id".into(), json!("copy-child")),
        ("status".into(), json!("CopyUnknown")),
    ]);
    for action in ["Computer.ReconcileCopy", "ReconcileCopy"] {
        assert!(matches!(
            engine.authorize(&context, action, "Computer", &attributes),
            AuthzDecision::Allow { .. }
        ));
    }
    for action in [
        "Computer.CopyStarted",
        "CopyStarted",
        "Computer.CopyFailed",
        "CopyFailed",
    ] {
        assert!(!matches!(
            engine.authorize(&context, action, "Computer", &attributes),
            AuthzDecision::Allow { .. }
        ));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn packaged_reconciliation_reads_the_recorded_name_and_emits_a_valid_native_callback() {
    use std::collections::BTreeMap;
    use std::sync::RwLock;
    use temper_wasm::{
        SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
    };
    let child_id = "01a0778b-09f1-75e3-84d1-2eda36a64f6b";
    let name = format!("copy-{child_id}-source-machine");
    let context = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "Computer".into(),
        entity_id: child_id.into(),
        trigger_action: "ReconcileCopy".into(),
        wasm_module: Some("computer_copy_start".into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"CopyUnknown", "fields":{"machine_id":"source-machine", "provider":"tensorlake"}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "tensorlake_api_key".into(),
            "local-test-only".into(),
        )]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    // All network responses are local in-memory fixtures. An unexpected POST
    // gets an error; no provider connection or real credential is available.
    let host = SimWasmHost::new().with_default_response(500,"unexpected request")
        .with_response("https://api.tensorlake.ai/sandboxes/source-machine",200,&json!({"id":"source-machine","namespace":"local-project"}).to_string())
        .with_response(&format!("https://api.tensorlake.ai/sandboxes/{name}"),200,&json!({"id":"destination-machine","namespace":"local-project","name":name,"status":"running","sandbox_url":"https://destination-machine.sandbox.tensorlake.ai"}).to_string());
    let engine = WasmEngine::new().unwrap();
    let bytes = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-compute/wasm/computer_copy_start/computer_copy_start.wasm"),
    )
    .unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let result = engine
        .invoke(
            &hash,
            &context,
            Arc::new(host),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert!(result.success, "{result:?}");
    assert_eq!(result.callback_action, "CopyStarted");
    assert_eq!(result.callback_params["name"], name);
    assert_eq!(
        result.callback_params["source_machine_id"],
        "source-machine"
    );
    assert_eq!(result.callback_params["machine_id"], "destination-machine");
    let mut sim = child(467);
    sim.step(
        "copy",
        "CopyFailed",
        &json!({"error_message":"response lost"}).to_string(),
    )
    .unwrap();
    sim.step("copy", "ReconcileCopy", "{}").unwrap();
    sim.step(
        "copy",
        &result.callback_action,
        &result.callback_params.to_string(),
    )
    .unwrap();
    sim.assert_status("copy", "Copying");
}
