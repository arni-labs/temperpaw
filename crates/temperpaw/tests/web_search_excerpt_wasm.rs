//! Native search request and excerpt metadata at the actual guest boundary.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
fn artifact(engine: &WasmEngine, _module: &str) -> String {
    let path=std::env::var("ARN518_WEB_SEARCH_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-research/wasm/web_search/target/wasm32-unknown-unknown/release/web_search.wasm",env!("CARGO_MANIFEST_DIR")));
    engine
        .compile_and_cache(&std::fs::read(path).unwrap())
        .unwrap()
}
async fn run(engine: &WasmEngine, module: &str, fields: Value, host: Arc<dyn WasmHost>) -> Value {
    let context = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "WebQuery".into(),
        entity_id: "fixture".into(),
        trigger_action: "ExecuteSearch".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([
            ("temper_api_url".into(), "http://fixture".into()),
            ("exa_api_key".into(), "fixture-only".into()),
        ]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    serde_json::to_value(
        engine
            .invoke(
                &artifact(engine, module),
                &context,
                host,
                &WasmResourceLimits {
                    max_memory: 256 * 1024 * 1024,
                    max_fuel: 10_000_000_000,
                    ..Default::default()
                },
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap(),
    )
    .unwrap()
}

#[derive(Default)]
struct Capture {
    requests: Mutex<Vec<Value>>,
}
#[async_trait::async_trait]
impl WasmHost for Capture {
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unexpected binary".into())
    }
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Ok("fixture-only".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        _: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        assert_eq!(method, "POST");
        assert_eq!(url, "https://api.exa.ai/search");
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(body).unwrap());
        Ok((200,json!({"results":[{"title":"Historical source","url":"https://example.org/source","text":"🦀".repeat(4999),"publishedDate":"2025-01-01"},{"text":"é".repeat(5000)}]}).to_string()))
    }
}
#[tokio::test]
async fn real_guest_requests_larger_excerpts_and_preserves_unicode_limit_metadata() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(Capture::default());
    let result = run(
        &engine,
        "web_search",
        json!({"query":"focused source"}),
        host.clone(),
    )
    .await;
    assert_eq!(result["callback_action"], "RecordResults");
    assert_eq!(
        host.requests.lock().unwrap()[0]["contents"]["text"]["maxCharacters"],
        5000
    );
    let rows: Value =
        serde_json::from_str(result["callback_params"]["results"].as_str().unwrap()).unwrap();
    assert_eq!(rows[0]["text"], "🦀".repeat(4999));
    assert_eq!(rows[0]["published_at"], "2025-01-01");
    assert_eq!(rows[0]["text_max_characters"], 5000);
    assert_eq!(rows[0]["text_limit_reached"], false);
    assert_eq!(rows[1]["text_limit_reached"], true);
    assert!(rows[1]["published_at"].is_null());
}
