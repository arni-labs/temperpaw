//! Webhook trigger — authenticated HTTP admission for external webhook events.
//!
//! The trigger resolves a governed route, authenticates the exact request
//! bytes, applies replay and resource budgets, then creates one WebhookEvent
//! and dispatches one Received action. Routing and processing remain WASM
//! integrations on WebhookEvent state transitions.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

use crate::PawApiClient;

use super::admission::{
    HARD_MAX_BODY_BYTES, MAX_DELIVERY_ID_BYTES, MAX_IN_FLIGHT_ADMISSIONS, RATE_WINDOW, RateWindow,
    WebhookAuthScheme, WebhookRouteSnapshot, required_header, route_field, signature_matches,
    webhook_event_id,
};

const MAX_TRACKED_ROUTE_WINDOWS: usize = 4096;

/// Tenant-scoped in-process capability for resolving a validated webhook key.
///
/// Startup owns the backing vault and closes over the active tenant. The
/// public webhook boundary receives only this narrow read capability; signing
/// secrets never traverse an HTTP endpoint.
pub type WebhookSecretResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Configuration for the webhook trigger.
#[derive(Debug, Clone)]
pub struct WebhookTriggerConfig {
    /// Port to bind the webhook HTTP listener.
    pub port: u16,
}

/// Webhook trigger state shared across request handlers.
struct TriggerState {
    api: PawApiClient,
    secrets: WebhookSecretResolver,
    rate_windows: Mutex<BTreeMap<String, RateWindow>>,
    in_flight: Arc<Semaphore>,
}

#[derive(Debug)]
struct WebhookAdmissionIdentity {
    event_id: String,
    route_id: String,
    route_key: String,
    delivery_id: String,
    payload_digest: String,
    route_snapshot_digest: String,
}

impl WebhookAdmissionIdentity {
    fn create_fields(&self) -> Value {
        json!({
            "Id": self.event_id,
            "route_key": self.route_key,
            "webhook_route_id": self.route_id,
            "delivery_id": self.delivery_id,
            "payload_digest": self.payload_digest,
            "route_snapshot_digest": self.route_snapshot_digest,
            "authentication_scheme": "hmac-sha256",
        })
    }

    fn matches_stable_identity(&self, entity: &Value) -> bool {
        entity
            .get("entity_id")
            .or_else(|| entity.get("Id"))
            .and_then(Value::as_str)
            == Some(self.event_id.as_str())
            && [
                ("route_key", self.route_key.as_str()),
                ("webhook_route_id", self.route_id.as_str()),
                ("delivery_id", self.delivery_id.as_str()),
                ("payload_digest", self.payload_digest.as_str()),
                ("authentication_scheme", "hmac-sha256"),
            ]
            .into_iter()
            .all(|(name, expected)| route_field(entity, name) == Some(expected))
    }

    fn matches_route_snapshot(&self, entity: &Value) -> bool {
        route_field(entity, "route_snapshot_digest") == Some(self.route_snapshot_digest.as_str())
    }
}

#[derive(Clone, Copy)]
struct WebhookEventLog<'a> {
    operation: &'a str,
    outcome: &'a str,
    route_key: &'a str,
    event_id: &'a str,
    status: u16,
    payload_bytes: usize,
    error: &'a str,
}

fn log_webhook_event(event: WebhookEventLog<'_>) {
    tracing::info!(
        observability_event = "temperpaw.webhook",
        webhook.operation = event.operation,
        webhook.outcome = event.outcome,
        webhook.route_key = event.route_key,
        webhook.event_id = event.event_id,
        webhook.status = event.status,
        webhook.payload_bytes = event.payload_bytes,
        error.message = event.error,
        "webhook trigger event"
    );
}

/// Webhook trigger — HTTP endpoint that creates WebhookEvent entities.
pub struct WebhookTrigger {
    config: WebhookTriggerConfig,
    api: PawApiClient,
    secrets: WebhookSecretResolver,
}

/// Build the webhook trigger router.
///
/// This is used both by the standalone trigger listener and by production
/// deployments that expose the trigger on the primary HTTP port.
pub fn router(api: PawApiClient, secrets: WebhookSecretResolver) -> Router {
    let state = Arc::new(TriggerState {
        api,
        secrets,
        rate_windows: Mutex::new(BTreeMap::new()),
        in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_ADMISSIONS)),
    });

    Router::new()
        .route("/triggers/webhook/{route_key}", post(handle_webhook))
        .layer(DefaultBodyLimit::max(HARD_MAX_BODY_BYTES))
        .with_state(state)
}

