use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use super::tests::governed_route;
use super::*;
use crate::PawApiConfig;

#[test]
fn duplicate_security_headers_are_rejected() {
    let name = axum::http::HeaderName::from_static("x-temper-signature");
    let mut headers = HeaderMap::new();
    headers.append(&name, "sha256=00".parse().unwrap());
    headers.append(&name, "sha256=11".parse().unwrap());
    assert!(required_header(&headers, &name, "signature").is_err());
}

#[tokio::test]
async fn configured_body_and_rate_budgets_fail_before_entity_creation() {
    let api = PawApiClient::new(PawApiConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        tenant: "tenant-a".to_string(),
        api_key: None,
    });
    let state = TriggerState {
        api,
        secrets: Arc::new(|_| None),
        rate_windows: tokio::sync::Mutex::new(BTreeMap::new()),
        in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT_ADMISSIONS)),
    };
    let mut route = WebhookRouteSnapshot::from_entity(&governed_route()).unwrap();
    route.max_deliveries_per_minute = 1;
    assert!(consume_rate_budget(&state, &route).await);
    assert!(!consume_rate_budget(&state, &route).await);

    let now = Instant::now();
    {
        let mut windows = state.rate_windows.lock().await;
        windows.clear();
        windows.insert(
            "expired-route".to_string(),
            RateWindow {
                started_at: now - RATE_WINDOW,
                accepted: 1,
            },
        );
    }
    assert!(consume_rate_budget(&state, &route).await);
    assert!(
        !state
            .rate_windows
            .lock()
            .await
            .contains_key("expired-route")
    );

    {
        let mut windows = state.rate_windows.lock().await;
        windows.clear();
        for index in 0..MAX_TRACKED_ROUTE_WINDOWS {
            windows.insert(
                format!("route-{index}"),
                RateWindow {
                    started_at: now,
                    accepted: 0,
                },
            );
        }
    }
    route.route_id = "overflow-route".to_string();
    assert!(!consume_rate_budget(&state, &route).await);

    let mut oversized = governed_route();
    oversized["fields"]["max_body_bytes"] = json!((HARD_MAX_BODY_BYTES + 1).to_string());
    assert!(WebhookRouteSnapshot::from_entity(&oversized).is_err());
}
