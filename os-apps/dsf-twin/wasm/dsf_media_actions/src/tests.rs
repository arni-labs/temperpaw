use super::*;
use dsf_resource_common::{Host, Invocation, Request, Response, Runtime};
use serde_json::{Value, json};
use std::collections::VecDeque;
const JOB1: &str = "10000000-0000-4000-8000-000000000001";
const JOB2: &str = "10000000-0000-4000-8000-000000000002";
const TARGET: &str = "20000000-0000-4000-8000-000000000001";
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
        let mut reply = self.replies.pop_front().expect("unexpected request")?;
        if reply.body.contains("__PROBE__") {
            let request_id = self
                .requests
                .iter()
                .flat_map(|request| &request.headers)
                .find(|(name, _)| name == "x-request-id")
                .unwrap()
                .1
                .clone();
            reply.body = reply.body.replace("__PROBE__", &request_id);
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
            .map(|body| {
                Ok(Response {
                    status: 200,
                    body: body.to_string(),
                })
            })
            .collect(),
        requests: vec![],
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
fn target() -> Target {
    serde_json::from_value(json!({"application_id":"dsf","environment_id":"production","api_resource_id":"railway-1","bucket_resource_id":"r2-1","token_secret":"dsf_admin"})).unwrap()
}
fn change(two: bool) -> Change {
    let mut jobs = vec![
        json!({"id":JOB1,"target_type":"story","target_id":TARGET,"media_type":"cover_image","max_cost_cents":2}),
    ];
    if two {
        jobs.push(json!({"id":JOB2,"target_type":"story","target_id":TARGET,"media_type":"cover_image","max_cost_cents":2}));
    }
    serde_json::from_value(
        json!({"generations":jobs,"max_cost_cents":4,"cost_authority_ref":"ask-1"}),
    )
    .unwrap()
}
fn op(change: &Change) -> Invocation {
    Invocation::parse("pipeline-1",&json!({"status":"RetrySelectedExecuting","operation_sequence":1,"operation_key":"op-1","effort_id":"effort-1","request_revision":"a".repeat(40),"request_configuration":serde_json::to_string(change).unwrap(),"proof_ref":"proof-1","config_ref":"file-1","config_sha256":"b".repeat(64),"execution_attempts":1,"application_id":"dsf","environment_id":"production","api_resource_id":"railway-1","bucket_resource_id":"r2-1","selected_generation_ids":serde_json::to_string(&change.generations.iter().map(|g|g.id).collect::<Vec<_>>()).unwrap(),"cost_authority_ref":"ask-1"})).unwrap()
}
fn status(id: &str, attempt: &str, state: &str) -> Value {
    json!({"generation_id":id,"target_type":"story","target_id":TARGET,"media_type":"cover_image","attempt_id":attempt,"status":state,"media_url":format!("https://media.deep-sci-fi.world/media/story/{TARGET}/cover_image/{id}/{attempt}.png"),"cost_usd":0.02})
}
fn response(i: &Invocation, outcomes: &[(&str, &str)]) -> Value {
    json!({"operation_id":operation_id(i).to_string(),"replayed":false,"queued":outcomes.iter().filter(|(_,outcome)|*outcome=="claimed").count(),"generations":outcomes.iter().map(|(id,outcome)|json!({"generation_id":id,"outcome":outcome})).collect::<Vec<_>>()})
}
fn receipt(i: &Invocation, outcomes: &[(&str, &str)]) -> Value {
    json!({"operation_id":operation_id(i).to_string(),"generation_ids":outcomes.iter().map(|(id,_)|id).collect::<Vec<_>>(),"endpoint":"/api/media/retry-stuck","response":response(i,outcomes)})
}
#[test]
fn provider_identity_is_stable_for_replay_but_changes_with_resource_or_sequence() {
    let c = change(false);
    let i = op(&c);
    assert_eq!(operation_id(&i), operation_id(&i));
    let mut newer = i.clone();
    newer.sequence += 1;
    assert_ne!(operation_id(&i), operation_id(&newer));
    newer = i.clone();
    newer.resource_id = "pipeline-2".into();
    assert_ne!(operation_id(&i), operation_id(&newer));
}
#[test]
fn selected_retry_reads_receipt_first_and_posts_only_exact_jobs() {
    let c = change(false);
    let i = op(&c);
    let mut h = mock(vec![
        json!({"status":"healthy","git_sha":"a".repeat(40)}),
        status(JOB1, "00000000-0000-4000-8000-000000000000", "failed"),
        response(&i, &[(JOB1, "claimed")]),
    ]);
    h.replies.push_front(Ok(Response {
        status: 404,
        body: "{}".into(),
    }));
    insert_links(&mut h, 3);
    RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert!(h.requests[0].url.contains("/recovery-operations/"));
    let body: Value = serde_json::from_str(&h.requests[10].body).unwrap();
    assert_eq!(body["generation_ids"], json!([JOB1]));
    assert_eq!(body["operation_id"], operation_id(&i).to_string());
    assert_eq!(
        h.requests[10].url,
        format!("{DSF_API}/api/media/retry-stuck")
    );
}
#[test]
fn partial_receipt_keeps_ownership_until_every_claimed_job_is_terminal() {
    let c = change(true);
    let i = op(&c);
    let id = operation_id(&i).to_string();
    for state in ["generating", "completed"] {
        let mut h = mock(vec![
            receipt(&i, &[(JOB1, "claimed"), (JOB2, "ineligible")]),
            status(JOB1, &id, state),
        ]);
        let result = RetrySelected::verify(&mut rt(&mut h), &target(), &c, &i, &verification());
        if state == "generating" {
            assert!(matches!(result, Err(Error::Pending(_))));
        } else {
            assert!(matches!(result, Err(Error::ProviderFailed(_))));
        }
    }
}
fn verification() -> Verification {
    serde_json::from_value(json!({"application":{"kind":"railway","resource_id":"railway-1","origin":"https://api.deep-sci-fi.world"},"flow":{"kind":"media"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})).unwrap()
}

#[test]
fn malformed_or_partial_receipts_never_schedule_unrecorded_work() {
    let c = change(true);
    let i = op(&c);
    let good = receipt(&i, &[(JOB1, "claimed"), (JOB2, "ineligible")]);
    for invalid in [
        {
            let mut v = good.clone();
            v["response"]["queued"] = json!(2);
            v
        },
        {
            let mut v = good.clone();
            v["response"]["generations"][1]["generation_id"] = json!(JOB1);
            v
        },
        {
            let mut v = good.clone();
            v["generation_ids"] = json!([JOB1, JOB1]);
            v
        },
        {
            let mut v = good.clone();
            v["response"]["replayed"] = json!(true);
            v
        },
        {
            let mut v = good.clone();
            v["operation_id"] = json!("00000000-0000-4000-8000-000000000000");
            v
        },
    ] {
        let mut h = mock(vec![invalid]);
        assert!(RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i).is_err());
        assert_eq!(h.requests.len(), 1);
    }
    let mut h = mock(vec![good]);
    RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
    assert_eq!(h.requests.len(), 1);
}
#[test]
fn a_failed_job_does_not_release_another_claimed_job_that_is_running() {
    let c = change(true);
    let i = op(&c);
    let attempt = operation_id(&i).to_string();
    let mut h = mock(vec![
        receipt(&i, &[(JOB1, "claimed"), (JOB2, "claimed")]),
        status(JOB1, &attempt, "failed"),
        status(JOB2, &attempt, "generating"),
    ]);
    assert!(matches!(
        RetrySelected::verify(&mut rt(&mut h), &target(), &c, &i, &verification()),
        Err(Error::Pending(_))
    ));
    assert_eq!(h.requests.len(), 3);
}
#[test]
fn missing_receipt_with_an_existing_attempt_does_not_repeat_the_repair() {
    let c = change(false);
    let i = op(&c);
    let mut h = mock(vec![
        json!({"status":"healthy","git_sha":"a".repeat(40)}),
        status(JOB1, &operation_id(&i).to_string(), "generating"),
    ]);
    h.replies.push_front(Ok(Response {
        status: 404,
        body: "{}".into(),
    }));
    assert!(matches!(
        RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i),
        Err(Error::Pending(_))
    ));
    assert!(h.requests.iter().all(|request| request.method == "GET"));
}
#[test]
fn verification_reads_attempt_artifact_price_health_and_matching_datadog() {
    let c = change(false);
    let i = op(&c);
    let attempt = operation_id(&i).to_string();
    for matched in [false, true] {
        let mut h = mock(vec![
            receipt(&i, &[(JOB1, "claimed")]),
            status(JOB1, &attempt, "completed"),
            json!({}),
            json!({"status":"healthy","git_sha":"a".repeat(40)}),
            json!({"data":[{"attributes":{"service":"backend","env":"production","status":"ok","trace_id":"trace-123","custom":{"http":{"status_code":200,"route":"/api/health","url":"https://api.deep-sci-fi.world/api/health"},"git":{"commit":{"sha":"a".repeat(40)}},"dsf":{"request_id":if matched{"__PROBE__"}else{"other-operation"}}}}}]}),
        ]);
        insert_application_reads(&mut h, 3);
        let result = RetrySelected::verify(&mut rt(&mut h), &target(), &c, &i, &verification());
        assert_eq!(result.is_ok(), matched);
        assert_eq!(h.requests.len(), 8);
        assert_eq!(h.requests[2].method, "HEAD");
        assert!(h.requests[2].headers.is_empty());
    }
    let mut wrong = status(JOB1, &attempt, "completed");
    wrong["media_url"] = json!(format!(
        "https://media.deep-sci-fi.world/media/story/{TARGET}/cover_image/{JOB2}/{attempt}.png"
    ));
    let mut h = mock(vec![receipt(&i, &[(JOB1, "claimed")]), wrong]);
    assert!(RetrySelected::verify(&mut rt(&mut h), &target(), &c, &i, &verification()).is_err());
    assert_eq!(h.requests.len(), 2);
}
#[test]
fn selection_and_video_price_bounds_are_checked_before_the_write() {
    let mut c = change(false);
    let mut i = op(&c);
    i.resource["selected_generation_ids"] = json!(format!("[\"{JOB2}\"]"));
    assert!(RetrySelected::validate_change(&target(), &c, &i).is_err());
    c.generations[0].media_type = MediaType::Video;
    c.generations[0].max_cost_cents = 50;
    c.max_cost_cents = 50;
    let i = op(&c);
    for seconds in [4.0, 15.5, 15.0] {
        let mut job = status(JOB1, "00000000-0000-4000-8000-000000000000", "failed");
        job["media_type"] = json!("video");
        job["duration_seconds"] = json!(seconds);
        let mut h = mock(vec![
            json!({"status":"healthy","git_sha":"a".repeat(40)}),
            job,
        ]);
        h.replies.push_front(Ok(Response {
            status: 404,
            body: "{}".into(),
        }));
        assert!(RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i).is_err());
        assert!(h.requests.iter().all(|r| r.method == "GET"));
    }
}

