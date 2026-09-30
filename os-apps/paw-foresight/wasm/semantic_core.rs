// Append-only open hypothesis graph. Budgets bound execution, not the shape of futures.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const MODEL: &str = "jev-1.13.0";
pub const MAX_CALLS: usize = 5000;
pub const MAX_NODES: usize = 2048;
pub const MAX_MS: u64 = 3_600_000;
// The final writer needs wall-clock time after evaluation, not unused call credits.
pub const SYNTHESIS_TIME_RESERVE_MS: u64 = 180_000;
pub const MAX_TRACE_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_ROUNDS: u64 = 64;
// Questions, not HTTP requests: independent structural questions can share a call.
pub const WORLD_CALL_RESERVE: usize = 3600;
pub const WORLD_TIME_RESERVE_MS: u64 = 600_000;
// The reserved tail contains pair search followed by world evaluation.
pub const COMBINATION_CALL_BUDGET: usize = 1000;
pub const COMBINATION_TIME_BUDGET_MS: u64 = 180_000;
pub fn call_limit(program: &Value) -> usize {
    match program["stage"].as_str() {
        Some("worlds") => MAX_CALLS,
        Some("combinations") => MAX_CALLS - WORLD_CALL_RESERVE + COMBINATION_CALL_BUDGET,
        _ => MAX_CALLS - WORLD_CALL_RESERVE,
    }
}
pub fn time_limit(program: &Value) -> u64 {
    match program["stage"].as_str() {
        Some("worlds") => MAX_MS - SYNTHESIS_TIME_RESERVE_MS,
        Some("combinations") => MAX_MS - WORLD_TIME_RESERVE_MS + COMBINATION_TIME_BUDGET_MS,
        _ => MAX_MS - WORLD_TIME_RESERVE_MS,
    }
}
pub mod evidence {
    include!("semantic_evidence.rs");
}
pub mod evaluation {
    include!("semantic_evaluation.rs");
}
pub mod search {
    include!("semantic_search.rs");
}
pub mod batch {
    include!("semantic_batch.rs");
}
#[allow(unused_imports)] // Each native phase consumes a different shared entry point.
pub use evaluation::{evaluation_value, request, validate};
pub fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get("fields")
        .unwrap_or(v)
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
}
pub fn parse(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|_| "Invalid persisted semantic state".into())
}
/// Order prerequisites before dependent hypotheses without recursive stack growth.
pub fn plan(nodes: &[Value]) -> Result<Value, String> {
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err("Semantic graph exceeds its node budget or is empty".into());
    }
    let mut by_id = BTreeMap::new();
    for node in nodes {
        let id = field(node, "Id");
        if id.is_empty() || by_id.insert(id.to_owned(), node).is_some() {
            return Err("Missing or duplicate event identity".into());
        }
    }
    // Composed worlds have their own structural/whole-event planner. They are
    // never evidence or ordinary exploration tasks, including archived worlds.
    by_id.retain(|_, node| field(node, "kind") != "world");
    let mut prerequisites: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut dependents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut issues = vec![];
    for (id, node) in &by_id {
        let mut refs = BTreeSet::new();
        let edges = parse(field(node, "edges"))
            .ok()
            .and_then(|v| v.as_array().cloned());
        if let Some(edges) = edges {
            for edge in edges.iter().filter(|e| e["kind"] == "requires") {
                if let Some(target) = edge["to_id"].as_str().filter(|s| !s.is_empty()) {
                    if by_id.contains_key(target) {
                        refs.insert(target.to_owned());
                        dependents
                            .entry(target.to_owned())
                            .or_default()
                            .insert(id.clone());
                    } else {
                        issues.push(
                            json!({"nodeId":id,"target":target,"result":"missing_prerequisite"}),
                        );
                    }
                } else {
                    issues.push(json!({"nodeId":id,"result":"invalid_reference"}));
                }
            }
        } else {
            issues.push(json!({"nodeId":id,"result":"invalid_edges"}));
        }
        prerequisites.insert(id.clone(), refs);
    }
    let mut remaining: BTreeMap<String, usize> = prerequisites
        .iter()
        .map(|(id, refs)| (id.clone(), refs.len()))
        .collect();
    let mut ready: BTreeSet<String> = remaining
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut depths: BTreeMap<String, usize> = BTreeMap::new();
    let mut tasks = vec![];
    while let Some(id) = ready.pop_first() {
        let depth = prerequisites[&id]
            .iter()
            .map(|p| depths[p] + 1)
            .max()
            .unwrap_or(0);
        depths.insert(id.clone(), depth);
        let hypothesis = matches!(field(by_id[&id], "kind"), "scenario" | "revision");
        let functions: &[&str] = if hypothesis {
            &[
                "classify_temporal",
                "classify_gap",
                "estimate_likelihood",
                "evaluate_novelty",
                "decision_value",
            ]
        } else {
            &["classify_gap"]
        };
        for function in functions {
            tasks.push(json!({"nodeId":id,"function":function,"depth":depth}));
        }
        for dependent in dependents.get(&id).into_iter().flatten() {
            let count = remaining
                .get_mut(dependent)
                .ok_or("Planner lost a dependency")?;
            *count -= 1;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    for id in by_id.keys().filter(|id| !depths.contains_key(*id)) {
        issues.push(json!({"nodeId":id,"result":"cycle_or_cyclic_prerequisite"}));
    }
    // Temporal screening depends on dated evidence, not another candidate's scores.
    // Screen all available branches before revisiting expensive dependent judgments.
    // New append-only candidates go first; no topic or score ranking is imposed.
    let positions: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (field(n, "Id"), i))
        .collect();
    let mut screening: Vec<_> = tasks
        .iter()
        .filter(|t| t["function"] == "classify_temporal")
        .cloned()
        .collect();
    tasks.retain(|t| t["function"] != "classify_temporal");
    screening.sort_by_key(|t| std::cmp::Reverse(positions[field(t, "nodeId")]));
    // Same-depth nodes cannot depend on one another. Complete each assessment
    // wave before advancing: own earlier scores and all parent scores remain fresh.
    tasks.sort_by_key(|t| {
        (
            t["depth"].as_u64().unwrap_or(0),
            match field(t, "function") {
                "classify_gap" => 0,
                "estimate_likelihood" => 1,
                "evaluate_novelty" => 2,
                _ => 3,
            },
        )
    });
    screening.extend(tasks);
    let tasks = screening;
    Ok(
        json!({"schema":"foresight-open-semantic-v2","stage":"exploration","cursor":0,"tasks":tasks,"issues":issues,"results":{},"evaluations":{},"round":0,"rounds":[],"continue_exploring":true,"max_calls":MAX_CALLS,"max_nodes":MAX_NODES,"time_budget_ms":MAX_MS}),
    )
}
/// Missing classifications remain readable in historical runs; new plans classify first.
pub const MAX_APP_TRANSITIONS: u64 = 480;
pub const REASONING_TRANSITION_RESERVE: u64 = 44;
pub const MAX_REASONING_POLLS: u64 = 10;
pub fn transition_count(state: &Value) -> u64 {
    state["counters"]["transition_count"]
        .as_u64()
        .or_else(|| state["transition_count"].as_u64())
        .unwrap_or(0)
}
pub fn transition_limit(program: &Value) -> u64 {
    match field(program, "stage") {
        "worlds" => MAX_APP_TRANSITIONS - REASONING_TRANSITION_RESERVE,
        "combinations" => MAX_APP_TRANSITIONS - 2 * REASONING_TRANSITION_RESERVE - 128,
        _ => MAX_APP_TRANSITIONS - 2 * REASONING_TRANSITION_RESERVE - 128 - 32,
    }
}
pub fn temporal_allows_forecast(program: &Value, id: &str) -> bool {
    match program["results"][id]["classify_temporal"].as_str() {
        Some("future_change" | "uncertain") => true,
        Some("already_observed" | "mixed") => false,
        _ => program["baseline_status"].is_null(),
    }
}
pub fn skip_nonfuture_tasks(program: &mut Value) -> Result<(), String> {
    if program["stage"] == "worlds" {
        return Ok(());
    }
    let excluded: Vec<String> = program["results"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, _)| !temporal_allows_forecast(program, id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in excluded {
        for collection in ["results", "evaluations"] {
            if let Some(values) = program[collection][&id].as_object_mut() {
                for function in ["estimate_likelihood", "evaluate_novelty", "decision_value"] {
                    values.remove(function);
                }
            }
        }
    }
    let mut cursor = program["cursor"].as_u64().ok_or("Missing cursor")? as usize;
    let tasks = program["tasks"].as_array().ok_or("Missing tasks")?;
    while let Some(task) = tasks.get(cursor) {
        if matches!(
            field(task, "function"),
            "estimate_likelihood" | "evaluate_novelty" | "decision_value"
        ) && !temporal_allows_forecast(program, field(task, "nodeId"))
        {
            cursor += 1;
        } else {
            break;
        }
    }
    program["cursor"] = json!(cursor);
    Ok(())
}
pub fn temporal_criteria() -> Value {
    json!({"already_observed":"The full scoped claim is already observed at the evidence vantage; a future date alone does not make it a new change.","future_change":"The claim specifies a change beyond what the supplied dated baseline establishes.","mixed":"The claim conflates already observed conditions and distinct future changes and needs decomposition.","uncertain":"The supplied evidence cannot establish whether this scoped claim is already observed or a future change."})
}
pub fn gap_criteria() -> Value {
    json!({"none":"No specific causal gap identified; this does not establish truth.","timing":"The supplied interval is materially too short.","prerequisite":"A necessary causal prerequisite is missing.","evidence":"The key premise lacks supporting evidence in the supplied input.","uncertain":"Cannot distinguish reliably."})
}
#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: &str, kind: &str, refs: &[&str]) -> Value {
        json!({"Id":id,"kind":kind,"statement":id,"edges":refs.iter().map(|id|json!({"kind":"requires","to_id":id})).collect::<Vec<_>>().pipe()})
    }
    trait Encoded {
        fn pipe(self) -> String;
    }
    impl Encoded for Vec<Value> {
        fn pipe(self) -> String {
            serde_json::to_string(&self).unwrap()
        }
    }
    #[test]
    fn polling_reserve_preserves_all_stage_budgets() {
        assert_eq!(MAX_REASONING_POLLS, 10);
        assert_eq!(REASONING_TRANSITION_RESERVE, 44);
        let exploration = transition_limit(&json!({"stage":"exploration"}));
        let combinations = transition_limit(&json!({"stage":"combinations"}));
        let worlds = transition_limit(&json!({"stage":"worlds"}));
        assert_eq!((exploration, combinations, worlds), (232, 264, 436));
        assert_eq!(exploration + 32, combinations);
        assert_eq!(combinations + 44 + 128, worlds);
        assert_eq!(worlds + 44, MAX_APP_TRANSITIONS);
        assert_eq!(MAX_APP_TRANSITIONS + 32, 512);
        assert!(40 + 4 <= REASONING_TRANSITION_RESERVE);
    }
    #[test]
    fn late_branch_is_screened_before_earlier_candidates_repeat_deep_checks() {
        let mut nodes: Vec<_> = (0..20)
            .map(|i| node(&format!("old-{i:02}"), "scenario", &[]))
            .collect();
        nodes.push(node("late-novel-branch", "revision", &["old-00"]));
        let p = plan(&nodes).unwrap();
        let tasks = p["tasks"].as_array().unwrap();
        assert_eq!(tasks[0]["nodeId"], "late-novel-branch");
        assert!(
            tasks[..21]
                .iter()
                .all(|t| t["function"] == "classify_temporal")
        );
        let parent = tasks
            .iter()
            .position(|t| t["nodeId"] == "old-00" && t["function"] == "estimate_likelihood")
            .unwrap();
        let child = tasks
            .iter()
            .position(|t| {
                t["nodeId"] == "late-novel-branch" && t["function"] == "estimate_likelihood"
            })
            .unwrap();
        assert!(parent < child);
        assert_eq!(tasks.len(), 105); // Required evaluations remain scheduled.
    }
    #[test]
    fn observed_and_mixed_claims_are_classified_before_forecast_and_do_not_keep_stale_odds() {
        let mut p = plan(&[node("h", "scenario", &[])]).unwrap();
        assert_eq!(p["tasks"][0]["function"], "classify_temporal");
        for classification in ["already_observed", "mixed", "future_change", "uncertain"] {
            p["cursor"] = json!(2);
            p["results"]["h"] = json!({"classify_temporal":classification,"estimate_likelihood":"0.8","evaluate_novelty":"4"});
            p["evaluations"]["h"] = json!({"estimate_likelihood":{"probability":0.8}});
            skip_nonfuture_tasks(&mut p).unwrap();
            let excluded = matches!(classification, "already_observed" | "mixed");
            assert_eq!(p["cursor"], json!(if excluded { 5 } else { 2 }));
            assert_eq!(p["results"]["h"]["estimate_likelihood"].is_null(), excluded);
            assert_eq!(
                p["evaluations"]["h"]["estimate_likelihood"].is_null(),
                excluded
            );
            assert_eq!(p["results"]["h"]["classify_temporal"], classification);
        }
    }
    #[test]
    fn prerequisites_are_once_before_dependents() {
        let p = plan(&[
            node("a", "scenario", &["z"]),
            node("b", "scenario", &["z"]),
            node("z", "evidence", &[]),
        ])
        .unwrap();
        assert_eq!(p["tasks"].as_array().unwrap().len(), 11);
        assert_eq!(p["tasks"][2]["nodeId"], "z");
        assert_eq!(p["tasks"][1]["function"], "classify_temporal");
    }
    #[test]
    fn thousands_of_evaluations_are_planned_without_a_cartesian_product() {
        let nodes: Vec<_> = (0..1000)
            .map(|i| node(&format!("h{i}"), "scenario", &[]))
            .collect();
        let p = plan(&nodes).unwrap();
        assert_eq!(p["tasks"].as_array().unwrap().len(), 5000);
        assert_eq!(p["max_calls"], 5000);
    }
    #[test]
    fn cycles_and_missing_evidence_stay_explicit() {
        let p = plan(&[
            node("a", "scenario", &["b"]),
            node("b", "scenario", &["a"]),
            node("ok", "scenario", &["missing"]),
        ])
        .unwrap();
        assert_eq!(p["tasks"].as_array().unwrap().len(), 5);
        assert!(
            p["issues"]
                .to_string()
                .contains("cycle_or_cyclic_prerequisite")
        );
        assert!(p["issues"].to_string().contains("missing_prerequisite"));
    }
    #[test]
    fn deep_graph_does_not_get_cut_off_at_eight() {
        let nodes: Vec<_> = (0..100)
            .map(|i| node(&format!("h{i}"), "scenario", &[]))
            .collect();
        let mut nodes = nodes;
        for (i, node) in nodes.iter_mut().enumerate().take(100).skip(1) {
            node["edges"] = json!(format!(
                "[{{\"kind\":\"requires\",\"to_id\":\"h{}\"}}]",
                i - 1
            ));
        }
        let p = plan(&nodes).unwrap();
        assert_eq!(p["tasks"].as_array().unwrap().len(), 500);
        assert_eq!(p["tasks"][499]["depth"], 99);
    }
    #[test]
    fn identities_and_memory_budget_are_enforced() {
        let n = node("a", "scenario", &[]);
        assert!(plan(&[n.clone(), n]).is_err());
        assert!(plan(&[]).is_err());
        assert!(
            plan(
                &(0..=MAX_NODES)
                    .map(|i| node(&format!("h{i}"), "scenario", &[]))
                    .collect::<Vec<_>>()
            )
            .is_err()
        );
    }
}
