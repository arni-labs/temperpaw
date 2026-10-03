// Transport-only lossless encoding for a single joint-world judgment. Canonical
// requests and persisted receipts remain untouched; no semantic field is omitted.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const REFERENCE: &str = "$v";
const INSTRUCTION: &str = "Before evaluating, reconstruct state: recursively replace every single-key {$v:N} object in state.encoded_input with the exact original JSON value at state.exact_values[N]. Catalog entries are literal original values, not references to expand. The reconstructed object is the state used by all instructions below, including any common/cases and evidence-set encodings. Preserve every value and array order. Unreferenced catalog values are not additional premises. This encoding omits no evidence, judgment, condition or provenance. ";

fn count(value: &Value, counts: &mut BTreeMap<String, usize>) -> bool {
    match value {
        Value::Object(fields) => {
            if fields.contains_key(REFERENCE) {
                return false;
            }
            for child in fields.values() {
                if !count(child, counts) {
                    return false;
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                if !count(child, counts) {
                    return false;
                }
            }
        }
        _ => (),
    }
    if matches!(value, Value::String(_) | Value::Object(_) | Value::Array(_)) {
        let encoded = value.to_string();
        if encoded.len() > 24 {
            *counts.entry(encoded).or_default() += 1;
        }
    }
    true
}
fn encode(
    value: &Value,
    counts: &BTreeMap<String, usize>,
    indices: &mut BTreeMap<String, usize>,
    catalog: &mut Vec<Value>,
) -> Value {
    let key = value.to_string();
    if counts.get(&key).copied().unwrap_or(0) > 1 {
        let index = *indices.entry(key).or_insert_with(|| {
            catalog.push(value.clone());
            catalog.len() - 1
        });
        return json!({REFERENCE:index});
    }
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), encode(value, counts, indices, catalog)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|value| encode(value, counts, indices, catalog))
                .collect(),
        ),
        _ => value.clone(),
    }
}
// These fields identify execution/cache records. Branch state, source identities,
// rounds, world passes, normalized choices, raw distributions and unknown fields
// are estimate inputs and deliberately are not in this denylist.
const RECEIPTS: &str = "prior_evaluation_receipts";
fn pointer_segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
fn sha256_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.len() == 64 && text.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn take_known(
    fields: &mut serde_json::Map<String, Value>,
    key: &str,
    accepts: fn(&Value) -> bool,
    removed: &mut serde_json::Map<String, Value>,
) {
    if fields.get(key).is_some_and(accepts) {
        removed.insert(key.into(), fields.remove(key).unwrap());
    }
}
fn execution_metadata(
    context: &mut serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut removed = serde_json::Map::new();
    for key in ["prerequisite_input_fingerprint", "audit_input_fingerprint"] {
        take_known(context, key, sha256_value, &mut removed);
    }
    take_known(context, "world_revision", Value::is_u64, &mut removed);
    for key in ["task", "probability_comparison"] {
        let Some(fields) = context.get_mut(key).and_then(Value::as_object_mut) else {
            continue;
        };
        let mut addresses = serde_json::Map::new();
        if key == "task" {
            for field in ["nodeId", "function"] {
                take_known(fields, field, Value::is_string, &mut addresses);
            }
            take_known(fields, "depth", Value::is_u64, &mut addresses);
        } else {
            for field in ["context_hash", "proposition_hash"] {
                take_known(fields, field, sha256_value, &mut addresses);
            }
            take_known(
                fields,
                "component_hashes",
                |value| {
                    value
                        .as_object()
                        .is_some_and(|values| values.values().all(sha256_value))
                },
                &mut addresses,
            );
            if fields.len() == 1 && fields.get("version") == Some(&json!(1)) {
                addresses.insert("version".into(), fields.remove("version").unwrap());
            }
        }
        if !addresses.is_empty() {
            let empty = fields.is_empty();
            removed.insert(key.into(), Value::Object(addresses));
            if empty {
                context.remove(key);
            }
        }
    }
    removed
}
fn project_evaluations(evaluations: &mut Value, path: &str, paths: &mut Vec<String>) {
    let Some(evaluations) = evaluations.as_object_mut() else {
        return;
    };
    for (function, evaluation) in evaluations {
        let Some(context) = evaluation.get_mut("context").and_then(Value::as_object_mut) else {
            continue;
        };
        if !execution_metadata(context).is_empty() {
            paths.push(format!("{path}/{}/context", pointer_segment(function)));
        }
    }
}
fn project_case(state: &mut Value, path: &str, paths: &mut Vec<String>) {
    if let Some(evaluations) = state.get_mut("evaluations") {
        project_evaluations(evaluations, &format!("{path}/evaluations"), paths);
    }
    if let Some(prerequisites) = state.get_mut("prerequisites").and_then(Value::as_array_mut) {
        for (index, prerequisite) in prerequisites.iter_mut().enumerate() {
            if let Some(evaluations) = prerequisite.get_mut("evaluations") {
                project_evaluations(
                    evaluations,
                    &format!("{path}/prerequisites/{index}/evaluations"),
                    paths,
                );
            }
        }
    }
}
fn estimate_inputs(request: &Value) -> Value {
    if request["state"].get(RECEIPTS).is_some() {
        return request.clone();
    }
    let mut projected = request.clone();
    let mut paths = vec![];
    if let Some(cases) = projected["state"]
        .get_mut("cases")
        .and_then(Value::as_object_mut)
    {
        for (key, case) in cases {
            project_case(
                case,
                &format!("/state/cases/{}", pointer_segment(key)),
                &mut paths,
            );
        }
    } else {
        project_case(&mut projected["state"], "/state", &mut paths);
    }
    if !paths.is_empty() {
        projected["state"][RECEIPTS] = json!({
            "canonical_request_sha256":format!("{:x}",Sha256::digest(request.to_string().as_bytes())),
            "context_paths":paths,
            "semantics":"These JSON pointers locate the original prior evaluation contexts in the canonical request. Only execution task addresses and cache/comparison hashes were omitted here. Original receipts remain saved; hashes are identities, not evidence. All judgments, conditions, source IDs, rounds and unknown fields remain estimate inputs."
        });
    }
    projected
}
/// Separate prior judgments from their execution metadata, then losslessly encode
/// repeated values. The canonical request is never mutated.
pub fn project(request: &Value) -> Value {
    let previous = encode_exact(request);
    let projected = encode_exact(&estimate_inputs(request));
    if projected.to_string().len() < previous.to_string().len() {
        projected
    } else {
        previous
    }
}

