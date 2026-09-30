// Short references are a view of the append-only snapshot, never new identities.
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub const PREFIX: &str = "ref_";

// Select observed evidence for a reasoning view; callers retain the full snapshot
// reference namespace when projecting this view alongside existing hypotheses.
pub fn evidence_snapshot(snapshot: &Value) -> Value {
    json!({"world":snapshot["world"],"nodes":snapshot["nodes"].as_array().into_iter().flatten()
        .filter(|node| matches!(node["kind"].as_str(),Some("evidence"|"research_evidence")))
        .cloned().collect::<Vec<_>>()})
}

pub struct References {
    forward: BTreeMap<String, String>,
    reverse: BTreeMap<String, String>,
}
impl References {
    pub fn new(snapshot: &Value) -> Result<Self, String> {
        let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            let id = node["Id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Missing node identity")?;
            if id.starts_with(PREFIX) {
                return Err("Node identity occupies reserved reference namespace".into());
            }
            let alias = format!("{PREFIX}{:04}", index + 1);
            if forward.insert(id.to_owned(), alias.clone()).is_some() {
                return Err("Duplicate node identity".into());
            }
            reverse.insert(alias, id.to_owned());
        }
        Ok(Self { forward, reverse })
    }
    pub fn resolve(&self, id: &str) -> String {
        self.reverse
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    }
    pub fn project(&self, value: &Value) -> Value {
        self.project_field(value, "")
    }
    fn project_field(&self, value: &Value, field: &str) -> Value {
        match value {
            Value::Object(object) => {
                let mut projected = serde_json::Map::new();
                for (key, value) in object {
                    projected.insert(
                        self.forward.get(key).unwrap_or(key).clone(),
                        self.project_field(value, key),
                    );
                }
                Value::Object(projected)
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| self.project_field(value, field))
                    .collect(),
            ),
            Value::String(text) if field == "edges" => match serde_json::from_str::<Value>(text) {
                Ok(edges) => json!(self.project(&edges).to_string()),
                Err(_) => value.clone(),
            },
            Value::String(text)
                if matches!(
                    field,
                    "Id" | "nodeId"
                        | "to_id"
                        | "target"
                        | "parent"
                        | "hypothesis_id"
                        | "scenario_ids"
                        | "requires"
                        | "world_id"
                        | "component_ids"
                        | "counter_ids"
                        | "evidence_ids"
                        | "from_ids"
                        | "from_id"
                        | "active_world_ids"
                        | "supports"
                        | "candidate_ids"
                        | "pair_ids"
                        | "prior_hypothesis_ids"
                        | "alternative_hypothesis_ids"
                ) =>
            {
                json!(self.forward.get(text).unwrap_or(text))
            }
            _ => value.clone(),
        }
    }
    fn resolve_world_fields(&self, value: &mut Value, field: &str) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    self.resolve_world_fields(value, key);
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.resolve_world_fields(value, field);
                }
            }
            Value::String(text)
                if matches!(
                    field,
                    "world_id"
                        | "component_ids"
                        | "counter_ids"
                        | "evidence_ids"
                        | "from_ids"
                        | "from_id"
                        | "to_id"
                        | "active_world_ids"
                        | "supports"
                        | "candidate_ids"
                        | "pair_ids"
                        | "prior_hypothesis_ids"
                        | "alternative_hypothesis_ids"
                ) =>
            {
                *text = self.resolve(text)
            }
            _ => (),
        }
    }
    pub fn resolve_generated(&self, generated: &mut Value) {
        self.resolve_world_fields(generated, "");
        if let Some(hypotheses) = generated["hypotheses"].as_array_mut() {
            for hypothesis in hypotheses {
                if let Some(parent) = hypothesis["parent"].as_str() {
                    hypothesis["parent"] = json!(self.resolve(parent));
                }
                if let Some(requires) = hypothesis["requires"].as_array_mut() {
                    for reference in requires {
                        if let Some(id) = reference.as_str() {
                            *reference = json!(self.resolve(id));
                        }
                    }
                }
            }
        }
        if let Some(outcomes) = generated["outcomes"].as_array_mut() {
            for outcome in outcomes {
                if let Some(id) = outcome["hypothesis_id"].as_str() {
                    outcome["hypothesis_id"] = json!(self.resolve(id));
                }
                if let Some(references) = outcome["scenario_ids"].as_array_mut() {
                    for reference in references {
                        if let Some(id) = reference.as_str() {
                            *reference = json!(self.resolve(id));
                        }
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_remain_stable_after_append_and_preserve_claim_text() {
        let a = "en-01a0ba3d-11c5-79f1-b578-741b76950dee";
        let b = "en-01a0ba3d-13e3-7bf1-b2ad-8751edb87e9c";
        let s = json!({"nodes":[{"Id":a},{"Id":b}]});
        let refs = References::new(&s).unwrap();
        let projected=refs.project(&json!({"nodes":[{"Id":b,"statement":a,"edges":json!([{"to_id":a}]).to_string()}],"assessments":{a:{"classify_gap":"none"}}}));
        assert_eq!(projected["nodes"][0]["Id"], "ref_0002");
        assert_eq!(projected["nodes"][0]["statement"], a);
        assert_eq!(
            serde_json::from_str::<Value>(projected["nodes"][0]["edges"].as_str().unwrap())
                .unwrap()[0]["to_id"],
            "ref_0001"
        );
        assert_eq!(projected["assessments"]["ref_0001"]["classify_gap"], "none");
        let mut later = s.clone();
        later["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"new"}));
        assert_eq!(References::new(&later).unwrap().resolve("ref_0002"), b);
        assert_eq!(
            refs.resolve("en-01a0ba3d-13e3-7bf1-b578-741b76950dee"),
            "en-01a0ba3d-13e3-7bf1-b578-741b76950dee"
        );
    }
}
