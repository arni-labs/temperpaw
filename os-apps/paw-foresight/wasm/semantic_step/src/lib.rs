use temper_wasm_sdk::prelude::*;
mod core {
    include!("../../semantic_core.rs");
}
fn next_phase(
    snapshot: &Value,
    program: &mut Value,
    trace_len: usize,
    elapsed_ms: u64,
) -> &'static str {
    let exhausted = if matches!(
        program["stop_reason"].as_str(),
        Some("trace_budget" | "provider_error" | "time_budget" | "transition_budget")
    ) {
        program["stop_reason"].as_str().unwrap().to_owned()
    } else if trace_len >= core::call_limit(program) {
        "call_budget".into()
    } else if elapsed_ms >= core::time_limit(program) {
        "time_budget".into()
    } else if program["stage"] != "worlds"
        && program["round"].as_u64().unwrap_or(0) >= core::MAX_ROUNDS
    {
        "round_budget".into()
    } else if program["stage"] != "worlds"
        && snapshot["nodes"].as_array().map_or(0, Vec::len) >= core::MAX_NODES - 6
    {
        "node_budget".into()
    } else {
        String::new()
    };
    if program["stage"] == "combinations" {
        core::search::finish_combinations(program);
        if !exhausted.is_empty() {
            program["stop_reason"] = json!(exhausted);
        }
        return "compose";
    }
    if program["stage"] == "worlds" {
        if core::search::refine_worlds(snapshot, program, trace_len, elapsed_ms, &exhausted) {
            return "refine";
        }
        let active = program["active_world_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut unresolved = false;
        let mut has_conflict = false;
        let mut next_questions = 0;
        if !program["world_audits"].is_object() {
            program["world_audits"] = json!({});
        }
        for world in snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| active.contains(&n["Id"]))
        {
            let audit = core::search::audit_world(world, program);
            unresolved |= audit["status"] != "no_conflict_found";
            has_conflict |= audit["status"] == "conflicts_found";
            next_questions += core::search::world_tasks(world).len();
            program["world_audits"][core::field(world, "Id")] = audit;
        }
        let mut revision_allowed = false;
        if has_conflict {
            let mut tasks: Vec<_> = snapshot["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| active.contains(&n["Id"]))
                .flat_map(core::search::world_tasks)
                .collect();
            tasks.insert(0, core::search::world_set_task(&active));
            let mut admission = core::search::refinement_admission(snapshot, program, &tasks);
            let required = admission["required_transitions"]
                .as_u64()
                .unwrap_or(u64::MAX)
                .saturating_add(core::REASONING_TRANSITION_RESERVE);
            revision_allowed = admission["admitted"] == true
                && required <= admission["remaining_transitions"].as_u64().unwrap_or(0);
            admission["required_transitions"] = json!(required);
            admission["admitted"] = json!(revision_allowed);
            program["recomposition_admission"] = admission;
        }
        // Bounded feedback loop, with fresh immutable worlds and fresh audit contexts.
        // Unknowns may remain; never rename a rewrite 'a gap cleared'.
        if has_conflict
            && revision_allowed
            && exhausted.is_empty()
            && program["world_revision"].as_u64().unwrap_or(1) < 3
            && core::MAX_CALLS.saturating_sub(trace_len) >= next_questions
            && elapsed_ms < core::MAX_MS.saturating_sub(180_000)
        {
            program["stop_reason"] = json!("world_revision_needed");
            return "compose";
        }
        program["stop_reason"] = json!(if !exhausted.is_empty() {
            &exhausted
        } else if unresolved {
            "world_audits_incomplete"
        } else {
            "worlds_evaluated"
        });
        return "synthesize";
    }
    if !exhausted.is_empty() {
        program["stop_reason"] = json!(exhausted);
        "compose"
    } else if request_mixed_decomposition(snapshot, program) {
        program["stop_reason"] = json!("temporal_decomposition_needed");
        "explore"
    } else if program["continue_exploring"] == false {
        if program["independent_challenge"]["status"] != "completed" {
            program["independent_challenge"] =
                json!({"status":"pending","trigger":"candidate_generation_reported_saturation"});
            program["stop_reason"] = json!("independent_challenge_pending");
            return "challenge";
        }
        program["stop_reason"] = json!("exploration_converged");
        "compose"
    } else {
        program["stop_reason"] = json!("round_evaluated");
        "explore"
    }
}
fn request_mixed_decomposition(snapshot: &Value, program: &mut Value) -> bool {
    let mut requested = program["temporal_decomposition_requested"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let before = requested.len();
    for node in snapshot["nodes"].as_array().into_iter().flatten() {
        let id = core::field(node, "Id");
        if program["results"][id]["classify_temporal"] == "mixed" && !requested.contains(&json!(id))
        {
            requested.push(json!(id));
        }
    }
    let added = requested.len() > before;
    program["temporal_decomposition_requested"] = json!(requested);
    added
}

fn plan_combination_phase(
    snapshot: &Value,
    program: &mut Value,
    calls: usize,
    elapsed: u64,
) -> bool {
    let search = json!({"stage":"combinations"});
    program["stage"] == "exploration"
        && program["combination_search"].is_null()
        && !matches!(
            program["stop_reason"].as_str(),
            Some("provider_error" | "trace_budget")
        )
        && program["transition_count"].as_u64().unwrap_or(0) < core::transition_limit(&search)
        && elapsed < core::time_limit(&search)
        && core::search::plan_combinations(
            snapshot,
            program,
            core::call_limit(&search).saturating_sub(calls),
        )
}

// Reserve a full independent challenge and a subsequent evaluation window
// before ordinary exploration spends the transition budget. Never reset it.
fn challenge_due(snapshot: &Value, program: &Value, upcoming_transitions: u64) -> bool {
    let limit = core::transition_limit(program);
    let transitions = program["transition_count"].as_u64().unwrap_or(0);
    let trigger = limit.saturating_sub(core::REASONING_TRANSITION_RESERVE + 32);
    program["stage"] == "exploration"
        && program["baseline_status"] == "established"
        && program["independent_challenge"].is_null()
        && transitions.saturating_add(upcoming_transitions) >= trigger
        && transitions.saturating_add(core::REASONING_TRANSITION_RESERVE) < limit
        && snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|node| matches!(core::field(node, "kind"), "scenario" | "revision"))
}