fn config() -> ResourceConfig<Target> {
    ResourceConfig {
        version: 2,
        resource_id: "pipeline-1".into(),
        target: target(),
        verification: verification(),
        required_ask_ids: vec!["ask-1".into()],
    }
}
#[test]
fn paid_selection_requires_the_exact_answered_ask_and_numeric_ceiling() {
    let c = change(false);
    let i = op(&c);
    let allowed = json!({"effort_id":"effort-1","status":"Answered","who":"human-1","chose":json!({"max_cost_cents":20000,"agent_auth":"subscriptions_only"}).to_string()});
    for invalid in [
        {
            let mut v = allowed.clone();
            v["status"] = json!("Open");
            v
        },
        {
            let mut v = allowed.clone();
            v["effort_id"] = json!("other-effort");
            v
        },
        {
            let mut v = allowed.clone();
            v["chose"] = json!("yes");
            v
        },
        {
            let mut v = allowed.clone();
            v["chose"] =
                json!(json!({"max_cost_cents":1,"agent_auth":"subscriptions_only"}).to_string());
            v
        },
        {
            let mut v = allowed.clone();
            v["chose"] = json!(
                json!({"max_cost_cents":"20000","agent_auth":"subscriptions_only"}).to_string()
            );
            v
        },
        {
            let mut v = allowed.clone();
            v["chose"] = json!(json!({"max_cost_cents":20000,"agent_auth":"paid_api"}).to_string());
            v
        },
    ] {
        let mut h = mock(vec![invalid]);
        assert!(RetrySelected::validate_authority(&mut rt(&mut h), &config(), &c, &i).is_err());
    }
    let mut h = mock(vec![allowed]);
    RetrySelected::validate_authority(&mut rt(&mut h), &config(), &c, &i).unwrap();
    assert_eq!(
        h.requests[0].url,
        "https://temper.invalid/tdata/Asks('ask-1')"
    );
    let mut missing = config();
    missing.required_ask_ids.clear();
    let mut h = mock(vec![]);
    assert!(RetrySelected::validate_authority(&mut rt(&mut h), &missing, &c, &i).is_err());
    assert!(h.requests.is_empty());
}

