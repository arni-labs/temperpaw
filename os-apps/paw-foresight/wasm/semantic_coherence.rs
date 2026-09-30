use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
fn proposition(node: &Value) -> Value {
    json!({"id":node["Id"],"statement":node["statement"],"resolve_by":node["resolve_by"],"component_ids":node["component_ids"],"branch_conditions":node["branch_conditions"]})
}
/// Compare only exact question/baseline/source content, not merely matching source IDs.
pub fn receipt(state: &Value) -> Value {
    let mut components = json!({});
    for prerequisite in state["prerequisites"].as_array().into_iter().flatten() {
        if let Some(id) = prerequisite["id"].as_str() {
            components[id] = json!(hash(&proposition(&prerequisite["node"])));
        }
    }
    json!({"version":1,"context_hash":hash(&json!({"world":state["world"],"baseline":state["baseline"],"source_evidence":state["source_evidence"]})),"proposition_hash":hash(&proposition(&state["node"])),"component_hashes":components})
}
fn probability(program: &Value, id: &str) -> Option<f64> {
    let evaluation = &program["evaluations"][id]["estimate_likelihood"];
    if evaluation["type"] != "noul" {
        return None;
    }
    let p = evaluation["probability"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))?;
    let current = program["results"][id]["estimate_likelihood"]
        .as_str()?
        .parse::<f64>()
        .ok()?;
    (p == current).then_some(p)
}
/// Independent estimates may disagree without the world itself being impossible.
pub fn audit(world: &Value, program: &Value) -> Value {
    let id = super::field(world, "Id");
    let joint = &program["evaluations"][id]["estimate_likelihood"];
    let context = &joint["context"]["probability_comparison"];
    let mut findings = vec![];
    let mut compared = 0;
    if let Some(p) = probability(program, id)
        && context["version"] == 1
        && context["proposition_hash"] == hash(&proposition(world))
    {
        for component in world["component_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let marginal = &program["evaluations"][component]["estimate_likelihood"];
            let other = &marginal["context"]["probability_comparison"];
            let Some(q) = probability(program, component) else {
                continue;
            };
            if other["version"] != 1
                || !context["context_hash"].is_string()
                || context["context_hash"] != other["context_hash"]
                || !context["component_hashes"][component].is_string()
                || context["component_hashes"][component] != other["proposition_hash"]
            {
                continue;
            }
            compared += 1;
            if p > q + 1e-12 {
                findings.push(json!({"kind":"joint_exceeds_component","world_id":id,"component_id":component,"joint_probability":p,"component_probability":q}));
            }
        }
    }
    json!({"status":if !findings.is_empty() {"inconsistent"} else if compared > 0 {"no_violation_found"} else {"not_comparable"},"compared_components":compared,"findings":findings})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_context_detects_joint_violation_without_repairing_odds() {
        let node = json!({"Id":"h","statement":"Event","resolve_by":"2036-10-01"});
        let world = json!({"Id":"w","statement":"Joint event","component_ids":["h"]});
        let base = json!({"world":{"target_date":"2036-10-01"},"baseline":{"as_of":"2026-10-01"},"source_evidence":[{"Id":"e","statement":"Exact observed fact"}]});
        let mut marginal = base.clone();
        marginal["node"] = node.clone();
        let mut joint = base;
        joint["node"] = world.clone();
        joint["prerequisites"] = json!([{"id":"h","node":node}]);
        let mut p = json!({"evaluations":{"h":{"estimate_likelihood":{"type":"noul","probability":0.42,"context":{"probability_comparison":receipt(&marginal)}}},"w":{"estimate_likelihood":{"type":"noul","probability":0.49,"context":{"probability_comparison":receipt(&joint)}}}}});
        p["results"] =
            json!({"h":{"estimate_likelihood":"0.42"},"w":{"estimate_likelihood":"0.49"}});
        let original = p.clone();
        assert_eq!(audit(&world, &p)["status"], "inconsistent");
        assert_eq!(p, original);
        for key in ["world", "baseline", "source_evidence"] {
            let mut changed = marginal.clone();
            changed[key] = json!({"changed":true});
            p["evaluations"]["h"]["estimate_likelihood"]["context"]["probability_comparison"] =
                receipt(&changed);
            assert_eq!(audit(&world, &p)["status"], "not_comparable");
        }
        p = original.clone();
        p["evaluations"]["h"]["estimate_likelihood"]["context"] = json!({});
        assert_eq!(audit(&world, &p)["status"], "not_comparable");
        p = original.clone();
        p["results"]["h"]["estimate_likelihood"] = Value::Null;
        assert_eq!(audit(&world, &p)["status"], "not_comparable");
        p = original;
        p["evaluations"]["w"]["estimate_likelihood"]["probability"] = json!(0.4);
        p["results"]["w"]["estimate_likelihood"] = json!("0.4");
        assert_eq!(audit(&world, &p)["status"], "no_violation_found");
    }
}
