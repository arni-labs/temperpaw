use crate::*;
use serde_json::{Value, json};
use std::collections::VecDeque;

struct Fixture {
    replies: VecDeque<Response>,
    requests: Vec<Request>,
}
impl Host for Fixture {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        self.requests.push(Request {
            method: request.method,
            url: request.url.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        });
        let mut response = self.replies.pop_front().expect("unexpected request");
        if response.body.contains("__PROBE__") {
            let id = &self
                .requests
                .iter()
                .flat_map(|r| &r.headers)
                .find(|(name, _)| name == "x-request-id")
                .unwrap()
                .1;
            response.body = response.body.replace("__PROBE__", id);
        }
        Ok(response)
    }
    fn secret(&mut self, name: &str) -> Result<String, Error> {
        assert_ne!(name, "production-admin");
        Ok("fixture".into())
    }
}
fn fixture(domain: &str, trace_origin: &str) -> Fixture {
    let mut data: Value =
        serde_json::from_str(include_str!("../tests/fixtures/railway_application.json")).unwrap();
    data["domains"]["data"]["serviceInstance"]["domains"]["customDomains"][0]["domain"] =
        json!(domain);
    let mut replies = VecDeque::new();
    for key in ["row", "configuration", "domains"] {
        replies.push_back(Response {
            status: 200,
            body: data[key]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| data[key].to_string()),
        });
    }
    replies.push_back(Response {
        status: 200,
        body: json!({"status":"healthy","git_sha":"a".repeat(40)}).to_string(),
    });
    replies.push_back(Response{status:200,body:json!({"data":[{"attributes":{"service":"backend","env":"production","status":"ok","trace_id":"trace-1","custom":{"git":{"commit":{"sha":"a".repeat(40)}},"dsf":{"request_id":"__PROBE__"},"http":{"status_code":200,"url":format!("{trace_origin}/api/health")}}}}]}).to_string()});
    Fixture {
        replies,
        requests: vec![],
    }
}
fn verification(origin: &str) -> Verification {
    serde_json::from_value(json!({"application":{"kind":"railway","resource_id":"api-1","origin":origin},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})).unwrap()
}
fn invocation() -> Invocation {
    Invocation::parse("api-1",&json!({"status":"DeployVerifying","operation_key":"operation-1","operation_sequence":1,"effort_id":"effort-1","request_revision":"a".repeat(40),"request_configuration":"{}","proof_ref":"proof-1","config_ref":"config-1","config_sha256":"a".repeat(64),"execution_attempts":1})).unwrap()
}
fn runtime(host: &mut Fixture) -> Runtime<'_, Fixture> {
    Runtime {
        host,
        base: "https://temper.invalid",
        tenant: "default",
        now_ms: 2000,
    }
}

#[test]
fn matching_production_revision_cannot_verify_a_staging_origin_or_another_instance() {
    let stage = "https://staging.deep-sci-fi.world";
    let v = verification(stage);
    assert!(v.application.railway(Some("different-resource")).is_err());
    let mut h = fixture("api.deep-sci-fi.world", DSF_API);
    assert!(matches!(
        verify_product(
            &mut runtime(&mut h),
            &v,
            &invocation(),
            stage,
            Some(&"a".repeat(40))
        ),
        Err(Error::Binding(_))
    ));
    assert_eq!(
        h.requests.len(),
        3,
        "production health and trace must not be consulted"
    );
}
#[test]
fn same_revision_and_request_id_still_require_a_trace_from_the_proved_staging_origin() {
    let stage = "https://staging.deep-sci-fi.world";
    for trace in [DSF_API, stage] {
        let mut h = fixture("staging.deep-sci-fi.world", trace);
        let result = verify_product(
            &mut runtime(&mut h),
            &verification(stage),
            &invocation(),
            stage,
            Some(&"a".repeat(40)),
        );
        assert_eq!(result.is_ok(), trace == stage);
        assert_eq!(h.requests[3].url, format!("{stage}/api/health"));
        assert!(
            h.requests[3]
                .headers
                .iter()
                .all(|(name, _)| name != "authorization")
        );
    }
}
#[test]
fn unbound_sources_and_production_admin_credentials_cannot_authorize_staging_operations() {
    let unbound: ApplicationBinding = serde_json::from_value(json!({"kind":"unbound"})).unwrap();
    assert!(unbound.railway(None).is_err());
    assert!(unbound.vercel("project").is_err());
    let stage = "https://staging.deep-sci-fi.world";
    let mut v = verification(stage);
    v.flow = Flow::OperationalSnapshot {
        schema_version: "0033".into(),
        secret_name: "production-admin".into(),
    };
    let mut h = fixture("staging.deep-sci-fi.world", stage);
    assert!(matches!(
        verify_product(&mut runtime(&mut h), &v, &invocation(), stage, None),
        Err(Error::Binding(_))
    ));
    assert_eq!(h.requests.len(), 4);
}

#[test]
fn generated_railway_domain_uses_verified_parent_project_when_domain_project_is_null() {
    for (collection, project, expected) in [
        ("serviceDomains", Value::Null, true),
        ("serviceDomains", json!("foreign-project"), false),
        ("customDomains", Value::Null, false),
    ] {
        let mut h = fixture("stage.up.railway.app", "https://stage.up.railway.app");
        let response = &mut h.replies[2];
        let mut data: Value = serde_json::from_str(&response.body).unwrap();
        let domains = &mut data["data"]["serviceInstance"]["domains"];
        let mut domain = domains["customDomains"][0].clone();
        domain["projectId"] = project;
        domains["customDomains"] = json!([]);
        domains[collection] = json!([domain]);
        response.body = data.to_string();
        let v = verification("https://stage.up.railway.app");
        assert_eq!(
            railway_application_origin(&mut runtime(&mut h), &v.application, Some("api-1")).is_ok(),
            expected
        );
    }
}
