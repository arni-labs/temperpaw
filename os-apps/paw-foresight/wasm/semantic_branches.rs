// Immutable hypothetical conditions. Evidence and marginal probabilities never become assignments.
use super::field;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn strings(v: &Value) -> Result<Vec<&str>, String> {
    v.as_array()
        .ok_or("Branch event_ids must be an array")?
        .iter()
        .map(|v| v.as_str().ok_or("Branch event ID must be a string".into()))
        .collect()
}
fn hypothesis<'a>(snapshot: &'a Value, id: &str) -> Result<&'a Value, String> {
    snapshot["nodes"]
        .as_array()
        .ok_or("Missing nodes")?
        .iter()
        .find(|n| field(n, "Id") == id && matches!(field(n, "kind"), "scenario" | "revision"))
        .ok_or_else(|| format!("Branch premise {id} is not an existing hypothesis"))
}
/// Materialize parent-first clauses, without choosing an individual failed event.
pub fn state(snapshot: &Value, id: &str, target: Option<&str>) -> Result<Value, String> {
    let branches = snapshot["branches"]
        .as_array()
        .ok_or("Missing branch catalog")?;
    let mut chain = vec![];
    let mut seen = BTreeSet::new();
    let mut current = id;
    while !current.is_empty() {
        if !seen.insert(current) {
            return Err("Cyclic hypothetical branch inheritance".into());
        }
        let branch = branches
            .iter()
            .find(|b| field(b, "id") == current)
            .ok_or("Unknown branch reference")?;
        chain.push(branch);
        current = field(branch, "parent_branch_id");
    }
    chain.reverse();
    let mut clauses = vec![];
    let mut prior_date = "";
    for branch in &chain {
        if !branch["parent_branch_id"].is_null() && !branch["parent_branch_id"].is_string() {
            return Err("parent_branch_id must be a string or null".into());
        }
        let by = field(branch, "by");
        if by < field(&snapshot["world"], "last_ingest_date") {
            return Err("Branch deadline precedes baseline".into());
        }
        if !super::search::date(by) || by < prior_date {
            return Err("Branch dates must be valid nondecreasing milestones".into());
        }
        if let Some(horizon) = snapshot["world"]["target_date"]
            .as_str()
            .filter(|v| !v.is_empty())
            && by > horizon
        {
            return Err("Branch deadline exceeds question horizon".into());
        }
        prior_date = by;
        let kind = field(&branch["condition"], "kind");
        if !matches!(kind, "all_occurring" | "not_all_occurring") {
            return Err("Unknown branch condition kind".into());
        }
        let ids = strings(&branch["condition"]["event_ids"])?;
        if ids.is_empty() || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err("Branch needs distinct nonempty premise IDs".into());
        }
        let mut events = vec![];
        for event in ids {
            if Some(event) == target {
                return Err("Branch cannot condition on its target".into());
            }
            let node = hypothesis(snapshot, event)?;
            events.push(json!({"id":event,"statement":node["statement"],"by":by}));
        }
        clauses.push(json!({"branch_id":branch["id"],"kind":kind,"by":by,"events":events}));
    }
    compatible(&clauses, &[])?;
    Ok(
        json!({"branch_id":id,"hypothetical":true,"baseline_ref":"baseline","as_of":snapshot["world"]["last_ingest_date"],"history":chain.iter().map(|b|b["id"].clone()).collect::<Vec<_>>(),"conditions":clauses}),
    )
}
/// Positive assignments by an earlier/equal deadline cannot satisfy a failed conjunction.
fn compatible(clauses: &[Value], components: &[(&str, &str)]) -> Result<(), String> {
    let mut positive: BTreeMap<&str, &str> = BTreeMap::new();
    for &(id, by) in components {
        positive.insert(id, by);
    }
    for clause in clauses.iter().filter(|c| c["kind"] == "all_occurring") {
        for event in clause["events"].as_array().ok_or("Invalid branch events")? {
            let id = field(event, "id");
            let by = field(event, "by");
            positive
                .entry(id)
                .and_modify(|old| {
                    if by < *old {
                        *old = by;
                    }
                })
                .or_insert(by);
        }
    }
    for clause in clauses.iter().filter(|c| c["kind"] == "not_all_occurring") {
        if clause["events"]
            .as_array()
            .ok_or("Invalid branch events")?
            .iter()
            .all(|event| {
                positive
                    .get(field(event, "id"))
                    .is_some_and(|by| *by <= field(event, "by"))
            })
        {
            return Err("Contradictory hypothetical conditions: all events occur by a deadline at which their conjunction fails".into());
        }
    }
    Ok(())
}
pub fn validate(snapshot: &Value) -> Result<(), String> {
    for node in snapshot["nodes"].as_array().into_iter().flatten() {
        if !node["branch_id"].is_null() && !node["branch_id"].is_string() {
            return Err("branch_id must be string or null".into());
        }
    }

    let Some(branches) = snapshot["branches"].as_array() else {
        if snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|n| !field(n, "branch_id").is_empty())
        {
            return Err("Missing branch catalog".into());
        }
        return Ok(());
    };
    if branches.len() > super::MAX_NODES {
        return Err("Branch budget exceeded".into());
    }
    let mut identities = BTreeSet::new();
    for b in branches {
        let id = field(b, "id");
        if id.is_empty() || !identities.insert(id) {
            return Err("Duplicate or empty branch identity".into());
        }
        state(snapshot, id, None)?;
    }
    // Premise dependencies include explicit requires and inherited branch premises.
    // This catches indirect self-conditioning, including a premise downstream of the target.
    let mut dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in snapshot["nodes"].as_array().ok_or("Missing nodes")? {
        if !matches!(field(node, "kind"), "scenario" | "revision") {
            continue;
        }
        if !node["branch_id"].is_null() && !node["branch_id"].is_string() {
            return Err("branch_id must be a string or null".into());
        }
        let id = field(node, "Id");
        let mut deps = BTreeSet::new();
        let edges = super::parse(field(node, "edges"))?;
        for edge in edges
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["kind"] == "requires")
        {
            if hypothesis(snapshot, field(edge, "to_id")).is_ok() {
                deps.insert(field(edge, "to_id").to_owned());
            }
        }
        if !field(node, "branch_id").is_empty() {
            let s = state(snapshot, field(node, "branch_id"), Some(id))?;
            for c in s["conditions"].as_array().unwrap() {
                for e in c["events"].as_array().unwrap() {
                    deps.insert(field(e, "id").to_owned());
                }
            }
        }
        dependencies.insert(id.to_owned(), deps);
    }
    let mut done = BTreeSet::new();
    loop {
        let ready: Vec<_> = dependencies
            .iter()
            .filter(|(id, deps)| !done.contains(*id) && deps.iter().all(|d| done.contains(d)))
            .map(|(id, _)| id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        done.extend(ready);
    }
    if done.len() != dependencies.len() {
        return Err("Cyclic or downstream branch conditioning".into());
    }
    Ok(())
}
pub fn world_conditions(snapshot: &Value, component_ids: &Value) -> Result<Value, String> {
    let mut clauses = vec![];
    let mut seen = BTreeSet::new();
    let horizon = field(&snapshot["world"], "target_date");
    let ids = strings(component_ids)?;
    for id in &ids {
        let n = hypothesis(snapshot, id)?;
        if field(n, "branch_id").is_empty() {
            continue;
        }
        let s = state(snapshot, field(n, "branch_id"), Some(id))?;
        for c in s["conditions"].as_array().unwrap() {
            if seen.insert(field(c, "branch_id").to_owned()) {
                clauses.push(c.clone());
            }
        }
    }
    compatible(
        &clauses,
        &ids.iter().map(|id| (*id, horizon)).collect::<Vec<_>>(),
    )?;
    Ok(json!(clauses))
}

