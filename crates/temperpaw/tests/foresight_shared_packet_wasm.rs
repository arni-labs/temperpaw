//! Full guest boundary proof: preparation freezes once, both consumers use it.
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
};
use temper_wasm::{
    StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

struct Host {
    calls: Mutex<Vec<(String, String, String)>>,
    packet: Value,
}
#[async_trait]
impl WasmHost for Host {
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        _headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), url.into(), body.into()));
        let state = &self.packet["state"];
        let answer = if url.contains("Worlds(") {
            json!({"fields":{"semantic_evaluation_mode":"shadow","agent_model":"gpt-5.5","agent_provider":"openai_codex","graph_snapshot_file_id":"g-1","last_ingest_date":"2026-09-17","target_date":"2027-09-17"}}).to_string()
        } else if url.contains("Endpoints(") {
            json!({"fields":{"world_id":"w-1","bundle_file_id":"b-1"}}).to_string()
        } else if url.contains("Files('r-1')/$value") {
            state["repair"].as_str().unwrap().into()
        } else if url.contains("Files('b-1')/$value") {
            state["endpoint_bundle"].as_str().unwrap().into()
        } else if url.contains("Files('g-1')/$value") {
            state["observed_graph"].as_str().unwrap().into()
        } else if url.contains("Workspaces?") {
            json!({"value":[{"entity_id":"ws-1"}]}).to_string()
        } else if url.ends_with("/Agents") {
            json!({"entity_id":"a-1"}).to_string()
        } else if url.ends_with("/Sessions") {
            json!({"entity_id":"s-1"}).to_string()
        } else if url.contains("Paths(") || url.ends_with("/TemperPaw.Configure") {
            "{}".into()
        } else if url == "https://api.typesafe.ai/v1/systemone" {
            let mut answers = json!({});
            for k in ["contradiction", "incentive", "lag", "miracle"] {
                answers[k] = json!({"type":"choice","choice":"unknown","confidence":0.8,"probabilities":{"clear":0.05,"defect":0.05,"unknown":0.9}});
            }
            json!({"model":"jev-1.13.0","answers":answers,"usage":{"input_tokens":100}}).to_string()
        } else {
            return Err(format!("unexpected request {method} {url}"));
        };
        Ok((200, answer))
    }
    fn get_secret(&self, _key: &str) -> Result<String, String> {
        Err("no secret lookup".into())
    }
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unexpected binary request".into())
    }
    fn log(&self, _: &str, _: &str) {}
}
fn module(name: &str) -> Vec<u8> {
    static MODULES: OnceLock<Mutex<BTreeMap<String, Vec<u8>>>> = OnceLock::new();
    let mut modules = MODULES.get_or_init(Default::default).lock().unwrap();
    modules.entry(name.into()).or_insert_with(||{
  let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
  let out=std::process::Command::new("bash").current_dir(&root).args(["-c","set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm \"$1\" wasm32-unknown-unknown --locked","build"]).arg(root.join(format!("os-apps/paw-foresight/wasm/{name}"))).output().unwrap();
  assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
  std::fs::read(String::from_utf8(out.stdout).unwrap().trim()).unwrap()
 }).clone()
}
async fn invoke(name: &str, fields: Value, host: Arc<Host>) -> Value {
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let hash = engine.compile_and_cache(&module(name)).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: if name == "evaluate_semantics" {
            "SemanticEvaluation"
        } else {
            "Path"
        }
        .into(),
        entity_id: "p-1".into(),
        trigger_action: "test".into(),
        wasm_module: Some(name.into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"Repaired","fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([
            ("temper_api_url".into(), "https://temper.test".into()),
            ("typesafe_api_key".into(), "fixture".into()),
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
                &hash,
                &ctx,
                host,
                &WasmResourceLimits::default(),
                Arc::new(RwLock::new(StreamRegistry::default())),
            )
            .await
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn both_evaluators_receive_identical_full_packet_without_refetching_documents() {
    let host = Arc::new(Host {
        calls: Mutex::new(vec![]),
        packet: json!({"state":{"repair":"Repair requires B after A.","endpoint_bundle":"Endpoint requires B BEFORE A. Critical information absent from the earlier experiment.","observed_graph":"A occurs on a fixed date."}}),
    });
    let mut fields = json!({"world_id":"w-1","endpoint_id":"ep-1","repair_log_file_id":"r-1","required_node_ids":"[]","cost_flags":"[]"});
    let prepared = invoke("prepare_challenge", fields.clone(), host.clone()).await;
    assert_eq!(
        prepared["callback_action"], "ChallengePrepared",
        "{prepared}"
    );
    let params = &prepared["callback_params"];
    fields["challenge_packet_json"] = params["challenge_packet_json"].clone();
    fields["challenge_packet_sha256"] = params["challenge_packet_sha256"].clone();
    let raw = fields["challenge_packet_json"].as_str().unwrap();
    let packet: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        packet["state"]["endpoint_bundle"],
        host.packet["state"]["endpoint_bundle"]
    );
    let reads_after_preparation = host
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, url, _)| url.contains("/$value"))
        .count();
    assert_eq!(reads_after_preparation, 3);
    let adversary = invoke("spawn_adversaries", fields.clone(), host.clone()).await;
    assert_eq!(adversary["success"], true, "{adversary}");
    let evaluation = invoke("evaluate_semantics", fields.clone(), host.clone()).await;
    assert_eq!(evaluation["callback_action"], "Record", "{evaluation}");
    {
        let calls = host.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(_, url, _)| url.contains("/$value"))
                .count(),
            3,
            "consumers must not refetch or truncate documents"
        );
        let config: Value = serde_json::from_str(
            &calls
                .iter()
                .find(|(_, url, _)| url.ends_with("/TemperPaw.Configure"))
                .unwrap()
                .2,
        )
        .unwrap();
        assert_eq!(config["tools_enabled"], "temper_action,temper_write");
        assert!(
            config["user_message"]
                .as_str()
                .unwrap()
                .contains("not already represented in state.repair_flags")
        );
        assert!(config["user_message"].as_str().unwrap().ends_with(raw));
        assert!(
            config["user_message"]
                .as_str()
                .unwrap()
                .contains(fields["challenge_packet_sha256"].as_str().unwrap())
        );
        let jev: Value = serde_json::from_str(
            &calls
                .iter()
                .find(|(_, url, _)| url == "https://api.typesafe.ai/v1/systemone")
                .unwrap()
                .2,
        )
        .unwrap();
        assert_eq!(jev["state"], packet["state"]);
        assert_eq!(jev["questions"], packet["questions"]);
    }
    fields["challenge_packet_json"] = json!(raw.replace("BEFORE", "AFTER"));
    let rejected = invoke("evaluate_semantics", fields, host).await;
    assert_eq!(
        rejected["callback_params"]["error_message"],
        "evaluation_packet_hash_mismatch"
    );
}

