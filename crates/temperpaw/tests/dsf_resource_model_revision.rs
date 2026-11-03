//! Resource model edits retain provider identity and fence completed evidence.
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};
use temper_jit::table::TransitionTable;
use temper_runtime::scheduler::SimActorHandler;
use temper_server::entity_actor::sim_handler::EntityActorHandler;

fn app() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin")
}

fn registered(name: &str) -> EntityActorHandler {
    let source = fs::read_to_string(app().join(format!("specs/{name}.ioa.toml"))).unwrap();
    let table = Arc::new(TransitionTable::from_ioa_source(&source));
    let mut actor = EntityActorHandler::new("resource", "subject", table);
    actor.init().unwrap();
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(app().join("specs/module-contracts.json")).unwrap(),
    )
    .unwrap();
    let ioa = temper_spec::automaton::parse_automaton(&source).unwrap();
    let mut params = json!({});
    for param in manifest["resources"][&ioa.automaton.name]["register"]
        .as_array()
        .unwrap()
    {
        let key = param["name"].as_str().unwrap();
        params[key] = json!(format!("initial-{key}"));
    }
    step(&mut actor, "Register", params);
    actor
}

fn step(actor: &mut EntityActorHandler, action: &str, params: Value) -> Value {
    actor
        .handle_message(action, &params.to_string())
        .unwrap_or_else(|error| panic!("{action}: {error}"))
}

fn revision(sequence: u64) -> Value {
    json!({"expected_model_sequence":sequence, "name":"Production API",
        "source_repository":"owner/application", "dependency_ids":"[\"database-1\"]",
        "config_ref":"configuration-2", "config_sha256":"a".repeat(64),
        "allowed_operations":"[\"Deploy\"]", "model_provenance_ref":"inspection-2"})
}

#[test]
fn every_resource_can_revise_its_model_without_a_provider_operation() {
    for name in [
        "railway_service_instance",
        "vercel_project",
        "supabase_project",
        "cloudflare_r2_bucket",
        "datadog_monitor",
        "media_pipeline",
    ] {
        let mut actor = registered(name);
        let revised = step(&mut actor, "ReviseModel", revision(0));
        assert_eq!(actor.current_status(), "Active", "{name}");
        assert_eq!(revised["counters"]["model_sequence"], 1);
        assert_eq!(revised["counters"]["operation_sequence"], 0);
        assert_eq!(revised["fields"]["config_ref"], "configuration-2");
        assert_eq!(revised["fields"]["dependency_ids"], "[\"database-1\"]");
        assert!(
            actor.pending_callbacks().is_empty(),
            "{name} performs no provider I/O"
        );
    }
}

#[test]
fn stale_model_edits_identity_changes_and_missing_provenance_are_refused() {
    let mut actor = registered("railway_service_instance");
    step(&mut actor, "ReviseModel", revision(0));
    let before = actor.events_json();
    let mut forgeries = vec![revision(0)];
    for key in [
        "project_id",
        "service_id",
        "environment_id",
        "application_id",
        "environment_name",
        "intended_configuration",
        "operation_verified",
    ] {
        let mut forged = revision(1);
        forged[key] = json!("replacement");
        forgeries.push(forged);
    }
    for key in ["config_ref", "config_sha256", "model_provenance_ref"] {
        let mut missing = revision(1);
        missing[key] = json!("");
        forgeries.push(missing);
    }
    for params in forgeries {
        assert!(
            actor
                .handle_message("ReviseModel", &params.to_string())
                .is_err()
        );
        assert_eq!(actor.events_json(), before);
        assert!(actor.pending_callbacks().is_empty());
    }
    let revised = step(&mut actor, "ReviseModel", revision(1));
    assert_eq!(revised["counters"]["model_sequence"], 2);
    assert_eq!(revised["fields"]["project_id"], "initial-project_id");
    assert_eq!(
        revised["fields"]["intended_configuration"],
        "initial-intended_configuration"
    );
}

fn start_deploy(actor: &mut EntityActorHandler) {
    step(
        actor,
        "Deploy",
        json!({"operation_key":"deploy-1", "expected_operation_sequence":0,
        "effort_id":"effort-1", "request_revision":"revision-1",
        "request_configuration":"{}", "proof_ref":"proof-1"}),
    );
    step(
        actor,
        "DeployValidationSucceeded",
        json!({"operation_key":"deploy-1",
        "expected_operation_sequence":1, "validation_evidence_ref":"validation-1",
        "intended_revision":"revision-1"}),
    );
    step(actor, "DeployExecute", json!({}));
}

#[test]
fn model_cannot_change_during_execution_and_revision_invalidates_old_verification() {
    let mut actor = registered("railway_service_instance");
    start_deploy(&mut actor);
    let before = actor.events_json();
    assert!(
        actor
            .handle_message("ReviseModel", &revision(0).to_string())
            .is_err()
    );
    assert_eq!(actor.events_json(), before);
    assert_eq!(actor.current_status(), "DeployExecuting");
    step(
        &mut actor,
        "DeployExecutionSucceeded",
        json!({"operation_key":"deploy-1",
        "expected_operation_sequence":1, "provider_execution_id":"deployment-1",
        "provider_evidence_ref":"provider-1"}),
    );
    step(&mut actor, "DeployVerify", json!({}));
    let verified = step(
        &mut actor,
        "DeployVerificationSucceeded",
        json!({
        "operation_key":"deploy-1", "expected_operation_sequence":1,
        "verified_resource_id":"subject", "verified_revision":"revision-1",
        "provider_evidence_ref":"provider-1", "flow_evidence_ref":"probe-1",
        "telemetry_evidence_ref":"trace-1"}),
    );
    assert_eq!(verified["fields"]["operation_verified"], true);
    assert_eq!(verified["fields"]["deploy_verified"], true);
    let revised = step(&mut actor, "ReviseModel", revision(0));
    assert_eq!(revised["fields"]["operation_verified"], false);
    assert_eq!(revised["fields"]["deploy_verified"], false);
    assert_eq!(revised["counters"]["operation_sequence"], 1);
    assert_eq!(revised["fields"]["provider_execution_id"], "deployment-1");
    assert_eq!(revised["fields"]["verified_revision"], "revision-1");
    assert!(actor.pending_callbacks().is_empty());
}