impl WebhookTrigger {
    /// Create a new webhook trigger.
    pub fn new(
        config: WebhookTriggerConfig,
        api: PawApiClient,
        secrets: WebhookSecretResolver,
    ) -> Self {
        Self {
            config,
            api,
            secrets,
        }
    }

    /// Start the webhook trigger HTTP listener.
    ///
    /// Listens on `/triggers/webhook/{route_key}` for POST requests.
    /// For each request: creates ONE WebhookEvent entity, dispatches ONE
    /// Received action, returns the event ID.
    pub async fn run(&self) -> Result<(), String> {
        let app = router(self.api.clone(), self.secrets.clone());

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        tracing::info!(
            observability_event = "temperpaw.webhook",
            webhook.operation = "listener",
            webhook.outcome = "ready",
            webhook.status = 0,
            listen.addr = %addr,
            "webhook trigger listener ready"
        );

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("webhook trigger bind failed: {e}"))?;

        axum::serve(listener, app)
            .await
            .map_err(|e| format!("webhook trigger serve failed: {e}"))
    }
}

/// Handle an incoming webhook POST.
///
/// Authenticate first, then create one entity and dispatch one action.
async fn handle_webhook(
    State(state): State<Arc<TriggerState>>,
    Path(route_key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let payload_bytes = body.len();
    let _admission_permit = state
        .in_flight
        .clone()
        .try_acquire_owned()
        .map_err(|_| rejection(StatusCode::TOO_MANY_REQUESTS, "webhook admission is busy"))?;

    let route = match load_route(&state.api, &route_key).await {
        Ok(Some(route)) => route,
        Ok(None) => {
            return Err(rejection(
                StatusCode::NOT_FOUND,
                "webhook route was not found",
            ));
        }
        Err(error) => return Err(rejection(StatusCode::SERVICE_UNAVAILABLE, &error)),
    };
    if route.route_key != route_key {
        return Err(rejection(
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook route lookup returned a mismatched route",
        ));
    }
    if payload_bytes > route.max_body_bytes {
        return Err(rejection(
            StatusCode::PAYLOAD_TOO_LARGE,
            "webhook payload exceeds the route budget",
        ));
    }

    let signature = required_header(&headers, &route.signature_header, "signature")
        .map_err(|error| rejection(StatusCode::UNAUTHORIZED, &error))?;
    let delivery_id = required_header(&headers, &route.delivery_id_header, "delivery ID")
        .map_err(|error| rejection(StatusCode::BAD_REQUEST, &error))?;
    if delivery_id.len() > MAX_DELIVERY_ID_BYTES {
        return Err(rejection(
            StatusCode::BAD_REQUEST,
            "webhook delivery ID exceeds its budget",
        ));
    }

    let secret = (state.secrets)(&route.secret_ref)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            rejection(
                StatusCode::SERVICE_UNAVAILABLE,
                "webhook signing secret is unavailable",
            )
        })?;
    if route.auth_scheme != WebhookAuthScheme::HmacSha256
        || !signature_matches(secret.as_bytes(), &body, &signature)
    {
        return Err(rejection(
            StatusCode::UNAUTHORIZED,
            "webhook signature verification failed",
        ));
    }

    if !consume_rate_budget(&state, &route).await {
        return Err(rejection(
            StatusCode::TOO_MANY_REQUESTS,
            "webhook route admission budget exhausted",
        ));
    }

    let raw_payload = std::str::from_utf8(&body).map_err(|_| {
        rejection(
            StatusCode::BAD_REQUEST,
            "webhook payload must be valid UTF-8",
        )
    })?;
    let normalized_payload = normalize_json_object(raw_payload)
        .map_err(|error| rejection(StatusCode::BAD_REQUEST, error))?;

    let identity = WebhookAdmissionIdentity {
        event_id: webhook_event_id(&state.api.config().tenant, &route.route_id, &delivery_id),
        route_id: route.route_id.clone(),
        route_key: route.route_key.clone(),
        delivery_id,
        payload_digest: hex::encode(Sha256::digest(&body)),
        route_snapshot_digest: route.digest(),
    };

    let existing = match state
        .api
        .create_entity("WebhookEvents", identity.create_fields())
        .await
    {
        Ok(existing) => existing,
        Err(e) => {
            log_webhook_event(WebhookEventLog {
                operation: "create_entity",
                outcome: "error",
                route_key: &route_key,
                event_id: &identity.event_id,
                status: 500,
                payload_bytes,
                error: &e,
            });
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "create WebhookEvent failed" })),
            ));
        }
    };

    // Temper collection POST is an atomic get-or-create: every successful
    // response contains the authoritative stored winner. Compare that response
    // before dispatch so concurrent different-content reservations cannot race
    // through a separate read.
    if !identity.matches_stable_identity(&existing) {
        return Err(rejection(
            StatusCode::CONFLICT,
            "webhook delivery ID is already bound to different admission content",
        ));
    }
    if entity_status(&existing) != Some("Created") {
        return Ok(Json(json!({
            "event_id": identity.event_id,
            "status": "duplicate",
        })));
    }
    if !identity.matches_route_snapshot(&existing) {
        return Err(rejection(
            StatusCode::CONFLICT,
            "webhook route changed after delivery reservation",
        ));
    }

    // ONE action: dispatch the authenticated immutable envelope.
    let dispatch_result = state
        .api
        .dispatch_action(
            "WebhookEvents",
            &identity.event_id,
            "TemperPaw.Ingest.Received",
            json!({
                "raw_payload": raw_payload,
                "normalized_payload": normalized_payload,
                "route_key": route.route_key,
                "source_type": route.source_type,
                "target_entity_type": route.target_entity_type,
                "target_action": route.target_action,
                "webhook_route_id": route.route_id,
                "route_snapshot_digest": identity.route_snapshot_digest,
                "payload_digest": identity.payload_digest,
                "delivery_id": identity.delivery_id,
                "authentication_scheme": "hmac-sha256",
                "monitor_resolution_enabled": route.monitor_resolution_enabled,
                "dedup_enabled": route.dedup_enabled,
                "dedup_window_minutes": route.dedup_window_minutes,
            }),
        )
        .await;

    if let Err(e) = dispatch_result {
        let transitioned = state
            .api
            .get_entity("WebhookEvents", &identity.event_id)
            .await
            .ok()
            .and_then(|entity| entity_status(&entity).map(str::to_string))
            .is_some_and(|status| status != "Created");
        if transitioned {
            return Ok(Json(json!({
                "event_id": identity.event_id,
                "status": "duplicate",
            })));
        }
        log_webhook_event(WebhookEventLog {
            operation: "dispatch_received",
            outcome: "error",
            route_key: &route_key,
            event_id: &identity.event_id,
            status: 503,
            payload_bytes,
            error: &e,
        });
        return Err(rejection(
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook event was created but admission dispatch failed; retry this delivery",
        ));
    } else {
        log_webhook_event(WebhookEventLog {
            operation: "receive",
            outcome: "success",
            route_key: &route_key,
            event_id: &identity.event_id,
            status: 200,
            payload_bytes,
            error: "",
        });
    }

    Ok(Json(json!({
        "event_id": identity.event_id,
        "status": "accepted",
    })))
}

