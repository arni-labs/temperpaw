//! Actual guest proof of inherited hypothetical state and temporal forecast gating.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
struct Provider(Value);
#[async_trait::async_trait]
impl WasmHost for Provider {
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unused".into())
    }
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        _: &[(String, String)],
        _: &str,
    ) -> Result<(u16, String), String> {
        assert_eq!(method, "GET");
        assert!(url.ends_with("?$select=Status,error_message,error"));
        assert_ne!(self.0["no_http"], true);
        if self.0["Status"] == "Denied" {
            return Err("authorization denied for http_call: no matching permit policy".into());
        }
        Ok((200, self.0.to_string()))
    }
}
async fn invoke(
    engine: &WasmEngine,
    hash: &str,
    attempt: u64,
    recorded: &str,
    status: &str,
    error: &str,
) -> (String, Value) {
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "World".into(),
        entity_id: "world".into(),
        trigger_action: "CheckResearchSession".into(),
        wasm_module: Some("seed_session".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"research_session_id":"child","research_session_attempt":recorded},"counters":{"research_attempt":attempt}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "temper_api_url".into(),
            "https://fixture.invalid".into(),
        )]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let host = Provider(
        json!({"Status":status,"error":error,"error_message":"","no_http":recorded=="1"&&attempt==2}),
    );
    let out = engine
        .invoke(
            hash,
            &ctx,
            Arc::new(host),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    (out.callback_action, out.callback_params)
}
#[tokio::test]
async fn actual_monitor_bounds_retries_preserves_auth_error_and_ignores_old_session() {
    let engine = WasmEngine::new().unwrap();
    let hash=engine.compile_and_cache(&std::fs::read(format!("{}/../../os-apps/paw-foresight/wasm/seed_session/target/wasm32-unknown-unknown/release/seed_session.wasm",env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    for (attempt, recorded, status, error, expected) in [
        (1, "1", "Completed", "", "ResearchIncomplete"),
        (2, "2", "Completed", "", "ResearchIncomplete"),
        (3, "3", "Completed", "", "ResearchFailed"),
        (1, "1", "Failed", "auth denied", "ResearchFailed"),
        (2, "1", "Completed", "", "ResearchPending"),
        (1, "1", "Executing", "", "ResearchPending"),
        (1, "1", "Denied", "", "ResearchMonitorUnavailable"),
    ] {
        let (a, p) = invoke(&engine, &hash, attempt, recorded, status, error).await;
        assert_eq!(a, expected);
        assert_eq!(p["expected_research_attempt"], attempt);
        assert_eq!(p["expected_research_session_id"], "child");
        if !error.is_empty() {
            assert_eq!(p["error_message"], error);
        }
    }
}
