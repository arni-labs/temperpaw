use super::*;
use dsf_resource_common::{Error, Host, Invocation, Request, ResourceAction, Response, Runtime};
use serde_json::{Value, json};
use std::collections::VecDeque;

struct MockHost {
    replies: VecDeque<Result<Response, Error>>,
    requests: Vec<Request>,
}
impl Host for MockHost {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        self.requests.push(Request {
            method: request.method,
            url: request.url.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        });
        let mut response = self
            .replies
            .pop_front()
            .expect("unexpected provider call")?;
        if response.body.contains("__PROBE__") {
            let probe = self
                .requests
                .iter()
                .flat_map(|request| &request.headers)
                .find(|(name, _)| name == "x-request-id")
                .expect("health probe precedes Datadog")
                .1
                .clone();
            response.body = response.body.replace("__PROBE__", &probe);
        }
        Ok(response)
    }
    fn secret(&mut self, _: &str) -> Result<String, Error> {
        Ok("test-secret".into())
    }
}
fn host(body: Value) -> MockHost {
    MockHost {
        replies: VecDeque::from([Ok(Response {
            status: 200,
            body: body.to_string(),
        })]),
        requests: vec![],
    }
}
fn invocation(resource: Value, configuration: Value) -> Invocation {
    let mut row = resource;
    row["status"] = json!("ApplyConfigurationExecuting");
    row["operation_sequence"] = json!(1);
    row["operation_key"] = json!("operation-1");
    row["effort_id"] = json!("effort-1");
    row["request_revision"] = json!("");
    row["request_configuration"] = json!(configuration.to_string());
    row["proof_ref"] = json!("proof-1");
    row["config_ref"] = json!("file-1");
    row["config_sha256"] = json!("a".repeat(64));
    row["execution_attempts"] = json!(1);
    Invocation::parse("resource-1", &row).unwrap()
}
fn runtime(host: &mut MockHost) -> Runtime<'_, MockHost> {
    Runtime {
        host,
        base: "https://temper.invalid",
        tenant: "default",
        now_ms: 1000,
    }
}
fn target() -> Target {
    serde_json::from_value(
        json!({"account_id":"a".repeat(32),"bucket_name":"dsf-media","token_secret":"r2_token"}),
    )
    .unwrap()
}
fn policy() -> Value {
    json!({"rules":[{"allowed":{"origins":["https://deep-sci-fi.world"],"methods":["GET","HEAD"]},"maxAgeSeconds":3600}]})
}
fn change() -> Change {
    serde_json::from_value(policy()).unwrap()
}
fn operation() -> Invocation {
    invocation(
        json!({"account_id":"a".repeat(32),"bucket_name":"dsf-media"}),
        policy(),
    )
}
#[test]
fn cors_write_has_fixed_account_bucket_and_documented_body() {
    let mut h = host(json!({"success":true,"result":{}}));
    ApplyConfiguration::execute(&mut runtime(&mut h), &target(), &change(), &operation()).unwrap();
    assert_eq!(h.requests.len(), 1);
    assert_eq!(h.requests[0].method, "PUT");
    assert_eq!(
        h.requests[0].url,
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/dsf-media/cors",
            "a".repeat(32)
        )
    );
    assert_eq!(
        serde_json::from_str::<Value>(&h.requests[0].body).unwrap(),
        policy()
    );
}
#[test]
fn cors_reconciliation_compares_policy_without_rewriting_objects() {
    let mut actual = policy();
    actual["rules"][0]["allowed"]["methods"] = json!(["HEAD", "GET"]);
    let mut h = host(json!({"success":true,"result":actual}));
    ApplyConfiguration::observe(&mut runtime(&mut h), &target(), &change(), &operation()).unwrap();
    assert_eq!(h.requests[0].method, "GET");
    let mut h = host(json!({"success":true,"result":{"rules":[]}}));
    assert!(matches!(
        ApplyConfiguration::observe(&mut runtime(&mut h), &target(), &change(), &operation()),
        Err(Error::Absent(_))
    ));
}
#[test]
fn invalid_cors_or_provider_envelope_cannot_pass() {
    let mut invalid = policy();
    invalid["rules"][0]["allowed"]["origins"] = json!(["https://user:secret@example.com/path"]);
    let change: Change = serde_json::from_value(invalid).unwrap();
    assert!(ApplyConfiguration::validate_change(&target(), &change, &operation()).is_err());
    assert!(serde_json::from_value::<Change>(json!({"rules":[],"delete_objects":true})).is_err());
    let mut h = host(json!({"success":false,"result":policy()}));
    assert!(
        ApplyConfiguration::observe(
            &mut runtime(&mut h),
            &target(),
            &super::tests::change(),
            &operation()
        )
        .is_err()
    );
}

#[test]
fn verification_requires_provider_read_live_health_and_the_matching_datadog_span() {
    let verification=serde_json::from_value(json!({"application":{"kind":"railway","resource_id":"api-1","origin":"https://api.deep-sci-fi.world"},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})).unwrap();
    for matched in [false, true] {
        let mut h = host(json!({"success":true,"result":policy()}));
        let fixture: Value = serde_json::from_str(include_str!(
            "../../dsf_resource_common/tests/fixtures/railway_application.json"
        ))
        .unwrap();
        for key in ["row", "configuration", "domains"] {
            let body = fixture[key]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| fixture[key].to_string());
            h.replies.push_back(Ok(Response { status: 200, body }));
        }
        let sha = "a".repeat(40);
        h.replies.push_back(Ok(Response {
            status: 200,
            body: json!({"status":"healthy","git_sha":sha}).to_string(),
        }));
        h.replies.push_back(Ok(Response{status:200,body:json!({"data":[{"attributes":{"service":"backend","env":"production","status":"ok","trace_id":"trace-123","custom":{"git":{"commit":{"sha":sha}},"dsf":{"request_id":if matched {"__PROBE__"} else {"other-probe"}},"http":{"status_code":200,"route":"/api/health","url":"https://api.deep-sci-fi.world/api/health"}}}}]}).to_string()}));
        let result = ApplyConfiguration::verify(
            &mut runtime(&mut h),
            &target(),
            &change(),
            &operation(),
            &verification,
        );
        assert_eq!(h.requests.len(), 6);
        assert_eq!(h.requests[0].method, "GET");
        assert_eq!(
            h.requests[4].url,
            "https://api.deep-sci-fi.world/api/health"
        );
        assert_eq!(
            h.requests[5].url,
            "https://api.datadoghq.com/api/v2/spans/events/search"
        );
        if matched {
            let evidence = result.unwrap();
            assert_eq!(evidence.observed_configuration, operation().configuration);
            assert_eq!(
                evidence.telemetry_ref,
                "https://app.datadoghq.com/apm/trace/trace-123"
            );
        } else {
            assert!(matches!(result, Err(Error::Pending(_))));
        }
    }
}