/// The same eligibility decision is used by the composition catalog and validator.
pub fn future_eligible(snapshot: &Value, program: &Value, id: &str) -> bool {
    if !super::temporal_allows_forecast(program, id) {
        return false;
    }
    let Ok(node) = hypothesis(snapshot, id) else {
        return false;
    };
    if field(node, "branch_id").is_empty() {
        return true;
    }
    let Ok(s) = state(snapshot, field(node, "branch_id"), Some(id)) else {
        return false;
    };
    s["conditions"].as_array().into_iter().flatten().all(|c| {
        c["events"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|e| super::temporal_allows_forecast(program, field(e, "id")))
    })
}

/// One fallible recorded search ranking chooses a premise, never its truth value.
/// Both signs share its inherited state; the complement is not an invented opposite.
pub fn challenge_rollout(snapshot: &Value, program: &Value) -> Option<Value> {
    let nodes = snapshot["nodes"].as_array()?;
    let replaced: BTreeSet<_> = nodes.iter().filter_map(|n| n["parent"].as_str()).collect();
    let mut choices = vec![];
    for node in nodes {
        let id = field(node, "Id");
        if !matches!(field(node, "kind"), "scenario" | "revision")
            || replaced.contains(id)
            || !future_eligible(snapshot, program, id)
            || !matches!(
                program["results"][id]["classify_gap"].as_str(),
                Some("evidence" | "prerequisite" | "timing" | "uncertain")
            )
        {
            continue;
        }
        let current = &program["results"][id]["decision_value"];
        let historical = &program["historical_search_guidance"][id]["decision_value"];
        let (raw, guidance) = if current.is_string() {
            let evaluation = &program["evaluations"][id]["decision_value"];
            let same_sources = match (
                evaluation["context"]["evidence_ids"].as_array(),
                program["evidence_ids"].as_array(),
            ) {
                (Some(a), Some(b)) => {
                    a.iter().filter_map(Value::as_str).collect::<BTreeSet<_>>()
                        == b.iter().filter_map(Value::as_str).collect::<BTreeSet<_>>()
                }
                _ => false,
            };
            let verified = evaluation["type"] == "score"
                && evaluation["score"].as_f64()
                    == current.as_str().and_then(|v| v.parse::<f64>().ok())
                && same_sources;
            (
                current,
                json!({"result":current,"evaluation":evaluation,"current":if verified {json!(true)}else{Value::Null}}),
            )
        } else {
            (&historical["result"], historical.clone())
        };
        let Some(score) = raw
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| s.is_finite() && (0.0..=4.0).contains(s))
        else {
            continue;
        };
        let by = node["resolve_by"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(field(&snapshot["world"], "target_date"));
        let parent = node["branch_id"].as_str().filter(|s| !s.is_empty());
        let roots = json!([
            {"id":"challenge-premise-on","parent_branch_id":parent,"condition":{"kind":"all_occurring","event_ids":[id]},"by":by},
            {"id":"challenge-premise-off","parent_branch_id":parent,"condition":{"kind":"not_all_occurring","event_ids":[id]},"by":by}
        ]);
        let mut probe = snapshot.clone();
        if !probe["branches"].is_array() {
            probe["branches"] = json!([]);
        }
        probe["branches"]
            .as_array_mut()
            .unwrap()
            .extend(roots.as_array().unwrap().clone());
        if state(&probe, "challenge-premise-on", None).is_err()
            || state(&probe, "challenge-premise-off", None).is_err()
        {
            continue;
        }
        choices.push((score,id,json!({"nodeId":id,"statement":node["statement"],"guidance":guidance,"branches":roots,"interpretation":"Both states are hypothetical. not_all_occurring negates the exact event, including its scope; it does not assert an opposite mechanism or identify why it fails. Recorded decision guidance selects investigation, not likelihood or truth."})));
    }
    choices.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    choices.into_iter().next().map(|(_, _, v)| v)
}

