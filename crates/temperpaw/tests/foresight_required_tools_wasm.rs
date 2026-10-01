//! Actual guest proof of inherited hypothetical state and temporal forecast gating.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
struct Provider {
    calls: std::sync::Mutex<Vec<Value>>,
    succeeds: bool,
    request: std::sync::Mutex<Vec<u8>>,
    response: std::sync::Mutex<Vec<u8>>,
}
#[async_trait::async_trait]
impl WasmHost for Provider {
    async fn http_stream_begin_outbound(
        &self,
        _: temper_wasm::http_stream::HttpRequestHead,
    ) -> Result<temper_wasm::http_stream::HttpStreamHandles, String> {
        self.request.lock().unwrap().clear();
        Ok(temper_wasm::http_stream::HttpStreamHandles {
            request_body: temper_wasm::http_stream::StreamHandle(1),
            response_body: temper_wasm::http_stream::StreamHandle(2),
        })
    }
    async fn http_stream_try_write(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
        chunk: Vec<u8>,
    ) -> Result<usize, temper_wasm::http_stream::StreamError> {
        let n = chunk.len();
        self.request.lock().unwrap().extend(chunk);
        Ok(n)
    }
    async fn http_stream_close(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<(), temper_wasm::http_stream::StreamError> {
        Ok(())
    }
    async fn http_stream_response_head(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<temper_wasm::http_stream::HttpResponseHead, String> {
        let body = self.request.lock().unwrap().clone();
        let (_, response) = self
            .http_call_binary("POST", "https://fixture.invalid", &[], &body)
            .await?;
        *self.response.lock().unwrap() = response;
        Ok(temper_wasm::http_stream::HttpResponseHead {
            status: 200,
            headers: vec![],
        })
    }
    async fn http_stream_read(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<Vec<u8>, temper_wasm::http_stream::StreamError> {
        Ok(std::mem::take(&mut *self.response.lock().unwrap()))
    }
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Ok("fixture-key".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_call_binary(
        &self,
        _: &str,
        url: &str,
        _: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        assert!(url.contains("fixture.invalid"));
        let mut calls = self.calls.lock().unwrap();
        calls.push(serde_json::from_slice(body).unwrap());
        let tool = self.succeeds && calls.len() > 1;
        let output = if tool {
            json!([{"type":"function_call","call_id":"call_real","name":"execute","arguments":"{\"code\":\"return 1\"}"}])
        } else {
            json!([{"type":"message","content":[{"type":"output_text","text":"Tool call pretend: execute()"}]}])
        };
        Ok((200,format!("data: {}\n\n",json!({"type":"response.completed","response":{"output":output,"usage":{"input_tokens":1,"output_tokens":1}}})).into_bytes()))
    }
    async fn http_call(
        &self,
        _: &str,
        url: &str,
        _: &[(String, String)],
        _: &str,
    ) -> Result<(u16, String), String> {
        Err(format!("unexpected ordinary HTTP {url}"))
    }
}
async fn invoke(
    engine: &WasmEngine,
    hash: &str,
    required: bool,
    succeeds: bool,
) -> (String, Value, Vec<Value>) {
    let prepared = json!({"version":1,"messages":[{"role":"user","content":"Use the actual tool"}],"tools":[{"name":"execute","description":"execute","input_schema":{"type":"object","properties":{"code":{"type":"string"}}}}],"system_prompt":"test","system_prompt_hash":"hash","system_prompt_file_id":"","conversation_file_id":"","session_file_id":"","session_leaf_id":"","workspace_id":"","use_session_tree":false,"context_tokens":1,"context_bytes":1,"entries_loaded":1,"content_files_loaded":0});
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "Session".into(),
        entity_id: "session".into(),
        trigger_action: "CallProvider".into(),
        wasm_module: Some("provider_caller".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"prepared_context_inline_json":prepared.to_string(),"provider":"openai","model":"test","tool_choice":if required{"required"}else{"auto"}}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([
            ("openai_api_key".into(), "fixture-key".into()),
            (
                "openai_api_url".into(),
                "https://fixture.invalid/responses".into(),
            ),
        ]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let host = Arc::new(Provider {
        calls: std::sync::Mutex::new(vec![]),
        succeeds,
        request: std::sync::Mutex::new(vec![]),
        response: std::sync::Mutex::new(vec![]),
    });
    let out = engine
        .invoke(
            hash,
            &ctx,
            host.clone(),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    let requests = host.calls.lock().unwrap().clone();
    (out.callback_action, out.callback_params, requests)
}
#[tokio::test]
async fn required_calls_are_real_bounded_and_auto_is_unchanged() {
    let engine = WasmEngine::new().unwrap();
    let hash=engine.compile_and_cache(&std::fs::read(format!("{}/../../os-apps/paw-agent/wasm/provider_caller/target/wasm32-unknown-unknown/release/provider_caller.wasm",env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let (a, p, r) = invoke(&engine, &hash, true, true).await;
    assert_eq!(a, "ProviderResponseReady", "{p}");
    assert_eq!(r.len(), 2);
    let artifact: Value =
        serde_json::from_str(p["provider_response_inline_json"].as_str().unwrap()).unwrap();
    assert_eq!(artifact["stop_reason"], "tool_use");
    assert_eq!(artifact["content"][0]["id"], "call_real");
    assert!(r[1].to_string().contains("tool"));
    let (a, p, r) = invoke(&engine, &hash, true, false).await;
    assert_eq!(r.len(), 3);
    assert_ne!(a, "ProviderResponseReady", "{p}");
    let (a, p, r) = invoke(&engine, &hash, false, false).await;
    assert_eq!(a, "ProviderResponseReady", "{p}");
    assert_eq!(r.len(), 1);
}
