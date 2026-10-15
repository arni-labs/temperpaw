use super::*;
use dsf_resource_common::{Host, Invocation, Request, Response, Runtime};
use serde_json::{Value, json};
use std::collections::VecDeque;
struct Mock {
    replies: VecDeque<Result<Response, Error>>,
    requests: Vec<Request>,
}
impl Host for Mock {
    fn request(&mut self, r: &Request) -> Result<Response, Error> {
        self.requests.push(Request {
            method: r.method,
            url: r.url.clone(),
            headers: r.headers.clone(),
            body: r.body.clone(),
        });
        let mut reply = self.replies.pop_front().expect("unexpected HTTP call")?;
        if reply.body.contains("__PROBE__") {
            let id = self
                .requests
                .iter()
                .flat_map(|request| &request.headers)
                .find(|(name, _)| name == "x-request-id")
                .unwrap()
                .1
                .clone();
            reply.body = reply.body.replace("__PROBE__", &id);
        }
        if reply.body.contains("__HEALTH_URL__") {
            let health = self
                .requests
                .iter()
                .find(|r| r.url.ends_with("/api/health"))
                .unwrap();
            reply.body = reply.body.replace("__HEALTH_URL__", &health.url);
        }
        Ok(reply)
    }
    fn secret(&mut self, _: &str) -> Result<String, Error> {
        Ok("test-secret".into())
    }
}
fn mock(values: Vec<Value>) -> Mock {
    Mock {
        replies: values
            .into_iter()
            .map(|v| {
                Ok(Response {
                    status: 200,
                    body: v.to_string(),
                })
            })
            .collect(),
        requests: vec![],
    }
}
fn rt(h: &mut Mock) -> Runtime<'_, Mock> {
    Runtime {
        host: h,
        base: "https://temper.invalid",
        tenant: "default",
        now_ms: 2000,
    }
}
fn target() -> Target {
    serde_json::from_value(json!({"project_id":"prj-1","account_id":"team-1","project_name":"dsf","git_repository_id":12,"token_secret":"vercel_token","allowed_aliases":["deep-sci-fi.world"]})).unwrap()
}
fn op(change: Value) -> Invocation {
    Invocation::parse("resource-1",&json!({"status":"DeployExecuting","operation_sequence":1,"operation_key":"op-1","effort_id":"effort-1","request_revision":"a".repeat(40),"request_configuration":change.to_string(),"proof_ref":"proof-1","config_ref":"file-1","config_sha256":"b".repeat(64),"execution_attempts":1,"project_id":"prj-1","account_id":"team-1","deployment_target":"production","rollback_execution_id":"dpl-old","alias":"deep-sci-fi.world","provider_execution_id":""})).unwrap()
}
fn project(id: &str) -> Value {
    json!({"id":"prj-1","accountId":"team-1","name":"dsf","link":{"type":"github","repoId":12},"targets":{"production":{"id":id}},"buildCommand":"npm run build"})
}
fn deployment(id: &str) -> Value {
    json!({"id":id,"projectId":"prj-1","target":"production","readyState":"READY","url":"dsf-abc.vercel.app","createdAt":1500,"meta":{"githubCommitSha":"a".repeat(40),"dsfOperationKey":"op-1","dsfOperationSequence":"1"}})
}
fn deploy_change() -> DeployChange {
    serde_json::from_value(
        json!({"target":"production","baseline_deployment_id":"dpl-old","not_before_ms":1000}),
    )
    .unwrap()
}
#[test]
fn deploy_posts_exact_revision_with_key_and_sequence_only_once() {
    let c = deploy_change();
    let i = op(serde_json::to_value(&c).unwrap());
    let mut h = mock(vec![
        project("dpl-old"),
        json!({"deployments":[]}),
        deployment("dpl-new"),
    ]);
    Deploy::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert_eq!(h.requests.len(), 3);
    let r = &h.requests[2];
    assert_eq!(r.method, "POST");
    assert_eq!(
        r.url,
        "https://api.vercel.com/v13/deployments?teamId=team-1"
    );
    let b: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(b["gitSource"]["sha"], "a".repeat(40));
    assert_eq!(b["meta"]["dsfOperationSequence"], "1");
    let mut uncertain = mock(vec![project("dpl-old"), json!({"deployments":[]})]);
    uncertain.replies.push_back(Err(Error::Transport));
    assert!(Deploy::execute(&mut rt(&mut uncertain), &target(), &c, &i).is_err());
    assert_eq!(uncertain.requests.len(), 3);
    let mut retry = i;
    retry.execution_attempts = 2;
    let mut h = mock(vec![project("dpl-old"), json!({"deployments":[]})]);
    assert!(matches!(
        Deploy::execute(&mut rt(&mut h), &target(), &c, &retry),
        Err(Error::Pending(_))
    ));
    assert!(h.requests.iter().all(|r| r.method == "GET"));
}
#[test]
fn rollback_accepts_empty_201_and_observes_actual_production_pointer() {
    let c: RollbackChange = serde_json::from_value(
        json!({"target":"production","deployment_id":"dpl-old","baseline_deployment_id":"dpl-new"}),
    )
    .unwrap();
    let i = op(serde_json::to_value(&c).unwrap());
    let mut h = mock(vec![project("dpl-new"), deployment("dpl-old")]);
    h.replies.push_back(Ok(Response {
        status: 201,
        body: String::new(),
    }));
    Rollback::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert_eq!(
        h.requests[2].url,
        "https://api.vercel.com/v1/projects/prj-1/rollback/dpl-old?teamId=team-1"
    );
    assert!(h.requests[2].body.is_empty());
    let mut h = mock(vec![
        project("dpl-old"),
        deployment("dpl-old"),
        json!({"alias":"deep-sci-fi.world","deploymentId":"dpl-old","projectId":"prj-1"}),
    ]);
    Rollback::observe(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert!(h.requests.iter().all(|r| r.method == "GET"));
    let mut h = mock(vec![project("dpl-new")]);
    assert!(matches!(
        Rollback::observe(&mut rt(&mut h), &target(), &c, &i),
        Err(Error::Pending(_))
    ));
}
#[test]
fn configuration_patch_has_only_declared_project_fields() {
    let c: ConfigurationChange =
        serde_json::from_value(json!({"target":"production","build_command":"npm run build"}))
            .unwrap();
    let i = op(serde_json::to_value(&c).unwrap());
    let mut h = mock(vec![project("dpl-old"), project("dpl-old")]);
    ApplyConfiguration::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert_eq!(h.requests[1].method, "PATCH");
    assert_eq!(
        h.requests[1].url,
        "https://api.vercel.com/v9/projects/prj-1?teamId=team-1"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&h.requests[1].body).unwrap(),
        json!({"buildCommand":"npm run build"})
    );
    assert!(
        serde_json::from_value::<ConfigurationChange>(json!({"target":"production","env":[]}))
            .is_err()
    );
}