fn descends_from(hypothesis: &Value, root: &str, branches: &[Value]) -> bool {
    let mut branch = field(hypothesis, "branch_id");
    let mut seen = BTreeSet::new();
    while !branch.is_empty() && seen.insert(branch) {
        if branch == root {
            return true;
        }
        branch = branches
            .iter()
            .find(|b| field(b, "id") == branch)
            .map(|b| field(b, "parent_branch_id"))
            .unwrap_or("");
    }
    false
}

/// Check supplied roots before accepting any generated nodes. Descendants may
/// branch further, but each side must have an actual bound consequence.
pub fn validate_challenge_rollout(generated: &Value, rollout: &Value) -> Result<(), String> {
    let hypotheses = generated["hypotheses"]
        .as_array()
        .ok_or("Missing challenge hypotheses")?;
    if hypotheses.is_empty() {
        return Ok(());
    }
    let branches = generated["branches"]
        .as_array()
        .ok_or("Selected challenge premise requires both supplied branch states")?;
    for expected in rollout["branches"].as_array().unwrap() {
        let id = field(expected, "id");
        let actual = branches
            .iter()
            .find(|b| field(b, "id") == id)
            .ok_or("Missing selected challenge branch")?;
        for key in ["parent_branch_id", "condition", "by"] {
            if actual[key] != expected[key] {
                return Err(format!(
                    "Challenge branch {id} must preserve supplied {key}"
                ));
            }
        }
        let bound = hypotheses.iter().any(|h| descends_from(h, id, branches));
        if !bound {
            return Err(format!(
                "Challenge branch {id} needs a bound consequence; branch records alone are not a rollout"
            ));
        }
    }
    if hypotheses.iter().any(|h| {
        !rollout["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| descends_from(h, field(root, "id"), branches))
    }) {
        return Err(
            "Every selected-rollout hypothesis must bind to one supplied root or its descendants"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn challenge_selects_recorded_guidance_and_enforces_both_inherited_signs() {
        let mut snapshot = fixture();
        snapshot["world"]["target_date"] = json!("2027-09-30");
        let mut p = json!({"baseline_status":"established","results":{"a":{"classify_temporal":"future_change","classify_gap":"evidence","decision_value":"2"},"b":{"classify_temporal":"future_change","classify_gap":"uncertain","decision_value":"3"}}});
        let rollout = challenge_rollout(&snapshot, &p).unwrap();
        assert_eq!(rollout["nodeId"], "b");
        assert_eq!(rollout["branches"][0]["parent_branch_id"], "on");
        let reply = json!({"branches":rollout["branches"],"hypotheses":[{"branch_id":"challenge-premise-on"},{"branch_id":"challenge-premise-off"}]});
        validate_challenge_rollout(&reply, &rollout).unwrap();
        assert!(rollout["guidance"]["current"].is_null());
        let mut extra = reply.clone();
        extra["hypotheses"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"unbound"}));
        assert!(validate_challenge_rollout(&extra, &rollout).is_err());
        let mut dropped_parent = reply.clone();
        dropped_parent["branches"][1]["parent_branch_id"] = Value::Null;
        assert!(validate_challenge_rollout(&dropped_parent, &rollout).is_err());
        let mut bad = reply.clone();
        bad["branches"][1]["condition"]["kind"] = json!("all_occurring");
        assert!(validate_challenge_rollout(&bad, &rollout).is_err());
        bad = reply.clone();
        bad["hypotheses"].as_array_mut().unwrap().pop();
        assert!(validate_challenge_rollout(&bad, &rollout).is_err());
        p["results"]["b"]["classify_temporal"] = json!("already_observed");
        assert_eq!(challenge_rollout(&snapshot, &p).unwrap()["nodeId"], "a");
        p["historical_search_guidance"]["a"]["decision_value"] =
            json!({"result":"2","current":false,"recorded_round":1});
        p["results"]["a"]
            .as_object_mut()
            .unwrap()
            .remove("decision_value");
        assert_eq!(
            challenge_rollout(&snapshot, &p).unwrap()["guidance"]["current"],
            false
        );
        p["results"]["a"]["classify_gap"] = json!("none");
        assert!(challenge_rollout(&snapshot, &p).is_none());
    }
    fn fixture() -> Value {
        json!({"world":{"last_ingest_date":"2026-09-30","target_date":"2027-09-30"},"nodes":[
            {"Id":"a","kind":"scenario","statement":"A happens","edges":"[]"},
            {"Id":"b","kind":"scenario","statement":"B happens","edges":"[]","branch_id":"on"},
            {"Id":"c","kind":"scenario","statement":"C happens","edges":"[]","branch_id":"second"},
            {"Id":"d","kind":"scenario","statement":"D happens","edges":"[]","branch_id":"third"},
            {"Id":"e","kind":"scenario","statement":"E happens","edges":"[]","branch_id":"off"}],
            "branches":[
                {"id":"on","condition":{"kind":"all_occurring","event_ids":["a"]},"by":"2027-01-01"},
                {"id":"second","parent_branch_id":"on","condition":{"kind":"all_occurring","event_ids":["b"]},"by":"2027-03-01"},
                {"id":"third","parent_branch_id":"second","condition":{"kind":"not_all_occurring","event_ids":["c","e"]},"by":"2027-06-01"},
                {"id":"off","condition":{"kind":"not_all_occurring","event_ids":["a"]},"by":"2027-01-01"}]})
    }
    #[test]
    fn layered_conditions_preserve_disjunction_without_target_or_unrelated_assignments() {
        let s = fixture();
        validate(&s).unwrap();
        let before = s.clone();
        let state = state(&s, "third", Some("d")).unwrap();
        assert_eq!(state["history"], json!(["on", "second", "third"]));
        assert_eq!(state["conditions"][2]["kind"], "not_all_occurring");
        assert_eq!(
            state["conditions"][2]["events"].as_array().unwrap().len(),
            2
        );
        assert_eq!(s, before);
        assert!(world_conditions(&s, &json!(["b", "e"])).is_err());
        assert!(world_conditions(&s, &json!(["b", "c", "d"])).is_ok());
    }
    #[test]
    fn malformed_foreign_cyclic_self_and_contradictory_conditions_reject() {
        for (path, value) in [
            ("parent", json!(42)),
            ("date", json!("2020-01-01")),
            ("event", json!(["absent"])),
            ("self", json!(["b"])),
            ("cycle", json!("second")),
            ("contradiction", json!(["a"])),
        ] {
            let mut s = fixture();
            match path {
                "parent" => s["branches"][0]["parent_branch_id"] = value,
                "date" => s["branches"][0]["by"] = value,
                "event" | "self" => s["branches"][0]["condition"]["event_ids"] = value,
                "cycle" => s["branches"][0]["parent_branch_id"] = value,
                _ => {
                    s["branches"][1]["condition"]["kind"] = json!("not_all_occurring");
                    s["branches"][1]["condition"]["event_ids"] = value;
                }
            }
            assert!(validate(&s).is_err(), "{path}");
        }
    }
}
