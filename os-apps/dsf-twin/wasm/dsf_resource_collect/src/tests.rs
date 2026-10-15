use super::*;
use dsf_resource_common::{Request, Response};
use std::collections::VecDeque;
struct Mock {
    replies: VecDeque<Response>,
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
        Ok(self.replies.pop_front().expect("unexpected HTTP request"))
    }
    fn secret(&mut self, _: &str) -> Result<String, Error> {
        Ok("test-provider-secret".into())
    }
}
fn rt(host: &mut Mock) -> Runtime<'_, Mock> {
    Runtime {
        host,
        base: "https://temper.invalid",
        tenant: "default",
        now_ms: 2000,
    }
}
fn config() -> Value {
    json!({"version":3,"resource_id":"resource-1","target":{"project_id":"prj-1","account_id":"team-1","project_name":"dsf","git_repository_id":12,"token_secret":"vercel_token"},"verification":{"application":{"kind":"unbound"},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"web","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}}})
}
fn row() -> Value {
    json!({"status":"Refreshing","refresh_sequence":1,"observed_sequence":7,"config_ref":"file-1","config_sha256":format!("{:x}",Sha256::digest(config().to_string().as_bytes())),"project_id":"prj-1","account_id":"team-1"})
}
fn mock(response: Response) -> Mock {
    Mock {
        replies: VecDeque::from([
            Response {
                status: 200,
                body: config().to_string(),
            },
            response,
        ]),
        requests: vec![],
    }
}
#[test]
fn typed_collection_reads_registered_project_and_emits_correlated_evidence() {
    let mut h=mock(Response{status:200,body:json!({"id":"prj-1","accountId":"team-1","name":"dsf","targets":{"production":{"id":"dpl-1","readyState":"READY","meta":{"githubCommitSha":"a".repeat(40)}}},"buildCommand":"npm run build","env":[{"value":"PRIVATE"}]}).to_string()});
    let callback = collect::<Vercel>(&mut rt(&mut h), "resource-1", &row()).unwrap();
    assert_eq!(callback.action, "CollectionMeasured");
    assert_eq!(callback.params["error_message"], "");
    assert_eq!(callback.params["expected_refresh_sequence"], 1);
    assert_eq!(callback.params["collected_expected_resource_sequence"], 7);
    assert!(!callback.params.to_string().contains("PRIVATE"));
    assert_eq!(
        h.requests[0].url,
        "https://temper.invalid/tdata/Files('file-1')/$value"
    );
    assert_eq!(
        h.requests[1].url,
        "https://api.vercel.com/v9/projects/prj-1?teamId=team-1"
    );
}
#[test]
fn denied_or_malformed_provider_read_is_not_absence() {
    for response in [
        Response {
            status: 403,
            body: "PRIVATE".into(),
        },
        Response {
            status: 200,
            body: "{}".into(),
        },
    ] {
        let mut h = mock(response);
        let callback = collect::<Vercel>(&mut rt(&mut h), "resource-1", &row()).unwrap();
        assert_eq!(callback.action, "CollectionInaccessible");
        assert!(!callback.params.to_string().contains("PRIVATE"));
    }
    let mut h = mock(Response {
        status: 404,
        body: "{}".into(),
    });
    assert_eq!(
        collect::<Vercel>(&mut rt(&mut h), "resource-1", &row())
            .unwrap()
            .action,
        "CollectionAbsent"
    );
}
#[test]
fn collection_uses_committed_invocation_without_rereading_lagging_projection() {
    let mut captured = row();
    captured["refresh_sequence"] = json!(9);
    captured["observed_sequence"] = json!(12);
    let mut host = mock(Response {
        status: 404,
        body: "{}".into(),
    });
    let callback = collect::<Vercel>(&mut rt(&mut host), "resource-1", &captured).unwrap();
    assert_eq!(callback.params["expected_refresh_sequence"], 9);
    assert_eq!(callback.params["collected_expected_resource_sequence"], 12);
    assert_eq!(host.requests.len(), 2);
    assert!(
        host.requests
            .iter()
            .all(|request| !request.url.contains("DsfVercelProjects"))
    );
}

#[test]
fn captured_configuration_hash_mismatch_refuses_before_provider_read() {
    let mut captured = row();
    captured["config_sha256"] = json!("wrong");
    let mut host = mock(Response {
        status: 200,
        body: "{}".into(),
    });
    assert!(collect::<Vercel>(&mut rt(&mut host), "resource-1", &captured).is_err());
    assert_eq!(host.requests.len(), 1);
}

fn raw(values: Vec<Value>) -> Mock {
    Mock {
        replies: values
            .into_iter()
            .map(|value| Response {
                status: 200,
                body: value.to_string(),
            })
            .collect(),
        requests: vec![],
    }
}
#[test]
fn railway_reads_only_the_exact_project_service_and_environment() {
    let target = dsf_railway_actions::Target {
        project_id: "project-1".into(),
        service_id: "service-1".into(),
        environment_id: "env-1".into(),
        token_secret: "railway_token".into(),
    };
    let mut value = json!({"data":{"service":{"id":"service-1","projectId":"project-1"},"serviceInstance":{"id":"instance-1","serviceId":"service-1","environmentId":"env-1","latestDeployment":{"id":"deployment-1","status":"SUCCESS","createdAt":"2026-09-06T00:00:00Z","meta":{"commitHash":"a".repeat(40),"secret":"PRIVATE"}},"startCommand":"run","variables":{"SECRET":"PRIVATE"}}}});
    value["data"]["serviceInstance"]["activeDeployments"] =
        json!([value["data"]["serviceInstance"]["latestDeployment"].clone()]);
    let mut h = raw(vec![value.clone()]);
    let facts = Railway::read(&mut rt(&mut h), &target).unwrap();
    assert_eq!(facts.revision, "a".repeat(40));
    assert!(!facts.values.to_string().contains("PRIVATE"));
    let query: Value = serde_json::from_str(&h.requests[0].body).unwrap();
    assert_eq!(query["variables"]["environmentId"], "env-1");
    assert!(!h.requests[0].body.contains("mutation"));
    let mut foreign = value;
    foreign["data"]["service"]["projectId"] = json!("other");
    assert!(Railway::read(&mut rt(&mut raw(vec![foreign])), &target).is_err());
}
#[test]
fn supabase_and_r2_read_configuration_without_recording_credentials() {
    let target = dsf_supabase_actions::Target {
        project_ref: "abcdefghijklmnopqrst".into(),
        token_secret: "supa".into(),
    };
    let mut h = raw(vec![
        json!({"id":"id-1","ref":"abcdefghijklmnopqrst","status":"ACTIVE_HEALTHY","database":{"password":"PRIVATE"}}),
        json!({"max_connections":100,"statement_timeout":"5000","work_mem":"4MB","log_connections":true,"jwt_secret":"PRIVATE"}),
    ]);
    let facts = Supabase::read(&mut rt(&mut h), &target).unwrap();
    assert_eq!(facts.outcome, "ACTIVE_HEALTHY");
    assert!(!facts.values.to_string().contains("PRIVATE"));
    assert!(h.requests[1].url.ends_with("/config/database/postgres"));
    let target = dsf_r2_actions::Target {
        account_id: "a".repeat(32),
        bucket_name: "media".into(),
        token_secret: "cf".into(),
    };
    let mut h = raw(vec![
        json!({"success":true,"result":{"name":"media","location":"enam","secret":"PRIVATE"}}),
        json!({"success":true,"result":{"rules":[]}}),
    ]);
    let facts = R2::read(&mut rt(&mut h), &target).unwrap();
    assert_eq!(facts.values["cors"], json!({"rules":[]}));
    assert!(!facts.values.to_string().contains("PRIVATE"));
    assert!(h.requests.iter().all(|r| r.method == "GET"));
}
#[test]
fn datadog_monitor_reads_real_monitor_identity_and_preserves_no_data_state() {
    let target = dsf_datadog_actions::Target {
        site: "datadoghq.com".into(),
        organization_id: "org-1".into(),
        monitor_id: 123,
        api_key_secret: "dd_api".into(),
        app_key_secret: "dd_app".into(),
    };
    let value = json!({"id":123,"type":"metric alert","query":"avg(last_5m):sum:requests{service:dsf} > 1","overall_state":"No Data","message":"PRIVATE","options":{"notify_no_data":true}});
    let mut h = raw(vec![value.clone()]);
    let facts = Datadog::read(&mut rt(&mut h), &target).unwrap();
    assert!(facts.coverage == Coverage::Measured);
    assert_eq!(facts.outcome, "No Data");
    assert!(facts.revision.is_empty());
    assert!(!facts.values.to_string().contains("PRIVATE"));
    assert_eq!(
        h.requests[0].url,
        "https://api.datadoghq.com/api/v1/monitor/123"
    );
    assert_eq!(h.requests[0].headers.len(), 2);
    let mut wrong = value;
    wrong["id"] = json!(124);
    assert!(Datadog::read(&mut rt(&mut raw(vec![wrong])), &target).is_err());
}
#[test]
fn media_snapshot_distinguishes_zero_work_stale_data_and_unavailable_endpoint() {
    let target = dsf_media_actions::Target {
        application_id: "dsf".into(),
        environment_id: "production".into(),
        api_resource_id: "api-1".into(),
        bucket_resource_id: "r2-1".into(),
        token_secret: "dsf".into(),
    };
    let value = json!({"snapshot_version":1,"participant_limit":1,"job_limit":20,"observed_at":"1970-01-01T00:00:02Z","service":"deep-sci-fi-backend","environment":"production","revision":"a".repeat(40),"media":{"counts":{"pending":0},"oldest_unfinished_at":null,"jobs":[],"has_more":false},"participants":{"private":"PRIVATE"}});
    let mut h = raw(vec![value.clone()]);
    let measured = Media::read(&mut rt(&mut h), &target).unwrap();
    assert!(measured.coverage == Coverage::Measured);
    assert_eq!(measured.values["counts"]["pending"], 0);
    assert!(!measured.values.to_string().contains("PRIVATE"));
    let mut h = raw(vec![value]);
    let mut runtime = rt(&mut h);
    runtime.now_ms = 100000;
    assert!(Media::read(&mut runtime, &target).unwrap().coverage == Coverage::Stale);
    assert!(
        error_fact(Error::Http(404, "DSF snapshot"), Media::NOT_FOUND_IS_ABSENT).coverage
            == Coverage::Inaccessible
    );
}

#[test]
fn railway_running_revision_does_not_adopt_the_latest_queued_deployment() {
    let target = dsf_railway_actions::Target {
        project_id: "project-1".into(),
        service_id: "service-1".into(),
        environment_id: "env-1".into(),
        token_secret: "railway".into(),
    };
    let deployment = |id: &str, state: &str, sha: &str| json!({"id":id,"status":state,"createdAt":"2026-09-06T00:00:00Z","meta":{"commitHash":sha}});
    let value = json!({"data":{"service":{"id":"service-1","projectId":"project-1"},"serviceInstance":{"id":"instance-1","serviceId":"service-1","environmentId":"env-1","latestDeployment":deployment("new","BUILDING",&"b".repeat(40)),"activeDeployments":[deployment("old","SUCCESS",&"a".repeat(40))]}}});
    let facts = Railway::read(&mut rt(&mut raw(vec![value])), &target).unwrap();
    assert_eq!(facts.revision, "a".repeat(40));
    assert_eq!(
        facts.values["latest_deployment"]["revision"],
        "b".repeat(40)
    );
}

#[test]
fn collection_failures_explain_static_bindings_without_exposing_payloads() {
    assert!(
        failure_message(&Error::Binding("configuration hash differs"))
            .contains("configuration hash differs")
    );
    assert!(failure_message(&Error::Http(403, "Temper")).contains("403"));
    for error in [
        Error::Field("PRIVATE".into()),
        Error::Proof("PRIVATE".into()),
    ] {
        assert!(!failure_message(&error).contains("PRIVATE"));
    }
}

#[test]
fn never_incremented_resource_sequence_reads_as_its_declared_initial() {
    let mut captured = row();
    captured
        .as_object_mut()
        .expect("captured row object")
        .remove("observed_sequence");
    let mut h=mock(Response{status:200,body:json!({"id":"prj-1","accountId":"team-1","name":"dsf","targets":{"production":{"id":"dpl-1","readyState":"READY","meta":{"githubCommitSha":"a".repeat(40)}}},"buildCommand":"npm run build"}).to_string()});
    let callback = collect::<Vercel>(&mut rt(&mut h), "resource-1", &captured).unwrap();
    assert_eq!(callback.action, "CollectionMeasured");
    assert_eq!(callback.params["collected_expected_resource_sequence"], 0);
    assert_eq!(callback.params["expected_refresh_sequence"], 1);
}

#[test]
fn present_resource_sequence_of_the_wrong_type_still_refuses_the_collection() {
    for wrong in [json!("7"), json!(-1), json!(1.5), json!(null)] {
        let mut captured = row();
        captured["observed_sequence"] = wrong;
        let mut host = mock(Response {
            status: 404,
            body: "{}".into(),
        });
        assert!(matches!(
            collect::<Vercel>(&mut rt(&mut host), "resource-1", &captured),
            Err(Error::Field(name)) if name == "observed_sequence"
        ));
    }
}

#[test]
fn absent_refresh_sequence_refuses_with_the_not_refreshing_message() {
    let mut captured = row();
    captured
        .as_object_mut()
        .expect("captured row object")
        .remove("refresh_sequence");
    let mut host = mock(Response {
        status: 200,
        body: "{}".into(),
    });
    assert!(matches!(
        collect::<Vercel>(&mut rt(&mut host), "resource-1", &captured),
        Err(Error::Binding("resource is not refreshing"))
    ));
    assert!(host.requests.is_empty());
}