#[test]
fn valid_video_duration_uses_the_selected_price_ceiling() {
    for seconds in [5, 10, 15] {
        let mut c = change(false);
        c.generations[0].media_type = MediaType::Video;
        c.generations[0].max_cost_cents = seconds * 5;
        c.max_cost_cents = seconds * 5;
        let i = op(&c);
        let mut job = status(JOB1, "00000000-0000-4000-8000-000000000000", "failed");
        job["media_type"] = json!("video");
        job["duration_seconds"] = json!(seconds);
        let mut h = mock(vec![
            json!({"status":"healthy","git_sha":"a".repeat(40)}),
            job,
            response(&i, &[(JOB1, "claimed")]),
        ]);
        h.replies.push_front(Ok(Response {
            status: 404,
            body: "{}".into(),
        }));
        insert_links(&mut h, 3);
        RetrySelected::execute(&mut rt(&mut h), &target(), &c, &i).unwrap();
        assert_eq!(h.requests.last().unwrap().method, "POST");
    }
}

#[test]
fn matching_preview_row_and_config_cannot_use_production_media_api() {
    let mut target = target();
    target.environment_id = "preview".into();
    let c = change(false);
    let mut row = op(&c).resource;
    row["environment_id"] = json!("preview");
    assert!(RetrySelected::validate_target(&target, &row).is_err());
}

