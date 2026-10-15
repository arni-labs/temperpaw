use crate::*;
use serde::Deserialize;
use serde_json::json;

fn resource() -> Value {
    json!({"status":"ApplyConfigurationExecuting", "operation_sequence":2,
        "operation_key":"change-2", "effort_id":"effort-1", "request_revision":"",
        "request_configuration":"{\"replicas\":2}", "proof_ref":"proof-1",
        "config_ref":"target-1", "config_sha256":"a".repeat(64), "execution_attempts":1})
}

#[test]
fn successive_operations_do_not_adopt_the_previous_provider_receipt() {
    let mut row = resource();
    row["provider_execution_id"] = json!("deployment-first");
    row["provider_known"] = json!(true);
    let first = Invocation::parse("railway-project-service-env", &row).unwrap();
    assert_eq!(first.execution_id.as_deref(), Some("deployment-first"));
    row["operation_sequence"] = json!(3);
    row["operation_key"] = json!("change-3");
    row["provider_known"] = json!(false);
    let second = Invocation::parse("railway-project-service-env", &row).unwrap();
    assert_eq!(second.execution_id, None);
    assert_eq!(second.resource["provider_execution_id"], "deployment-first");
    row["provider_execution_id"] = json!("deployment-second");
    row["provider_known"] = json!(true);
    let observed = Invocation::parse("railway-project-service-env", &row).unwrap();
    assert_eq!(observed.execution_id.as_deref(), Some("deployment-second"));
}

#[test]
fn late_phase_cannot_act_after_resource_advanced_within_same_operation() {
    let mut row = resource();
    let invocation = Invocation::parse("railway-project-service-env", &row).unwrap();
    row["status"] = "ApplyConfigurationVerifying".into();
    assert!(invocation.confirm_current(&row).is_err());
}

#[test]
fn reused_key_does_not_accept_previous_sequence() {
    let mut row = resource();
    let invocation = Invocation::parse("railway-project-service-env", &row).unwrap();
    row["operation_sequence"] = 3.into();
    assert!(invocation.confirm_current(&row).is_err());
}

#[test]
fn changed_target_or_requested_bytes_invalidates_invocation() {
    let row = resource();
    let invocation = Invocation::parse("railway-project-service-env", &row).unwrap();
    for name in [
        "config_sha256",
        "config_ref",
        "request_configuration",
        "request_revision",
        "effort_id",
        "proof_ref",
    ] {
        let mut changed = row.clone();
        changed[name] = "different".into();
        assert!(invocation.confirm_current(&changed).is_err(), "{name}");
    }
    assert!(invocation.confirm_current(&row).is_ok());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    project_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    replicas: u64,
}
struct ApplyConfiguration;
impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = Change;
    const ENTITY_TYPE: &'static str = "DsfRailwayServiceInstance";
    const ENTITY_SET: &'static str = "DsfRailwayServiceInstances";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        if required(resource, "project_id")? != target.project_id {
            return Err(Error::Binding("target changed"));
        }
        Ok(())
    }
    fn validate_change(_: &Target, change: &Change, _: &Invocation) -> Result<(), Error> {
        if change.replicas == 0 {
            return Err(Error::Binding("replicas must be positive"));
        }
        Ok(())
    }
    fn execute(
        _: &mut Runtime<impl Host>,
        _: &Target,
        _: &Change,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        panic!("proof boundary must reject before provider execution")
    }
    fn observe(
        _: &mut Runtime<impl Host>,
        _: &Target,
        _: &Change,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        panic!("not used in authority tests")
    }
    fn verify(
        _: &mut Runtime<impl Host>,
        _: &Target,
        _: &Change,
        _: &Invocation,
        _: &Verification,
    ) -> Result<Evidence, Error> {
        panic!("not used in authority tests")
    }
}

#[test]
fn configuration_proof_binds_exact_action_target_sequence_and_bytes() {
    let invocation = Invocation::parse("railway-project-service-env", &resource()).unwrap();
    let artifact = json!({"resource_change": {
        "resource_id":invocation.resource_id, "entity_type":ApplyConfiguration::ENTITY_TYPE,
        "action":ApplyConfiguration::ACTION, "operation_key":invocation.operation_key,
        "operation_sequence":invocation.sequence, "revision":"", "configuration_sha256":invocation.change_digest()
    }});
    assert!(authority::validate_change_proof::<ApplyConfiguration>(&invocation, &artifact).is_ok());
    for name in [
        "resource_id",
        "entity_type",
        "action",
        "operation_key",
        "operation_sequence",
        "revision",
        "configuration_sha256",
    ] {
        let mut changed = artifact.clone();
        changed["resource_change"][name] = "other".into();
        assert!(
            authority::validate_change_proof::<ApplyConfiguration>(&invocation, &changed).is_err(),
            "{name}"
        );
    }
}

#[test]
fn configuration_proof_uses_real_source_commit_without_inventing_deployed_revision() {
    let invocation = Invocation::parse("railway-project-service-env", &resource()).unwrap();
    let commit = "b".repeat(40);
    let effort = json!({"status":"Merged", "head_sha":commit, "proof_attached":true,
        "e2e_ok":true, "review_passed":true, "evaluation_passed":true,"proof_packet_ids":["proof-1"]});
    let proof = json!({"effort_id":"effort-1", "commit":commit});
    assert!(authority::validate_records(&invocation, &effort, &proof).is_ok());
    let mut different = proof;
    different["commit"] = "c".repeat(40).into();
    assert!(authority::validate_records(&invocation, &effort, &different).is_err());
}

#[test]
fn callback_uses_captured_sequence_without_reading_newer_resource() {
    let invocation = Invocation::parse("railway-project-service-env", &resource()).unwrap();
    let callback = invocation.callback::<ApplyConfiguration>(
        "ExecutionUncertain",
        json!({"error_message":"transport unavailable"}),
    );
    assert_eq!(callback.action, "ApplyConfigurationExecutionUncertain");
    assert_eq!(callback.params["expected_operation_sequence"], 2);
    assert_eq!(callback.params["operation_key"], "change-2");
}

#[test]
fn first_attempt_parses_when_the_kernel_omits_never_incremented_counters() {
    let mut row = resource();
    let fields = row.as_object_mut().expect("resource object");
    fields.remove("execution_attempts");
    let invocation = Invocation::parse("railway-project-service-env", &row).unwrap();
    assert_eq!(invocation.execution_attempts, 0);
    assert_eq!(invocation.sequence, 2);
}

#[test]
fn absent_operation_sequence_reports_that_the_operation_has_not_started() {
    let mut row = resource();
    row.as_object_mut()
        .expect("resource object")
        .remove("operation_sequence");
    assert!(matches!(
        Invocation::parse("railway-project-service-env", &row),
        Err(Error::Binding("operation has not started"))
    ));
}

#[test]
fn counters_present_with_the_wrong_type_remain_binding_failures() {
    for wrong in [json!("2"), json!(-2), json!(2.5), json!(null)] {
        let mut row = resource();
        row["execution_attempts"] = wrong;
        assert!(matches!(
            Invocation::parse("railway-project-service-env", &row),
            Err(Error::Field(name)) if name == "execution_attempts"
        ));
    }
}
