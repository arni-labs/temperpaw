//! chain_proof_ready — one concern: the attached ProofPacket passes validate.py.
//!
//! Fired by Effort.AttachProofPacket. GETs proof_packet_id. On any miss,
//! set_error_result so on_failure retracts proof_attached.
//!
//! Does not dispatch. Does not write rows.

use std::collections::BTreeMap;

use temper_wasm_sdk::prelude::*;

#[cfg(all(target_arch = "wasm32", not(feature = "library")))]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let id = str_of(&ctx, &fields, "proof_packet_id")
            .ok_or_else(|| "chain_proof_ready: missing proof_packet_id".to_string())?;
        let base_url = resolve_api_url(&ctx);
        let headers = odata_headers(&ctx);
        let packet = get_entity(&ctx, &base_url, &headers, "ProofPackets", &id)?;
        proof_packet_holds(&packet, None)?;
        ctx.log(
            "info",
            &format!("chain_proof_ready: ProofPacket {id} holds"),
        );
        set_success_result(
            "",
            &json!({ "status": "proof_ready", "proof_packet_id": id }),
        );
        Ok(())
    })();
    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

pub fn proof_packet_holds(packet: &Value, require_commit: Option<&str>) -> Result<(), String> {
    let fields = packet.get("fields").unwrap_or(packet);
    let status = str_field(fields, "status").or_else(|| str_field(fields, "Status"));
    if status.as_deref() != Some("Recorded") {
        return Err(format!(
            "chain_proof_ready: status {status:?} is not Recorded"
        ));
    }
    if !bool_of(fields, "record_present") {
        return Err("chain_proof_ready: record_present is false".to_string());
    }
    let commit = str_field(fields, "commit")
        .or_else(|| str_field(fields, "Commit"))
        .unwrap_or_default();
    if !is_full_sha(&commit) {
        return Err("chain_proof_ready: commit is not a 40-char sha".to_string());
    }
    if let Some(required) = require_commit
        && required != commit
    {
        return Err(format!(
            "chain_proof_ready: commit {commit} != required {required}"
        ));
    }
    let record = json!({
        "commit": commit,
        "changed_surface": json_field(fields, "changed_surface")?,
        "blast_radius": json_field(fields, "blast_radius")?,
        "features": json_field(fields, "features")?,
        "tests": json_field(fields, "tests")?,
        "independent_verifier": json_field(fields, "independent_verifier")?,
    });
    validate_proof(&record).map_err(|e| format!("chain_proof_ready: {e}"))
}

fn validate_proof(r: &Value) -> Result<(), String> {
    let changed = str_array(r, "changed_surface")?;
    if changed.is_empty() {
        return Err("changed_surface is empty".to_string());
    }
    let blast = str_array(r, "blast_radius")?;
    let features = array(r, "features")?;
    if features.is_empty() {
        return Err("features is empty".to_string());
    }
    let mut verification_by_key: BTreeMap<String, String> = BTreeMap::new();
    for (i, f) in features.iter().enumerate() {
        if !f.is_object() {
            return Err(format!("features[{i}] is not an object"));
        }
        let key = str_req(f, "key").map_err(|e| format!("features[{i}].{e}"))?;
        let verification = str_req(f, "verification").map_err(|e| format!("features[{i}].{e}"))?;
        let verdict = str_req(f, "verdict").map_err(|e| format!("features[{i}].{e}"))?;
        if verdict == "fail" {
            return Err(format!("feature '{key}' has verdict fail"));
        }
        verification_by_key.insert(key.to_string(), verification.to_string());
    }
    let tests = r
        .get("tests")
        .filter(|v| v.is_object())
        .ok_or_else(|| "tests is missing".to_string())?;
    if str_req(tests, "result")? != "pass" {
        return Err("tests.result is not pass".to_string());
    }
    let iv = r
        .get("independent_verifier")
        .filter(|v| v.is_object())
        .ok_or_else(|| "independent_verifier is missing".to_string())?;
    if iv.get("agrees").and_then(|v| v.as_bool()) != Some(true) {
        return Err("independent_verifier.agrees is false".to_string());
    }
    let reran = str_array(iv, "reran")?;
    for key in changed.iter().chain(blast.iter()) {
        match verification_by_key.get(key) {
            None => return Err(format!("changed/blast feature '{key}' is missing")),
            Some(v) if v != "rerun" => {
                return Err(format!(
                    "changed/blast feature '{key}' has verification '{v}', must be rerun"
                ));
            }
            _ => {}
        }
        if !reran.iter().any(|r| r == key) {
            return Err(format!("independent_verifier did not rerun '{key}'"));
        }
    }
    Ok(())
}

