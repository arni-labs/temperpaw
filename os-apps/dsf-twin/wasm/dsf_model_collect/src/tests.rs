use super::*;
use std::collections::VecDeque;

struct FakeHost {
    responses: VecDeque<Response>,
    requests: Vec<Request>,
}
impl FakeHost {
    fn new(bodies: Vec<Value>) -> Self {
        Self {
            responses: bodies
                .into_iter()
                .map(|v| Response {
                    status: 200,
                    body: v.to_string(),
                })
                .collect(),
            requests: Vec::new(),
        }
    }
}
impl Host for FakeHost {
    fn request(&mut self, request: &Request) -> Result<Response, String> {
        self.requests.push(Request {
            method: request.method,
            url: request.url.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        });
        self.responses
            .pop_front()
            .ok_or("unexpected HTTP call".into())
    }
    fn secret(&mut self, _: &str) -> Result<String, String> {
        Ok("DO_NOT_RECORD_THIS_TOKEN".into())
    }
}
fn config(source: Value, id: &str) -> Config {
    serde_json::from_value(config_json(source, id)).unwrap()
}
fn config_json(source: Value, id: &str) -> Value {
    json!({"subject_type":"DsfFlow","subject_id":"subject-1","provider_id":id,"secret_name":"provider_token","interval_seconds":300,"source":source})
}
fn dd() -> Value {
    json!({"provider":"datadog","site":"datadoghq.com","app_key_secret":"dd_app_key","query":"sum:trace.http.request.hits{service:dsf}.as_count()","window_seconds":300,"max_age_seconds":120})
}
fn sync() -> Value {
    json!({"source_config_ref":"config-1","resource_id":"subject-1","subject_type":"DsfFlow","sync_sequence":7,"source_kind":"datadog"})
}
fn resource(_provider: &str, _id: &str) -> Value {
    json!({"status":"Active"})
}

#[test]
fn datadog_empty_series_is_absent_not_zero() {
    let result = parse_source(
        &config(dd(), "api"),
        &json!({"status":"ok","series":[]}),
        1_000_000,
    )
    .unwrap();
    assert_eq!(result.coverage, Coverage::Absent);
    assert_eq!(result.revision, "");
    assert_eq!(result.outcome, "no_numeric_points");
}

#[test]
fn measured_zero_is_distinct_from_null_or_old_data() {
    let cfg = config(dd(), "api");
    let body = |point| json!({"status":"ok","series":[{"metric":"hits","scope":"service:dsf","pointlist":[point]}]});
    assert_eq!(
        parse_source(&cfg, &body(json!([990000, 0])), 1_000_000)
            .unwrap()
            .coverage,
        Coverage::Measured
    );
    assert_eq!(
        parse_source(&cfg, &body(json!([990000, null])), 1_000_000)
            .unwrap()
            .coverage,
        Coverage::Absent
    );
    assert_eq!(
        parse_source(&cfg, &body(json!([800000, 4])), 1_000_000)
            .unwrap()
            .coverage,
        Coverage::Stale
    );
    assert_eq!(
        parse_source(&cfg, &body(json!([600000, 4])), 1_000_000)
            .unwrap()
            .coverage,
        Coverage::Absent
    );
}

#[test]
fn configuration_cannot_inject_endpoints_or_secret_headers() {
    let mut json = config_json(dd(), "api");
    json["endpoint"] = json!("https://attacker.example");
    assert!(serde_json::from_value::<Config>(json).is_err());
    let mut json = dd();
    json["site"] = json!("datadoghq.com@attacker.example");
    assert!(provider_request(&config(json, "api"), 1_000_000).is_err());
    assert!(
        serde_json::from_value::<Config>(config_json(
            json!({"provider":"vercel","team_id":"team-1","target":"production"}),
            "prj-1"
        ))
        .is_err()
    );
}
#[test]
fn datadog_request_is_bounded_and_url_encoded() {
    let cfg = config(dd(), "api");
    let mut req = provider_request(&cfg, 1_000_000).unwrap();
    authorize_request(&mut req, &cfg, &mut FakeHost::new(vec![])).unwrap();
    assert!(
        req.url
            .starts_with("https://api.datadoghq.com/api/v1/query?from=700&to=1000&query=sum%3A")
    );
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.headers
            .iter()
            .filter(|(k, _)| k.starts_with("DD-"))
            .count(),
        2
    );
}

