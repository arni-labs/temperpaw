// Comparison bindings describe proposed trajectories, never additional assumed facts.
use super::field;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn contract() -> &'static str {
    "Return comparison_frame:{description,evidence_ids:[existing evidence refs]} once for the set (description 1–800 characters; evidence_ids 1–16 distinct refs). It describes the SAME underlying situation compared across worlds, preserving the original question and horizon, not a new assumed fact or narrowed event scope. Each world returns trajectory_binding:{organizing_component_ids:[its defining component refs],organizing_branch_ids:[its inherited branch IDs],downstream_component_ids:[its defining component refs],counterpart_world_id:another proposed world's local id}. Component arrays contain at most 12 distinct refs each; organizing_branch_ids at most 2048 distinct IDs. Bind downstream changes through existing chain links or branch ancestry; do not invent causal links to satisfy this format. Parallel effects may remain. A missing or unsupported binding remains unresolved. The named counterpart must differ in organizing mechanism and consequences within the common situation, not merely region or topic. Existing event scopes remain exact; a comparison frame does not specialize a broad claim to a chosen place or assume its future conditions true."
}

/// Reject malformed public types; absence remains a truthful unresolved binding.
pub fn validate_proposal(frame: &Value, binding: &Value) -> Result<(), String> {
    for (name, value) in [("comparison_frame", frame), ("trajectory_binding", binding)] {
        if !value.is_null() && !value.is_object() {
            return Err(format!("{name} must be an object or null"));
        }
    }
    for (name, value) in [
        ("comparison_frame.description", &frame["description"]),
        (
            "trajectory_binding.counterpart_world_id",
            &binding["counterpart_world_id"],
        ),
    ] {
        if !value.is_null() && !value.is_string() {
            return Err(format!("{name} must be a string or null"));
        }
    }
    for (name, value) in [
        ("comparison_frame.evidence_ids", &frame["evidence_ids"]),
        (
            "trajectory_binding.organizing_component_ids",
            &binding["organizing_component_ids"],
        ),
        (
            "trajectory_binding.organizing_branch_ids",
            &binding["organizing_branch_ids"],
        ),
        (
            "trajectory_binding.downstream_component_ids",
            &binding["downstream_component_ids"],
        ),
    ] {
        if !value.is_null()
            && value
                .as_array()
                .is_none_or(|items| items.iter().any(|v| !v.is_string()))
        {
            return Err(format!("{name} must be an array of strings or null"));
        }
    }
    Ok(())
}

pub fn frame(snapshot: &Value, proposed: &Value) -> Value {
    json!({"description":proposed["description"],"evidence_ids":proposed["evidence_ids"],"original_question":snapshot["world"]["description"],"horizon":snapshot["world"]["target_date"]})
}