fn link_replies() -> Vec<Value> {
    let railway = json!({"project_id":"project-1","service_id":"service-1","environment_id":"env-uuid","token_secret":"railway_token"});
    let bucket = json!({"account_id":"0123456789abcdef0123456789abcdef","bucket_name":"dsf-media","token_secret":"cf_token"});
    let config = |id: &str, target: Value| json!({"version":3,"resource_id":id,"target":target,"verification":json!({"application":{"kind":"unbound"},"flow":{"kind":"media"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}})});
    let api_config = config("railway-1", railway.clone());
    let bucket_config = config("r2-1", bucket.clone());
    let mut api_row = railway;
    api_row["status"] = json!("Active");
    api_row["config_ref"] = json!("api-config");
    api_row["config_sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(api_config.to_string().as_bytes())
    ));
    let mut bucket_row = bucket;
    bucket_row["config_ref"] = json!("bucket-config");
    bucket_row["config_sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(bucket_config.to_string().as_bytes())
    ));
    let mut replies = vec![
        api_row,
        api_config,
        bucket_row,
        bucket_config,
        json!({"data":{"service":{"id":"service-1","projectId":"project-1"},"serviceInstance":{"serviceId":"service-1","environmentId":"env-uuid","domains":{"customDomains":[{"id":"domain-1","domain":"api.deep-sci-fi.world","projectId":"project-1","serviceId":"service-1","environmentId":"env-uuid","deletedAt":null}]}}}}),
        json!({"success":true,"result":{"name":"dsf-media"}}),
        json!({"success":true,"result":{"domains":[{"domain":"media.deep-sci-fi.world","enabled":true,"status":{"ownership":"active","ssl":"active"}}]}}),
    ];
    let mut domain = replies.remove(4);
    domain["data"]["serviceInstance"]["domains"]["serviceDomains"] = json!([]);
    replies.insert(2, domain);
    replies
}

#[test]
fn production_media_links_require_exact_provider_domains_and_config_hashes() {
    let mut host = mock(link_replies());
    links::verify(&mut rt(&mut host), &target()).unwrap();
    assert_eq!(host.requests.len(), 7);
    assert!(host.requests.iter().all(
        |r| r.method == "GET" || (r.method == "POST" && r.url.contains("railway.com/graphql"))
    ));
    for case in 0..7 {
        let mut values = link_replies();
        match case {
            0 => values[0]["config_sha256"] = json!("wrong"),
            1 => values[2]["data"]["service"]["projectId"] = json!("other-project"),
            2 => {
                values[2]["data"]["serviceInstance"]["domains"]["customDomains"][0]["domain"] =
                    json!("preview.example.com")
            }
            3 => {
                values[2]["data"]["serviceInstance"]["domains"]["customDomains"][0]["deletedAt"] =
                    json!("2026-09-01T00:00:00Z")
            }
            4 => values[5]["result"]["name"] = json!("other-bucket"),
            5 => values[6]["result"]["domains"][0]["enabled"] = json!(false),
            _ => values[6]["result"]["domains"][0]["status"]["ownership"] = json!("pending"),
        }
        let mut host = mock(values);
        assert!(
            links::verify(&mut rt(&mut host), &target()).is_err(),
            "case {case}"
        );
    }
}

