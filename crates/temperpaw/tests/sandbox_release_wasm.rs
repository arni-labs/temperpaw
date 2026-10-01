//! Terminal cleanup must never enter the sandbox provisioning path.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn module_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        if let Ok(path) = std::env::var("SANDBOX_RELEASE_WASM_PATH") { return std::fs::read(path).unwrap(); }
        let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output=std::process::Command::new("bash").current_dir(root).args(["-c","set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-agent/wasm/sandbox_provisioner wasm32-unknown-unknown --locked"]).output().expect("build sandbox provisioner WASM");
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        std::fs::read(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
    })
}
struct RecordingHost {
    inner: SimWasmHost,
    calls: Mutex<Vec<(String, String, String)>>,
}
#[async_trait::async_trait]
impl WasmHost for RecordingHost {
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), url.into(), body.into()));
        self.inner.http_call(method, url, headers, body).await
    }
    async fn http_call_binary(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        self.calls.lock().unwrap().push((
            method.into(),
            url.into(),
            String::from_utf8_lossy(body).into_owned(),
        ));
        self.inner
            .http_call_binary(method, url, headers, body)
            .await
    }
    fn get_secret(&self, key: &str) -> Result<String, String> {
        self.inner.get_secret(key)
    }
    fn log(&self, level: &str, message: &str) {
        self.inner.log(level, message)
    }
}
async fn invoke(
    status: &str,
    mode: bool,
    fields: Value,
    response: Option<(&str, u16)>,
) -> (Value, Vec<(String, String, String)>) {
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let hash = engine.compile_and_cache(module_bytes()).unwrap();
    let mut inner = SimWasmHost::new().with_default_response(500, "unexpected provider IO");
    if let Some((url, status)) = response {
        inner = inner.with_response(url, status, "{}");
    }
    let host = Arc::new(RecordingHost {
        inner,
        calls: Mutex::new(vec![]),
    });
    let mut config = BTreeMap::from([
        ("sandbox_provider".into(), "tensorlake".into()),
        ("tensorlake_api_key".into(), "fixture".into()),
        ("modal_token_id".into(), "fixture".into()),
        ("modal_bridge_url".into(), "https://fixture".into()),
    ]);
    if mode {
        config.insert("mode".into(), "release".into());
    }
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "Session".into(),
        entity_id: "s1".into(),
        trigger_action: "Complete".into(),
        wasm_module: Some("sandbox_provisioner".into()),
        trigger_params: json!({}),
        entity_state: json!({"status":status,"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: config,
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let result = engine
        .invoke(
            &hash,
            &ctx,
            host.clone(),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    let calls = host.calls.lock().unwrap().clone();
    (serde_json::to_value(result).unwrap(), calls)
}
#[tokio::test]
async fn terminal_or_explicit_release_without_handle_never_provisions() {
    for (status, mode) in [
        ("Completed", false),
        ("Failed", false),
        ("Cancelled", false),
        ("Running", true),
    ] {
        let (r, calls) = invoke(status, mode, json!({}), None).await;
        assert_eq!(r["success"], true, "{r}");
        assert_eq!(r["callback_params"]["status"], "noop", "{r}");
        assert!(calls.is_empty(), "{calls:?}");
    }
}
#[tokio::test]
async fn static_handle_is_not_destroyed() {
    let (r,calls)=invoke("Completed",false,json!({"sandbox_provider":"static","sandbox_id":"static-sandbox","sandbox_url":"https://fixture"}),None).await;
    assert_eq!(r["callback_params"]["status"], "noop");
    assert!(calls.is_empty());
}
#[tokio::test]
async fn tensorlake_deletes_only_existing_handle_and_accepts_already_gone() {
    let url = "https://api.tensorlake.ai/sandboxes/existing-123";
    for status in [200, 404] {
        let (r, calls) = invoke(
            "Completed",
            false,
            json!({"sandbox_provider":"tensorlake","sandbox_id":"existing-123"}),
            Some((url, status)),
        )
        .await;
        assert_eq!(r["success"], true);
        assert_eq!(r["callback_params"]["status"], "released", "{r}");
        assert_eq!(calls, vec![("DELETE".into(), url.into(), String::new())]);
    }
}
#[tokio::test]
async fn modal_terminates_only_existing_handle_and_accepts_already_gone() {
    let url = "https://fixture-terminate.modal.run?authorization=Bearer%20fixture";
    let (r, calls) = invoke(
        "Completed",
        false,
        json!({"sandbox_provider":"modal","sandbox_id":"existing-modal"}),
        Some((url, 404)),
    )
    .await;
    assert_eq!(r["callback_params"]["status"], "released", "{r}");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "POST");
    assert_eq!(calls[0].1, url);
    assert_eq!(
        serde_json::from_str::<Value>(&calls[0].2).unwrap(),
        json!({"sandbox_id":"existing-modal"})
    );
}
#[tokio::test]
async fn release_failure_does_not_fail_terminal_transition() {
    let (r, calls) = invoke(
        "Completed",
        false,
        json!({"sandbox_provider":"tensorlake","sandbox_id":"existing-123"}),
        None,
    )
    .await;
    assert_eq!(r["success"], true);
    assert_eq!(r["callback_params"]["status"], "release_failed");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "DELETE");
}