fn references(value: &Value, name: &str, issues: &mut Vec<String>) -> Vec<String> {
    let Some(values) = value.as_array() else {
        issues.push(format!("{name} must be an array"));
        return vec![];
    };
    let maximum = if name == "comparison_frame.evidence_ids" {
        16
    } else if name == "organizing_branch_ids" {
        super::MAX_NODES
    } else {
        12
    };
    if values.len() > maximum || (name == "comparison_frame.evidence_ids" && values.is_empty()) {
        issues.push(format!(
            "{name} has {} items; maximum {maximum}, evidence requires at least one",
            values.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for v in values {
        match v.as_str().filter(|s| !s.is_empty()) {
            Some(id) if seen.insert(id.to_owned()) => (),
            _ => issues.push(format!("{name} needs distinct nonempty identifiers")),
        }
    }
    seen.into_iter().collect()
}

/// Derive support from immutable declared paths, not from narrative similarity.
/// Supported means references and paths exist, not that their mechanisms are true.
pub fn audit(snapshot: &Value, world: &Value, active: &[Value]) -> Value {
    let mut issues = vec![];
    let frame = &world["comparison_frame"];
    if frame["original_question"] != snapshot["world"]["description"]
        || frame["horizon"] != snapshot["world"]["target_date"]
    {
        issues.push("Comparison frame must preserve the original question and horizon".into());
    }
    if frame["description"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 800)
    {
        issues.push("Comparison frame description needs 1–800 characters".into());
    }
    let nodes = snapshot["nodes"].as_array().unwrap();
    for id in references(
        &frame["evidence_ids"],
        "comparison_frame.evidence_ids",
        &mut issues,
    ) {
        if !nodes
            .iter()
            .any(|n| n["Id"] == id && matches!(field(n, "kind"), "evidence" | "research_evidence"))
        {
            issues.push(format!("Unknown comparison evidence {id}"));
        }
    }
    let binding = &world["trajectory_binding"];
    let counterpart = field(binding, "counterpart_world_id");
    if counterpart == field(world, "Id") || !active.iter().any(|n| n["Id"] == counterpart) {
        issues.push("Named counterpart must be another world in this revision".into());
    }
    let components: BTreeSet<_> = world["component_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let organizers = references(
        &binding["organizing_component_ids"],
        "organizing_component_ids",
        &mut issues,
    );
    let branches = references(
        &binding["organizing_branch_ids"],
        "organizing_branch_ids",
        &mut issues,
    );
    let downstream = references(
        &binding["downstream_component_ids"],
        "downstream_component_ids",
        &mut issues,
    );
    if organizers.is_empty() && branches.is_empty() {
        issues.push("No organizing premise is bound".into());
    }
    if downstream.is_empty() {
        issues.push("No downstream event is bound".into());
    }
    for id in organizers.iter().chain(&downstream) {
        if !components.contains(id.as_str()) {
            issues.push(format!("Binding event {id} is not a defining component"));
        }
    }
    for id in &branches {
        if !world["branch_conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["branch_id"] == *id)
        {
            issues.push(format!(
                "Organizing branch {id} is not inherited by this world"
            ));
        }
    }
    let mut paths = vec![];
    for target in &downstream {
        if organizers.contains(target) {
            issues.push(format!(
                "Organizing event {target} cannot be its own downstream consequence"
            ));
            continue;
        }
        // Reachability records a claimed path; every full link retains all its
        // prerequisites. Other contributory routes are not assumed or combined.
        let mut reached: BTreeMap<String, Vec<Value>> =
            organizers.iter().map(|id| (id.clone(), vec![])).collect();
        let links: Vec<_> = world["chain"].as_array().into_iter().flatten().collect();
        for _ in 0..links.len() {
            for link in &links {
                let to = field(link, "to_id");
                if reached.contains_key(to) {
                    continue;
                }
                if let Some(path) = link["from_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .find_map(|id| reached.get(id))
                    .cloned()
                {
                    let mut path = path;
                    path.push((*link).clone());
                    reached.insert(to.into(), path);
                }
            }
        }
        if let Some(path) = reached.get(target).filter(|p| !p.is_empty()) {
            paths.push(json!({"target_event_id":target,"kind":"declared_chain","links":path}));
            continue;
        }
        let node = nodes.iter().find(|n| n["Id"] == *target);
        let state = node
            .and_then(|n| n["branch_id"].as_str())
            .and_then(|id| super::branches::state(snapshot, id, Some(target)).ok());
        let inherited: Vec<_> = state
            .as_ref()
            .into_iter()
            .flat_map(|s| s["conditions"].as_array().into_iter().flatten())
            .filter(|c| branches.iter().any(|id| c["branch_id"] == *id))
            .cloned()
            .collect();
        if !inherited.is_empty() {
            paths.push(json!({"target_event_id":target,"kind":"hypothetical_branch","conditions":inherited}));
        } else {
            issues.push(format!(
                "No declared chain or inherited organizing branch supports {target}"
            ));
        }
    }
    json!({"status":if issues.is_empty(){"supported"}else{"unresolved"},"issues":issues,"paths":paths,"interpretation":"Existing path support only; not established causation, same-situation proof or additional assumptions"})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Vec<Value>) {
        let snapshot = json!({"world":{"description":"How will everyday travel change?","target_date":"2030-01-01"},"nodes":[{"Id":"e","kind":"evidence"},{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"}]});
        let world = json!({"Id":"w1","comparison_contract":"v1","comparison_frame":frame(&snapshot,&json!({"description":"The same travel system","evidence_ids":["e"]})),"component_ids":["a","b","c"],"trajectory_binding":{"organizing_component_ids":["a"],"organizing_branch_ids":[],"downstream_component_ids":["c"],"counterpart_world_id":"w2"},"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","by":"2028-01-01","mechanism":"Declared mechanism"},{"id":"bc","from_ids":["b"],"to_id":"c","by":"2029-01-01","mechanism":"Next mechanism"}]});
        (
            snapshot,
            world,
            vec![json!({"Id":"w1"}), json!({"Id":"w2"})],
        )
    }
    #[test]
    fn malformed_types_are_not_published_as_unresolved_values() {
        assert!(validate_proposal(&Value::Null, &Value::Null).is_ok());
        for frame in [
            json!({"description":{}}),
            json!({"evidence_ids":"e"}),
            json!({"evidence_ids":["e",3]}),
        ] {
            assert!(validate_proposal(&frame, &Value::Null).is_err());
        }
        assert!(
            validate_proposal(&json!({}), &json!("invalid"))
                .unwrap_err()
                .contains("trajectory_binding")
        );
        assert!(validate_proposal(&json!({}), &json!({"organizing_component_ids":[1]})).is_err());
    }

    #[test]
    fn binding_support_requires_actual_paths_and_live_counterpart() {
        let (snapshot, world, active) = fixture();
        let result = audit(&snapshot, &world, &active);
        assert_eq!(result["status"], "supported");
        assert_eq!(result["paths"][0]["links"], world["chain"]);
        for field in ["chain", "trajectory_binding"] {
            let mut invalid = world.clone();
            invalid[field] = Value::Null;
            assert_eq!(audit(&snapshot, &invalid, &active)["status"], "unresolved");
        }
        let mut changed = world.clone();
        changed["trajectory_binding"]["counterpart_world_id"] = json!("archived");
        assert_eq!(audit(&snapshot, &changed, &active)["status"], "unresolved");
        changed = world.clone();
        changed["comparison_frame"]["original_question"] = json!("Different question");
        assert_eq!(audit(&snapshot, &changed, &active)["status"], "unresolved");
        changed = world.clone();
        changed["trajectory_binding"]["downstream_component_ids"] = json!(["outside"]);
        assert_eq!(audit(&snapshot, &changed, &active)["status"], "unresolved");
    }
    #[test]
    fn inherited_branch_can_organize_without_a_positive_component() {
        let (mut snapshot, mut world, active) = fixture();
        snapshot["branches"] = json!([
            {"id":"off","parent_branch_id":null,"by":"2028-01-01","condition":{"kind":"not_all_occurring","event_ids":["a","b"]}},
            {"id":"child","parent_branch_id":"off","by":"2029-01-01","condition":{"kind":"all_occurring","event_ids":["b"]}}
        ]);
        snapshot["nodes"][3]["branch_id"] = json!("child");
        let state = super::super::branches::state(&snapshot, "child", Some("c")).unwrap();
        world["branch_conditions"] = state["conditions"].clone();
        world["trajectory_binding"]["organizing_component_ids"] = json!([]);
        world["trajectory_binding"]["organizing_branch_ids"] = json!(["off"]);
        world["chain"] = json!([]);
        let result = audit(&snapshot, &world, &active);
        assert_eq!(result["status"], "supported");
        assert_eq!(
            result["paths"][0]["conditions"],
            json!([state["conditions"][0]])
        );
        assert_eq!(
            result["paths"][0]["conditions"][0]["events"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        snapshot["nodes"][3]["branch_id"] = Value::Null;
        assert_eq!(audit(&snapshot, &world, &active)["status"], "unresolved");
    }

    #[test]
    fn descriptive_frame_does_not_change_joint_probability_definition() {
        let (mut snapshot, world, _) = fixture();
        let mut world = world;
        world["kind"] = json!("world");
        world["edges"] = json!(json!([{"kind":"requires","to_id":"a"},{"kind":"requires","to_id":"b"},{"kind":"requires","to_id":"c"}]).to_string());
        world["statement"] = json!("Exact joint event");
        world["counter_ids"] = json!([]);
        world["assumptions"] = json!([]);
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(world.clone());
        let task = json!({"nodeId":"w1","function":"estimate_likelihood"});
        let program = json!({"tasks":[task],"cursor":0,"results":{},"evaluations":{}});
        let with = super::super::request(&snapshot, &program).unwrap();
        let node = snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap();
        for key in [
            "comparison_contract",
            "comparison_frame",
            "trajectory_binding",
        ] {
            node.as_object_mut().unwrap().remove(key);
        }
        let without = super::super::request(&snapshot, &program).unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn binding_bounds_match_component_and_evidence_contracts() {
        let (snapshot, mut world, active) = fixture();
        world["comparison_frame"]["evidence_ids"] =
            json!((0..17).map(|i| format!("e{i}")).collect::<Vec<_>>());
        let result = audit(&snapshot, &world, &active);
        assert!(
            result["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("17 items; maximum 16"))
        );
        world["trajectory_binding"]["organizing_component_ids"] =
            json!((0..13).map(|i| format!("c{i}")).collect::<Vec<_>>());
        assert!(
            audit(&snapshot, &world, &active)["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("13 items; maximum 12"))
        );
    }
}
