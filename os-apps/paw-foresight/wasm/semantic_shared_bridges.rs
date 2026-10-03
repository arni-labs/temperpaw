// Explicit shared mechanisms. Identity never implies equal assessment context.
use super::{field, identifier, list, text};
use serde_json::{Value, json};
use std::collections::BTreeSet;
fn exact_keys(value: &Value, allowed: &[&str]) -> Result<(), String> {
    let object = value.as_object().ok_or("Shared bridge must be an object")?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Unknown shared bridge semantic field: {key}"));
    }
    Ok(())
}
pub fn materialize(reply: &mut Value, search: &mut Value) -> Result<(), String> {
    if reply.get("bridges").is_none() && reply.get("shared_bridge_contract").is_none() {
        return Ok(());
    }
    if reply["shared_bridge_contract"] != 1 {
        return Err("Unsupported shared bridge contract".into());
    }
    let declarations = reply["bridges"]
        .as_array()
        .filter(|v| v.len() <= 48)
        .ok_or("Expected bounded bridge declarations")?;
    let mut registry = search.get("bridges").cloned().unwrap_or(json!({}));
    for bridge in declarations {
        exact_keys(
            bridge,
            &[
                "id",
                "from_ids",
                "to_id",
                "by",
                "mechanism",
                "assumptions",
                "endpoint_dependencies",
            ],
        )?;
        let id = identifier(&bridge["id"])?;
        if id.starts_with("ref_") || registry.get(id).is_some() {
            return Err("Shared bridge identity is immutable or reserved".into());
        }
        list(&bridge["from_ids"], 24)?;
        identifier(&bridge["to_id"])?;
        text(&bridge["by"], 32)?;
        text(&bridge["mechanism"], 800)?;
        for assumption in bridge["assumptions"]
            .as_array()
            .filter(|v| v.len() <= 16)
            .ok_or("Missing bridge assumptions")?
        {
            text(assumption, 800)?;
        }
        let dependencies = list(&bridge["endpoint_dependencies"], 12)?;
        if dependencies.is_empty() {
            return Err("Bridge needs original endpoint dependencies".into());
        }
        for dependency in dependencies {
            if !search["endpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|e| e["id"] == dependency)
            {
                return Err("Unknown bridge endpoint dependency".into());
            }
        }
        registry[id] = bridge.clone();
    }
    for route in reply["routes"].as_array_mut().into_iter().flatten() {
        let endpoint = route["endpoint_id"].clone();
        for link in route["chain"].as_array_mut().into_iter().flatten() {
            if let Some(reference) = link.get("bridge_ref").cloned() {
                exact_keys(link, &["bridge_ref"])?;
                let bridge = registry
                    .get(reference.as_str().ok_or("Invalid bridge reference")?)
                    .ok_or("Unknown shared bridge reference")?;
                if !bridge["endpoint_dependencies"]
                    .as_array()
                    .unwrap()
                    .contains(&endpoint)
                {
                    return Err("Bridge does not declare this endpoint dependency".into());
                }
                *link = json!({"id":reference,"bridge_ref":reference,"from_ids":bridge["from_ids"],"to_id":bridge["to_id"],"by":bridge["by"],"mechanism":bridge["mechanism"],"assumptions":bridge["assumptions"]});
            }
        }
    }
    search["shared_bridge_contract"] = json!(1);
    search["bridges"] = registry;
    Ok(())
}
/// Preserve unknown definition fields; remove only recorded execution addresses.
fn semantic_branch(mut branch: Value) -> Value {
    if let Some(map) = branch.as_object_mut() {
        for key in ["id", "world_id", "link_id", "parent_state_ids"] {
            map.remove(key);
        }
        if let Some(history) = map.get_mut("history").and_then(Value::as_array_mut) {
            for entry in history {
                if let Some(m) = entry.as_object_mut() {
                    m.remove("state_id");
                    m.remove("link_id");
                }
            }
        }
    }
    branch
}
pub fn project_request(
    snapshot: &Value,
    program: &Value,
    task: &Value,
    request: &mut Value,
) -> Result<(), String> {
    if task["function"] != "check_transition" {
        return Ok(());
    }
    let Some(reference) = request["state"]["link"]["bridge_ref"].as_str() else {
        return Ok(());
    };
    let bridge = program["endpoint_search"]["bridges"]
        .get(reference)
        .ok_or("Missing canonical shared bridge")?;
    let link = &request["state"]["link"];
    for key in ["from_ids", "to_id", "by", "mechanism", "assumptions"] {
        if link[key] != bridge[key] {
            return Err(format!("Materialized bridge changed {key}"));
        }
    }
    exact_keys(
        link,
        &[
            "id",
            "bridge_ref",
            "from_ids",
            "to_id",
            "by",
            "mechanism",
            "assumptions",
        ],
    )?;
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let state = &request["state"];
    let mut ids = BTreeSet::new();
    for id in bridge["from_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(&bridge["to_id"]))
    {
        ids.insert(id.as_str().ok_or("Invalid bridge event")?.to_owned());
    }
    for key in [
        "prerequisite_events",
        "ancestor_events",
        "unassigned_route_events",
    ] {
        for node in state[key].as_array().into_iter().flatten() {
            ids.insert(field(node, "Id").to_owned());
        }
    }
    let definitions: Vec<_> = ids
        .iter()
        .map(|id| {
            nodes
                .iter()
                .find(|n| n["Id"] == *id)
                .cloned()
                .ok_or("Missing bridge definition")
        })
        .collect::<Result<_, _>>()?;
    let mut dependencies = Vec::new();
    for id in bridge["endpoint_dependencies"].as_array().unwrap() {
        let original = program["endpoint_search"]["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["id"] == *id)
            .ok_or("Missing bridge dependency")?;
        let amendments: Vec<_> = program["endpoint_search"]["amendments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["endpoint_id"] == *id)
            .collect();
        let mut novelty = program["endpoint_novelty"][id.as_str().unwrap()].clone();
        // Prior comparison request bodies repeat old research; their judgments
        // remain limitations, not new evidence. Canonical receipts stay stored.
        for check in ["initial_check", "final_check"] {
            if let Some(object) = novelty[check].as_object_mut() {
                object.remove("request");
                object.remove("task");
            }
        }
        dependencies
            .push(json!({"original":original,"amendments":amendments,"novelty_admission":novelty}));
    }
    let judgments:Value=ids.iter().map(|id| (id.clone(),json!({"normalized":program["results"][id],"evaluations":program["evaluations"][id]}))).collect();
    // Start from the complete structural state: future unknown fields survive.
    let mut local = state.clone();
    local
        .as_object_mut()
        .unwrap()
        .remove("previous_world_judgments");
    local["assessment_contract"] = json!("shared_bridge_v1");
    local["bridge"] = bridge.clone();
    local["link"] = bridge.clone();
    local["branch_state"] = semantic_branch(state["branch_state"].clone());
    let world = nodes
        .iter()
        .find(|node| node["Id"] == task["world_id"])
        .ok_or("Missing bridge route")?;
    let mut extensions = world.clone();
    for key in [
        "Id",
        "kind",
        "route_only",
        "archived",
        "statement",
        "component_ids",
        "counter_ids",
        "chain",
        "assumptions",
        "edges",
        "endpoint_id",
        "grounding_evidence_ids",
        "root_connections",
    ] {
        extensions.as_object_mut().unwrap().remove(key);
    }
    // Unknown route/world fields stay in the input, preventing unsafe reuse.
    local["route_context_extensions"] = extensions;
    let history_ids: BTreeSet<_> = state["branch_state"]["history"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["link_id"].as_str())
        .collect();
    let ancestors: Vec<_> = world["chain"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| history_ids.contains(field(link, "id")))
        .map(|link| {
            let mut value = link.clone();
            value.as_object_mut().unwrap().remove("id");
            value
        })
        .collect();
    local["ancestor_mechanisms"] = json!(ancestors);
    local["root_connections"] = json!(
        world["root_connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|root| ids.contains(field(root, "component_id")))
            .collect::<Vec<_>>()
    );
    let qualifications:Vec<_>=definitions.iter().filter(|node|node["branch_id"].as_str().is_some_and(|s|!s.is_empty())).map(|node| Ok(json!({"event_id":node["Id"],"qualification":super::super::branches::state(snapshot,field(node,"branch_id"),Some(field(node,"Id")))?}))).collect::<Result<_,String>>()?;
    local["event_qualifications"] = json!(qualifications);
    local["definitions"] = json!(definitions);
    local["declared_dependencies"] = json!(dependencies);
    local["prerequisite_judgments"] = judgments;
    local["snapshot_branches"] = snapshot["branches"].clone();
    local["world"]["bridge_assumptions"] = bridge["assumptions"].clone();
    request["state"] = local;
    request["questions"]["result"]["instructions"] = json!(format!(
        "{} This is shared_bridge_v1: assess only this bridge under its explicit ancestor state and assumptions. Declared endpoint originals and amendments preserve scope, not events assumed to have happened; do not condition on downstream outcomes. Event qualifications preserve signed branch restrictions. Prior raw and normalized judgments are model assessments, not source evidence. Different containing routes do not create independent confirmation.",
        field(&request["questions"]["result"], "instructions")
    ));
    Ok(())
}
pub const PROVIDER_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const PROVIDER_CONTRACT: &str = "typesafe.systemone.v1";
fn fingerprint_for(
    request: &Value,
    provider: &str,
    endpoint: &str,
    contract: &str,
) -> Option<String> {
    if request["state"]["assessment_contract"] != "shared_bridge_v1" {
        return None;
    }
    use sha2::{Digest, Sha256};
    let identity = json!({"provider":provider,"endpoint":endpoint,"provider_contract":contract,"request":request});
    Some(format!(
        "{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    ))
}
pub fn fingerprint(request: &Value) -> Option<String> {
    fingerprint_for(request, "typesafe", PROVIDER_ENDPOINT, PROVIDER_CONTRACT)
}
/// A receipt is owned by one real provider question. Consumers retain provenance.
pub fn record(
    program: &mut Value,
    request: &Value,
    evaluation: &Value,
    decision: &str,
    trace_index: usize,
    http_call: &Value,
) {
    if let Some(key) = fingerprint(request) {
        program["shared_bridge_receipts"][&key] = json!({"input_fingerprint":key,"evaluation":evaluation,"decision":decision,"trace_index":trace_index,"http_call_id":http_call});
    }
}
pub fn reuse_current(snapshot: &Value, program: &mut Value) -> Result<bool, String> {
    let cursor = program["cursor"].as_u64().unwrap_or(0) as usize;
    let Some(task) = program["tasks"]
        .as_array()
        .and_then(|tasks| tasks.get(cursor))
        .cloned()
    else {
        return Ok(false);
    };
    if task["function"] != "check_transition" {
        return Ok(false);
    }
    let request = super::super::search::request(snapshot, program, &task)?;
    let Some(key) = fingerprint(&request) else {
        return Ok(false);
    };
    let Some(receipt) = program["shared_bridge_receipts"].get(&key).cloned() else {
        return Ok(false);
    };
    let node = field(&task, "nodeId");
    let mut evaluation = receipt["evaluation"].clone();
    evaluation["context"]["task"] = task.clone();
    evaluation["context"]["branch_state"] = request["state"]["branch_state"].clone();
    evaluation["context"]["audit_input_fingerprint"] = json!(
        super::super::search::audit_input_fingerprint(snapshot, &task, &request)
    );
    evaluation["context"]["shared_bridge_reuse"] = json!({"input_fingerprint":key,"trace_index":receipt["trace_index"],"http_call_id":receipt["http_call_id"],"meaning":"Same complete assessment input; not an independent provider judgment"});
    program["results"][node]["check_transition"] = receipt["decision"].clone();
    program["evaluations"][node]["check_transition"] = evaluation;
    program["cursor"] = json!(cursor + 1);
    Ok(true)
}
#[cfg(test)]
mod tests {
    include!("semantic_shared_bridge_tests.rs");
}