#[test]
fn a_late_preparation_cannot_replace_a_newer_repair_packet() {
    use temper_jit::TransitionTable;
    use temper_runtime::scheduler::SimActorHandler;
    use temper_server::entity_actor::sim_handler::EntityActorHandler;
    let table = TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/path.ioa.toml"
    ));
    let mut actor = EntityActorHandler::new("Path", "p-1", Arc::new(table));
    actor.init().unwrap();
    actor
        .handle_message(
            "RepairComplete",
            r#"{"repair_log_file_id":"new-log","cost_flags":"[]","required_node_ids":"[]"}"#,
        )
        .unwrap();
    let stale = json!({"expected_repair_log_file_id":"old-log","challenge_packet_json":"old","challenge_packet_sha256":"old"});
    assert!(
        actor
            .handle_message("ChallengePrepared", &stale.to_string())
            .is_err()
    );
    let fresh = json!({"expected_repair_log_file_id":"new-log","challenge_packet_json":"new","challenge_packet_sha256":"new"});
    assert!(
        actor
            .handle_message("ChallengePrepared", &fresh.to_string())
            .is_ok()
    );
}

/// Explicit opt-in acceptance replay. No credentials or live calls in ordinary CI.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires explicit frozen packets, live credential file and output path"]
async fn live_jev_guest_replays_the_frozen_comparison_packets() {
    use temper_wasm::ProductionWasmHost;
    let input = std::env::var("FORESIGHT_COMPARISON_PACKETS").expect("frozen packets path");
    let output = std::env::var("FORESIGHT_COMPARISON_OUTPUT").expect("output path");
    let key_path = std::env::var("FORESIGHT_TYPESAFE_KEY_FILE").expect("credential file path");
    let key = std::fs::read_to_string(key_path).unwrap();
    let packets: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(input).unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let hash = engine
        .compile_and_cache(&module("evaluate_semantics"))
        .unwrap();
    let host = Arc::new(ProductionWasmHost::new(BTreeMap::new()));
    let start = std::time::Instant::now();
    let mut rows = vec![];
    for chunk in packets.chunks(3) {
        let mut jobs=chunk.iter().map(|p| {
            let engine=&engine;let hash=&hash;let key=&key;let host=host.clone();
            async move {
                let ctx=WasmInvocationContext { tenant:"default".into(),entity_type:"SemanticEvaluation".into(),entity_id:p["path_id"].as_str().unwrap().into(),trigger_action:"Evaluate".into(),wasm_module:Some("evaluate_semantics".into()),trigger_params:json!({}),entity_state:json!({"status":"Running","fields":{"parent_id":p["path_id"],"challenge_packet_json":p["packet_json"],"challenge_packet_sha256":p["packet_sha256"]}}),agent_id:None,session_id:None,integration_config:BTreeMap::from([("typesafe_api_key".into(),key.trim().into())]),trace_id:String::new(),workflow_root_entity_type:None,workflow_root_entity_id:None,workflow_run_id:None,http_request:None };
                let elapsed=std::time::Instant::now();
                let result=engine.invoke(hash,&ctx,host,&WasmResourceLimits::default(),Arc::new(RwLock::new(StreamRegistry::default()))).await.unwrap();
                assert_eq!(result.callback_action,"Record","live evaluator did not record: {:?}",result.error);
                let record:Value=serde_json::from_str(result.callback_params["result_json"].as_str().unwrap()).unwrap();
                assert_eq!(record["packet_sha256"],p["packet_sha256"]);
                assert_eq!(record["request"]["state"],p["packet"]["state"]);
                assert_eq!(record["request"]["questions"],p["packet"]["questions"]);
                json!({"path_id":p["path_id"],"packet_sha256":p["packet_sha256"],"seconds":elapsed.elapsed().as_secs_f64(),"record":record})
            }
        });
        async fn optional<F: std::future::Future<Output = Value>>(f: Option<F>) -> Option<Value> {
            match f {
                Some(f) => Some(f.await),
                None => None,
            }
        }
        let (a, b, c) = (jobs.next(), jobs.next(), jobs.next());
        let (a, b, c) = tokio::join!(optional(a), optional(b), optional(c));
        rows.extend([a, b, c].into_iter().flatten());
        std::fs::write(&output,serde_json::to_vec_pretty(&json!({"rows":rows,"batch_seconds":start.elapsed().as_secs_f64(),"implementation":"compiled evaluate_semantics WASM via ProductionWasmHost"})).unwrap()).unwrap();
    }
}