fn operational_snapshot() -> Value {
    let jobs =
        json!({"counts":{"pending":2},"oldest_unfinished_at":null,"jobs":[],"has_more":false});
    json!({
        "snapshot_version":1,"participant_limit":200,"job_limit":20,"observed_at":"1970-01-01T00:16:30Z","revision":"a".repeat(40),
        "service":"deep-sci-fi-backend","environment":"production",
        "schema":{"current_version":"old","expected_version":"new","is_current":false},
        "participant_summary":{"total":201,"agents":200,"humans":1,"active_last_24h":2,"heartbeat_last_24h":1},
        "participants":{"items":[],"next_cursor":"10000000-0000-4000-8000-000000000200"},
        "action_queue":jobs,"media":jobs,"notifications":jobs,
        "private_product_content":"DO_NOT_RECORD"
    })
}
#[test]
fn dsf_snapshot_is_paginated_and_does_not_infer_outages() {
    let cfg = config(
        json!({"provider":"dsf_operations","service":"deep-sci-fi-backend","environment":"production","max_age_seconds":120}),
        "deep-sci-fi-backend",
    );
    let parsed = parse_source(&cfg, &operational_snapshot(), 1_000_000).unwrap();
    assert_eq!(parsed.coverage, Coverage::Measured);
    assert_eq!(parsed.outcome, "snapshot_present");
    assert_eq!(parsed.facts["participant_inventory_complete"], false);
    assert_eq!(
        parsed.facts["participants"]["next_cursor"],
        "10000000-0000-4000-8000-000000000200"
    );
    assert_eq!(parsed.facts["schema"]["is_current"], false);
    assert_eq!(parsed.facts["notifications"]["counts"]["pending"], 2);
    assert!(!parsed.facts.to_string().contains("DO_NOT_RECORD"));
}
#[test]
fn dsf_snapshot_unknown_version_or_wrong_environment_cannot_be_current() {
    let cfg = config(
        json!({"provider":"dsf_operations","service":"deep-sci-fi-backend","environment":"production","max_age_seconds":120}),
        "deep-sci-fi-backend",
    );
    let mut snapshot = operational_snapshot();
    snapshot["snapshot_version"] = json!(2);
    assert!(parse_source(&cfg, &snapshot, 1_000_000).is_err());
    snapshot["snapshot_version"] = json!(1);
    snapshot["environment"] = json!("staging");
    assert!(parse_source(&cfg, &snapshot, 1_000_000).is_err());
    snapshot["environment"] = json!("production");
    assert_eq!(
        parse_source(&cfg, &snapshot, 1_200_000).unwrap().coverage,
        Coverage::Stale
    );
}
#[test]
fn datadog_unsorted_points_retain_the_evidence_for_latest_timestamp() {
    let parsed = parse_source(
        &config(dd(), "api"),
        &json!({"status":"ok","series":[{"metric":"hits","pointlist":[[990000,4],[800000,2]]}]}),
        1_000_000,
    )
    .unwrap();
    assert_eq!(
        parsed.facts["series"][0]["latest_point"],
        json!([990000, 4.0])
    );
    assert_eq!(parsed.facts["latest_at_ms"], 990000);
}
#[test]
fn absent_provider_credentials_still_materialize_access_evidence() {
    struct MissingSecret(FakeHost);
    impl Host for MissingSecret {
        fn request(&mut self, r: &Request) -> Result<Response, String> {
            self.0.request(r)
        }
        fn secret(&mut self, _: &str) -> Result<String, String> {
            Err("private host diagnostic".into())
        }
    }
    let mut host = MissingSecret(FakeHost::new(vec![
        config_json(dd(), "api"),
        resource("datadog", "api"),
    ]));
    let mut fields = sync();
    fields["source_kind"] = json!("datadog");
    let out = collect(
        &mut host,
        "https://temper.example",
        "default",
        "sync-1",
        &fields,
        1_000_000,
    )
    .unwrap();
    assert_eq!(out.action, "CollectionInaccessible");
    assert_eq!(out.params["outcome"], "credential_unavailable");
    assert!(
        out.params["evidence_ref"]
            .as_str()
            .unwrap()
            .starts_with("https://api.datadoghq.com/")
    );
    assert_eq!(host.0.requests.len(), 2);
    assert!(!out.params.to_string().contains("private host diagnostic"));
}
#[test]
fn github_git_ref_is_one_path_segment_and_commit_metadata_is_redacted() {
    let cfg = config(
        json!({"provider":"github","owner":"org","repository":"repo","git_ref":"feature/topic?bad=1"}),
        "repo-1",
    );
    let req = provider_request(&cfg, 1_000_000).unwrap();
    assert_eq!(
        req.url,
        "https://api.github.com/repos/org/repo/commits/feature%2Ftopic%3Fbad%3D1"
    );
    let parsed = parse_source(&cfg,&json!({"sha":"a".repeat(40),"commit":{"message":"DO_NOT_RECORD","committer":{"email":"DO_NOT_RECORD","date":"2026-09-06T00:00:00Z"},"tree":{"sha":"tree"}}}),1_000_000).unwrap();
    assert_eq!(parsed.revision, "a".repeat(40));
    assert!(!parsed.facts.to_string().contains("DO_NOT_RECORD"));
}