fn is_full_sha(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn json_field(fields: &Value, name: &str) -> Result<Value, String> {
    let pascal = pascal(name);
    let raw = fields
        .get(name)
        .or_else(|| fields.get(&pascal))
        .cloned()
        .unwrap_or(json!([]));
    match raw {
        Value::String(s) => {
            serde_json::from_str(&s).map_err(|e| format!("{name} is not JSON: {e}"))
        }
        other => Ok(other),
    }
}

fn array(r: &Value, name: &str) -> Result<Vec<Value>, String> {
    match json_field(r, name)? {
        Value::Array(items) => Ok(items),
        _ => Err(format!("{name} is not an array")),
    }
}

fn str_array(r: &Value, name: &str) -> Result<Vec<String>, String> {
    Ok(array(r, name)?
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect())
}

fn str_req<'a>(r: &'a Value, name: &str) -> Result<&'a str, String> {
    r.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{name} is missing"))
}

fn str_field(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(all(target_arch = "wasm32", not(feature = "library")))]
fn str_of(ctx: &Context, fields: &Value, name: &str) -> Option<String> {
    ctx.trigger_params
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| str_field(fields, name))
        .or_else(|| str_field(fields, &pascal(name)))
}

fn bool_of(fields: &Value, name: &str) -> bool {
    let pascal = pascal(name);
    fields
        .get(name)
        .or_else(|| fields.get(&pascal))
        .and_then(|v| v.as_bool())
        == Some(true)
}

fn pascal(field: &str) -> String {
    field
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(all(target_arch = "wasm32", not(feature = "library")))]
fn resolve_api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|value| !value.is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

#[cfg(all(target_arch = "wasm32", not(feature = "library")))]
fn odata_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

#[cfg(all(target_arch = "wasm32", not(feature = "library")))]
fn get_entity(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
) -> Result<Value, String> {
    if id.is_empty() || id.contains('\'') || id.contains('/') {
        return Err(format!("chain_proof_ready: bad {set} id"));
    }
    let url = format!("{}/tdata/{set}('{id}')", base_url.trim_end_matches('/'));
    let resp = ctx.http_call("GET", &url, headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "chain_proof_ready: GET {set} {id} HTTP {}",
            resp.status
        ));
    }
    serde_json::from_str(&resp.body).map_err(|e| format!("chain_proof_ready: {set} {id} body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn packet() -> Value {
        json!({
            "fields": {
                "status": "Recorded",
                "record_present": true,
                "commit": SHA,
                "changed_surface": "[\"door\"]",
                "blast_radius": "[]",
                "features": "[{\"key\":\"door\",\"verification\":\"rerun\",\"verdict\":\"pass\"}]",
                "tests": "{\"result\":\"pass\"}",
                "independent_verifier": "{\"agrees\":true,\"reran\":[\"door\"]}",
            }
        })
    }

    #[test]
    fn recorded_proof_holds() {
        assert!(proof_packet_holds(&packet(), Some(SHA)).is_ok());
    }

    #[test]
    fn missing_rerun_fails() {
        let mut p = packet();
        p["fields"]["independent_verifier"] = json!("{\"agrees\":true,\"reran\":[]}");
        assert!(
            proof_packet_holds(&p, None)
                .unwrap_err()
                .contains("did not rerun")
        );
    }

    #[test]
    fn fail_verdict_fails() {
        let mut p = packet();
        p["fields"]["features"] =
            json!("[{\"key\":\"door\",\"verification\":\"rerun\",\"verdict\":\"fail\"}]");
        assert!(
            proof_packet_holds(&p, None)
                .unwrap_err()
                .contains("verdict fail")
        );
    }

    #[test]
    fn draft_fails() {
        let mut p = packet();
        p["fields"]["status"] = json!("Drafting");
        assert!(
            proof_packet_holds(&p, None)
                .unwrap_err()
                .contains("not Recorded")
        );
    }
}
