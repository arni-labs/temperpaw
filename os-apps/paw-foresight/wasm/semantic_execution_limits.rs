// Provider execution limits are not judgments about the imagined future.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn fingerprint(request: &Value) -> String {
    let input = json!({"provider":"typesafe","endpoint":super::endpoints::bridges::PROVIDER_ENDPOINT,"provider_contract":super::endpoints::bridges::PROVIDER_CONTRACT,"request":request});
    format!("{:x}", Sha256::digest(input.to_string().as_bytes()))
}
pub fn lookup_request<'a>(program: &'a Value, task: &Value, request: &Value) -> Option<&'a Value> {
    if task["function"] != "check_transition" {
        return None;
    }
    program["context_limited_checks"]
        .get(fingerprint(request))
        .filter(|receipt| {
            receipt["status"] == "not_evaluated" && receipt["reason"] == "context_limit"
        })
}
pub fn lookup(snapshot: &Value, program: &Value, task: &Value) -> Option<Value> {
    if task["function"] != "check_transition" {
        return None;
    }
    let request = super::evaluation::request_task(snapshot, program, task).ok()?;
    lookup_request(program, task, &request).cloned()
}
pub fn record(
    program: &mut Value,
    task: &Value,
    request: &Value,
    bytes: usize,
    trace: usize,
    http: u64,
) {
    let hash = fingerprint(request);
    program["context_limited_checks"][&hash] = json!({"status":"not_evaluated","reason":"context_limit","input_fingerprint":hash,"task":task,"request_bytes":bytes,"trace_index":trace,"http_call_id":http});
}
pub fn skip_current(snapshot: &Value, program: &mut Value) -> Result<bool, String> {
    let cursor = program["cursor"].as_u64().ok_or("Missing task cursor")? as usize;
    let Some(task) = program["tasks"]
        .as_array()
        .and_then(|tasks| tasks.get(cursor))
    else {
        return Ok(false);
    };
    if lookup(snapshot, program, task).is_none() {
        return Ok(false);
    }
    program["cursor"] = json!(cursor + 1);
    Ok(true)
}
/// Results stay null. This annotates why execution was not possible, distinct
/// from a real uncertain answer and from work which has not yet been attempted.
pub fn annotate_audit(snapshot: &Value, world: &Value, program: &Value, audit: &mut Value) {
    let mut limited = 0;
    for task in super::search::audit_tasks(world, program) {
        let Some(receipt) = lookup(snapshot, program, &task) else {
            continue;
        };
        let id = format!(
            "{}/{}",
            super::field(&task, "nodeId"),
            super::field(&task, "function")
        );
        if let Some(check) = audit["checks"].as_array_mut().and_then(|checks| {
            checks
                .iter_mut()
                .find(|check| check["id"] == id && check["result"].is_null())
        }) {
            check["execution"] = receipt;
            limited += 1;
        }
    }
    audit["context_limited_checks"] = json!(limited);
    let planned = audit["planned_checks"].as_u64().unwrap_or(0);
    let evaluated = audit["completed_checks"].as_u64().unwrap_or(0);
    audit["pending_checks"] = json!(planned.saturating_sub(evaluated + limited));
    // Preserve the existing status vocabulary; explicit execution receipts carry
    // the distinction, and a limited check can never make a route checked.
}