fn verification() -> Verification {
    serde_json::from_value(json!({"application":{"kind":"vercel","resource_id":"resource-1","origin":"https://deep-sci-fi.world"},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"frontend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})).unwrap()
}
fn proof_replies(probe: &str) -> Vec<Value> {
    vec![
        json!({"status":"healthy","git_sha":"a".repeat(40)}),
        json!({"data":[{"attributes":{"service":"frontend","env":"production","status":"ok","trace_id":"trace-123","custom":{"git":{"commit":{"sha":"a".repeat(40)}},"dsf":{"request_id":probe},"http":{"status_code":200,"route":"/api/health","url":"__HEALTH_URL__"}}}}]}),
    ]
}
#[test]
fn deployment_adoption_rejects_foreign_project_stale_sequence_and_incomplete_search() {
    let c = deploy_change();
    let i = op(serde_json::to_value(&c).unwrap());
    for (path, value) in [
        ("/projectId", json!("prj-other")),
        ("/meta/dsfOperationSequence", json!("0")),
        ("/meta/githubCommitSha", json!("b".repeat(40))),
    ] {
        let mut row = deployment("dpl-new");
        *row.pointer_mut(path).unwrap() = value;
        let mut h = mock(vec![
            project("dpl-old"),
            json!({"deployments":[{"uid":"dpl-new","meta":{"dsfOperationKey":"op-1","dsfOperationSequence":"1"}}]}),
            row,
        ]);
        assert!(Deploy::execute(&mut rt(&mut h), &target(), &c, &i).is_err());
        assert!(h.requests.iter().all(|r| r.method == "GET"));
    }
    let mut h = mock(vec![
        project("dpl-old"),
        json!({"deployments":[],"pagination":{"next":123}}),
    ]);
    assert!(matches!(
        Deploy::execute(&mut rt(&mut h), &target(), &c, &i),
        Err(Error::Pending(_))
    ));
    let mut foreign = project("dpl-old");
    foreign["accountId"] = json!("team-other");
    let mut h = mock(vec![foreign]);
    assert!(Deploy::execute(&mut rt(&mut h), &target(), &c, &i).is_err());
    assert_eq!(h.requests.len(), 1);
}
#[test]
fn preview_omits_production_target_and_verifies_the_provider_deployment_origin() {
    let c: DeployChange = serde_json::from_value(
        json!({"target":"preview","baseline_deployment_id":"dpl-old","not_before_ms":1000}),
    )
    .unwrap();
    let mut i = op(serde_json::to_value(&c).unwrap());
    i.resource["deployment_target"] = json!("preview");
    let mut row = deployment("dpl-new");
    row["target"] = Value::Null;
    let mut h = mock(vec![
        project("dpl-old"),
        json!({"deployments":[]}),
        row.clone(),
    ]);
    Deploy::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert!(
        serde_json::from_str::<Value>(&h.requests[2].body)
            .unwrap()
            .get("target")
            .is_none()
    );
    i.execution_id = Some("dpl-new".into());
    for probe in ["__PROBE__", "unrelated-request"] {
        let mut replies = vec![project("dpl-old"), row.clone()];
        replies.extend(proof_replies(probe));
        let mut h = mock(replies);
        let result = Deploy::verify(&mut rt(&mut h), &target(), &c, &i, &verification());
        assert_eq!(result.is_ok(), probe == "__PROBE__");
        assert_eq!(h.requests[2].url, "https://dsf-abc.vercel.app/api/health");
        assert!(
            h.requests[2]
                .headers
                .iter()
                .all(|(name, _)| name != "authorization")
        );
    }
    row["url"] = json!("evil.example");
    let mut h = mock(vec![project("dpl-old"), row]);
    assert!(Deploy::verify(&mut rt(&mut h), &target(), &c, &i, &verification()).is_err());
    assert_eq!(h.requests.len(), 2);
}
#[test]
fn selected_alias_binds_both_owners_and_verifies_that_alias() {
    let c:AliasChange=serde_json::from_value(json!({"target":"production","alias":"deep-sci-fi.world","deployment_id":"dpl-new","revision":"a".repeat(40)})).unwrap();
    let mut i = op(serde_json::to_value(&c).unwrap());
    i.resource["provider_execution_id"] = json!("dpl-new");
    SetAlias::validate_change(&target(), &c, &i).unwrap();
    let alias = json!({"alias":"deep-sci-fi.world","deploymentId":"dpl-new","projectId":"prj-1","uid":"alias-1"});
    let mut h = mock(vec![
        project("dpl-new"),
        deployment("dpl-new"),
        json!({"alias":"deep-sci-fi.world","deploymentId":"dpl-old","projectId":"prj-1"}),
        json!({"alias":"deep-sci-fi.world","uid":"alias-1"}),
    ]);
    SetAlias::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert_eq!(
        h.requests[3].url,
        "https://api.vercel.com/v2/deployments/dpl-new/aliases?teamId=team-1"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&h.requests[3].body).unwrap(),
        json!({"alias":"deep-sci-fi.world"})
    );
    let mut replies = vec![project("dpl-new"), deployment("dpl-new"), alias.clone()];
    replies.extend(proof_replies("__PROBE__"));
    let mut h = mock(replies);
    SetAlias::verify(&mut rt(&mut h), &target(), &c, &i, &verification()).unwrap();
    assert_eq!(h.requests[3].url, "https://deep-sci-fi.world/api/health");
    let mut foreign = alias;
    foreign["projectId"] = json!("prj-other");
    let mut h = mock(vec![project("dpl-new"), deployment("dpl-new"), foreign]);
    assert!(SetAlias::execute(&mut rt(&mut h), &target(), &c, &i).is_err());
    assert!(h.requests.iter().all(|r| r.method == "GET"));
    let mut wrong_target = target();
    wrong_target.allowed_aliases = vec!["allowed.example".into()];
    assert!(SetAlias::validate_change(&wrong_target, &c, &i).is_err());
}
#[test]
fn rollback_and_configuration_require_live_health_and_correlated_datadog() {
    let c: RollbackChange = serde_json::from_value(
        json!({"target":"production","deployment_id":"dpl-old","baseline_deployment_id":"dpl-new"}),
    )
    .unwrap();
    let i = op(serde_json::to_value(&c).unwrap());
    let mut replies = vec![
        project("dpl-old"),
        deployment("dpl-old"),
        json!({"alias":"deep-sci-fi.world","deploymentId":"dpl-old","projectId":"prj-1"}),
    ];
    replies.extend(proof_replies("__PROBE__"));
    let mut h = mock(replies);
    Rollback::verify(&mut rt(&mut h), &target(), &c, &i, &verification()).unwrap();
    assert_eq!(h.requests[3].url, "https://deep-sci-fi.world/api/health");
    let c: ConfigurationChange =
        serde_json::from_value(json!({"target":"production","build_command":"npm run build"}))
            .unwrap();
    let i = op(serde_json::to_value(&c).unwrap());
    let mut replies = vec![
        project("dpl-old"),
        deployment("dpl-old"),
        json!({"alias":"deep-sci-fi.world","deploymentId":"dpl-old","projectId":"prj-1"}),
    ];
    replies.extend(proof_replies("__PROBE__"));
    let mut h = mock(replies);
    let evidence =
        ApplyConfiguration::verify(&mut rt(&mut h), &target(), &c, &i, &verification()).unwrap();
    assert_eq!(evidence.observed_configuration, i.configuration);
    let mut malformed = project("dpl-old");
    malformed["buildCommand"] = json!(42);
    let mut h = mock(vec![malformed]);
    assert!(matches!(
        ApplyConfiguration::observe(&mut rt(&mut h), &target(), &c, &i),
        Err(Error::Response(_))
    ));
    let mut different = project("dpl-old");
    different["buildCommand"] = json!("other");
    let mut h = mock(vec![different]);
    assert!(matches!(
        ApplyConfiguration::observe(&mut rt(&mut h), &target(), &c, &i),
        Err(Error::Absent(_))
    ));
}

#[test]
fn production_alias_from_another_project_cannot_verify_a_matching_deployment_revision() {
    let c = deploy_change();
    let mut i = op(serde_json::to_value(&c).unwrap());
    i.execution_id = Some("dpl-new".into());
    let mut replies = vec![
        project("dpl-new"),
        deployment("dpl-new"),
        json!({"alias":"deep-sci-fi.world","projectId":"other-project","deploymentId":"dpl-new"}),
    ];
    replies.extend(proof_replies("__PROBE__"));
    let mut h = mock(replies);
    assert!(matches!(
        Deploy::verify(&mut rt(&mut h), &target(), &c, &i, &verification()),
        Err(Error::Binding(_))
    ));
    assert_eq!(
        h.requests.len(),
        3,
        "unrelated production health cannot verify this project"
    );
}
