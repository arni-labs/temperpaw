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
    serde_json::from_value(json!({"site":"datadoghq.com","organization_id":"org-1","monitor_id":123,"api_key_secret":"dd_api","app_key_secret":"dd_app"})).unwrap()
}
fn change() -> Change {
    serde_json::from_value(json!({"name":"DSF API errors","query":"avg(last_5m):sum:trace.fastapi.request.errors{service:dsf}.as_count() > 0","tags":["env:production","service:dsf"],"options":{"notify_no_data":true,"no_data_timeframe":10}})).unwrap()
}
fn operation() -> Invocation {
    invocation(
        json!({"site":"datadoghq.com","organization_id":"org-1","monitor_id":"123"}),
        serde_json::to_value(change()).unwrap(),
    )
}
#[test]
fn monitor_write_uses_two_named_credentials_and_exact_numeric_id() {
    let mut h = host(json!({"id":123}));
    ApplyConfiguration::execute(&mut runtime(&mut h), &target(), &change(), &operation()).unwrap();
    assert_eq!(h.requests.len(), 1);
    assert_eq!(h.requests[0].method, "PUT");
    assert_eq!(
        h.requests[0].url,
        "https://api.datadoghq.com/api/v1/monitor/123"
    );
    assert!(
        h.requests[0]
            .headers
            .iter()
            .any(|(name, _)| name == "DD-API-KEY")
    );
    assert!(
        h.requests[0]
            .headers
            .iter()
            .any(|(name, _)| name == "DD-APPLICATION-KEY")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&h.requests[0].body).unwrap(),
        serde_json::to_value(change()).unwrap()
    );
}
#[test]
fn monitor_read_compares_requested_fields_and_refuses_another_monitor() {
    let mut actual = serde_json::to_value(change()).unwrap();
    actual["id"] = json!(123);
    actual["tags"] = json!(["service:dsf", "env:production"]);
    actual["overall_state"] = json!("OK");
    let mut h = host(actual.clone());
    ApplyConfiguration::observe(&mut runtime(&mut h), &target(), &change(), &operation()).unwrap();
    assert_eq!(h.requests[0].method, "GET");
    actual["id"] = json!(999);
    let mut h = host(actual);
    assert!(
        ApplyConfiguration::observe(&mut runtime(&mut h), &target(), &change(), &operation())
            .is_err()
    );
}
#[test]
fn arbitrary_origins_and_unknown_monitor_fields_are_rejected() {
    let mut t = target();
    t.site = "attacker.invalid".into();
    assert!(ApplyConfiguration::validate_target(&t, &operation().resource).is_err());
    assert!(serde_json::from_value::<Change>(json!({"restricted_roles":[]})).is_err());
    let mut row = operation().resource;
    row["organization_id"] = json!("other");
    assert!(ApplyConfiguration::validate_target(&target(), &row).is_err());
}
#[test]
fn malformed_monitor_read_is_not_proof_of_absence() {
    let mut h = host(json!({"id":123,"name":false}));
    assert!(matches!(
        ApplyConfiguration::observe(&mut runtime(&mut h), &target(), &change(), &operation()),
        Err(Error::Response(_))
    ));
}

#[test]
fn verification_requires_provider_read_live_health_and_the_matching_datadog_span() {
    let verification=serde_json::from_value(json!({"application":{"kind":"railway","resource_id":"api-1","origin":"https://api.deep-sci-fi.world"},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})).unwrap();
    for matched in [false, true] {
        let mut h = host({
            let mut value = serde_json::to_value(change()).unwrap();
            value["id"] = json!(123);
            value
        });
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