// Research can invalidate every hypothesis judgment, so optional generation
// must leave room to reassess the current graph, not only its pending suffix.
// This is a planning estimate, not a promise about an unseen generated payload.
fn exploration_admission(snapshot: &Value, program: &Value) -> Result<Value, String> {
    let mut scratch = core::plan(snapshot["nodes"].as_array().ok_or("Missing nodes")?)?;
    for key in ["baseline", "batch_byte_cap", "evidence_ids", "round"] {
        if !program[key].is_null() {
            scratch[key] = program[key].clone();
        }
    }
    // Missing temporal results in this cache-free scratch plan must not erase
    // forecast costs. Legacy eligibility is used only for unsent size planning;
    // no classifications, requests or evaluations from it enter the run.
    scratch["baseline_status"] = Value::Null;
    let task_count = scratch["tasks"].as_array().unwrap().len();
    let mut batches = 0u64;
    let mut cursor = 0usize;
    while cursor < task_count {
        scratch["cursor"] = json!(cursor);
        let batch = core::batch::prepare(snapshot, &scratch, task_count - cursor)?;
        cursor += batch.tasks.len();
        batches += 1;
    }
    let evaluation_transitions = batches.saturating_mul(2);
    let required = core::REASONING_TRANSITION_RESERVE + evaluation_transitions + 32;
    let remaining = core::transition_limit(program)
        .saturating_sub(program["transition_count"].as_u64().unwrap_or(0));
    Ok(
        json!({"admitted":remaining >= required,"remaining_transitions":remaining,"required_transitions":required,"reasoning_reserve":core::REASONING_TRANSITION_RESERVE,"current_graph_evaluation_transitions":evaluation_transitions,"new_work_reserve":32,"estimated_batches":batches,"current_graph_tasks":task_count,"unseen_payload_bounded":false}),
    )
}