#[test]
fn dsf_service_selector_must_match_bound_provider_identity() {
    let cfg = config(
        json!({"provider":"dsf_operations","service":"other-service","environment":"production","max_age_seconds":120}),
        "deep-sci-fi-backend",
    );
    assert!(provider_request(&cfg, 1_000_000).is_err());
}

#[test]
fn model_sync_binds_real_flow_subjects_and_records_safe_access_failures() {
    let mut host = FakeHost::new(vec![config_json(dd(), "api"), json!({"status":"Active"})]);
    host.responses.push_back(Response {
        status: 403,
        body: "DO_NOT_RECORD_PRIVATE_BODY".into(),
    });
    let result = collect(
        &mut host,
        "https://temper.example",
        "default",
        "sync-1",
        &sync(),
        1_000_000,
    )
    .unwrap();
    assert_eq!(result.action, "CollectionInaccessible");
    assert_eq!(
        host.requests[1].url,
        "https://temper.example/tdata/DsfFlows('subject-1')"
    );
    assert!(!result.params.to_string().contains("DO_NOT_RECORD"));
    let mut config = config_json(dd(), "api");
    config["subject_id"] = json!("other");
    let mut host = FakeHost::new(vec![config]);
    assert!(
        collect(
            &mut host,
            "https://temper.example",
            "default",
            "sync-1",
            &sync(),
            1_000_000
        )
        .is_err()
    );
    assert_eq!(host.requests.len(), 1);
}
#[test]
fn participant_continuation_keeps_page_evidence_and_uses_the_returned_cursor() {
    let config = config_json(
        json!({"provider":"dsf_operations","service":"deep-sci-fi-backend","environment":"production","max_age_seconds":120}),
        "deep-sci-fi-backend",
    );
    let mut fields = sync();
    fields["source_kind"] = json!("dsf_operations");
    let mut host = FakeHost::new(vec![
        config.clone(),
        json!({"status":"Active"}),
        operational_snapshot(),
    ]);
    let first = collect(
        &mut host,
        "https://temper.example",
        "default",
        "sync-1",
        &fields,
        1_000_000,
    )
    .unwrap();
    assert_eq!(
        first.params["source_cursor"],
        "10000000-0000-4000-8000-000000000200"
    );
    fields["source_cursor"] = first.params["source_cursor"].clone();
    fields["sync_sequence"] = json!(8);
    let mut page = operational_snapshot();
    page["participants"]["next_cursor"] = Value::Null;
    let mut host = FakeHost::new(vec![config, json!({"status":"Active"}), page]);
    let second = collect(
        &mut host,
        "https://temper.example",
        "default",
        "sync-1",
        &fields,
        1_000_000,
    )
    .unwrap();
    assert!(
        host.requests[2]
            .url
            .ends_with("&participant_cursor=10000000-0000-4000-8000-000000000200")
    );
    assert_eq!(second.params["source_cursor"], "");
    assert_ne!(
        first.params["observation_id"],
        second.params["observation_id"]
    );
    let facts: Value = serde_json::from_str(second.params["summary"].as_str().unwrap()).unwrap();
    assert_eq!(facts["participant_inventory_complete"], false);
    assert!(
        facts["participant_page_start_cursor"]
            .as_str()
            .unwrap()
            .ends_with("0200")
    );
}

#[test]
fn snapshot_rejects_invalid_revision_cursor_or_page_limits() {
    let cfg = config(
        json!({"provider":"dsf_operations","service":"deep-sci-fi-backend","environment":"production","max_age_seconds":120}),
        "deep-sci-fi-backend",
    );
    for (path, value) in [
        ("/revision", json!("not-a-commit")),
        ("/participants/next_cursor", json!("not-a-uuid")),
        ("/participant_limit", json!(1)),
    ] {
        let mut row = operational_snapshot();
        *row.pointer_mut(path).unwrap() = value;
        assert!(parse_source(&cfg, &row, 1_000_000).is_err());
    }
}

