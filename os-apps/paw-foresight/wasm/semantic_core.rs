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
    if endpoints::enabled(program) && program["stage"] == "exploration" {
        return MAX_CALLS - WORLD_CALL_RESERVE / 2;
    }
    match program["stage"].as_str() {
        Some("worlds") => MAX_CALLS,
        Some("routes") => MAX_CALLS - WORLD_CALL_RESERVE / 2,
        Some("combinations") => MAX_CALLS - WORLD_CALL_RESERVE + COMBINATION_CALL_BUDGET,
        _ => MAX_CALLS - WORLD_CALL_RESERVE,
    }
}
// Research closes before the bounded drain of already accepted endpoint work.
// This consumes two minutes of the existing ten-minute finalization reserve;
// it never moves the original one-hour deadline or admits further research.
pub const ENDPOINT_EVALUATION_DRAIN_MS: u64 = 120_000;
pub fn time_limit(program: &Value) -> u64 {
    if program["audit_policy_version"] == 2 && program["admitted_work"]["admitted"] == true
        && matches!(field(program,"stage"), "exploration" | "routes" | "proposals") {
        return MAX_MS - WORLD_TIME_RESERVE_MS;
    }
    if endpoints::enabled(program)
        && (matches!(program["stage"].as_str(), Some("exploration" | "routes"))
            || (program["stage"] == "proposals"
                && program["endpoint_proposal_attempt"]["pool_stage"] == "deferred"))
    {
        return research_time_limit(program) + ENDPOINT_EVALUATION_DRAIN_MS;
    }
    research_time_limit(program)
}
/// Observed child duration includes native provider retries and polling latency.
/// It starts at launch setup, never at the parent run's original clock.
pub fn start_reasoning_timing(program: &mut Value, phase: &str, now_ms: u64) {
    let correction_key = match phase {
        "seed" => "baseline_correction",
        "compose" => "composition_correction",
        "synthesize" => "presentation_repair",
        _ => "response_correction",
    };
    let correction = program[correction_key]["attempt"].as_u64().is_some_and(|attempt|attempt>0);
    let episode_start = if correction && program["reasoning_timing"]["phase"] == phase {
        program["reasoning_timing"]["episode_started_at_ms"]
            .as_u64()
            .or_else(|| program["reasoning_timing"]["started_at_ms"].as_u64())
            .filter(|start| *start > 0 && *start <= now_ms)
            .unwrap_or(now_ms)
    } else {
        now_ms
    };
    program["reasoning_timing"] =
        json!({"phase":phase,"started_at_ms":now_ms,"episode_started_at_ms":episode_start});
}
pub fn finish_reasoning_timing(program: &mut Value, phase: &str, now_ms: u64) {
    let timing = &program["reasoning_timing"];
    if timing["phase"] != phase || timing["completed"] == true {
        return;
    }
    let Some(start) = timing["started_at_ms"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= now_ms)
    else {
        return;
    };
    let elapsed = now_ms - start;
    let prior = program["reasoning_durations_ms"][phase]
        .as_u64()
        .unwrap_or(0);
    program["reasoning_durations_ms"][phase] = json!(prior.max(elapsed));
    program["reasoning_timing"]["elapsed_ms"] = json!(elapsed);
    program["reasoning_timing"]["completed"] = json!(true);
    let episode_start = program["reasoning_timing"]["episode_started_at_ms"]
        .as_u64()
        .unwrap_or(start);
    let episode_elapsed = now_ms.saturating_sub(episode_start);
    let prior_episode = program["reasoning_episode_durations_ms"][phase]
        .as_u64()
        .unwrap_or(0);
    program["reasoning_episode_durations_ms"][phase] = json!(prior_episode.max(episode_elapsed));
    program["reasoning_timing"]["episode_elapsed_ms"] = json!(episode_elapsed);
}
pub fn generation_duration(program: &Value, phase: &str) -> u64 {
    program["reasoning_episode_durations_ms"][phase]
        .as_u64()
        .filter(|v| *v > 0)
        .or_else(|| {
            program["reasoning_durations_ms"][phase]
                .as_u64()
                .filter(|v| *v > 0)
        })
        .unwrap_or(900_000)
}
pub fn research_admission(program: &Value, phase: &str, elapsed_ms: u64) -> Value {
    // The native idle allowance is a conservative first-sample planning estimate,
    // not an upper bound: progressing children may take longer.
    let observed = program["reasoning_episode_durations_ms"][phase]
        .as_u64()
        .filter(|v| *v > 0);
    let predicted = generation_duration(program, phase);
    let completion_deadline = time_limit(program);
    let required = predicted.saturating_add(ENDPOINT_EVALUATION_DRAIN_MS);
    json!({"admitted":!(program["route_finalization"]["admitted"] == true && phase != "compose" && phase != "synthesize") && elapsed_ms.saturating_add(required) < completion_deadline && elapsed_ms < research_time_limit(program).saturating_sub(120_000),"phase":phase,"predicted_generation_ms":predicted,"basis":if observed.is_some(){"maximum_observed_generation_episode"}else if program["reasoning_durations_ms"][phase].as_u64().is_some_and(|v|v>0){"maximum_observed_same_phase"}else{"native_idle_allowance_fallback"},"evaluation_reserve_ms":ENDPOINT_EVALUATION_DRAIN_MS,"completion_deadline_ms":completion_deadline,"elapsed_ms":elapsed_ms,"guaranteed":false})
}
/// Reserve the whole useful unit, without claiming unknown output has a known
/// request count. The receiver checks its actual mandatory plan before applying it.
pub fn backward_work_admission(program: &mut Value, elapsed_ms: u64) -> bool {
    if program["audit_policy_version"] != 2 {
        return true;
    }
    let generation_ms = generation_duration(program, "backward");
    let final_transitions = 2 * REASONING_ADMISSION_RESERVE + 32;
    let remaining = MAX_APP_TRANSITIONS.saturating_sub(transition_count(program));
    let capacity = remaining.saturating_sub(REASONING_ADMISSION_RESERVE + final_transitions);
    let time_ok = elapsed_ms
        .saturating_add(generation_ms)
        .saturating_add(ENDPOINT_EVALUATION_DRAIN_MS)
        .saturating_add(WORLD_TIME_RESERVE_MS)
        < MAX_MS;
    let admitted = capacity >= 16 && time_ok;
    let receipt = json!({"admitted":admitted,"status":if admitted{"generating"}else{"declined"},"generation_ms":generation_ms,"evaluation_transition_capacity":capacity,"generation_transition_reserve":REASONING_ADMISSION_RESERVE,"finalization_transition_reserve":final_transitions,"evaluation_time_reserve_ms":ENDPOINT_EVALUATION_DRAIN_MS,"finalization_time_reserve_ms":WORLD_TIME_RESERVE_MS,"elapsed_ms":elapsed_ms,"output_cost_known":false,"estimated_mandatory_transitions":null,"estimate_basis":"Generated graph and changed evidence are not known before the reply; native receiver must measure mandatory work before applying it.","original_deadline_ms":MAX_MS,"reason":if admitted{Value::Null}else if !time_ok{json!("time_budget")}else{json!("transition_budget")}});
    if admitted {
        program["admitted_work"] = receipt;
    } else {
        program["backward_work_refusal"] = receipt;
    }
    admitted
}

