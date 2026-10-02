//! Actual module regression with synthetic HTTP responses; no live policy changes.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

#[tokio::test]
#[ignore = "requires locally built request_approval WASM"]
async fn callback_lookup_failure_preserves_human_pause_and_attempts_notification() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bytes = std::fs::read(root.join("os-apps/paw-agent/wasm/request_approval/target/wasm32-unknown-unknown/release/request_approval.wasm")).unwrap();
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "Session".into(),
        entity_id: "ss-test".into(),
        trigger_action: "PauseForApproval".into(),
        wasm_module: Some("request_approval".into()),
        trigger_params: json!({}),
        entity_state: json!({"Status":"WaitingForApproval","fields":{"agent_id":"aj-test","pending_decision_id":"PD-test","pending_tool_context":"{\"method\":\"EventNode.Retire\"}"}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "temper_api_url".into(),
            "https://temper.test".into(),
        )]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    for (lookup_status, webhook_status) in [(404, 200), (404, 503), (200, 200)] {
        let host = SimWasmHost::new().with_default_response(200,"{}")
            .with_response("https://temper.test/tdata/GovernanceDecisions?$filter=pending_decision_id eq 'PD-test'&$top=1", lookup_status, "{\"value\":[{\"entity_id\":\"gd-test\"}]}")
            .with_response("https://temper.test/tdata/ChannelSessions?$filter=Status eq 'Active' and session_entity_id eq 'ss-test'&$top=1",200,"{\"value\":[{\"channel_id\":\"channel-test\",\"thread_id\":\"thread-test\"}]}")
            .with_response("https://temper.test/tdata/Channels?$filter=Status eq 'Connected' and channel_id eq 'channel-test'&$top=1",200,"{\"value\":[{\"webhook_url\":\"https://notification.test\"}]}")
            .with_response("https://notification.test",webhook_status,"{}");
        let result = engine
            .invoke(
                &hash,
                &ctx,
                Arc::new(host),
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap();
        assert!(result.success, "{result:?}");
        assert_eq!(
            result.callback_action, "",
            "Must neither fail nor resume the paused Session"
        );
        let p = result.callback_params;
        assert_eq!(p["decision_id"], "PD-test");
        assert_eq!(p["callback_registered"], lookup_status == 200);
        if lookup_status == 404 {
            assert_eq!(p["status"], "waiting_for_out_of_band_approval");
            assert_eq!(p["callback_error"], "GD query failed (HTTP 404)");
            assert!(
                p["recovery_required"]
                    .as_str()
                    .unwrap()
                    .contains("Human decision")
            );
        } else {
            assert_eq!(p["status"], "notified");
            assert_eq!(p["callback_error"], Value::Null);
        }
        if webhook_status == 503 {
            assert_eq!(p["delivery"], "failed");
            assert!(
                p["error"]
                    .as_str()
                    .unwrap()
                    .contains("webhook POST failed (HTTP 503)"),
                "Must reach notification after callback404"
            );
        }
    }
}
