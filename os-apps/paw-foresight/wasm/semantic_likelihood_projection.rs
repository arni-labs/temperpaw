// Transport-only lossless encoding for a single joint-world judgment. Canonical
// requests and persisted receipts remain untouched; no semantic field is omitted.
use serde_json::{Value, json};
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
/// Keep the original request if encoding does not reduce the entire payload or
/// if source data already uses the reserved reference key (avoid collisions).
pub fn project(request: &Value) -> Value {
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
mod tests {
    use super::*;
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