/// Keep the original request if encoding does not reduce the entire payload or
/// if source data already uses the reserved reference key (avoid collisions).
fn encode_exact(request: &Value) -> Value {
    let mut counts = BTreeMap::new();
    if !count(&request["state"], &mut counts) {
        return request.clone();
    }
    let mut catalog = vec![];
    let encoded = encode(
        &request["state"],
        &counts,
        &mut BTreeMap::new(),
        &mut catalog,
    );
    if catalog.is_empty() {
        return request.clone();
    }
    let mut projected = request.clone();
    projected["state"] = json!({"encoded_input":encoded,"exact_values":catalog});
    if let Some(questions) = projected["questions"].as_object_mut() {
        for question in questions.values_mut() {
            question["instructions"] = json!(format!(
                "{INSTRUCTION}{}",
                question["instructions"].as_str().unwrap_or("")
            ));
        }
    }
    if projected.to_string().len() < request.to_string().len() {
        projected
    } else {
        request.clone()
    }
}

#[cfg(test)]
pub fn restore(request: &Value) -> Value {
    fn expand(value: &Value, catalog: &[Value]) -> Value {
        if let Some(fields) = value.as_object() {
            if fields.len() == 1 && fields.contains_key(REFERENCE) {
                return catalog[fields[REFERENCE].as_u64().unwrap() as usize].clone();
            }
            return Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), expand(value, catalog)))
                    .collect(),
            );
        }
        if let Some(items) = value.as_array() {
            return Value::Array(items.iter().map(|value| expand(value, catalog)).collect());
        }
        value.clone()
    }
    let Some(catalog) = request["state"]["exact_values"].as_array() else {
        return request.clone();
    };
    let mut restored = request.clone();
    restored["state"] = expand(&request["state"]["encoded_input"], catalog);
    for question in restored["questions"].as_object_mut().unwrap().values_mut() {
        question["instructions"] = json!(
            question["instructions"]
                .as_str()
                .unwrap()
                .strip_prefix(INSTRUCTION)
                .unwrap()
        );
    }
    restored
}

#[cfg(test)]
pub fn previous_encoding(request: &Value) -> Value {
    encode_exact(request)
}

