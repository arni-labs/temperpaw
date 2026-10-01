//! Freeze one complete packet before starting either evaluator.
use sha2::{Digest, Sha256};
use temper_wasm_sdk::prelude::*;
fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get("fields")
        .unwrap_or(v)
        .get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
}
fn identifier(s: &str) -> Result<&str, String> {
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid_entity_reference".into());
    }
    Ok(s)
}
fn read(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    path: &str,
) -> Result<String, String> {
    let r = ctx.http_call("GET", &format!("{api}/tdata/{path}"), headers, "")?;
    if !(200..300).contains(&r.status) {
        return Err(format!("snapshot_http_{}", r.status));
    }
    if r.body.len() > 256 * 1024 {
        return Err("source_exceeds_limit".into());
    }
    Ok(r.body)
}

fn prepare(ctx: &Context) -> Result<Value, String> {
    let world = identifier(field(&ctx.entity_state, "world_id"))?;
    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("missing_temper_api_url")?;
    let headers = vec![
        ("content-type".into(), "application/json".into()),
        ("x-tenant-id".into(), ctx.tenant.clone()),
        ("x-temper-principal-kind".into(), "agent".into()),
        ("x-temper-principal-id".into(), ctx.entity_id.clone()),
        ("x-temper-agent-type".into(), "system".into()),
    ];
    let w: Value = serde_json::from_str(&read(ctx, api, &headers, &format!("Worlds('{world}')"))?)
        .map_err(|_| "invalid_world")?;
    match field(&w, "semantic_evaluation_mode") {
        "" | "off" => {
            return Ok(json!({"schema_version":"foresight-evaluation-packet-v2","mode":"off"}));
        }
        "shadow" => {}
        _ => return Err("invalid_semantic_evaluation_mode".into()),
    }
    let repair_id = identifier(field(&ctx.entity_state, "repair_log_file_id"))?;
    let repair = read(ctx, api, &headers, &format!("Files('{repair_id}')/$value"))?;
    let endpoint_id = identifier(field(&ctx.entity_state, "endpoint_id"))?;
    let endpoint: Value = serde_json::from_str(&read(
        ctx,
        api,
        &headers,
        &format!("Endpoints('{endpoint_id}')"),
    )?)
    .map_err(|_| "invalid_endpoint")?;
    if field(&endpoint, "world_id") != world {
        return Err("cross_world_endpoint".into());
    }
    let bundle_id = identifier(field(&endpoint, "bundle_file_id"))?;
    let bundle = read(ctx, api, &headers, &format!("Files('{bundle_id}')/$value"))?;
    let graph_id = identifier(field(&w, "graph_snapshot_file_id"))?;
    let graph = read(ctx, api, &headers, &format!("Files('{graph_id}')/$value"))?;
    if [repair.trim(), bundle.trim(), graph.trim()]
        .iter()
        .any(|s| s.is_empty())
    {
        return Err("empty_evidence_document".into());
    }
    let required: Value = serde_json::from_str(field(&ctx.entity_state, "required_node_ids"))
        .map_err(|_| "invalid_required_node_ids")?;
    let ids = required.as_array().ok_or("invalid_required_node_ids")?;
    if ids.len() > 32 {
        return Err("too_many_required_nodes".into());
    }
    let mut nodes = Vec::new();
    for id in ids {
        let id = identifier(id.as_str().ok_or("invalid_node_id")?)?;
        let n: Value =
            serde_json::from_str(&read(ctx, api, &headers, &format!("EventNodes('{id}')"))?)
                .map_err(|_| "invalid_event_node")?;
        if field(&n, "world_id") != world {
            return Err("cross_world_node".into());
        }
        nodes.push(n.get("fields").cloned().unwrap_or(n));
    }
    let flags = field(&ctx.entity_state, "cost_flags");
    let flags: Value = serde_json::from_str(if flags.is_empty() { "[]" } else { flags })
        .map_err(|_| "invalid_repair_flags")?;
    let state = json!({"world_id":world,"path_id":ctx.entity_id,"observation_cutoff":field(&w,"last_ingest_date"),"target_date":field(&w,"target_date"),"repair":repair,"endpoint_bundle":bundle,"repair_flags":flags,"required_nodes":nodes,"observed_graph":graph});
    let questions: Value = serde_json::from_str(include_str!("../../evaluation_contract.json"))
        .map_err(|_| "invalid_evaluation_contract")?;
    let packet = json!({"schema_version":"foresight-evaluation-packet-v2","mode":"shadow","state":state,"questions":questions});
    if packet.to_string().len() > 128 * 1024 {
        return Err("evaluation_packet_exceeds_128k_bytes".into());
    }
    Ok(packet)
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_ptr: i32, _len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let packet = prepare(&ctx)?.to_string();
        let hash = format!("{:x}", Sha256::digest(packet.as_bytes()));
        set_success_result(
            "ChallengePrepared",
            &json!({"challenge_packet_json":packet,"challenge_packet_sha256":hash,"expected_repair_log_file_id":field(&ctx.entity_state,"repair_log_file_id")}),
        );
        Ok(())
    })();
    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}