fn insert_links(host: &mut Mock, index: usize) {
    for (offset, body) in link_replies().into_iter().enumerate() {
        host.replies.insert(
            index + offset,
            Ok(Response {
                status: 200,
                body: body.to_string(),
            }),
        );
    }
}

#[test]
fn wrong_production_resource_link_prevents_the_repair_post() {
    let c = change(false);
    let i = op(&c);
    let mut host = mock(vec![
        json!({"status":"healthy","git_sha":"a".repeat(40)}),
        status(JOB1, "00000000-0000-4000-8000-000000000000", "failed"),
    ]);
    host.replies.push_front(Ok(Response {
        status: 404,
        body: "{}".into(),
    }));
    let mut links = link_replies();
    links[2]["data"]["serviceInstance"]["domains"]["customDomains"] = json!([]);
    for body in links {
        host.replies.push_back(Ok(Response {
            status: 200,
            body: body.to_string(),
        }));
    }
    assert!(RetrySelected::execute(&mut rt(&mut host), &target(), &c, &i).is_err());
    assert!(
        !host
            .requests
            .iter()
            .any(|request| request.url.ends_with("/api/media/retry-stuck"))
    );
}

#[test]
fn maximum_selected_batch_verifies_twenty_artifacts_in_bounded_reads() {
    let mut c = change(false);
    c.generations = (1..=20)
        .map(|number| Generation {
            id: Uuid::from_u128(number),
            target_type: TargetType::Story,
            target_id: TARGET.parse().unwrap(),
            media_type: MediaType::CoverImage,
            max_cost_cents: 2,
        })
        .collect();
    c.max_cost_cents = 40;
    let i = op(&c);
    let attempt = operation_id(&i).to_string();
    let ids: Vec<String> = c.generations.iter().map(|g| g.id.to_string()).collect();
    let outcomes: Vec<(&str, &str)> = ids.iter().map(|id| (id.as_str(), "claimed")).collect();
    let mut replies = vec![receipt(&i, &outcomes)];
    replies.extend(ids.iter().map(|id| status(id, &attempt, "completed")));
    replies.extend((0..20).map(|_| json!({})));
    replies.push(json!({"status":"healthy","git_sha":"a".repeat(40)}));
    replies.push(json!({"data":[{"attributes":{"service":"backend","env":"production","status":"ok","trace_id":"trace-123","custom":{"http":{"status_code":200,"route":"/api/health","url":"https://api.deep-sci-fi.world/api/health"},"git":{"commit":{"sha":"a".repeat(40)}},"dsf":{"request_id":"__PROBE__"}}}}]}));
    let mut host = mock(replies);
    insert_application_reads(&mut host, 41);
    RetrySelected::verify(&mut rt(&mut host), &target(), &c, &i, &verification()).unwrap();
    assert_eq!(host.requests.len(), 46);
    assert_eq!(
        host.requests
            .iter()
            .filter(|request| request.method == "HEAD")
            .count(),
        20
    );
}

fn insert_application_reads(h: &mut Mock, index: usize) {
    let mut application: Value = serde_json::from_str(include_str!(
        "../../dsf_resource_common/tests/fixtures/railway_application.json"
    ))
    .unwrap();
    application["row"]["id"] = json!("railway-1");
    let mut config: Value =
        serde_json::from_str(application["configuration"].as_str().unwrap()).unwrap();
    config["resource_id"] = json!("railway-1");
    let raw = config.to_string();
    application["configuration"] = json!(raw);
    application["row"]["config_sha256"] = json!(format!("{:x}", Sha256::digest(raw.as_bytes())));
    for (offset, key) in ["row", "configuration", "domains"].into_iter().enumerate() {
        let body = application[key]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| application[key].to_string());
        h.replies
            .insert(index + offset, Ok(Response { status: 200, body }));
    }
}