fn step(ctx: &Context) -> Result<(), String> {
    let mut program = core::parse(core::field(&ctx.entity_state, "program_json"))?;
    let trace = core::parse(core::field(&ctx.entity_state, "trace_json"))?;

    program["transition_count"] = json!(core::transition_count(&ctx.entity_state));
    if core::transition_count(&ctx.entity_state) >= core::transition_limit(&program) {
        program["stop_reason"] = json!("transition_budget");
    }
    core::skip_nonfuture_tasks(&mut program)?;
    let cursor = program["cursor"].as_u64().ok_or("Missing cursor")? as usize;
    let started = core::field(&ctx.entity_state, "started_at_ms")
        .parse::<u64>()
        .map_err(|_| "Missing run start time")?;
    let elapsed = (Context::get_time_millis() as u64).saturating_sub(started);
    let count = program["tasks"].as_array().ok_or("Missing tasks")?.len();
    let calls = trace.as_array().ok_or("Missing trace")?.len();
    if trace.to_string().len().saturating_add(192 * 1024) > core::MAX_TRACE_BYTES {
        program["stop_reason"] = json!("trace_budget");
    }
    let stopped = matches!(
        program["stop_reason"].as_str(),
        Some(
            "trace_budget" | "provider_error" | "time_budget" | "call_budget" | "transition_budget"
        )
    );
    let snapshot = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
    if program["stage"] == "worlds" && program["tasks"][0]["function"] == "check_world_set" {
        let task = program["tasks"][0].clone();
        let id = core::field(&task, "nodeId").to_owned();
        if program["world_set_audit"]["task_id"] != id
            && let Some(verdict) = program["results"][&id]["check_world_set"]
                .as_str()
                .map(str::to_owned)
        {
            let mut audit = json!({"task_id":id,"revision":program["world_revision"],"world_ids":task["world_ids"],"verdict":verdict,"evaluation":program["evaluations"][&id]["check_world_set"],"correction_status":"not_needed"});
            let mut revise = false;
            if verdict == "complementary_slices" {
                let mut admission = core::search::refinement_admission(
                    &snapshot,
                    &program,
                    program["tasks"].as_array().unwrap(),
                );
                // This is correction of an untested draft, not optional replacement
                // of completed probabilities. Reserve expected packed work plus two
                // extra HTTP attempts overall; do not promise every batch can retry.
                let required = admission["estimated_batches"]
                    .as_u64()
                    .unwrap_or(u64::MAX)
                    .saturating_mul(2)
                    .saturating_add(core::REASONING_TRANSITION_RESERVE + 2 + 4);
                admission["retry_attempts_total"] = json!(2);
                admission
                    .as_object_mut()
                    .unwrap()
                    .remove("retry_attempts_per_batch");
                revise = !stopped
                    && program["world_revision"].as_u64().unwrap_or(1) < 3
                    && admission["estimated_batches"].is_u64()
                    && required <= admission["remaining_transitions"].as_u64().unwrap_or(0)
                    && calls + program["tasks"].as_array().unwrap().len() < core::MAX_CALLS
                    && elapsed < core::time_limit(&program).saturating_sub(120_000);
                admission["required_transitions"] = json!(required);
                admission["admitted"] = json!(revise);
                program["world_set_admission"] = admission;
                audit["correction_status"] = json!(if revise {
                    "revision_requested"
                } else if program["world_revision"].as_u64().unwrap_or(1) >= 3 {
                    "revision_limit"
                } else {
                    "transition_budget"
                });
            }
            program["world_set_audits"][&id] = audit.clone();
            program["world_set_audit"] = audit;
            if revise {
                program["stop_reason"] = json!("world_set_revision_needed");
                set_success_result(
                    "Reason",
                    &json!({"phase":"compose","program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
                );
                return Ok(());
            }
            set_success_result(
                "SearchPlanned",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
    }
    if !stopped
        && calls < core::call_limit(&program)
        && elapsed < core::time_limit(&program)
        && challenge_due(&snapshot, &program, 0)
    {
        program["independent_challenge"] =
            json!({"status":"pending","trigger":"reserved_transition_window"});
        program["stop_reason"] = json!("independent_challenge_pending");
        program["remaining_calls"] = json!(core::MAX_CALLS.saturating_sub(calls));
        program["remaining_round_tasks"] = json!(count.saturating_sub(cursor));
        set_success_result(
            "Reason",
            &json!({"phase":"challenge","program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
        );
        return Ok(());
    }
    if cursor >= count
        || calls >= core::call_limit(&program)
        || elapsed >= core::time_limit(&program)
        || stopped
    {
        program["remaining_calls"] = json!(core::MAX_CALLS.saturating_sub(calls));
        program["remaining_round_tasks"] = json!(count.saturating_sub(cursor));
        let mut phase = next_phase(&snapshot, &mut program, calls, elapsed);
        // A further generation phase can consume its full allowance before the
        // next step. Challenge now rather than jump over the reserved window.
        if phase == "explore"
            && challenge_due(&snapshot, &program, core::REASONING_TRANSITION_RESERVE)
        {
            program["independent_challenge"] =
                json!({"status":"pending","trigger":"reserved_before_next_exploration"});
            program["stop_reason"] = json!("independent_challenge_pending");
            phase = "challenge";
        }
        if phase == "explore" {
            let has_hypotheses = snapshot["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|node| matches!(core::field(node, "kind"), "scenario" | "revision"));
            // The first hypothesis generation is required work, not optional
            // research. Do not price a repeat of baseline evidence against it.
            let admission = if !has_hypotheses {
                json!({"admitted":true,"reason":"initial_hypotheses_required"})
            } else {
                exploration_admission(&snapshot, &program)
                    .unwrap_or_else(|error| json!({"admitted":false,"planning_error":error}))
            };
            let admitted = admission["admitted"] == true;
            program["exploration_admission"] = admission;
            if !admitted {
                program["stop_reason"] = json!("transition_budget");
                phase = "compose";
            }
        }
        if phase == "compose" && program["baseline_status"] == "established" {
            let eligible = snapshot["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|node| {
                    matches!(core::field(node, "kind"), "scenario" | "revision")
                        && core::temporal_allows_forecast(&program, core::field(node, "Id"))
                })
                .count();
            if eligible < 3 {
                set_success_result(
                    "Fail",
                    &json!({"error_message":format!("Cannot compose worlds: only {eligible} current eligible hypotheses; at least three are required. Not enough currently evaluated future candidates are available.")}),
                );
                return Ok(());
            }
        }
        if phase == "refine" {
            set_success_result(
                "SearchPlanned",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
        if phase == "compose" && plan_combination_phase(&snapshot, &mut program, calls, elapsed) {
            set_success_result(
                "SearchPlanned",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
        set_success_result(
            "Reason",
            &json!({"phase":phase,"program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
        );
    } else {
        let request = core::request(&snapshot, &program)?;
        set_success_result("Evaluate", &json!({"request_json":request.to_string()}));
    }
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host().and_then(|ctx| step(&ctx)) {
        Ok(()) => (),
        Err(e) => set_success_result("Fail", &json!({"error_message":e})),
    };
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_challenge_has_a_reserved_window_without_repeating_or_overrunning() {
        let limit = core::transition_limit(&json!({"stage":"exploration"}));
        let trigger = limit - core::REASONING_TRANSITION_RESERVE - 32;
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario"}]});
        let mut p = json!({"stage":"exploration","baseline_status":"established","transition_count":trigger-1});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["transition_count"] = json!(trigger - 6);
        assert!(challenge_due(
            &snapshot,
            &p,
            core::REASONING_TRANSITION_RESERVE
        ));
        p["transition_count"] = json!(trigger);
        assert!(challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = json!({"status":"pending"});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = json!({"status":"completed"});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = Value::Null;
        p["transition_count"] = json!(limit - core::REASONING_TRANSITION_RESERVE);
        assert!(!challenge_due(&snapshot, &p, 0));
        p["transition_count"] = json!(trigger);
        p["stage"] = json!("worlds");
        assert!(!challenge_due(&snapshot, &p, 0));
    }

    #[test]
    fn optional_research_reserves_all_rechecks_without_singleton_overcounting() {
        let nodes:Vec<_>=(0..16).map(|i|json!({"Id":format!("h{i}"),"kind":"scenario","statement":"A future event","edges":"[]"})).collect();
        let snapshot = json!({"world":{},"nodes":nodes});
        let p = json!({"stage":"exploration","baseline_status":"established","transition_count":150,"results":{"h0":{"classify_temporal":"already_observed"}}});
        let before = p.clone();
        let cost = exploration_admission(&snapshot, &p).unwrap();
        assert_eq!(cost["current_graph_tasks"], 80);
        assert_eq!(cost["estimated_batches"], 5);
        assert_eq!(cost["current_graph_evaluation_transitions"], 10);
        assert_eq!(cost["required_transitions"], 86);
        assert_eq!(cost["admitted"], false);
        assert_eq!(p, before);
        let mut early = p;
        early["transition_count"] = json!(20);
        assert_eq!(
            exploration_admission(&snapshot, &early).unwrap()["admitted"],
            true
        );
    }

    fn evaluated_world(label: &str) -> (Value, Value) {
        let world = json!({"Id":"w","kind":"world","statement":"A, B and C occur together","component_ids":["a","b","c"],"counter_ids":[],"edges":"[]","chain":[]});
        let snapshot = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},world]});
        let tasks = core::search::world_tasks(&world);
        let mut program = json!({"stage":"worlds","world_pass":1,"world_revision":1,"active_world_ids":["w"],"evidence_ids":["source"],"tasks":tasks,"results":{},"evaluations":{}});
        for task in tasks {
            let function = core::field(&task, "function");
            let result = match function {
                "estimate_likelihood" => "0.42",
                "check_world_consistency" => label,
                _ => "compatible",
            };
            program["results"][core::field(&task, "nodeId")][function] = json!(result);
        }
        (snapshot, program)
    }

    #[test]
    fn transition_budget_reserves_world_work_and_writer_without_clock_reset() {
        assert_eq!(core::transition_limit(&json!({"stage":"exploration"})), 232);
        assert_eq!(
            core::transition_limit(&json!({"stage":"combinations"})),
            264
        );
        assert_eq!(core::transition_limit(&json!({"stage":"worlds"})), 436);
        const { assert!(core::MAX_APP_TRANSITIONS + 32 <= 512) };
        let mut p = json!({"stage":"exploration","stop_reason":"transition_budget"});
        assert_eq!(
            next_phase(&json!({"nodes":[]}), &mut p, 1001, 1000),
            "compose"
        );
        assert_eq!(p["stop_reason"], "transition_budget");
        let mut p = json!({"stage":"worlds","stop_reason":"transition_budget","active_world_ids":[],"world_pass":1});
        assert_eq!(
            next_phase(&json!({"nodes":[]}), &mut p, 1001, 1000),
            "synthesize"
        );
        assert_eq!(p["stop_reason"], "transition_budget");
    }
    #[test]
    fn world_evaluation_stops_with_real_time_reserved_for_final_writing() {
        let snapshot = json!({"nodes":[]});
        let mut program = json!({"stage":"worlds","active_world_ids":[],"tasks":[],"cursor":0});
        let cutoff = core::time_limit(&program);
        assert_eq!(core::MAX_MS - cutoff, 180_000);
        assert_eq!(
            next_phase(&snapshot, &mut program, 100, cutoff),
            "synthesize"
        );
        assert_eq!(program["stop_reason"], "time_budget");
    }
    #[test]
    fn mixed_temporal_claim_gets_a_decomposition_round_before_saturation() {
        let snapshot = json!({"nodes":[{"Id":"mixed","kind":"scenario"}]});
        let mut program = json!({"stage":"exploration","continue_exploring":false,"results":{"mixed":{"classify_temporal":"mixed"}},"independent_challenge":{"status":"completed"}});
        assert_eq!(next_phase(&snapshot, &mut program, 10, 1000), "explore");
        assert_eq!(program["stop_reason"], "temporal_decomposition_needed");
        assert_eq!(next_phase(&snapshot, &mut program, 10, 1000), "compose");
    }
    #[test]
    fn uncertain_completed_world_keeps_its_probability_and_history_after_refinement() {
        let (snapshot, mut program) = evaluated_world("uncertain");
        let results = program["results"].clone();
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "refine");
        let first = program["world_refinement"]["w"]["rounds"][0].clone();
        program["results"] = results.clone();
        assert_eq!(next_phase(&snapshot, &mut program, 40, 2000), "synthesize");
        assert_eq!(program["world_audits"]["w"]["status"], "uncertain");
        assert_eq!(program["stop_reason"], "world_audits_incomplete");
        assert_eq!(program["results"], results);
        assert_eq!(program["active_world_ids"], json!(["w"]));
        assert_eq!(program["world_revision"], 1);
        assert_eq!(program["world_refinement"]["w"]["rounds"][0], first);
        assert_eq!(
            program["world_refinement"]["w"]["rounds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn explicit_conflict_still_recomposes_after_completed_refinement() {
        let (snapshot, mut program) = evaluated_world("conflict");
        let results = program["results"].clone();
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "refine");
        program["results"] = results;
        assert_eq!(next_phase(&snapshot, &mut program, 40, 2000), "compose");
        assert_eq!(program["world_audits"]["w"]["status"], "conflicts_found");
        assert_eq!(program["stop_reason"], "world_revision_needed");
    }

    #[test]
    fn self_declared_saturation_gets_one_independent_challenge_before_pair_search() {
        let snapshot = json!({"nodes":[]});
        let mut program = json!({"stage":"exploration","continue_exploring":false});
        assert_eq!(next_phase(&snapshot, &mut program, 408, 1000), "challenge");
        assert_eq!(program["independent_challenge"]["status"], "pending");
        assert!(program["combination_search"].is_null());
        program["independent_challenge"]["status"] = json!("completed");
        program["continue_exploring"] = json!(true);
        assert_eq!(next_phase(&snapshot, &mut program, 430, 1000), "explore");
        program["continue_exploring"] = json!(false);
        assert_eq!(next_phase(&snapshot, &mut program, 450, 1000), "compose");
        let mut exhausted = json!({"stage":"exploration","continue_exploring":false});
        assert_eq!(next_phase(&snapshot, &mut exhausted, 1400, 1000), "compose");
        assert!(exhausted["independent_challenge"].is_null());
    }

    #[test]
    fn evaluates_multiple_rounds_instead_of_one_deepening() {
        let s = json!({"nodes":[]});
        let mut p = json!({"round":3,"continue_exploring":true});
        assert_eq!(next_phase(&s, &mut p, 1000, 900000), "explore");
        assert_eq!(p["stop_reason"], "round_evaluated");
    }
    #[test]
    fn operational_budget_stops_without_claiming_convergence() {
        let s = json!({"nodes":[]});
        let mut p = json!({"continue_exploring":true});
        assert_eq!(next_phase(&s, &mut p, 5000, 1), "compose");
        assert_eq!(p["stop_reason"], "call_budget");
    }
    #[test]
    fn generator_can_conclude_without_filling_a_fixed_number() {
        let s = json!({"nodes":[]});
        let mut p =
            json!({"continue_exploring":false,"independent_challenge":{"status":"completed"}});
        assert_eq!(next_phase(&s, &mut p, 213, 1000), "compose");
        assert_eq!(p["stop_reason"], "exploration_converged");
    }
    #[test]
    fn exploration_cap_preserves_pair_search_and_world_capacity() {
        let mut program = json!({"stage":"exploration","continue_exploring":true});
        let snapshot = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"}]});
        let calls = core::call_limit(&program);
        assert_eq!(calls, 1400);
        assert_eq!(next_phase(&snapshot, &mut program, calls, 0), "compose");
        let search_limit = core::call_limit(&json!({"stage":"combinations"}));
        assert_eq!(search_limit, 2400);
        assert!(plan_combination_phase(&snapshot, &mut program, calls, 0));
        assert_eq!(program["tasks"].as_array().unwrap().len(), 3);
        assert!(core::call_limit(&program) > calls);
        assert_eq!(core::MAX_CALLS - search_limit, 2600);
    }

    #[test]
    fn exploration_time_boundary_can_search_but_hard_stops_cannot() {
        let snapshot = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"}]});
        let mut program = json!({"stage":"exploration","stop_reason":"time_budget"});
        let boundary = core::time_limit(&program);
        assert!(plan_combination_phase(
            &snapshot,
            &mut program,
            1400,
            boundary
        ));
        assert_eq!(core::time_limit(&program), boundary + 180_000);
        assert_eq!(core::MAX_MS - core::time_limit(&program), 420_000);
        for reason in ["provider_error", "trace_budget"] {
            let mut stopped = json!({"stage":"exploration","stop_reason":reason});
            assert!(!plan_combination_phase(
                &snapshot,
                &mut stopped,
                1400,
                boundary
            ));
        }
        let mut expired = json!({"stage":"exploration","stop_reason":"time_budget"});
        assert!(!plan_combination_phase(
            &snapshot,
            &mut expired,
            1400,
            boundary + 180_000
        ));
    }

    #[test]
    fn reserves_calls_and_time_for_world_evaluation() {
        let snapshot = json!({"nodes":[]});
        for (calls, time) in [
            (core::MAX_CALLS - core::WORLD_CALL_RESERVE, 0),
            (0, core::MAX_MS - 600_000),
        ] {
            let mut program = json!({"stage":"exploration","continue_exploring":true});
            assert_eq!(next_phase(&snapshot, &mut program, calls, time), "compose");
        }
        let program = json!({"stage":"worlds"});
        assert_eq!(core::call_limit(&program), core::MAX_CALLS);
        assert_eq!(
            core::time_limit(&program),
            core::MAX_MS - core::SYNTHESIS_TIME_RESERVE_MS
        );
    }
    #[test]
    fn provider_failure_composes_qualitative_worlds_without_retrying_provider() {
        let mut p = json!({"stop_reason":"provider_error","results":{}});
        assert_eq!(next_phase(&json!({"nodes":[]}), &mut p, 1, 0), "compose");
        p["stage"] = json!("worlds");
        assert_eq!(next_phase(&json!({"nodes":[]}), &mut p, 1, 0), "synthesize");
    }
    #[test]
    fn world_conflict_triggers_revision_but_never_an_endless_rewrite() {
        let s = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},{"Id":"w","kind":"world","component_ids":["a","b","c"],"counter_ids":[],"edges":"[]","chain":[]}]});
        let mut p = json!({"stage":"worlds","world_revision":1,"active_world_ids":["w"],"results":{"w":{"check_world_consistency":"conflict"}}});
        assert_eq!(next_phase(&s, &mut p, 300, 1000), "compose");
        assert_eq!(p["world_audits"]["w"]["status"], "conflicts_found");
        p["world_revision"] = json!(3);
        assert_eq!(next_phase(&s, &mut p, 300, 1000), "synthesize");
        assert_eq!(p["stop_reason"], "world_audits_incomplete");
    }
}
