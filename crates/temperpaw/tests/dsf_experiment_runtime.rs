//! Exact experiment IOA under the production evaluator, with delayed callbacks.
use serde_json::json;
use std::{fs, path::PathBuf, sync::Arc};
use temper_jit::table::TransitionTable;
use temper_runtime::scheduler::{FaultConfig, SimActorSystem, SimActorSystemConfig};
use temper_server::entity_actor::sim_handler::EntityActorHandler;

fn simulator(seed: u64) -> SimActorSystem {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/dsf-twin/specs/experiment.ioa.toml"),
    )
    .unwrap();
    let handler = EntityActorHandler::new(
        "DsfExperiment",
        "variant-a",
        Arc::new(TransitionTable::from_ioa_source(&source)),
    )
    .with_ioa_invariants(&source);
    let mut sim = SimActorSystem::new(SimActorSystemConfig {
        seed,
        faults: FaultConfig::none(),
        ..Default::default()
    });
    sim.register_actor("experiment", Box::new(handler));
    sim.step("experiment", "Configure", &json!({"effort_id":"effort-1", "branch":"codex/arn467-variant-a",
        "source_revision":"a".repeat(40),"computer_id":"arni-big","database_id":"dsf_variant_a",
        "media_bucket":"dsf-variant-a","media_namespace":"experiments/variant-a/","permitted_external_calls":"[]",
        "manifest_ref":"manifest-a","manifest_sha256":"b".repeat(64)}).to_string()).unwrap();
    sim
}

#[test]
fn lost_preparation_and_late_results_cannot_start_another_execution() {
    for seed in 1..=50 {
        let mut sim = simulator(seed);
        sim.step("experiment", "Validate", "{}").unwrap();
        sim.assert_status("experiment", "ValidationPreparing");
        sim.step("experiment", "ValidationPreparationTimedOut", "{}")
            .unwrap();
        let prepared = json!({"expected_sequence":1,"exec_id":"exec-a","command":"safe-runner","phase_deadline_ms":"300000"});
        assert!(
            sim.step("experiment", "ValidationPrepared", &prepared.to_string())
                .is_err()
        );
        sim.step("experiment", "ResumeValidation", "{}").unwrap();
        let state = sim
            .step("experiment", "ValidationPrepared", &prepared.to_string())
            .unwrap();
        assert_eq!(state["fields"]["operation_sequence"], 1);
        sim.step("experiment", "ValidationTimedOut", "{}").unwrap();
        assert!(
            sim.step("experiment", "Cleanup", "{}").is_err(),
            "uncertain work must reconcile first"
        );
        sim.step("experiment", "ResumeValidation", "{}").unwrap();
        let state = sim
            .step("experiment", "ValidationPrepared", &prepared.to_string())
            .unwrap();
        assert_eq!(state["fields"]["exec_id"], "exec-a");
        let success = json!({"expected_sequence":1,"isolation_evidence_ref":"exec-a",
            "production_database_id":"production-db","production_media_bucket":"production-media"});
        sim.step("experiment", "IsolationSucceeded", &success.to_string())
            .unwrap();
        sim.step("experiment", "Run", "{}").unwrap();
        assert!(
            sim.step("experiment", "IsolationSucceeded", &success.to_string())
                .is_err()
        );
        assert!(!sim.has_violations());
    }
}

