// Bound a generation turn, not the ambition or number of original worlds.
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_COMMITMENTS: usize = 3;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub fn enabled(program: &Value) -> bool {
    matches!(
        program["endpoint_search"]["backward_batch_contract"].as_u64(),
        Some(1 | 2)
    )
}

/// Deterministic from persisted state: retries select the same obligations.
/// Missing commitments precede alternatives; least explored alternatives go first.
pub fn batch(program: &Value) -> Value {
    let routes: Vec<_> = program["endpoint_search"]["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let mut obligations = vec![];
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for commitment in endpoint["commitments"].as_array().into_iter().flatten() {
            let related: Vec<_> = routes
                .iter()
                .filter(|r| {
                    r["endpoint_id"] == endpoint["id"] && r["commitment_id"] == commitment["id"]
                })
                .collect();
            let unresolved = related.iter().find(|r| {
                matches!(r["status"].as_str(), Some("blocked" | "unresolved"))
                    && !routes.iter().any(|a| a["alternative_to"] == r["id"])
            });
            let priority = if related.is_empty() {
                0
            } else if unresolved.is_some() {
                1
            } else {
                2
            };
            obligations.push((priority, related.len(), json!({"endpoint_id":endpoint["id"],"commitment_id":commitment["id"],"statement":commitment["statement"],"alternative_to":unresolved.map(|r|r["id"].clone()),"reason":if priority == 0 {"missing_route"} else if priority == 1 {"unresolved_route"} else {"further_alternative"}})));
        }
    }
    obligations.sort_by_key(|(priority, attempts, _)| (*priority, *attempts));
    let complete_original = program["endpoint_search"]["backward_batch_contract"] == 2
        && obligations
            .first()
            .is_some_and(|(priority, _, _)| *priority == 0);
    let selected: Vec<Value> = if complete_original {
        // One coherent original per initial turn. Do not fragment every world
        // before there is any complete set of commitments to reconstruct.
        let endpoint = obligations[0].2["endpoint_id"].clone();
        obligations
            .into_iter()
            .filter(|(priority, _, item)| *priority == 0 && item["endpoint_id"] == endpoint)
            .map(|(_, _, item)| item)
            .collect()
    } else {
        obligations
            .into_iter()
            .take(MAX_COMMITMENTS)
            .map(|(_, _, item)| item)
            .collect()
    };
    let route_limit = if program["endpoint_search"]["backward_batch_contract"] == 1 {
        MAX_COMMITMENTS
    } else {
        selected.len()
    };
    json!({"contract":program["endpoint_search"]["backward_batch_contract"],"mode":if complete_original {"complete_original"} else {"alternatives"},"commitments":selected,"limits":{"routes":route_limit,"hypotheses":24,"research_evidence":8,"response_bytes":MAX_RESPONSE_BYTES}})
}

/// Called before any route mutations. Historical runs retain their old contract.
pub fn validate(program: &Value, generated: &Value) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    if generated.to_string().len() > MAX_RESPONSE_BYTES {
        return Err("Backward batch exceeds 64 KiB; return only the selected commitments and concise shared pieces".into());
    }
    let selected = batch(program);
    let route_limit = selected["limits"]["routes"].as_u64().unwrap_or(0) as usize;
    for (key, max) in [
        ("routes", route_limit),
        ("hypotheses", 24),
        ("research_evidence", 8),
        ("amendments", route_limit),
        ("branches", 24),
    ] {
        if generated[key]
            .as_array()
            .is_some_and(|items| items.len() > max)
        {
            return Err(format!("Backward batch exceeds {max} {key}"));
        }
    }
    let mut seen = BTreeSet::new();
    for route in generated["routes"].as_array().into_iter().flatten() {
        let obligation = selected["commitments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["endpoint_id"] == route["endpoint_id"]
                    && c["commitment_id"] == route["commitment_id"]
            })
            .ok_or("Backward route is outside the selected commitment batch")?;
        let key = (
            route["endpoint_id"].to_string(),
            route["commitment_id"].to_string(),
        );
        if !seen.insert(key) {
            return Err(
                "Return at most one route for each selected commitment in this turn".into(),
            );
        }
        if !obligation["alternative_to"].is_null()
            && route["alternative_to"] != obligation["alternative_to"]
        {
            return Err("Backward batch must address the selected unresolved route with its exact alternative_to ID".into());
        }
    }
    for amendment in generated["amendments"].as_array().into_iter().flatten() {
        if !selected["commitments"].as_array().unwrap().iter().any(|c| {
            c["endpoint_id"] == amendment["endpoint_id"]
                && c["commitment_id"] == amendment["commitment_id"]
        }) {
            return Err("Backward amendment is outside the selected commitment batch".into());
        }
    }
    Ok(())
}

