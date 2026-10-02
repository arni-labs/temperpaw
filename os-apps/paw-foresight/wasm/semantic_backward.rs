// Bound a generation turn, not the ambition or number of original worlds.
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_COMMITMENTS: usize = 3;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub fn enabled(program: &Value) -> bool {
    program["endpoint_search"]["backward_batch_contract"] == 1
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
    json!({"contract":1,"commitments":obligations.into_iter().take(MAX_COMMITMENTS).map(|(_,_,v)|v).collect::<Vec<_>>(),"limits":{"routes":MAX_COMMITMENTS,"hypotheses":24,"research_evidence":8,"response_bytes":MAX_RESPONSE_BYTES}})
}

/// Called before any route mutations. Historical runs retain their old contract.
pub fn validate(program: &Value, generated: &Value) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    if generated.to_string().len() > MAX_RESPONSE_BYTES {
        return Err("Backward batch exceeds 64 KiB; return only the selected commitments and concise shared pieces".into());
    }
    for (key, max) in [
        ("routes", MAX_COMMITMENTS),
        ("hypotheses", 24),
        ("research_evidence", 8),
        ("amendments", MAX_COMMITMENTS),
        ("branches", 24),
    ] {
        if generated[key]
            .as_array()
            .is_some_and(|items| items.len() > max)
        {
            return Err(format!("Backward batch exceeds {max} {key}"));
        }
    }
    let selected = batch(program);
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