#[test]
fn a_retried_selection_refuses_the_previous_attempt_s_result() {
    let mut sim = simulator(7);
    sim.step("experiment", "Validate", "{}").unwrap();
    let prepared = json!({"expected_sequence":1,"exec_id":"exec-a","command":"pinned runner","phase_deadline_ms":"300000"});
    sim.step("experiment", "ValidationPrepared", &prepared.to_string())
        .unwrap();
    sim.step("experiment","IsolationSucceeded",&json!({"expected_sequence":1,"production_database_id":"production-db","production_media_bucket":"production-media","isolation_evidence_ref":"exec-a"}).to_string()).unwrap();
    sim.step("experiment", "Run", "{}").unwrap();
    sim.step("experiment","RunPrepared",&json!({"expected_sequence":2,"exec_id":"exec-b","command":"pinned runner","phase_deadline_ms":"300000"}).to_string()).unwrap();
    let compared = sim
        .step(
            "experiment",
            "RunSucceeded",
            &json!({"expected_sequence":2,"result_ref":"result-1","test_evidence_ref":"tests-1"})
                .to_string(),
        )
        .unwrap();
    sim.assert_status("experiment", "Compared");
    // The two Exec phases have already spent the shared fence.
    assert_eq!(compared["fields"]["operation_sequence"], 2);

    let select = json!({"selection_ask_id":"ask-1","delivery_effort_id":"delivery-1"}).to_string();
    let selecting = sim.step("experiment", "Select", &select).unwrap();
    sim.assert_status("experiment", "Selecting");
    assert_eq!(
        selecting["fields"]["selection_sequence"], 1,
        "selection runs on its own fence"
    );

    sim.step(
        "experiment",
        "SelectionFailed",
        &json!({"expected_sequence":1,"error_message":"ask withdrawn"}).to_string(),
    )
    .unwrap();
    sim.assert_status("experiment", "Compared");
    let retried = sim.step("experiment", "Select", &select).unwrap();
    assert_eq!(retried["fields"]["selection_sequence"], 2);

    // The first attempt's late result names the spent fence and must not land.
    assert!(
        sim.step(
            "experiment",
            "SelectionSucceeded",
            &json!({"expected_sequence":1,"selection_evidence_ref":"stale"}).to_string()
        )
        .is_err(),
        "a superseded selection attempt must not promote the experiment"
    );
    sim.assert_status("experiment", "Selecting");
    sim.step(
        "experiment",
        "SelectionSucceeded",
        &json!({"expected_sequence":2,"selection_evidence_ref":"Asks('ask-1') -> Efforts('delivery-1')"}).to_string(),
    )
    .unwrap();
    sim.assert_status("experiment", "Selected");
    assert!(!sim.has_violations());
}

#[test]
fn prepared_starts_native_exec_and_reconciled_does_not_start_again() {
    use temper_server::{registry::SpecRegistry, trigger::sim_dispatcher::SimReactionSystem};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps");
    let ioa = fs::read_to_string(root.join("dsf-twin/specs/experiment.ioa.toml")).unwrap();
    let exec = fs::read_to_string(root.join("paw-compute/specs/exec.ioa.toml")).unwrap();
    let xml = fs::read_to_string(root.join("dsf-twin/specs/model.csdl.xml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("DsfExperiment", ioa.as_str()), ("Exec", exec.as_str())],
    );
    let mut sim = SimReactionSystem::new(
        SimActorSystemConfig {
            seed: 467,
            faults: FaultConfig::none(),
            ..Default::default()
        },
        registry.build_reaction_registry(),
        "default",
    );
    sim.register_entity(
        "experiment",
        "DsfExperiment",
        "variant-a",
        Arc::new(TransitionTable::from_ioa_source(&ioa)),
    );
    sim.register_entity(
        "exec",
        "Exec",
        "exec-a",
        Arc::new(TransitionTable::from_ioa_source(&exec)),
    );
    sim.step("experiment","Configure",&json!({"effort_id":"effort-1","branch":"codex/arn467-variant-a","source_revision":"a".repeat(40),"computer_id":"arni-big","database_id":"dsf_variant_a","media_bucket":"dsf-variant-a","media_namespace":"experiments/variant-a/","permitted_external_calls":"[]","manifest_ref":"file-1","manifest_sha256":"b".repeat(64)}).to_string()).unwrap();
    sim.step("experiment", "Validate", "{}").unwrap();
    let prepared=json!({"expected_sequence":1,"exec_id":"exec-a","command":"pinned runner command","phase_deadline_ms":"300000"}).to_string();
    sim.step("experiment", "ValidationPrepared", &prepared)
        .unwrap();
    sim.assert_status("exec", "Starting");
    assert_eq!(sim.last_results().len(), 1);
    assert!(sim.last_results()[0].success, "{:?}", sim.last_results());
    sim.step("experiment", "ValidationTimedOut", "{}").unwrap();
    sim.step("experiment", "ResumeValidation", "{}").unwrap();
    sim.step("experiment", "ValidationReconciled", &prepared)
        .unwrap();
    sim.assert_status("exec", "Starting");
    assert!(
        sim.last_results().is_empty(),
        "reconciliation must not dispatch Exec.Run twice"
    );
    assert!(!sim.has_violations());
}

#[test]
fn scheduler_faults_cannot_restart_or_rebind_an_accepted_run() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/dsf-twin/specs/experiment.ioa.toml"),
    )
    .unwrap();
    for seed in 1..=32 {
        let handler = EntityActorHandler::new(
            "DsfExperiment",
            "variant-a",
            Arc::new(TransitionTable::from_ioa_source(&source)),
        )
        .with_ioa_invariants(&source);
        let mut sim = SimActorSystem::new(SimActorSystemConfig {
            seed,
            max_ticks: 150,
            max_actions_per_actor: 40,
            faults: FaultConfig::heavy(),
        });
        sim.register_actor("experiment", Box::new(handler));
        sim.step("experiment","Configure",&json!({"effort_id":"effort-1","branch":"codex/arn467-variant-a","source_revision":"a".repeat(40),"computer_id":"arni-big","database_id":"dsf_variant_a","media_bucket":"dsf-variant-a","media_namespace":"experiments/variant-a/","permitted_external_calls":"[]","manifest_ref":"file-1","manifest_sha256":"b".repeat(64)}).to_string()).unwrap();
        sim.step("experiment", "Validate", "{}").unwrap();
        sim.step("experiment","ValidationPrepared",&json!({"expected_sequence":1,"exec_id":"exec-a","command":"pinned runner","phase_deadline_ms":"300000"}).to_string()).unwrap();
        sim.step("experiment","IsolationSucceeded",&json!({"expected_sequence":1,"production_database_id":"production-db","production_media_bucket":"production-media","isolation_evidence_ref":"exec-a"}).to_string()).unwrap();
        sim.step("experiment", "Run", "{}").unwrap();
        let result = sim.run_random();
        assert!(
            result.all_invariants_held,
            "seed={seed}: {:?}",
            sim.violations()
        );
        let events = sim.events_json("experiment");
        assert_eq!(
            events
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["action"] == "Run")
                .count(),
            1,
            "seed={seed}: {events}"
        );
    }
}