#[test]
fn scheduled_collection_waits_without_reading_sources_or_creating_observations() {
    let mut host = FakeHost::new(vec![]);
    let fields =
        json!({"sync_sequence":2,"scheduled_refresh":true,"next_due_at":"1970-01-01T00:20:00Z"});
    let result = collect(
        &mut host,
        "https://temper.invalid",
        "default",
        "sync-1",
        &fields,
        1_000_000,
    )
    .unwrap();
    assert_eq!(result.action, "CollectionDeferred");
    assert_eq!(result.params, json!({"expected_sequence":2}));
    assert!(host.requests.is_empty());
}

#[test]
fn manual_due_and_first_collections_read_the_provider() {
    for (scheduled, due) in [
        (false, Some("1970-01-01T00:20:00Z")),
        (true, Some("1970-01-01T00:10:00Z")),
        (true, Some("1970-01-01T00:16:40Z")),
        (true, None),
    ] {
        let mut fields = sync();
        fields["scheduled_refresh"] = json!(scheduled);
        if let Some(due) = due {
            fields["next_due_at"] = json!(due);
        }
        let mut host = FakeHost::new(vec![
            config_json(dd(), "api"),
            json!({"status":"Active"}),
            json!({"status":"ok","series":[]}),
        ]);
        let result = collect(
            &mut host,
            "https://temper.invalid",
            "default",
            "sync-1",
            &fields,
            1_000_000,
        )
        .unwrap();
        assert_eq!(result.action, "CollectionAbsent");
        assert_eq!(host.requests.len(), 3);
        assert_eq!(result.params["next_due_at"], "1970-01-01T00:21:40.000Z");
    }
}

#[test]
fn malformed_due_time_fails_without_reading_a_provider() {
    let mut host = FakeHost::new(vec![]);
    let fields = json!({"sync_sequence":2,"scheduled_refresh":true,"next_due_at":"not-a-time"});
    assert!(
        collect(
            &mut host,
            "https://temper.invalid",
            "default",
            "sync-1",
            &fields,
            1_000_000
        )
        .is_err()
    );
    assert!(host.requests.is_empty());
}

#[test]
fn participant_datadog_cohort_preserves_query_sequence_and_coverage() {
    let mut config = config_json(dd(), "public-world-readers");
    config["subject_type"] = json!("DsfParticipant");
    let mut fields = sync();
    fields["subject_type"] = json!("DsfParticipant");
    for (point, action) in [
        (json!([990000, 0]), "CollectionSucceeded"),
        (json!([990000, null]), "CollectionAbsent"),
        (json!([800000, 4]), "CollectionStale"),
    ] {
        let mut host = FakeHost::new(vec![
            config.clone(),
            json!({"status":"Active"}),
            json!({"status":"ok","series":[{"metric":"hits","pointlist":[point]}]}),
        ]);
        let result = collect(
            &mut host,
            "https://temper.example",
            "default",
            "participant-sync",
            &fields,
            1_000_000,
        )
        .unwrap();
        assert_eq!(result.action, action);
        assert_eq!(result.params["observation_id"], "participant-sync-7");
        assert_eq!(result.params["expected_sequence"], 7);
        assert_eq!(result.params["query"], host.requests[2].url);
        assert_eq!(result.params["evidence_ref"], host.requests[2].url);
        assert_eq!(
            host.requests[2].url,
            provider_request(&serde_json::from_value(config.clone()).unwrap(), 1_000_000)
                .unwrap()
                .url
        );
        assert_eq!(
            host.requests[1].url,
            "https://temper.example/tdata/DsfParticipants('subject-1')"
        );
        assert_eq!(host.requests.len(), 3);
        assert!(host.requests.iter().all(|request| request.method == "GET"));
    }
}

#[test]
fn participant_binding_rejects_github_and_mismatched_or_unsupported_subjects() {
    for (subject_type, subject_id, source) in [
        (
            "DsfParticipant",
            "subject-1",
            json!({"provider":"github","owner":"org","repository":"repo","git_ref":"main"}),
        ),
        ("DsfParticipant", "other-subject", dd()),
        ("InventedParticipant", "subject-1", dd()),
    ] {
        let mut config = config_json(source.clone(), "public-world-readers");
        config["subject_type"] = json!(subject_type);
        config["subject_id"] = json!(subject_id);
        let mut fields = sync();
        fields["subject_type"] = json!(subject_type);
        fields["source_kind"] = source["provider"].clone();
        let mut host = FakeHost::new(vec![config]);
        assert!(
            collect(
                &mut host,
                "https://temper.example",
                "default",
                "participant-sync",
                &fields,
                1_000_000
            )
            .is_err()
        );
        assert_eq!(
            host.requests.len(),
            1,
            "reject before subject or provider reads"
        );
    }
}