// Optional comparison work must leave the first two complete routes and the
// existing finalization window. Preserve phase-specific observed durations and
// the existing 15-minute fallback; this is admission, not a completion promise.
pub fn optional_repair_time_admission(program: &Value, elapsed_ms: u64) -> Value {
    let duration = |phase: &str| generation_duration(program, phase);
    let first_route_turns = if program["audit_policy_version"] == 2 {
        1
    } else {
        2
    };
    let route_ms = duration("backward").saturating_mul(first_route_turns);
    let generation_ms = duration("explore");
    let required = generation_ms
        .saturating_add(route_ms)
        .saturating_add(ENDPOINT_EVALUATION_DRAIN_MS)
        .saturating_add(WORLD_TIME_RESERVE_MS);
    json!({"admitted":elapsed_ms.saturating_add(required) < MAX_MS,"elapsed_ms":elapsed_ms,"required_ms":required,"generation_ms":generation_ms,"first_route_turns":first_route_turns,"route_reserve_ms":route_ms,"evaluation_reserve_ms":ENDPOINT_EVALUATION_DRAIN_MS,"finalization_reserve_ms":WORLD_TIME_RESERVE_MS,"original_deadline_ms":MAX_MS,"guaranteed":false})
}

pub fn research_time_limit(program: &Value) -> u64 {
    match program["stage"].as_str() {
        Some("worlds") => MAX_MS - SYNTHESIS_TIME_RESERVE_MS,
        Some("routes") => MAX_MS - WORLD_TIME_RESERVE_MS,
        Some("combinations") => MAX_MS - WORLD_TIME_RESERVE_MS + COMBINATION_TIME_BUDGET_MS,
        _ => MAX_MS - WORLD_TIME_RESERVE_MS,
    }
}
pub mod backward {
    include!("semantic_backward.rs");
}
pub mod endpoints {
    include!("semantic_endpoints.rs");
}
pub mod references_for_endpoints {
    include!("semantic_references.rs");
}
pub mod comparison {
    include!("semantic_comparison.rs");
}
pub mod coherence {
    include!("semantic_coherence.rs");
}
pub mod branches {
    include!("semantic_branches.rs");
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
pub mod execution_limits {
    include!("semantic_execution_limits.rs");
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
/// Rankings guide search; they are not truth/probability prerequisites. Preserve
/// their exact recorded basis outside current maps rather than re-score unchanged
/// append-only candidates whenever research adds evidence. Revisions have new IDs.
pub fn defer_recorded_rankings(program: &mut Value, previous: &Value) {
    let mut history = previous["historical_search_guidance"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(nodes) = previous["results"].as_object() {
        for (id, functions) in nodes {
            for function in ["evaluate_novelty", "decision_value"] {
                if functions[function].is_string() {
                    let evaluation = &previous["evaluations"][id][function];
                    let entry = history.entry(id.clone()).or_insert_with(|| json!({}));
                    entry[function] = json!({"result":functions[function],"evaluation":evaluation,
                        "recorded_round":evaluation["context"]["round"],
                        "evidence_ids":evaluation["context"]["evidence_ids"],"current":false});
                }
            }
        }
    }
    for (id, functions) in &history {
        for function in ["evaluate_novelty", "decision_value"] {
            if functions[function]["result"].is_string() {
                for collection in ["results", "evaluations"] {
                    if let Some(values) = program[collection][id].as_object_mut() {
                        values.remove(function);
                    }
                }
            }
        }
    }
    if let Some(tasks) = program["tasks"].as_array_mut() {
        tasks.retain(|task| {
            !matches!(
                field(task, "function"),
                "evaluate_novelty" | "decision_value"
            ) || !history
                .get(field(task, "nodeId"))
                .is_some_and(|functions| functions[field(task, "function")]["result"].is_string())
        });
    }
    program["historical_search_guidance"] = Value::Object(history);
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
        for clause in node["branch_state"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for event in clause["events"].as_array().into_iter().flatten() {
                let target = field(event, "id");
                if !by_id.contains_key(target) {
                    return Err("Missing branch premise in plan".into());
                }
                refs.insert(target.to_owned());
                dependents
                    .entry(target.to_owned())
                    .or_default()
                    .insert(id.clone());
            }
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
                "classify_claim_role",
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
        if hypothesis && !field(by_id[&id], "branch_id").is_empty() {
            tasks.push(json!({"nodeId":id,"function":"estimate_conditional","depth":depth}));
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
        .filter(|t| {
            matches!(
                field(t, "function"),
                "classify_claim_role" | "classify_temporal"
            )
        })
        .cloned()
        .collect();
    tasks.retain(|t| {
        !matches!(
            field(t, "function"),
            "classify_claim_role" | "classify_temporal"
        )
    });
    screening.sort_by_key(|t| {
        (
            t["function"] != "classify_claim_role",
            std::cmp::Reverse(positions[field(t, "nodeId")]),
        )
    });
    // Same-depth nodes cannot depend on one another. Complete each assessment
    // wave before advancing: own earlier scores and all parent scores remain fresh.
    tasks.sort_by_key(|t| {
        (
            t["depth"].as_u64().unwrap_or(0),
            match field(t, "function") {
                "classify_gap" => 0,
                "estimate_likelihood" => 1,
                "estimate_conditional" => 2,
                "evaluate_novelty" => 3,
                _ => 4,
            },
        )
    });
    screening.extend(tasks);
    let tasks = screening;
    let event_dependencies: BTreeMap<_, Vec<_>> = prerequisites
        .iter()
        .filter(|(id, _)| matches!(field(by_id[*id], "kind"), "scenario" | "revision"))
        .map(|(id, dependencies)| {
            (
                id,
                dependencies
                    .iter()
                    .filter(|dependency| {
                        matches!(field(by_id[*dependency], "kind"), "scenario" | "revision")
                    })
                    .collect(),
            )
        })
        .collect();
    Ok(
        json!({"schema":"foresight-open-semantic-v2","claim_role_contract":1,"event_dependencies":event_dependencies,"stage":"exploration","cursor":0,"tasks":tasks,"issues":issues,"results":{},"evaluations":{},"round":0,"rounds":[],"continue_exploring":true,"max_calls":MAX_CALLS,"max_nodes":MAX_NODES,"time_budget_ms":MAX_MS}),
    )
}
/// Missing classifications remain readable in historical runs; new plans classify first.
pub const MAX_APP_TRANSITIONS: u64 = 480;
/// Planning allowance for admitting new work, not a hard limit on an active child.
/// Actual reasoning remains bounded by the original clock and total transitions.
pub const REASONING_ADMISSION_RESERVE: u64 = 44;
pub fn transition_count(state: &Value) -> u64 {
    state["counters"]["transition_count"]
        .as_u64()
        .or_else(|| state["transition_count"].as_u64())
        .unwrap_or(0)
}
pub fn transition_limit(program: &Value) -> u64 {
    if program["audit_policy_version"] == 2
        && program["admitted_work"]["admitted"] == true
        && matches!(
            field(program, "stage"),
            "exploration" | "routes" | "proposals"
        )
    {
        return MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 32;
    }
    if program["route_finalization"]["admitted"] == true
        && matches!(program["stage"].as_str(), Some("routes" | "exploration"))
    {
        return MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 32;
    }
    // Deferred novelty is finalization work after route exploration, not a new
    // initial proposal search. Keep composition/writing and their tail reserved.
    if program["stage"] == "proposals"
        && program["endpoint_proposal_attempt"]["pool_stage"] == "deferred"
    {
        return MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 32;
    }
    if endpoints::enabled(program) && program["stage"] == "exploration" {
        return MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 64;
    }
    match field(program, "stage") {
        "worlds" => MAX_APP_TRANSITIONS - REASONING_ADMISSION_RESERVE,
        "routes" => MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 64,
        "combinations" => MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 128,
        _ => MAX_APP_TRANSITIONS - 2 * REASONING_ADMISSION_RESERVE - 128 - 32,
    }
}
pub fn temporal_allows_forecast(program: &Value, id: &str) -> bool {
    match program["results"][id]["classify_temporal"].as_str() {
        Some("future_change" | "uncertain") => true,
        Some("already_observed" | "mixed") => false,
        _ => program["baseline_status"].is_null(),
    }
}
/// New plans distinguish an event proposition from research commentary before
/// forecasting. Legacy programs retain their recorded admission semantics.
pub fn claim_role_allows_forecast(program: &Value, id: &str) -> bool {
    if program["claim_role_contract"] != 1 {
        return true;
    }
    let mut pending = vec![id];
    let mut visited = BTreeSet::new();
    while let Some(candidate) = pending.pop() {
        if !visited.insert(candidate) {
            continue;
        }
        if visited.len() > MAX_NODES
            || program["results"][candidate]["classify_claim_role"] != "event"
        {
            return false;
        }
        for dependency in program["event_dependencies"][candidate]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(dependency) = dependency.as_str() else {
                return false;
            };
            pending.push(dependency);
        }
    }
    true
}

pub fn forecast_allows(program: &Value, id: &str) -> bool {
    claim_role_allows_forecast(program, id) && temporal_allows_forecast(program, id)
}

pub fn forecast_exclusion_reason<'a>(program: &'a Value, id: &str) -> &'a str {
    if program["claim_role_contract"] == 1 {
        match program["results"][id]["classify_claim_role"].as_str() {
            Some("context") => return "Research context, not an event proposition",
            Some("unresolved") => return "Event proposition is unresolved",
            Some("event") => {}
            _ => return "Event admission pending",
        }
        if !claim_role_allows_forecast(program, id) {
            return "A required claim is not an admitted event proposition";
        }
    }
    program["results"][id]["classify_temporal"]
        .as_str()
        .unwrap_or("not evaluated in current evidence context")
}

/// Screening is independent of prior probabilities. A non-event remains in the
/// snapshot as context, but never receives event-specific judgments.
pub fn task_allowed(program: &Value, task: &Value) -> bool {
    if matches!(
        program["stage"].as_str(),
        Some("worlds" | "routes" | "proposals")
    ) || task["function"] == "classify_claim_role"
    {
        return true;
    }
    let id = field(task, "nodeId");
    if program["claim_role_contract"] == 1
        && program["event_dependencies"].get(id).is_some()
        && !claim_role_allows_forecast(program, id)
    {
        return false;
    }
    !matches!(
        field(task, "function"),
        "estimate_likelihood" | "estimate_conditional" | "evaluate_novelty" | "decision_value"
    ) || forecast_allows(program, id)
}

/// Invalidate before filtering a replanned task queue so newly required
/// admission checks cannot erase odds whose replacement tasks were removed.
pub fn clear_ineligible_forecasts(program: &mut Value) {
    if matches!(
        program["stage"].as_str(),
        Some("worlds" | "routes" | "proposals")
    ) {
        return;
    }
    let excluded: Vec<String> = program["results"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, _)| {
            // Composed worlds have a separate planner and immutable estimates;
            // event admission only owns the candidates in this plan.
            (program["claim_role_contract"] != 1
                || program["event_dependencies"].get(*id).is_some())
                && !forecast_allows(program, id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in excluded {
        for collection in ["results", "evaluations"] {
            if let Some(values) = program[collection][&id].as_object_mut() {
                for function in [
                    "estimate_likelihood",
                    "estimate_conditional",
                    "evaluate_novelty",
                    "decision_value",
                ] {
                    values.remove(function);
                }
            }
        }
    }
}

pub fn skip_nonfuture_tasks(program: &mut Value) -> Result<(), String> {
    if matches!(
        program["stage"].as_str(),
        Some("worlds" | "routes" | "proposals")
    ) {
        return Ok(());
    }
    clear_ineligible_forecasts(program);
    let mut cursor = program["cursor"].as_u64().ok_or("Missing cursor")? as usize;
    let tasks = program["tasks"].as_array().ok_or("Missing tasks")?;
    while let Some(task) = tasks.get(cursor) {
        if !task_allowed(program, task) {
            cursor += 1;
        } else {
            break;
        }
    }
    program["cursor"] = json!(cursor);
    Ok(())
}
/// Current normalized classifications, not proof of novelty or future occurrence.
/// Missing legacy classifications remain unknown rather than inferred continuations.
pub fn component_temporal(
    snapshot: &Value,
    program: &Value,
    world: &Value,
) -> Result<Value, String> {
    let mut records = Vec::new();
    for id in world["component_ids"].as_array().into_iter().flatten() {
        let id = id
            .as_str()
            .ok_or("Invalid component identity for temporal record")?;
        let node = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["Id"] == id)
            .ok_or("Missing component for temporal record")?;
        let statement = node["statement"]
            .as_str()
            .ok_or("Missing component statement for temporal record")?;
        let verdict = match program["results"][id]["classify_temporal"].as_str() {
            Some(value @ ("future_change" | "already_observed" | "mixed" | "uncertain")) => {
                json!(value)
            }
            _ => Value::Null,
        };
        records.push(json!({"node_id":id,"statement":statement,"verdict":verdict}));
    }
    Ok(json!(records))
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
    #[test]
    fn component_temporal_keeps_exact_normalized_judgments_without_inference() {
        let snapshot = json!({"nodes":[{"Id":"a","statement":"Exact scoped event"}]});
        let world = json!({"component_ids":["a"]});
        for verdict in ["future_change", "already_observed", "mixed", "uncertain"] {
            let program = json!({"results":{"a":{"classify_temporal":verdict}}});
            assert_eq!(
                component_temporal(&snapshot, &program, &world).unwrap(),
                json!([{"node_id":"a","statement":"Exact scoped event","verdict":verdict}])
            );
        }
        for program in [
            json!({}),
            json!({"results":{"a":{"classify_temporal":"continuation"}}}),
        ] {
            assert!(
                component_temporal(&snapshot, &program, &world).unwrap()[0]["verdict"].is_null()
            );
        }
    }

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
    fn historical_rankings_never_enter_current_probability_context() {
        let snapshot = json!({"world":{},"nodes":[{"Id":"h","kind":"scenario","statement":"Future H","edges":"[]"},{"Id":"new","kind":"revision","statement":"Future N","edges":"[{\"kind\":\"requires\",\"to_id\":\"h\"}]"}]});
        let evaluation = json!({"score":2.0,"context":{"round":1,"evidence_ids":["old-source"],"task":{"nodeId":"h","function":"evaluate_novelty"}}});
        let previous = json!({"results":{"h":{"classify_temporal":"future_change","classify_gap":"uncertain","estimate_likelihood":"0.4","evaluate_novelty":"2","decision_value":"3"}},"evaluations":{"h":{"evaluate_novelty":evaluation}}});
        let mut p = plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        p["results"] = previous["results"].clone();
        p["evaluations"] = previous["evaluations"].clone();
        defer_recorded_rankings(&mut p, &previous);
        assert_eq!(
            p["historical_search_guidance"]["h"]["evaluate_novelty"]["evaluation"],
            evaluation
        );
        assert_eq!(
            p["historical_search_guidance"]["h"]["evaluate_novelty"]["recorded_round"],
            1
        );
        assert_eq!(
            p["historical_search_guidance"]["h"]["evaluate_novelty"]["evidence_ids"],
            json!(["old-source"])
        );
        assert_eq!(
            p["historical_search_guidance"]["h"]["decision_value"]["recorded_round"],
            Value::Null
        );
        for function in ["evaluate_novelty", "decision_value"] {
            assert!(p["results"]["h"][function].is_null());
            assert!(p["evaluations"]["h"][function].is_null());
            assert!(
                !p["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["nodeId"] == "h" && t["function"] == function)
            );
            assert!(
                p["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["nodeId"] == "new" && t["function"] == function)
            );
        }
        p["results"]["h"]["classify_claim_role"] = json!("event");
        p["results"]["new"]["classify_claim_role"] = json!("event");
        p["results"]["new"]["classify_temporal"] = json!("future_change");
        let request = evaluation::request_task(
            &snapshot,
            &p,
            &json!({"nodeId":"new","function":"estimate_likelihood","depth":1}),
        )
        .unwrap();
        assert!(request["state"]["prerequisites"][0]["assessment"]["evaluate_novelty"].is_null());
        assert!(request["state"]["prerequisites"][0]["evaluations"]["evaluate_novelty"].is_null());
        assert_eq!(
            request["state"]["prerequisites"][0]["assessment"]["estimate_likelihood"],
            "0.4"
        );
        assert!(request["state"].get("historical_search_guidance").is_none());
    }

    #[test]
    fn admission_reserve_preserves_all_stage_budgets() {
        assert_eq!(REASONING_ADMISSION_RESERVE, 44);
        let exploration = transition_limit(&json!({"stage":"exploration"}));
        let combinations = transition_limit(&json!({"stage":"combinations"}));
        let worlds = transition_limit(&json!({"stage":"worlds"}));
        assert_eq!((exploration, combinations, worlds), (232, 264, 436));
        assert_eq!(exploration + 32, combinations);
        assert_eq!(combinations + 44 + 128, worlds);
        assert_eq!(worlds + 44, MAX_APP_TRANSITIONS);
        assert_eq!(MAX_APP_TRANSITIONS + 32, 512);
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
                .all(|t| t["function"] == "classify_claim_role")
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
        assert_eq!(tasks.len(), 126); // Required evaluations remain scheduled.
    }
    #[test]
    fn observed_and_mixed_claims_are_classified_before_forecast_and_do_not_keep_stale_odds() {
        let mut p = plan(&[node("h", "scenario", &[])]).unwrap();
        assert_eq!(p["tasks"][0]["function"], "classify_claim_role");
        for classification in ["already_observed", "mixed", "future_change", "uncertain"] {
            p["cursor"] = json!(3);
            p["results"]["h"] = json!({"classify_claim_role":"event","classify_temporal":classification,"estimate_likelihood":"0.8","evaluate_novelty":"4"});
            p["evaluations"]["h"] = json!({"estimate_likelihood":{"probability":0.8}});
            skip_nonfuture_tasks(&mut p).unwrap();
            let excluded = matches!(classification, "already_observed" | "mixed");
            assert_eq!(p["cursor"], json!(if excluded { 6 } else { 3 }));
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
        assert_eq!(p["tasks"].as_array().unwrap().len(), 13);
        assert_eq!(p["tasks"][4]["nodeId"], "z");
        assert_eq!(p["tasks"][1]["function"], "classify_claim_role");
    }
    #[test]
    fn thousands_of_evaluations_are_planned_without_a_cartesian_product() {
        let nodes: Vec<_> = (0..1000)
            .map(|i| node(&format!("h{i}"), "scenario", &[]))
            .collect();
        let p = plan(&nodes).unwrap();
        assert_eq!(p["tasks"].as_array().unwrap().len(), 6000);
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
        assert_eq!(p["tasks"].as_array().unwrap().len(), 6);
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
        assert_eq!(p["tasks"].as_array().unwrap().len(), 600);
        assert_eq!(p["tasks"][599]["depth"], 99);
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

#[cfg(test)]
mod claim_role_tests {
    include!("semantic_claim_role_tests.rs");
}

pub mod proposals {
    include!("semantic_proposals.rs");
}
