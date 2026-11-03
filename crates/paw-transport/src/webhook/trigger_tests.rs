use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use tokio::net::TcpListener;

use super::*;
use crate::PawApiConfig;

pub(super) fn governed_route() -> Value {
    json!({
        "entity_id": "route-1",
        "fields": {
            "route_key": "patrol-github",
            "source_type": "github",
            "target_entity_type": "Signal",
            "target_action": "TemperPaw.Patrol.Ingest",
            "auth_scheme": "hmac-sha256",
            "secret_ref": "patrol_github_webhook_secret",
            "signature_header": "x-hub-signature-256",
            "delivery_id_header": "x-github-delivery",
            "max_body_bytes": "262144",
            "max_deliveries_per_minute": "120",
            "monitor_resolution_enabled": "false",
            "dedup_enabled": "true",
            "dedup_window_minutes": "60"
        }
    })
}

#[test]
fn route_snapshot_requires_governed_authentication_configuration() {
    let snapshot = WebhookRouteSnapshot::from_entity(&governed_route()).unwrap();
    assert_eq!(snapshot.route_id, "route-1");
    assert_eq!(snapshot.auth_scheme, WebhookAuthScheme::HmacSha256);
    assert_eq!(snapshot.secret_ref, "patrol_github_webhook_secret");

    for field in [
        "secret_ref",
        "signature_header",
        "delivery_id_header",
        "target_entity_type",
        "target_action",
    ] {
        let mut route = governed_route();
        route["fields"][field] = Value::String(String::new());
        assert!(
            WebhookRouteSnapshot::from_entity(&route).is_err(),
            "empty {field} must fail closed"
        );
    }

    let mut route = governed_route();
    route["fields"]["auth_scheme"] = json!("none");
    assert!(WebhookRouteSnapshot::from_entity(&route).is_err());

    let mut route = governed_route();
    route["fields"]["secret_ref"] = json!("{secret:literal-confusion}");
    assert!(WebhookRouteSnapshot::from_entity(&route).is_err());

    for (field, value) in [
        ("route_key", "bad/route"),
        ("source_type", "bad source"),
        ("target_entity_type", "../../Admin"),
        ("target_action", "TemperPaw/Patrol/Submit"),
        ("monitor_resolution_enabled", "TRUE"),
        ("dedup_enabled", "yes"),
        ("dedup_window_minutes", "0"),
        ("dedup_window_minutes", "10081"),
    ] {
        let mut invalid = governed_route();
        invalid["fields"][field] = json!(value);
        assert!(
            WebhookRouteSnapshot::from_entity(&invalid).is_err(),
            "invalid {field}={value:?} must fail closed"
        );
    }
}

#[test]
fn hmac_verification_uses_raw_bytes_and_rejects_invalid_hex() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let body = br#"{"action":"opened"}"#;
    let mut mac = Hmac::<Sha256>::new_from_slice(b"correct-secret").unwrap();
    mac.update(body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

    assert!(signature_matches(b"correct-secret", body, &signature));
    assert!(!signature_matches(
        b"correct-secret",
        br#"{"action":"closed"}"#,
        &signature
    ));
    assert!(!signature_matches(
        b"correct-secret",
        body,
        "sha256=not-hex"
    ));
    assert!(!signature_matches(b"correct-secret", body, "sha256=00"));
}

