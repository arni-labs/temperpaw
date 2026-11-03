//! Webhook trigger — thin HTTP endpoint for external webhook ingestion.
//!
//! ONE entity, ONE action. Creates a WebhookEvent entity and dispatches
//! the Received action. Everything else (validation, routing, processing)
//! is handled by WASM integrations on WebhookEvent state transitions.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::PawApiClient;

/// Configuration for the webhook trigger.
#[derive(Debug, Clone)]
pub struct WebhookTriggerConfig {
    /// Port to bind the webhook HTTP listener.
    pub port: u16,
}

/// Webhook trigger state shared across request handlers.
struct TriggerState {
    api: PawApiClient,
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
}

/// Build the webhook trigger router.
///
/// This is used both by the standalone trigger listener and by production
/// deployments that expose the trigger on the primary HTTP port.
pub fn router(api: PawApiClient) -> Router {
    let state = Arc::new(TriggerState { api });

    Router::new()
        .route("/triggers/webhook/{route_key}", post(handle_webhook))
        .with_state(state)
}

impl WebhookTrigger {
    /// Create a new webhook trigger.
    pub fn new(config: WebhookTriggerConfig, api: PawApiClient) -> Self {
        Self { config, api }
    }

    /// Start the webhook trigger HTTP listener.
    ///
    /// Listens on `/triggers/webhook/{route_key}` for POST requests.
    /// For each request: creates ONE WebhookEvent entity, dispatches ONE
    /// Received action, returns the event ID.
    pub async fn run(&self) -> Result<(), String> {
        let app = router(self.api.clone());

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
/// ONE entity, ONE action. Everything else is WASM.
async fn handle_webhook(
    State(state): State<Arc<TriggerState>>,
    Path(route_key): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Serialize headers to JSON for the WASM integration to inspect.
    let headers_json = serialize_headers(&headers);
    let payload_bytes = body.len();

    // ONE entity: create WebhookEvent.
    let entity = match state.api.create_entity("WebhookEvents", json!({})).await {
        Ok(entity) => entity,
        Err(e) => {
            log_webhook_event(WebhookEventLog {
                operation: "create_entity",
                outcome: "error",
                route_key: &route_key,
                event_id: "",
                status: 500,
                payload_bytes,
                error: &e,
            });
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("create WebhookEvent failed: {e}") })),
            ));
        }
    };

    let event_id = entity
        .get("entity_id")
        .or_else(|| entity.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if event_id.is_empty() {
        log_webhook_event(WebhookEventLog {
            operation: "create_entity",
            outcome: "error",
            route_key: &route_key,
            event_id: "",
            status: 500,
            payload_bytes,
            error: "WebhookEvent created but no entity_id returned",
        });
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "WebhookEvent created but no entity_id returned" })),
        ));
    }

    // ONE action: dispatch Received.
    let dispatch_result = state
        .api
        .dispatch_action(
            "WebhookEvents",
            &event_id,
            "TemperPaw.Ingest.Received",
            json!({
                "raw_payload": body,
                "raw_headers": headers_json,
                "route_key": route_key.clone(),
            }),
        )
        .await;

    if let Err(e) = dispatch_result {
        log_webhook_event(WebhookEventLog {
            operation: "dispatch_received",
            outcome: "error",
            route_key: &route_key,
            event_id: &event_id,
            status: 202,
            payload_bytes,
            error: &e,
        });
        // Entity was created; WASM will handle error state.
        // Do not fail the HTTP response; the event exists for audit.
    } else {
        log_webhook_event(WebhookEventLog {
            operation: "receive",
            outcome: "success",
            route_key: &route_key,
            event_id: &event_id,
            status: 200,
            payload_bytes,
            error: "",
        });
    }

    Ok(Json(json!({
        "event_id": event_id,
        "status": "received",
    })))
}

/// Serialize HTTP headers to a JSON string.
fn serialize_headers(headers: &HeaderMap) -> String {
    let map: serde_json::Map<String, Value> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                Value::String(v.to_str().unwrap_or("").to_string()),
            )
        })
        .collect();
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".to_string())
}
