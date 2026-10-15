use super::*;
use std::collections::VecDeque;

struct TestHost {
    responses: VecDeque<Value>,
    requests: Vec<String>,
}
impl Host for TestHost {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        assert_eq!(request.url, API);
        self.requests.push(request.body.clone());
        Ok(Response {
            status: 200,
            body: self
                .responses
                .pop_front()
                .expect("unexpected provider call")
                .to_string(),
        })
    }
    fn secret(&mut self, name: &str) -> Result<String, Error> {
        assert_eq!(name, "railway-test-token");
        Ok("test-only".into())
    }
}
fn target() -> Target {
    Target {
        project_id: "project-1".into(),
        service_id: "service-1".into(),
        environment_id: "production-1".into(),
        token_secret: "railway-test-token".into(),
    }
}
fn invocation(action: &str, configuration: &Value) -> Invocation {
    Invocation::parse("railway-project-service-production",&json!({"status":format!("{action}Executing"),"project_id":"project-1","service_id":"service-1","environment_id":"production-1",
        "operation_key":"operation-1","operation_sequence":1,"effort_id":"effort-1","request_revision":"a".repeat(40),"request_configuration":configuration.to_string(),
        "proof_ref":"proof-1","config_ref":"config-1","config_sha256":"b".repeat(64),"execution_attempts":1,"rollback_execution_id":"previous-deployment"})).unwrap()
}
fn run<R>(host: &mut TestHost, f: impl FnOnce(&mut Runtime<TestHost>) -> R) -> R {
    f(&mut Runtime {
        host,
        base: "https://temper.example",
        tenant: "default",
        now_ms: 100_000,
    })
}
fn instance(latest: &str) -> Value {
    json!({"data":{"service":{"id":"service-1","projectId":"project-1"},"serviceInstance":{"id":"instance-1","serviceId":"service-1","environmentId":"production-1","latestDeployment":{"id":latest},"activeDeployments":[],"numReplicas":1}}})
}
fn no_deployments() -> Value {
    json!({"data":{"deployments":{"edges":[]}}})
}

fn deployment(id: &str, snapshot: &str) -> Value {
    json!({"data":{"deployment":{"id":id,"projectId":"project-1","serviceId":"service-1",
        "environmentId":"production-1","status":"SUCCESS","snapshotId":snapshot,
        "meta":{"commitHash":"a".repeat(40)},"canRollback":true}}})
}