#[test]
fn webhook_payload_normalization_requires_a_json_object() {
    assert_eq!(
        normalize_json_object(r#"{ "action": "opened" }"#).unwrap(),
        r#"{"action":"opened"}"#
    );
    assert!(normalize_json_object("not-json").is_err());
    assert!(normalize_json_object(r#""scalar""#).is_err());
    assert!(normalize_json_object("[]").is_err());
}

#[test]
fn replay_identity_is_stable_and_route_scoped() {
    let first = webhook_event_id("tenant-a", "route-a", "delivery-1");
    assert_eq!(first, webhook_event_id("tenant-a", "route-a", "delivery-1"));
    assert_ne!(first, webhook_event_id("tenant-a", "route-b", "delivery-1"));
    assert_ne!(first, webhook_event_id("tenant-b", "route-a", "delivery-1"));
    assert_ne!(first, webhook_event_id("tenant-a", "route-a", "delivery-2"));
    assert!(first.starts_with("wh-"));
}

#[test]
fn immutable_snapshot_digest_covers_target_capability() {
    let original = WebhookRouteSnapshot::from_entity(&governed_route()).unwrap();
    let mut mutated_route = governed_route();
    mutated_route["fields"]["target_action"] = json!("TemperPaw.Admin.Escalate");
    let mutated = WebhookRouteSnapshot::from_entity(&mutated_route).unwrap();

    assert_ne!(original.digest(), mutated.digest());
    assert_eq!(original.target_action, "TemperPaw.Patrol.Ingest");
}

#[test]
fn persisted_admission_identity_rejects_changed_payload_or_route() {
    let route = WebhookRouteSnapshot::from_entity(&governed_route()).unwrap();
    let identity = WebhookAdmissionIdentity {
        event_id: webhook_event_id("tenant-a", &route.route_id, "delivery-1"),
        route_id: route.route_id.clone(),
        route_key: route.route_key.clone(),
        delivery_id: "delivery-1".to_string(),
        payload_digest: hex::encode(Sha256::digest(br#"{"action":"opened"}"#)),
        route_snapshot_digest: route.digest(),
    };
    let mut fields = identity.create_fields();
    fields.as_object_mut().unwrap().remove("Id");
    let mut entity = json!({
        "entity_id": identity.event_id,
        "fields": fields,
    });
    assert!(identity.matches_stable_identity(&entity));
    assert!(identity.matches_route_snapshot(&entity));

    entity["fields"]["payload_digest"] = json!("changed");
    assert!(!identity.matches_stable_identity(&entity));
    entity["fields"]["payload_digest"] = json!(identity.payload_digest);
    entity["fields"]["webhook_route_id"] = json!("route-2");
    assert!(!identity.matches_stable_identity(&entity));
    entity["fields"]["webhook_route_id"] = json!(identity.route_id);
    entity["fields"]["route_snapshot_digest"] = json!("changed");
    assert!(identity.matches_stable_identity(&entity));
    assert!(!identity.matches_route_snapshot(&entity));
}

#[derive(Clone)]
struct MockWebhookApi {
    route: Arc<Mutex<Value>>,
    events: Arc<Mutex<BTreeMap<String, MockEvent>>>,
    dispatches: Arc<Mutex<Vec<Value>>>,
    create_attempts: Arc<AtomicUsize>,
    secret_http_reads: Arc<AtomicUsize>,
    mutate_after_next_route_read: Arc<AtomicBool>,
    fail_next_dispatch: Arc<AtomicBool>,
}

#[derive(Clone)]
struct MockEvent {
    status: String,
    fields: Value,
}

fn mock_event_entity(id: &str, event: &MockEvent) -> Value {
    let mut fields = event.fields.clone();
    fields["status"] = json!(event.status);
    json!({
        "entity_id": id,
        "fields": fields,
    })
}

impl Default for MockWebhookApi {
    fn default() -> Self {
        Self {
            route: Arc::new(Mutex::new(governed_route())),
            events: Arc::new(Mutex::new(BTreeMap::new())),
            dispatches: Arc::new(Mutex::new(Vec::new())),
            create_attempts: Arc::new(AtomicUsize::new(0)),
            secret_http_reads: Arc::new(AtomicUsize::new(0)),
            mutate_after_next_route_read: Arc::new(AtomicBool::new(false)),
            fail_next_dispatch: Arc::new(AtomicBool::new(false)),
        }
    }
}

async fn spawn_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

fn event_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/tdata/WebhookEvents('")?
        .split_once("')")
        .map(|(id, _)| id)
}

async fn mock_webhook_api(
    State(state): State<MockWebhookApi>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let path = uri.path();
    if method == Method::GET && path == "/tdata/WebhookRoutes" {
        let route = state.route.lock().unwrap().clone();
        if state
            .mutate_after_next_route_read
            .swap(false, Ordering::SeqCst)
        {
            state.route.lock().unwrap()["fields"]["target_action"] =
                json!("TemperPaw.Admin.Escalate");
        }
        return Json(json!({ "value": [route] })).into_response();
    }
    if method == Method::GET && path.starts_with("/paw/setup/secrets/") {
        state.secret_http_reads.fetch_add(1, Ordering::SeqCst);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if method == Method::POST && path == "/tdata/WebhookEvents" {
        state.create_attempts.fetch_add(1, Ordering::SeqCst);
        let mut value: Value = serde_json::from_slice(&body).unwrap();
        let id = value["Id"].as_str().unwrap().to_string();
        let mut events = state.events.lock().unwrap();
        if let Some(existing) = events.get(&id) {
            // Temper collection POST is get-or-create and returns success for
            // an existing caller-selected ID, including the authoritative
            // stored state selected by the atomic operation.
            return (StatusCode::CREATED, Json(mock_event_entity(&id, existing))).into_response();
        }
        value.as_object_mut().unwrap().remove("Id");
        let event = MockEvent {
            status: "Created".to_string(),
            fields: value,
        };
        let response = mock_event_entity(&id, &event);
        events.insert(id, event);
        return (StatusCode::CREATED, Json(response)).into_response();
    }
    if let Some(id) = event_id_from_path(path) {
        if method == Method::GET {
            let event = state.events.lock().unwrap().get(id).cloned().unwrap();
            return Json(mock_event_entity(id, &event)).into_response();
        }
        if method == Method::POST && path.ends_with("/TemperPaw.Ingest.Received") {
            if state.fail_next_dispatch.swap(false, Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let value: Value = serde_json::from_slice(&body).unwrap();
            state.dispatches.lock().unwrap().push(value);
            state.events.lock().unwrap().get_mut(id).unwrap().status = "Routing".to_string();
            return Json(json!({ "entity_id": id, "status": "Routing" })).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn signed_request(
    client: &reqwest::Client,
    base_url: &str,
    body: &str,
    delivery_id: &str,
    secret: &str,
) -> reqwest::RequestBuilder {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    client
        .post(format!("{base_url}/triggers/webhook/patrol-github"))
        .header("x-hub-signature-256", signature)
        .header("x-github-delivery", delivery_id)
        .body(body.to_string())
}

#[tokio::test]
async fn http_admission_rejects_before_persistence_and_suppresses_replay() {
    let backend_state = MockWebhookApi::default();
    let backend_url = spawn_server(
        Router::new()
            .fallback(any(mock_webhook_api))
            .with_state(backend_state.clone()),
    )
    .await;
    let api = PawApiClient::new(PawApiConfig {
        base_url: backend_url,
        tenant: "tenant-a".to_string(),
        api_key: None,
    });
    let secrets: WebhookSecretResolver = Arc::new(|key| {
        (key == "patrol_github_webhook_secret").then(|| "correct-secret".to_string())
    });
    let trigger_url = spawn_server(router(api, secrets)).await;
    let client = reqwest::Client::new();
    let body = r#"{ "action": "opened" }"#;

    backend_state.route.lock().unwrap()["fields"]["max_body_bytes"] = json!("4");
    let route_oversized = signed_request(
        &client,
        &trigger_url,
        body,
        "delivery-route-oversized",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(route_oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);
    backend_state.route.lock().unwrap()["fields"]["max_body_bytes"] = json!("262144");

    let globally_oversized = format!(r#"{{"data":"{}"}}"#, "a".repeat(HARD_MAX_BODY_BYTES));
    let global_rejection = signed_request(
        &client,
        &trigger_url,
        &globally_oversized,
        "delivery-global-oversized",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(global_rejection.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);

    let unsigned = client
        .post(format!("{trigger_url}/triggers/webhook/patrol-github"))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);

    let forged = signed_request(
        &client,
        &trigger_url,
        body,
        "delivery-forged",
        "wrong-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);

    backend_state.route.lock().unwrap()["fields"]["max_deliveries_per_minute"] = json!("2");
    for (delivery_id, invalid_body) in [
        ("delivery-malformed", "not-json"),
        ("delivery-scalar", r#""scalar""#),
    ] {
        let invalid = signed_request(
            &client,
            &trigger_url,
            invalid_body,
            delivery_id,
            "correct-secret",
        )
        .send()
        .await
        .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);
    }
    let exhausted = signed_request(
        &client,
        &trigger_url,
        body,
        "delivery-after-invalid-budget",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(exhausted.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 0);
    backend_state.route.lock().unwrap()["fields"]["max_deliveries_per_minute"] = json!("120");

    backend_state
        .mutate_after_next_route_read
        .store(true, Ordering::SeqCst);
    let accepted = signed_request(&client, &trigger_url, body, "delivery-1", "correct-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted_body: Value = accepted.json().await.unwrap();
    assert_eq!(accepted_body["status"], "accepted");

    let replay = signed_request(&client, &trigger_url, body, "delivery-1", "correct-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body: Value = replay.json().await.unwrap();
    assert_eq!(replay_body["status"], "duplicate");
    assert_eq!(replay_body["event_id"], accepted_body["event_id"]);

    let altered = signed_request(
        &client,
        &trigger_url,
        r#"{"action":"closed"}"#,
        "delivery-1",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(altered.status(), StatusCode::CONFLICT);

    backend_state
        .fail_next_dispatch
        .store(true, Ordering::SeqCst);
    let interrupted = signed_request(
        &client,
        &trigger_url,
        body,
        "delivery-interrupted",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(interrupted.status(), StatusCode::SERVICE_UNAVAILABLE);
    let interrupted_body: Value = interrupted.json().await.unwrap();
    assert_eq!(
        interrupted_body["error"],
        "webhook event was created but admission dispatch failed; retry this delivery"
    );

    let recovered = signed_request(
        &client,
        &trigger_url,
        body,
        "delivery-interrupted",
        "correct-secret",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered_body: Value = recovered.json().await.unwrap();
    assert_eq!(recovered_body["status"], "accepted");

    assert_eq!(backend_state.create_attempts.load(Ordering::SeqCst), 5);
    assert_eq!(
        backend_state.secret_http_reads.load(Ordering::SeqCst),
        0,
        "webhook signing secrets must be resolved in-process, never over HTTP"
    );
    let dispatches = backend_state.dispatches.lock().unwrap();
    assert_eq!(dispatches.len(), 2);
    assert_eq!(
        dispatches[0]["target_action"], "TemperPaw.Patrol.Ingest",
        "route mutation after authentication must not change the accepted capability"
    );
    assert_eq!(dispatches[0]["delivery_id"], "delivery-1");
    assert_eq!(dispatches[0]["authentication_scheme"], "hmac-sha256");
    assert_eq!(dispatches[0]["raw_payload"], body);
    assert_eq!(
        dispatches[0]["normalized_payload"],
        r#"{"action":"opened"}"#
    );
    assert!(
        dispatches[0]["route_snapshot_digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    );
    assert_eq!(dispatches[1]["delivery_id"], "delivery-interrupted");
}