#[tokio::test]
async fn preparation_timeout_rearms_on_resume_and_old_generation_is_cancelled() {
    use temper_runtime::{ActorSystem, tenant::TenantId};
    use temper_server::{
        registry::SpecRegistry,
        request_context::AgentContext,
        state::{DispatchCommand, ServerState},
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin/specs");
    // Only elapsed duration changes; the real strict actions and callbacks are unchanged.
    let ioa = fs::read_to_string(root.join("experiment.ioa.toml"))
        .unwrap()
        .replace("after_seconds = 120", "after_seconds = 1");
    let xml = fs::read_to_string(root.join("model.csdl.xml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("DsfExperiment", ioa.as_str())],
    );
    let state = ServerState::from_registry(ActorSystem::new("experiment-timers"), registry);
    let tenant = TenantId::from("default".to_owned());
    let ctx = AgentContext::for_service("experiment-timer-test");
    state
        .get_or_create_tenant_entity(&tenant, "DsfExperiment", "variant-a", json!({}))
        .await
        .unwrap();
    let dispatch = |action, params| {
        state.dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "DsfExperiment",
            entity_id: "variant-a",
            action,
            params,
            agent_ctx: &ctx,
            await_integration: false,
            await_reactions: true,
        })
    };
    assert!(dispatch("Configure",json!({"effort_id":"effort-1","branch":"codex/arn467-variant-a","source_revision":"a".repeat(40),"computer_id":"arni-big","database_id":"dsf_variant_a","media_bucket":"dsf-variant-a","media_namespace":"experiments/variant-a/","permitted_external_calls":"[]","manifest_ref":"file-1","manifest_sha256":"b".repeat(64)})).await.unwrap().success);
    let _ = dispatch("Validate", json!({})).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        dispatch(
            "ValidationUncertain",
            json!({"expected_sequence":1,"error_message":"injected lost response"})
        )
        .await
        .unwrap()
        .success
    );
    let _ = dispatch("ResumeValidation", json!({})).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let before = state
        .get_tenant_entity_state(&tenant, "DsfExperiment", "variant-a")
        .await
        .unwrap();
    assert_eq!(before.state.status, "ValidationPreparing");
    tokio::time::sleep(std::time::Duration::from_millis(650)).await;
    let after = state
        .get_tenant_entity_state(&tenant, "DsfExperiment", "variant-a")
        .await
        .unwrap();
    assert_eq!(after.state.status, "ValidationUnknown");
}
