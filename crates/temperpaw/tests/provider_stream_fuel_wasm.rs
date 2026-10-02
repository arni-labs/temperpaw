//! Valid small SSE deltas must finish at the deployed one-billion fuel limit.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
struct Provider {
    output_bytes: usize,
    chunk_bytes: usize,
    emit_deltas: bool,
    calls: std::sync::Mutex<Vec<Value>>,
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
        {
            let mut r = self.response.lock().unwrap();
            let n = r.len().min(self.chunk_bytes);
            Ok(r.drain(..n).collect())
        }
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
        let size = self.output_bytes;
        let mut response = String::new();
        for _ in 0..if self.emit_deltas { size / 2 } else { 0 } {
            response.push_str(&format!(
                "data: {}\n\n",
                json!({"type":"response.output_text.delta","delta":"ab"})
            ));
        }
        response.push_str(&format!("data: {}\n\n",json!({"type":"response.completed","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"ab".repeat(size/2)}]}],"usage":{"input_tokens":1,"output_tokens":1}}})));
        Ok((200, response.into_bytes()))
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
    output_bytes: usize,
    chunk_bytes: usize,
    emit_deltas: bool,
) -> (String, Value, Vec<Value>) {
    let mut prepared = json!({"version":1,"messages":[{"role":"user","content":"Compose a detailed answer"}],"tools":[],"system_prompt":"test","system_prompt_hash":"hash","system_prompt_file_id":"","conversation_file_id":"","session_file_id":"","session_leaf_id":"","workspace_id":"","use_session_tree":false,"context_tokens":1,"context_bytes":1,"entries_loaded":1,"content_files_loaded":0});
    if let Ok(path) = std::env::var("FORESIGHT_PROVIDER_SESSION_FIXTURE") {
        let session: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        prepared["messages"] = json!([{"role":"user","content":session["fields"]["user_message"]}]);
        prepared["system_prompt"] = session["fields"]["system_prompt"].clone();
    }
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "Session".into(),
        entity_id: "session".into(),
        trigger_action: "CallProvider".into(),
        wasm_module: Some("provider_caller".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"prepared_context_inline_json":prepared.to_string(),"provider":"openai","model":"test","tool_choice":"auto"}}),
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
        output_bytes,
        chunk_bytes,
        emit_deltas,
        calls: std::sync::Mutex::new(vec![]),
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
                max_fuel: 1_000_000_000,
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
async fn streamed_composition_fuel() {
    let engine = WasmEngine::new().unwrap();
    let hash=engine.compile_and_cache(&std::fs::read(std::env::var("FORESIGHT_PROVIDER_WASM").unwrap_or_else(|_|format!("{}/../../os-apps/paw-agent/wasm/provider_caller/target/wasm32-unknown-unknown/release/provider_caller.wasm",env!("CARGO_MANIFEST_DIR")))).unwrap()).unwrap();
    let (a, p, r) = invoke(&engine, &hash, 50_000, 8192, true).await;
    println!(
        "action={a} calls={} paramsbytes={}",
        r.len(),
        p.to_string().len()
    );
    assert_eq!(a, "ProviderResponseReady");
    let artifact: Value =
        serde_json::from_str(p["provider_response_inline_json"].as_str().unwrap()).unwrap();
    assert_eq!(artifact["content"][0]["text"], "ab".repeat(25_000));
    assert_eq!(r.len(), 1);
}

#[tokio::test]
async fn fragmented_completed_frame_keeps_exact_output_with_same_fuel() {
    let engine = WasmEngine::new().unwrap();
    let artifact = std::env::var("FORESIGHT_PROVIDER_WASM").unwrap_or_else(|_| format!("{}/../../os-apps/paw-agent/wasm/provider_caller/target/wasm32-unknown-unknown/release/provider_caller.wasm", env!("CARGO_MANIFEST_DIR")));
    let hash = engine
        .compile_and_cache(&std::fs::read(artifact).unwrap())
        .unwrap();
    // A valid completed SSE frame can contain the whole answer; network chunks
    // need not align with SSE lines or JSON tokens. No output is omitted.
    let (action, params, calls) = invoke(&engine, &hash, 100_000, 16, false).await;
    assert_eq!(action, "ProviderResponseReady");
    let artifact: Value =
        serde_json::from_str(params["provider_response_inline_json"].as_str().unwrap()).unwrap();
    assert_eq!(artifact["content"][0]["text"], "ab".repeat(50_000));
    assert_eq!(calls.len(), 1);
}