#[cfg(test)]
pub fn restore_receipts(request: &Value, canonical: &Value) -> Value {
    let mut restored = restore(request);
    let Some(binding) = restored["state"].as_object_mut().unwrap().remove(RECEIPTS) else {
        return restored;
    };
    assert_eq!(
        binding["canonical_request_sha256"],
        format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
    );
    for path in binding["context_paths"].as_array().unwrap() {
        let path = path.as_str().unwrap();
        let mut original = canonical
            .pointer(path)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        let removed = execution_metadata(&mut original);
        let context = restored.pointer_mut(path).unwrap().as_object_mut().unwrap();
        for (key, value) in removed {
            if let (Some(existing), Some(fields)) = (
                context.get_mut(&key).and_then(Value::as_object_mut),
                value.as_object(),
            ) {
                existing.extend(fields.clone());
            } else {
                context.insert(key, value);
            }
        }
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_execution_metadata_leaves_prior_evaluations() {
        let context = json!({"round":3,"world_pass":1,"world_revision":2,
            "evidence_ids":["source"],"branch_state":{"conditions":[{"event":"c","occurs":false}]},
            "task":{"nodeId":"c","function":"estimate_likelihood","depth":4,"future_semantics":{"scope":"keep exactly"}},
            "prerequisite_input_fingerprint":"a".repeat(64),
            "probability_comparison":{"version":1,"context_hash":"b".repeat(64),"component_hashes":(0..12).map(|i|(format!("component-{i}"),json!(format!("{i:064x}")))).collect::<serde_json::Map<_,_>>(),"unknown_semantics":{"signed":true}},
            "unknown_context":{"date":"2030-12-31"}});
        let original = json!({"state":{"prerequisites":[{"id":"c","evaluations":{"classify_gap":{"selected":"uncertain","answer":{"choice":"evidence","probabilities":{"evidence":0.45,"uncertain":0.55}},"context":context}}}],"baseline":{"facts":["Exact fact"]}},"questions":{"result":{"instructions":"Estimate joint world","type":"noul"}}});
        let packed = project(&original);
        let decoded = restore(&packed);
        let kept = &decoded["state"]["prerequisites"][0]["evaluations"]["classify_gap"];
        assert_eq!(kept["selected"], "uncertain");
        assert_eq!(
            kept["answer"],
            original["state"]["prerequisites"][0]["evaluations"]["classify_gap"]["answer"]
        );
        for key in [
            "round",
            "world_pass",
            "evidence_ids",
            "branch_state",
            "unknown_context",
        ] {
            assert_eq!(kept["context"][key], context[key]);
        }
        assert_eq!(
            kept["context"]["task"],
            json!({"future_semantics":{"scope":"keep exactly"}})
        );
        assert_eq!(
            kept["context"]["probability_comparison"],
            json!({"version":1,"unknown_semantics":{"signed":true}})
        );
        assert!(
            kept["context"]
                .get("prerequisite_input_fingerprint")
                .is_none()
        );
        assert_eq!(
            decoded["state"][RECEIPTS]["context_paths"],
            json!(["/state/prerequisites/0/evaluations/classify_gap/context"])
        );
        assert_eq!(restore_receipts(&packed, &original), original);
        let mut changed = original.clone();
        changed["state"]["prerequisites"][0]["evaluations"]["classify_gap"]["context"]["branch_state"]
            ["conditions"][0]["occurs"] = json!(true);
        assert_ne!(
            Sha256::digest(project(&changed).to_string().as_bytes()),
            Sha256::digest(packed.to_string().as_bytes())
        );
    }

    #[test]
    fn transport_projection_roundtrips_unknown_fields_and_changes_with_evidence() {
        let source = json!({"statement":"Exact dated source claim with important limitations and a sufficiently long identity.","date":"2026-10-03","unknown_semantic_field":{"signed_condition":false}});
        let original = json!({"model":"jev","state":{"sources":vec![source.clone();8],"conditions":[{"event":source,"occurs":false}],"receipt_hash":"0123456789abcdef"},"questions":{"result":{"instructions":"Judge the joint world", "type":"noul"}}});
        let packed = project(&original);
        assert!(packed.to_string().len() < original.to_string().len());
        assert_eq!(restore(&packed), original);
        let mut changed = original.clone();
        changed["state"]["sources"][0]["unknown_semantic_field"]["signed_condition"] = json!(true);
        assert_ne!(project(&changed), packed);
        assert_eq!(restore(&project(&changed)), changed);
        let collision = json!({"state":{"$v":2},"questions":{}});
        assert_eq!(project(&collision), collision);
    }
}
