use crate::*;
use serde::Deserialize;
use serde_json::json;

fn resource() -> Value {
    json!({"status":"ApplyConfigurationExecuting", "operation_sequence":2,
        "operation_key":"change-2", "effort_id":"effort-1", "request_revision":"",
        "request_configuration":"{\"replicas\":2}", "proof_ref":"proof-1",
        "config_ref":"target-1", "config_sha256":"a".repeat(64), "execution_attempts":1})
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
