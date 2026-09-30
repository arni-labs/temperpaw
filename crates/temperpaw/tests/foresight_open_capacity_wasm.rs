//! Simulated provider replies test the actual guest artifact; no live requests.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};
struct Provider {
    calls: AtomicUsize,
    inner: SimWasmHost,
}
#[async_trait::async_trait]
impl WasmHost for Provider {
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        _: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        assert_eq!(method, "POST");
        assert_eq!(url, "https://api.typesafe.ai/v1/systemone");
        self.calls.fetch_add(1, Ordering::SeqCst);
        let q: Value = serde_json::from_str(body).unwrap();
        let mut answers = json!({});
        for (key, q) in q["questions"].as_object().unwrap() {
            let answer = match q["type"].as_str().unwrap() {
                "noul" => json!({"type":"noul","noul":0.37}),
                "score" => {
                    json!({"type":"score","score":2.0,"probabilities":{"0":0.0,"1":0.0,"2":1.0,"3":0.0,"4":0.0}})
                }
                _ => {
                    let opts = q["criteria"].as_object().unwrap();
                    let choice = if opts.contains_key("none") {
                        "none"
                    } else {
                        "monitor"
                    };
                    let probabilities: serde_json::Map<String, Value> = opts
                        .keys()
                        .map(|k| (k.clone(), json!(if k == choice { 1.0 } else { 0.0 })))
                        .collect();
                    json!({"type":"choice","choice":choice,"probabilities":probabilities})
                }
            };
            answers[key] = answer;
        }
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
    async fn http_call_binary(
        &self,
        m: &str,
        u: &str,
        h: &[(String, String)],
        b: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        self.inner.http_call_binary(m, u, h, b).await
    }
    fn get_secret(&self, k: &str) -> Result<String, String> {
        self.inner.get_secret(k)
    }
    fn log(&self, l: &str, m: &str) {
        self.inner.log(l, m)
    }
}
#[test]
fn actual_guest_mixed_primitives_and_5000_trace_boundary() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
 let root=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
 let build=std::process::Command::new("bash").current_dir(&root).args(["-c","source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/semantic_call wasm32-unknown-unknown --locked"]).output().unwrap();assert!(build.status.success(),"{}",String::from_utf8_lossy(&build.stderr));
 let engine=WasmEngine::new().unwrap();let hash=engine.compile_and_cache(&std::fs::read(String::from_utf8(build.stdout).unwrap().trim()).unwrap()).unwrap();
 let functions=["classify_gap","estimate_likelihood","evaluate_novelty","decision_value","choose_next_operation"];
 let nodes:Vec<_>=(0..1000).map(|i|json!({"Id":format!("h{i}"),"kind":"scenario","statement":format!("Hypothesis {i} by 2027"),"resolve_by":"2027","edges":"[]"})).collect();
 let tasks:Vec<_>=(0..1000).flat_map(|i|functions.iter().map(move|f|json!({"nodeId":format!("h{i}"),"function":f,"depth":0}))).collect();
 let snapshot=json!({"world":{"Id":"world","target_date":"2027"},"nodes":nodes});
 let host=Arc::new(Provider{calls:AtomicUsize::new(0),inner:SimWasmHost::new()});
 let invoke=|program:Value,trace:Value|{let engine=&engine;let hash=&hash;let snapshot=&snapshot;let host=host.clone();async move{
 let ctx=WasmInvocationContext{tenant:"test".into(),entity_type:"SemanticRun".into(),entity_id:"capacity".into(),trigger_action:"Evaluate".into(),wasm_module:Some("semantic_call".into()),trigger_params:json!({}),entity_state:json!({"fields":{"program_json":program.to_string(),"trace_json":trace.to_string(),"snapshot_json":snapshot.to_string()},"events":vec![json!({"params":{"trace_json":trace.to_string(),"program_json":program.to_string()}});1]}),agent_id:None,session_id:None,integration_config:BTreeMap::from([("typesafe_api_key".into(),"fixture-only".into())]),trace_id:String::new(),workflow_root_entity_type:None,workflow_root_entity_id:None,workflow_run_id:None,http_request:None};
 serde_json::to_value(engine.invoke(hash,&ctx,host,&WasmResourceLimits{max_memory:256*1024*1024,max_fuel:10_000_000_000,..Default::default()},Arc::new(RwLock::new(StreamRegistry::default()))).await.unwrap()).unwrap()
 }};
 let mut current=json!({"cursor":0,"tasks":tasks,"results":{},"evaluations":{}});let mut current_trace=json!([]);let mut first=json!({});
 for _ in 0..8 { first=invoke(current,current_trace).await;assert_eq!(first["callback_action"],"Recorded","{first}");current=serde_json::from_str(first["callback_params"]["program_json"].as_str().unwrap()).unwrap();current_trace=serde_json::from_str(first["callback_params"]["trace_json"].as_str().unwrap()).unwrap(); }
 assert_eq!(first["callback_action"],"Recorded","{first}");
 let p:Value=serde_json::from_str(first["callback_params"]["program_json"].as_str().unwrap()).unwrap();let trace:Value=serde_json::from_str(first["callback_params"]["trace_json"].as_str().unwrap()).unwrap();
 assert_eq!(p["cursor"],8);assert_eq!(p["evaluations"]["h0"]["estimate_likelihood"]["probability"],0.37);assert_eq!(host.calls.load(Ordering::SeqCst),8);
 assert_eq!(trace[4]["request"]["state_ref"]["evaluations"]["estimate_likelihood"]["probability"],0.37);
 // Capacity fixture repeats real guest-produced trace shapes, not 4992 claimed executions.
 let full:Vec<_>=(0..4992).map(|i|{let mut e=trace[i%8].clone();e["index"]=json!(i);e}).collect();assert!(serde_json::to_string(&full).unwrap().len()<24*1024*1024);
 let mut p=p;p["stage"]=json!("worlds");p["cursor"]=json!(4992);
 let mut current=p;let mut current_trace=json!(full);let mut last=json!({});
 for _ in 0..8 { last=invoke(current,current_trace).await;assert_eq!(last["callback_action"],"Recorded","{last}");current=serde_json::from_str(last["callback_params"]["program_json"].as_str().unwrap()).unwrap();current_trace=serde_json::from_str(last["callback_params"]["trace_json"].as_str().unwrap()).unwrap(); }
 let p:Value=serde_json::from_str(last["callback_params"]["program_json"].as_str().unwrap()).unwrap();let trace:Value=serde_json::from_str(last["callback_params"]["trace_json"].as_str().unwrap()).unwrap();
 assert_eq!(p["cursor"],5000);assert_eq!(trace.as_array().unwrap().len(),5000);assert_eq!(host.calls.load(Ordering::SeqCst),16);
 println!("actual simulated HTTP calls=16; capacity trace entries=5000; serialized trace bytes={}",trace.to_string().len());
 let mut p=p;p["cursor"]=json!(4992);let mut padded=trace;padded.as_array_mut().unwrap().truncate(4992);let length=padded.to_string().len();padded[0]["response"]["capacity_fixture_padding"]=json!("x".repeat(24*1024*1024-100_000-length));
 let stopped=invoke(p,padded).await;assert_eq!(stopped["callback_action"],"Recorded","{stopped}");let p:Value=serde_json::from_str(stopped["callback_params"]["program_json"].as_str().unwrap()).unwrap();assert_eq!(p["stop_reason"],"trace_budget");assert_eq!(host.calls.load(Ordering::SeqCst),16);
});
}