#[test]
fn rollback_adopts_an_already_active_exact_deployment_without_mutation() {
    let change = json!({"baseline_deployment_id":"baseline","deployment_id":"previous-deployment"});
    let op = invocation("Rollback", &change);
    let mut active = instance("previous-deployment");
    active["data"]["serviceInstance"]["activeDeployments"] = json!([{"id":"previous-deployment"}]);
    let mut host = TestHost {
        responses: VecDeque::from([
            deployment("previous-deployment", "snapshot-1"),
            active,
            deployment("previous-deployment", "snapshot-1"),
        ]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert_eq!(
        run(&mut host, |runtime| Rollback::execute(
            runtime,
            &target(),
            &typed,
            &op
        ))
        .unwrap()
        .execution_id,
        "previous-deployment"
    );
    assert!(host.requests.iter().all(|r| !r.contains("mutation")));
}

#[test]
fn rollback_acceptance_requires_actual_active_revision_and_configuration_snapshot() {
    let change = json!({"baseline_deployment_id":"baseline","deployment_id":"previous-deployment"});
    let op = invocation("Rollback", &change);
    let mut active = instance("restored-deployment");
    active["data"]["serviceInstance"]["activeDeployments"] = json!([{"id":"restored-deployment"}]);
    let mut host = TestHost {
        responses: VecDeque::from([
            deployment("previous-deployment", "snapshot-1"),
            instance("baseline"),
            deployment("previous-deployment", "snapshot-1"),
            instance("baseline"),
            json!({"data":{"deploymentRollback":true}}),
            deployment("previous-deployment", "snapshot-1"),
            active,
            deployment("restored-deployment", "snapshot-1"),
        ]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert_eq!(
        run(&mut host, |runtime| Rollback::execute(
            runtime,
            &target(),
            &typed,
            &op
        ))
        .unwrap()
        .execution_id,
        "restored-deployment"
    );
    assert_eq!(
        host.requests
            .iter()
            .filter(|r| r.contains("mutation"))
            .count(),
        1
    );
}

#[test]
fn rollback_rejects_a_deployment_from_another_environment_before_write() {
    let change = json!({"baseline_deployment_id":"baseline","deployment_id":"previous-deployment"});
    let op = invocation("Rollback", &change);
    let mut wrong = deployment("previous-deployment", "snapshot-1");
    wrong["data"]["deployment"]["environmentId"] = "staging".into();
    let mut host = TestHost {
        responses: VecDeque::from([wrong]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert!(matches!(
        run(&mut host, |runtime| Rollback::execute(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Binding(_))
    ));
    assert_eq!(host.requests.len(), 1);
}

#[test]
fn unrelated_docker_history_does_not_prevent_exact_git_deployment() {
    let change = json!({"baseline_deployment_id":"baseline","not_before_ms":90_000});
    let op = invocation("Deploy", &change);
    let mut host = TestHost {
        responses: VecDeque::from([
            json!({"data":{"deployments":{"edges":[{"node":{"id":"old-docker","meta":{}}}]}}}),
            instance("baseline"),
            json!({"data":{"serviceInstanceDeployV2":"new-deployment"}}),
        ]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    let receipt = run(&mut host, |runtime| {
        Deploy::execute(runtime, &target(), &typed, &op)
    })
    .unwrap();
    assert_eq!(receipt.execution_id, "new-deployment");
    let write: Value = serde_json::from_str(host.requests.last().unwrap()).unwrap();
    assert_eq!(write["variables"]["environmentId"], "production-1");
    assert_eq!(write["variables"]["commitSha"], "a".repeat(40));
}

#[test]
fn changed_baseline_refuses_before_provider_write() {
    let change = json!({"baseline_deployment_id":"baseline","not_before_ms":90_000});
    let op = invocation("Deploy", &change);
    let mut host = TestHost {
        responses: VecDeque::from([no_deployments(), instance("external-deploy")]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert!(matches!(
        run(&mut host, |runtime| Deploy::execute(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Binding(_))
    ));
    assert!(host.requests.iter().all(|r| !r.contains("mutation")));
}

#[test]
fn uncertain_deployment_is_observed_without_resending() {
    let change = json!({"baseline_deployment_id":"baseline","not_before_ms":90_000});
    let mut op = invocation("Deploy", &change);
    op.execution_attempts = 2;
    let mut host = TestHost {
        responses: VecDeque::from([no_deployments()]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert!(matches!(
        run(&mut host, |runtime| Deploy::execute(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Pending(_))
    ));
    assert_eq!(host.requests.len(), 1);
    assert!(!host.requests[0].contains("mutation"));
}

#[test]
fn configuration_replay_adopts_exact_provider_read_without_mutation() {
    let change = json!({"numReplicas":1});
    let op = invocation("ApplyConfiguration", &change);
    let mut host = TestHost {
        responses: VecDeque::from([instance("baseline")]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert_eq!(
        run(&mut host, |runtime| ApplyConfiguration::execute(
            runtime,
            &target(),
            &typed,
            &op
        ))
        .unwrap()
        .execution_id,
        "instance-1"
    );
    assert_eq!(host.requests.len(), 1);
}

#[test]
fn configuration_difference_is_reported_as_confirmed_absence() {
    let change = json!({"numReplicas":2});
    let op = invocation("ApplyConfiguration", &change);
    let mut host = TestHost {
        responses: VecDeque::from([instance("baseline")]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert!(matches!(
        run(&mut host, |runtime| ApplyConfiguration::observe(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Absent(_))
    ));
    assert!(!host.requests[0].contains("mutation"));
}

#[test]
fn configuration_refuses_foreign_project_before_mutation() {
    let change = json!({"numReplicas":2});
    let op = invocation("ApplyConfiguration", &change);
    let mut foreign = instance("baseline");
    foreign["data"]["service"]["projectId"] = "other-project".into();
    let mut host = TestHost {
        responses: VecDeque::from([foreign]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    assert!(matches!(
        run(&mut host, |runtime| ApplyConfiguration::execute(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Binding(_))
    ));
    assert_eq!(host.requests.len(), 1);
}

#[test]
fn unknown_configuration_cannot_smuggle_provider_source_or_credentials() {
    for field in [
        "source",
        "registryCredentials",
        "serviceId",
        "environmentId",
        "token_secret",
    ] {
        let raw = json!({field:"forged","numReplicas":1});
        assert!(
            serde_json::from_value::<configuration::Configuration>(raw).is_err(),
            "{field}"
        );
    }
}

#[test]
fn next_deploy_ignores_previous_receipt_and_writes_only_after_discovery() {
    let change = json!({"baseline_deployment_id":"old-deployment","not_before_ms":90_000});
    let mut row = invocation("Deploy", &change).resource;
    row["operation_sequence"] = json!(2);
    row["operation_key"] = json!("operation-2");
    row["provider_execution_id"] = json!("old-deployment");
    row["provider_known"] = json!(false);
    let op = Invocation::parse("railway-project-service-production", &row).unwrap();
    let mut host = TestHost {
        responses: VecDeque::from([
            no_deployments(),
            instance("old-deployment"),
            json!({"data":{"serviceInstanceDeployV2":"new-deployment"}}),
        ]),
        requests: vec![],
    };
    let receipt = run(&mut host, |runtime| {
        Deploy::execute(
            runtime,
            &target(),
            &serde_json::from_value(change).unwrap(),
            &op,
        )
    })
    .unwrap();
    assert_eq!(receipt.execution_id, "new-deployment");
    assert!(host.requests[0].contains("DsfRailwayDeployments"));
    assert_eq!(
        host.requests
            .iter()
            .filter(|r| r.contains("mutation"))
            .count(),
        1
    );
}

#[test]
fn uncertain_next_deploy_discovers_current_receipt_without_reusing_history_or_writing() {
    let change = json!({"baseline_deployment_id":"old-deployment","not_before_ms":90_000});
    let mut row = invocation("Deploy", &change).resource;
    row["status"] = json!("DeployReconciling");
    row["operation_sequence"] = json!(2);
    row["operation_key"] = json!("operation-2");
    row["provider_execution_id"] = json!("old-deployment");
    row["provider_known"] = json!(false);
    let op = Invocation::parse("railway-project-service-production", &row).unwrap();
    let mut node = deployment("new-deployment", "new-snapshot")["data"]["deployment"].clone();
    node["createdAt"] = json!("1970-01-01T00:01:35Z");
    let mut host = TestHost {
        responses: VecDeque::from([json!({"data":{"deployments":{"edges":[{"node":node}]}}})]),
        requests: vec![],
    };
    let typed = serde_json::from_value(change).unwrap();
    let receipt = run(&mut host, |runtime| {
        Deploy::observe(runtime, &target(), &typed, &op)
    })
    .unwrap();
    assert_eq!(receipt.execution_id, "new-deployment");
    assert_eq!(host.requests.len(), 1);
    assert!(!host.requests[0].contains("mutation"));
    host.responses.push_back(no_deployments());
    assert!(matches!(
        run(&mut host, |runtime| Deploy::observe(
            runtime,
            &target(),
            &typed,
            &op
        )),
        Err(Error::Pending(_))
    ));
    assert!(host.requests.iter().all(|r| !r.contains("mutation")));
}