/// World-first routing chooses obligations by coverage and failed mechanisms,
/// not forward-search novelty/value rankings. Keep every factual and causal check.
pub fn retain_route_assessments(program: &mut Value) {
    if program["world_search_contract"] == 1
        && program["endpoint_search"]["backward_batch_contract"] == 2
        && let Some(tasks) = program["tasks"].as_array_mut()
    {
        tasks.retain(|task| {
            !matches!(
                task["function"].as_str(),
                Some("evaluate_novelty" | "decision_value")
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program() -> Value {
        json!({"endpoint_search":{"backward_batch_contract":1,"endpoints":(0..6).map(|e|json!({"id":format!("e{e}"),"commitments":(0..8).map(|c|json!({"id":format!("c{c}"),"statement":format!("Original {e}/{c}")})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"routes":[]}})
    }
    #[test]
    fn chunks_cover_every_original_before_retrying_failed_routes() {
        let mut p = program();
        let mut covered = BTreeSet::new();
        for round in 0..16 {
            let b = batch(&p);
            assert_eq!(b, batch(&p));
            let routes:Vec<_>=b["commitments"].as_array().unwrap().iter().enumerate().map(|(i,c)| {
                assert_eq!(c["reason"],"missing_route");
                assert!(covered.insert((c["endpoint_id"].to_string(),c["commitment_id"].to_string())));
                json!({"id":format!("r{round}_{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})
            }).collect();
            validate(&p, &json!({"routes":routes})).unwrap();
            p["endpoint_search"]["routes"]
                .as_array_mut()
                .unwrap()
                .extend(routes);
        }
        assert_eq!(covered.len(), 48);
        let first = batch(&p);
        for (i, c) in first["commitments"].as_array().unwrap().iter().enumerate() {
            assert_eq!(c["reason"], "unresolved_route");
            p["endpoint_search"]["routes"].as_array_mut().unwrap().push(json!({"id":format!("alt{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"alternative_to":c["alternative_to"],"status":"unresolved"}));
        }
        assert_ne!(
            batch(&p)["commitments"][0]["commitment_id"],
            first["commitments"][0]["commitment_id"]
        );
    }
    #[test]
    fn complete_original_batches_cover_six_worlds_in_six_turns_before_alternatives() {
        let mut p = program();
        p["endpoint_search"]["backward_batch_contract"] = json!(2);
        for round in 0..6 {
            let selected = batch(&p);
            assert_eq!(selected["mode"], "complete_original");
            assert_eq!(selected["commitments"].as_array().unwrap().len(), 8);
            assert_eq!(selected["limits"]["hypotheses"], 24);
            let routes: Vec<_> = selected["commitments"].as_array().unwrap().iter().enumerate().map(|(i,c)| {
                assert_eq!(c["endpoint_id"], format!("e{round}"));
                json!({"id":format!("r{round}_{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})
            }).collect();
            validate(&p, &json!({"routes":routes})).unwrap();
            p["endpoint_search"]["routes"]
                .as_array_mut()
                .unwrap()
                .extend(routes);
        }
        let alternatives = batch(&p);
        assert_eq!(alternatives["mode"], "alternatives");
        assert_eq!(alternatives["commitments"].as_array().unwrap().len(), 3);
        assert!(
            alternatives["commitments"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["reason"] == "unresolved_route")
        );
        assert!(validate(&p, &json!({"routes":vec![json!({});4]})).is_err());
    }

    #[test]
    fn route_planning_removes_only_unused_rankings_and_preserves_legacy() {
        let functions = [
            "classify_claim_role",
            "classify_temporal",
            "classify_gap",
            "estimate_likelihood",
            "estimate_conditional",
            "check_transition",
            "evaluate_novelty",
            "decision_value",
        ];
        let mut p = json!({"world_search_contract":1,"endpoint_search":{"backward_batch_contract":2},"tasks":functions.iter().map(|f|json!({"function":f})).collect::<Vec<_>>()});
        let original = p.clone();
        retain_route_assessments(&mut p);
        assert_eq!(p["tasks"].as_array().unwrap().len(), 6);
        for function in &functions[..6] {
            assert!(
                p["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["function"] == *function)
            );
        }
        let mut legacy = original.clone();
        legacy["world_search_contract"] = json!(0);
        let before = legacy.clone();
        retain_route_assessments(&mut legacy);
        assert_eq!(legacy, before);
        let mut v1 = original;
        v1["endpoint_search"]["backward_batch_contract"] = json!(1);
        let before = v1.clone();
        retain_route_assessments(&mut v1);
        assert_eq!(v1, before);
    }

    #[test]
    fn rejects_oversized_outside_and_duplicate_responses_without_changing_state() {
        let p = program();
        let original = p.clone();
        let r = json!({"endpoint_id":"e0","commitment_id":"c0"});
        assert!(validate(&p, &json!({"routes":[r.clone(),r.clone()]})).is_err());
        assert!(
            validate(
                &p,
                &json!({"routes":[{"endpoint_id":"e5","commitment_id":"c7"}]})
            )
            .is_err()
        );
        assert!(validate(&p, &json!({"hypotheses":vec![json!({});25]})).is_err());
        assert!(
            validate(
                &p,
                &json!({"exploration_note":"x".repeat(MAX_RESPONSE_BYTES)})
            )
            .is_err()
        );
        assert_eq!(p, original);
        let mut legacy = p.clone();
        legacy["endpoint_search"]
            .as_object_mut()
            .unwrap()
            .remove("backward_batch_contract");
        assert!(validate(&legacy, &json!({"routes":vec![r;48]})).is_ok());
    }
}