async fn load_route(
    api: &PawApiClient,
    route_key: &str,
) -> Result<Option<WebhookRouteSnapshot>, String> {
    if route_key.is_empty() || route_key.len() > 128 {
        return Ok(None);
    }
    let escaped = route_key.replace('\'', "''");
    let routes = api
        .query_entities(
            "WebhookRoutes",
            &format!("route_key eq '{escaped}' and Status eq 'Active'"),
            2,
        )
        .await
        .map_err(|_| "webhook route lookup failed".to_string())?;
    if routes.is_empty() {
        return Ok(None);
    }
    if routes.len() != 1 {
        return Err("webhook route key is not unique".into());
    }
    WebhookRouteSnapshot::from_entity(&routes[0]).map(Some)
}

async fn consume_rate_budget(state: &TriggerState, route: &WebhookRouteSnapshot) -> bool {
    let now = Instant::now();
    let mut windows = state.rate_windows.lock().await;
    windows.retain(|_, window| now.duration_since(window.started_at) < RATE_WINDOW);
    if !windows.contains_key(&route.route_id) && windows.len() >= MAX_TRACKED_ROUTE_WINDOWS {
        return false;
    }
    let window = windows
        .entry(route.route_id.clone())
        .or_insert_with(|| RateWindow {
            started_at: now,
            accepted: 0,
        });
    if window.accepted >= route.max_deliveries_per_minute {
        return false;
    }
    window.accepted += 1;
    true
}

fn entity_status(entity: &Value) -> Option<&str> {
    route_field(entity, "status").or_else(|| entity.get("Status").and_then(Value::as_str))
}

fn normalize_json_object(raw_payload: &str) -> Result<String, &'static str> {
    let payload: Value =
        serde_json::from_str(raw_payload).map_err(|_| "webhook payload must be valid JSON")?;
    if !payload.is_object() {
        return Err("webhook payload must be a JSON object");
    }
    serde_json::to_string(&payload).map_err(|_| "webhook payload normalization failed")
}

fn rejection(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message })))
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "trigger_budget_tests.rs"]
mod budget_tests;
