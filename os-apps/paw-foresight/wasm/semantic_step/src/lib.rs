use temper_wasm_sdk::prelude::*;
mod core {
    include!("../../semantic_core.rs");
}
/// Complete the admitted diverse answer before optional additional coverage can
/// change its evidence context. Compatibility is not proof of the route.
fn ready_admitted_worlds(snapshot: &Value, program: &Value) -> Option<Value> {
    if program["audit_policy_version"] != 2 {
        return None;
    }
    let bundles = core::endpoints::composition_bundles(snapshot, program);
    let ready: Vec<_> = bundles
        .as_array()?
        .iter()
        .filter(|bundle| {
            bundle["status"] == "compatible"
                && core::proposals::pool::current_novelty_passed(
                    snapshot,
                    program,
                    core::field(bundle, "endpoint_id"),
                )
        })
        .map(|bundle| bundle["endpoint_id"].clone())
        .collect();
    if ready.len() < 2 {
        return None;
    }
    let work = core::endpoints::pending_mandatory_work(snapshot, program).ok()?;
    (work["questions"] == 0).then(|| json!({"endpoint_ids":ready,"pending_mandatory_questions":0,"semantics":"Ready for composition, not causal proof. Unresolved route checks and unconstructed originals remain explicit; final world estimates still require evaluation."}))
}

fn next_phase(
    snapshot: &Value,
    program: &mut Value,
    trace_len: usize,
    elapsed_ms: u64,
) -> &'static str {
    if program["answer_checkpoint"].is_object() && program["targeted_repair"]["status"]=="admitted" {
        return "backward";
    }
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
    // A prior admission authorizes one round; it is not evidence of why search
    // eventually ended. Preserve a later hard budget exit before stage changes.
    if program["stage"] == "exploration" && exhausted.ends_with("_budget") {
        let previous = if program["exploration_admission"]["admitted"] == true {
            program["exploration_admission"].clone()
        } else {
            Value::Null
        };
        program["exploration_admission"] = json!({
            "admitted":false,"reason":exhausted,"observed_exit":true,
            "transition_count":program["transition_count"],"transition_limit":core::transition_limit(program),
            "remaining_transitions":core::transition_limit(program).saturating_sub(program["transition_count"].as_u64().unwrap_or(0)),
            "calls":trace_len,"call_limit":core::call_limit(program),"elapsed_ms":elapsed_ms,"time_limit_ms":core::time_limit(program),
            "previous_admission":previous
        });
    }
    if core::endpoints::enabled(program) && program["stage"] != "worlds" {
        if !program["endpoint_search"].is_object() {
            return "imagine";
        }
        if program["endpoint_search"]["routes"]
            .as_array()
            .is_none_or(Vec::is_empty)
            && program["endpoint_search"]["rounds"]
                .as_array()
                .is_none_or(Vec::is_empty)
        {
            if !exhausted.is_empty()
                || core::research_admission(program, "backward", elapsed_ms)["admitted"] != true
            {
                program["stop_reason"] = json!(if exhausted.is_empty() {
                    "time_budget"
                } else {
                    exhausted.as_str()
                });
                return "compose";
            }
            if !core::backward_work_admission(program, elapsed_ms) {
                program["stop_reason"] = program["backward_work_refusal"]["reason"].clone();
                return "compose";
            }
            return "backward";
        }
        if program["stage"] != "routes"
            && exhausted.is_empty()
            && core::endpoints::plan_routes(snapshot, program)
        {
            return "refine";
        }
        core::endpoints::finish_routes(snapshot, program);
        if program["audit_policy_version"] == 2 && program["admitted_work"]["status"] == "checking"
        {
            let cannot_finish = elapsed_ms >= core::time_limit(program)
                || core::transition_count(program)
                    >= core::MAX_APP_TRANSITIONS - 2 * core::REASONING_ADMISSION_RESERVE - 32
                || trace_len >= core::call_limit(program);
            match core::proposals::pool::defer_before_composition(snapshot, program, cannot_finish)
            {
                Ok(true) => return "refine",
                Ok(false) => {}
                Err(error) => {
                    program["deferred_novelty_recheck"] =
                        json!({"status":"not_admitted","error":error});
                }
            }
            program["admitted_work"]["status"] = json!("completed");
        }
        if let Some(receipt) = ready_admitted_worlds(snapshot, program) {
            program["initial_world_finalization"] = receipt;
            program["stop_reason"] = json!("admitted_worlds_ready_for_composition");
            return "compose";
        }
        if program["answer_checkpoint"].is_object() {
            // The one repair cannot schedule another generation. Current checks
            // and the normal composer still decide whether a new answer exists.
            match core::proposals::pool::defer_before_composition(snapshot,program,!exhausted.is_empty()) {
                Ok(true)=>return "refine", Ok(false)=>{}, Err(error)=>{program["deferred_novelty_recheck"]=json!({"status":"not_admitted","error":error});}
            }
            return "compose";
        }
        let needs_alternative = core::endpoints::alternative_needed(program);
        let remaining = core::transition_limit(&json!({"stage":"routes"}))
            .saturating_sub(core::transition_count(program));
        let allowed = exhausted.is_empty()
            && program["route_finalization"]["admitted"] != true
            && remaining >= core::REASONING_ADMISSION_RESERVE + 16
            && core::research_admission(program, "backward", elapsed_ms)["admitted"] == true;
        program["backward_admission"] = json!({"admitted":allowed,"remaining_transitions":remaining,"required_transitions":core::REASONING_ADMISSION_RESERVE+16,"alternative_required":needs_alternative,"time_admission":core::research_admission(program, "backward", elapsed_ms)});
        if allowed
            && (needs_alternative || program["continue_exploring"] != false)
            && core::backward_work_admission(program, elapsed_ms)
        {
            program["stage"] = json!("exploration");
            program["stop_reason"] = json!(if needs_alternative {
                "backward_alternative_needed"
            } else {
                "backward_search_continues"
            });
            return "backward";
        }
        program["stop_reason"] = json!(if !exhausted.is_empty() {
            exhausted.as_str()
        } else if needs_alternative {
            "backward_routes_unresolved"
        } else {
            "backward_routes_evaluated"
        });
        match core::proposals::pool::defer_before_composition(snapshot, program, !exhausted.is_empty()) {
            Ok(true) => return "refine",
            Ok(false) => {},
            Err(error) => { program["deferred_novelty_recheck"] = json!({"status":"not_admitted","error":error}); }
        }
        return "compose";
    }
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
        let mut incomplete = false;
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
            incomplete |= audit["completed_checks"].as_u64().unwrap_or(0)
                < audit["planned_checks"].as_u64().unwrap_or(0);
            has_conflict |= audit["status"] == "conflicts_found";
            next_questions += core::search::audit_tasks(world, program).len();
            program["world_audits"][core::field(world, "Id")] = audit;
        }
        let mut revision_allowed = false;
        if has_conflict {
            let mut tasks: Vec<_> = snapshot["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| active.contains(&n["Id"]))
                .flat_map(|world| core::search::audit_tasks(world, program))
                .collect();
            tasks.splice(0..0, core::search::world_set_tasks(&active));
            let mut admission = core::search::refinement_admission(snapshot, program, &tasks);
            let required = admission["required_transitions"]
                .as_u64()
                .unwrap_or(u64::MAX)
                .saturating_add(core::REASONING_ADMISSION_RESERVE);
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
        } else if incomplete {
            "world_audits_incomplete"
        } else if unresolved {
            "world_audits_unresolved"
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
    !core::endpoints::enabled(program)
        && program["stage"] == "exploration"
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
    let trigger = limit.saturating_sub(core::REASONING_ADMISSION_RESERVE + 32);
    // Endpoint search already challenges mechanisms through backward alternatives.
    // Its queued assessments must finish before admitting another research pass.
    !core::endpoints::enabled(program)
        && program["stage"] == "exploration"
        && program["baseline_status"] == "established"
        && program["independent_challenge"].is_null()
        && transitions.saturating_add(upcoming_transitions) >= trigger
        && transitions.saturating_add(core::REASONING_ADMISSION_RESERVE) < limit
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
    for key in [
        "baseline",
        "batch_byte_cap",
        "batch_byte_caps",
        "endpoint_proposal_contract",
        "world_search_contract",
        "stage",
        "evidence_ids",
        "round",
    ] {
        if !program[key].is_null() {
            scratch[key] = program[key].clone();
        }
    }
    // Missing temporal results in this cache-free scratch plan must not erase
    // forecast costs. Legacy eligibility is used only for unsent size planning;
    // no classifications, requests or evaluations from it enter the run.
    scratch["baseline_status"] = Value::Null;
    scratch["claim_role_contract"] = Value::Null;
    core::defer_recorded_rankings(&mut scratch, program);
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
    let required = core::REASONING_ADMISSION_RESERVE + evaluation_transitions + 32;
    let remaining = core::transition_limit(program)
        .saturating_sub(program["transition_count"].as_u64().unwrap_or(0));
    Ok(
        json!({"admitted":remaining >= required,"remaining_transitions":remaining,"required_transitions":required,"reasoning_reserve":core::REASONING_ADMISSION_RESERVE,"current_graph_evaluation_transitions":evaluation_transitions,"new_work_reserve":32,"estimated_batches":batches,"current_graph_tasks":task_count,"unseen_payload_bounded":false}),
    )
}

fn comparison_admission(program: &Value, remaining: u64, elapsed: u64) -> Value {
    let optional = program["proposal_pool"]["novelty_repair"]["status"] == "pending"
        || program["proposal_pool"]["creative_repair"]["status"] == "pending"
        || program["endpoint_proposal_attempt"]["status"] == "revision_requested";
    let optional_time = core::optional_repair_time_admission(program, elapsed);
    let completion_time = core::research_admission(program, "explore", elapsed);
    let transitions = core::proposals::pool::admits(remaining, 2, 160);
    let reason = if !transitions {
        Some("transition_budget")
    } else if completion_time["admitted"] != true || (optional && optional_time["admitted"] != true)
    {
        Some("time_budget")
    } else {
        None
    };
    json!({"admitted":reason.is_none(),"reason":reason,"optional_repair":optional,"remaining_transitions":remaining,"transition_admitted":transitions,"completion_time":completion_time,"optional_time":if optional {optional_time}else{Value::Null}})
}

fn admit_development(program: &mut Value, elapsed: u64) -> bool {
    let mut admission = core::optional_repair_time_admission(program, elapsed);
    let generation = core::generation_duration(program, "imagine");
    let required = admission["required_ms"]
        .as_u64()
        .unwrap()
        .saturating_add(generation);
    admission["development_ms"] = json!(generation);
    admission["required_ms"] = json!(required);
    admission["admitted"] = json!(elapsed.saturating_add(required) < core::MAX_MS);
    program["proposal_pool"]["development_admission"]["time_admission"] = admission.clone();
    if admission["admitted"] == true {
        return true;
    }
    program["proposal_pool"]["development_admission"]["admitted"] = json!(false);
    program["proposal_pool"]["development_admission"]["reason"] = json!("time_budget");
    program["proposal_pool"]["stage"] = json!("unresolved");
    program["endpoint_proposal_attempt"]["status"] = json!("unresolved");
    program["stop_reason"] = json!("time_budget");
    false
}

// Preserve why checks stopped separately from the judgments they would have produced.
fn proposal_resource_reason(program: &Value, calls: usize, elapsed: u64) -> Option<&'static str> {
    match core::field(program, "stop_reason") {
        "trace_budget" => return Some("trace_budget"),
        "provider_error" => return Some("provider_error"),
        "time_budget" => return Some("time_budget"),
        "call_budget" => return Some("call_budget"),
        "transition_budget" => return Some("transition_budget"),
        _ => {}
    }
    if calls >= core::call_limit(program) {
        Some("call_budget")
    } else if elapsed >= core::time_limit(program) {
        Some("time_budget")
    } else {
        None
    }
}

fn proposal_resource_limit(reason: &str) -> &'static str {
    match reason {
        "time_budget" => "the research time limit reserved for reconstruction and final writing",
        "call_budget" => "the reserved evaluation-call limit",
        "transition_budget" => "the reserved execution-step limit",
        "trace_budget" => "the saved-evaluation size limit",
        "provider_error" => "a provider failure",
        _ => "a resource limit",
    }
}

fn composition_unavailable_message(program: &Value, eligible: usize, calls: usize, elapsed: u64) -> String {
    if let Some(reason) = proposal_resource_reason(program, calls, elapsed) {
        let pending_checks = program["tasks"].as_array().map_or(0, Vec::len)
            .saturating_sub(program["cursor"].as_u64().unwrap_or(0) as usize);
        let pending_routes = program["endpoint_search"]["routes"].as_array().into_iter().flatten()
            .filter(|route| route["status"] == "pending").count();
        return format!("World reconstruction stopped because of {} ({reason}). {pending_checks} candidate checks and {pending_routes} pending routes remain saved; {eligible} hypotheses are currently eligible for composition. Unevaluated work is not a rejected future. No completed worlds or whole-world estimates are available.", proposal_resource_limit(reason));
    }
    format!("Cannot compose worlds: only {eligible} current eligible hypotheses; at least three are required. Not enough currently evaluated future candidates are available.")
}

fn contrast_resource_message(reason: &str) -> String {
    let limit = proposal_resource_limit(reason);
    format!("Present-day comparison research remains incomplete because of {limit} ({reason}). No current comparison checks were scheduled. Saved research and earlier receipts are preserved; this is not a failed novelty judgment. No endpoint was accepted and no whole-world estimates were made.")
}

fn proposal_resource_message(program: &Value, reason: &str) -> String {
    let limit = proposal_resource_limit(reason);
    let checks = program["endpoint_proposal_attempt"]["checks"].as_array();
    let planned = program["endpoint_proposal_attempt"]["tasks"]
        .as_array()
        .map_or(0, Vec::len);
    let completed = checks
        .into_iter()
        .flatten()
        .filter(|c| !c["evaluation"].is_null())
        .count();
    format!(
        "Candidate comparison stopped because of {limit} ({reason}). {completed} of {planned} proposal checks were completed. Unevaluated checks are not failed novelty judgments. Saved research and comparison receipts are preserved; no endpoint was accepted and no whole-world estimates were made."
    )
}

fn proposal_terminal_message(program: &Value) -> String {
    if let Some(reason) = proposal_resource_reason(program, 0, 0) {
        proposal_resource_message(program, reason)
    } else {
        "Endpoint proposal quality unresolved: the bounded search did not produce sufficiently distinct consequential worlds. No endpoint was accepted and no whole-world estimates were made.".into()
    }
}
fn pending_assessment(
    program: &Value,
    cursor: usize,
    count: usize,
    calls: usize,
    elapsed: u64,
    stopped: bool,
) -> bool {
    cursor < count
        && calls < core::call_limit(program)
        && elapsed < core::time_limit(program)
        && !stopped
}

// Finish accepted path work only when its actual packed workload and the
// current present-comparison checks both fit before the protected writer tail.
fn admit_route_finalization(
    snapshot: &Value,
    program: &mut Value,
    calls: usize,
    elapsed: u64,
) -> Result<bool, String> {
    if program["stage"] != "routes"
        || !core::endpoints::enabled(program)
        || program["route_finalization"].is_object()
        || elapsed >= core::time_limit(program)
        || matches!(
            core::field(program, "stop_reason"),
            "provider_error" | "trace_budget" | "time_budget" | "call_budget"
        )
    {
        return Ok(false);
    }
    let mut scratch = program.clone();
    let count = scratch["tasks"]
        .as_array()
        .ok_or("Missing route tasks")?
        .len();
    let start = scratch["cursor"].as_u64().ok_or("Missing route cursor")? as usize;
    if calls.saturating_add(count.saturating_sub(start)) >= core::call_limit(program) {
        return Ok(false);
    }
    let mut cursor = start;
    let mut batches = 0u64;
    while cursor < count {
        scratch["cursor"] = json!(cursor);
        let batch = core::batch::prepare(snapshot, &scratch, count - cursor)?;
        if batch.tasks.is_empty() {
            return Err("Route finalization cannot pack pending work".into());
        }
        cursor += batch.tasks.len();
        batches += 1;
    }
    let mut novelty = program.clone();
    core::proposals::pool::defer_before_composition(snapshot, &mut novelty, false)?;
    let novelty_cost = if novelty["deferred_novelty_recheck"] != program["deferred_novelty_recheck"]
    {
        novelty["deferred_novelty_recheck"]["required_transitions"]
            .as_u64()
            .unwrap_or(0)
    } else {
        0
    };
    let required = batches
        .saturating_mul(2)
        .saturating_add(novelty_cost)
        .saturating_add(4);
    let limit = core::MAX_APP_TRANSITIONS - 2 * core::REASONING_ADMISSION_RESERVE - 32;
    let remaining = limit.saturating_sub(core::transition_count(program));
    let admitted = required <= remaining;
    program["route_finalization"] = json!({"admitted":admitted,"pending_checks":count.saturating_sub(start),"estimated_batches":batches,"novelty_transitions":novelty_cost,"handoff_transitions":4,"required_transitions":required,"remaining_transitions":remaining,"transition_limit":limit,"prior_stop_reason":program["stop_reason"],"new_research_allowed":false});
    if admitted && program["stop_reason"] == "transition_budget" {
        program.as_object_mut().unwrap().remove("stop_reason");
    }
    Ok(admitted)
}

fn standalone_scope_repair(program: &Value) -> bool {
    program["scope_repair"]["status"] == "pending" && !core::proposals::pool::enabled(program)
}

fn step(ctx: &Context) -> Result<(), String> {
    let mut program = core::parse(core::field(&ctx.entity_state, "program_json"))?;
    let trace = core::parse(core::field(&ctx.entity_state, "trace_json"))?;

    program["transition_count"] = json!(core::transition_count(&ctx.entity_state));
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
    let snapshot = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
    if program["targeted_repair"]["status"]=="admitted" && (elapsed.saturating_add(core::generation_duration(&program,"backward")).saturating_add(core::ENDPOINT_EVALUATION_DRAIN_MS).saturating_add(program["targeted_repair"]["admission"]["finalization_time_reserve_ms"].as_u64().unwrap_or(core::WORLD_TIME_RESERVE_MS))>=core::MAX_MS || core::transition_count(&ctx.entity_state)>=core::MAX_APP_TRANSITIONS) {
        return Err("Optional repair reservation expired before dispatch; the completed initial answer is preserved in its original assessment context".into());
    }
    if core::transition_count(&ctx.entity_state) >= core::transition_limit(&program)
        && !admit_route_finalization(&snapshot, &mut program, calls, elapsed)?
    {
        program["stop_reason"] = json!("transition_budget");
    }
    let stopped = matches!(
        program["stop_reason"].as_str(),
        Some(
            "trace_budget" | "provider_error" | "time_budget" | "call_budget" | "transition_budget"
        )
    );
    if standalone_scope_repair(&program) {
        let remaining = core::transition_limit(&program)
            .saturating_sub(core::transition_count(&ctx.entity_state));
        let allowed = !stopped
            && calls < core::call_limit(&program)
            && core::research_admission(&program, "explore", elapsed)["admitted"] == true
            && remaining >= core::REASONING_ADMISSION_RESERVE + 32
            && core::field(&snapshot["world"], "hindcast_mode") == "false";
        if allowed {
            set_success_result(
                "Reason",
                &json!({"phase":"explore","program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
            );
            return Ok(());
        }
        program["scope_repair"] = json!({"status":"skipped","attempted":false,"coverage_certified":false,"reason":if core::field(&snapshot["world"], "hindcast_mode") != "false" {"frozen_evidence_only"} else {"resource_limit"}});
        set_success_result(
            "SearchPlanned",
            &json!({"program_json":program.to_string()}),
        );
        return Ok(());
    }
    if core::proposals::pool::research_pending(&program) {
        let recorded_refusal =
            program["proposal_pool"]["comparison_admission"]["admitted"] == false;
        let remaining =
            core::MAX_APP_TRANSITIONS.saturating_sub(core::transition_count(&ctx.entity_state));
        let admission = comparison_admission(&program, remaining, elapsed);
        program["proposal_pool"]["comparison_admission"] = admission.clone();
        if admission["optional_repair"] == true {
            program["proposal_pool"]["repair_time_admission"] = admission["optional_time"].clone();
        }
        if !stopped && admission["admitted"] == true {
            set_success_result(
                "Reason",
                &json!({"phase":"explore","program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
            );
        } else if core::proposals::pool::skip_prefreeze_repair(&mut program) {
            set_success_result(
                "SearchPlanned",
                &json!({"program_json":program.to_string()}),
            );
        } else {
            let reason = proposal_resource_reason(&program, calls, elapsed)
                .unwrap_or(admission["reason"].as_str().unwrap_or("transition_budget"));
            if recorded_refusal {
                set_success_result(
                    "Fail",
                    &json!({"error_message":contrast_resource_message(reason)}),
                );
            } else {
                program["stop_reason"] = json!(reason);
                set_success_result(
                    "SearchPlanned",
                    &json!({"program_json":program.to_string()}),
                );
            }
        }
        return Ok(());
    }
    if core::endpoints::enabled(&program)
        && !program["endpoint_search"].is_object()
        && program["endpoint_proposal_attempt"]["status"] != "checking"
    {
        if program["endpoint_proposal_attempt"]["status"] == "unresolved" {
            set_success_result(
                "Fail",
                &json!({"error_message":proposal_terminal_message(&program)}),
            );
            return Ok(());
        }
        if program["proposal_pool"]["stage"] == "enrich"
            && !admit_development(&mut program, elapsed)
        {
            set_success_result(
                "SearchPlanned",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
        let admission = core::research_admission(&program, "imagine", elapsed);
        if admission["admitted"] != true {
            set_success_result(
                "Fail",
                &json!({"error_message":format!("No admitted time for endpoint generation and reassessment; saved work remains available. {admission}")}),
            );
            return Ok(());
        }
        set_success_result(
            "Reason",
            &json!({"phase":"imagine","program_json":program.to_string(),"trace_json":trace.to_string(),"reasoning_phase_polls":0}),
        );
        return Ok(());
    }
    if program["stage"] == "proposals"
        && (cursor >= count
            || stopped
            || calls >= core::call_limit(&program)
            || elapsed >= core::time_limit(&program))
    {
        let remaining =
            core::transition_limit(&program).saturating_sub(core::transition_count(&program));
        let resource_exhausted =
            stopped || calls >= core::call_limit(&program) || elapsed >= core::time_limit(&program);
        if let Some(reason) = proposal_resource_reason(&program, calls, elapsed) {
            program["stop_reason"] = json!(reason);
        }
        let retry = !resource_exhausted
            && remaining >= core::REASONING_ADMISSION_RESERVE + 16
            && core::research_admission(&program, "imagine", elapsed)["admitted"] == true;
        if core::proposals::pool::enabled(&program) {
            let global_remaining =
                core::MAX_APP_TRANSITIONS.saturating_sub(core::transition_count(&ctx.entity_state));
            let bounded_retry = retry && core::proposals::pool::admits(global_remaining, 2, 160);
            core::proposals::pool::finish(
                &snapshot,
                &mut program,
                bounded_retry,
                resource_exhausted,
            )?;
        } else {
            core::proposals::finish(&mut program, retry, resource_exhausted)?;
        }
        if program["endpoint_proposal_attempt"]["status"] == "unresolved" && !resource_exhausted {
            program["stop_reason"] = json!("endpoint_proposal_quality");
        }
        // Publish the exact receipt before any next generation or terminal failure.
        set_success_result(
            "SearchPlanned",
            &json!({"program_json":program.to_string()}),
        );
        return Ok(());
    }
    if program["stage"] == "worlds"
        && let Some(mut audit) = core::search::pending_world_set_audit(
            &program,
            stopped || calls >= core::call_limit(&program) || elapsed >= core::time_limit(&program),
        )
    {
        let id = core::field(&audit, "task_id").to_owned();
        let verdict = core::field(&audit, "verdict").to_owned();
        let mut revise = false;
        let comparison_contract = snapshot["nodes"].as_array().unwrap().iter().any(|n| {
            n["comparison_contract"] == "v1"
                && program["active_world_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|id| n["Id"] == *id)
        });
        let correction_used = comparison_contract
            && program["world_set_audits"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
                .any(|a| a["correction_status"] == "revision_requested");
        let needs_correction =
            verdict == "complementary_slices" || audit["binding_unresolved"] == true;
        if needs_correction && correction_used {
            audit["correction_status"] = json!("revision_limit");
        }
        if needs_correction && !correction_used {
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
                .saturating_add(core::REASONING_ADMISSION_RESERVE + 2 + 4);
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
                && core::research_admission(&program, "compose", elapsed)["admitted"] == true;
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
    if !pending_assessment(&program, cursor, count, calls, elapsed, stopped) {
        program["remaining_calls"] = json!(core::MAX_CALLS.saturating_sub(calls));
        program["remaining_round_tasks"] = json!(count.saturating_sub(cursor));
        let mut phase = next_phase(&snapshot, &mut program, calls, elapsed);
        // A further generation phase can consume its full allowance before the
        // next step. Challenge now rather than jump over the reserved window.
        if phase == "explore"
            && challenge_due(&snapshot, &program, core::REASONING_ADMISSION_RESERVE)
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
                        && core::branches::future_eligible(
                            &snapshot,
                            &program,
                            core::field(node, "Id"),
                        )
                })
                .count();
            if eligible < 3 {
                set_success_result(
                    "Fail",
                    &json!({"error_message":composition_unavailable_message(&program, eligible, calls, elapsed)}),
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

    include!("../../semantic_repair_fixture.rs");

    #[test]
    fn repair_queue_uses_current_recorded_checks_and_never_repeats_no_change() {
        let (snapshot, mut p) = ready_pair();
        let queue = core::backward::repair_obligations(&snapshot, &p).unwrap();
        assert!(!queue.is_empty());
        assert_eq!(queue[0]["kind"], "root_gap");
        for item in &queue {
            for observed in item["observed_results"].as_array().unwrap() {
                let t = &observed["task"];
                assert_eq!(
                    observed["result"],
                    p["results"][core::field(t, "nodeId")][core::field(t, "function")]
                );
                assert_eq!(
                    observed["evaluation"],
                    p["evaluations"][core::field(t, "nodeId")][core::field(t, "function")]
                );
                assert!(observed.get("explanation").is_none());
            }
        }
        p["targeted_repair_history"] =
            json!([{"input_fingerprint":queue[0]["input_fingerprint"],"status":"no_change"}]);
        let after = core::backward::repair_obligations(&snapshot, &p).unwrap();
        assert!(
            !after
                .iter()
                .any(|item| item["input_fingerprint"] == queue[0]["input_fingerprint"])
        );
        p["baseline"]["unknowns"] = json!(["Changed present context"]);
        assert!(
            core::backward::repair_obligations(&snapshot, &p)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn complete_admitted_pair_precedes_optional_research_without_certifying_gaps() {
        let (snapshot, mut p) = ready_pair();
        assert_eq!(
            core::endpoints::pending_mandatory_work(&snapshot, &p).unwrap()["questions"],
            0
        );
        assert_eq!(next_phase(&snapshot, &mut p, 20, 1_000_000), "compose");
        assert_eq!(
            p["endpoint_search"]["endpoints"].as_array().unwrap().len(),
            3
        );
        assert!(
            p["endpoint_search"]["routes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "unresolved")
        );
        assert_eq!(
            p["initial_world_finalization"]["endpoint_ids"],
            json!(["a", "b"])
        );
        let mut stale = p.clone();
        stale["baseline"]["unknowns"] = json!(["New present uncertainty"]);
        assert!(ready_admitted_worlds(&snapshot, &stale).is_none());
        let mut pending = p.clone();
        pending["results"]["target-a"]["estimate_likelihood"] = Value::Null;
        assert!(ready_admitted_worlds(&snapshot, &pending).is_none());
        let mut historical = p.clone();
        historical
            .as_object_mut()
            .unwrap()
            .remove("audit_policy_version");
        assert!(ready_admitted_worlds(&snapshot, &historical).is_none());
    }

    #[test]
    #[ignore = "uses supplied frozen pass16 checkpoints"]
    fn captured_pass16_pair_finishes_before_third_original() {
        let dir = std::env::var("FORESIGHT_PASS16_DIR").unwrap();
        for topic in ["games", "food"] {
            let record: Value =
                serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{topic}.json")).unwrap())
                    .unwrap();
            let snapshot = core::parse(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
            let mut p = core::parse(record["fields"]["program_json"].as_str().unwrap()).unwrap();
            let originals = p["endpoint_search"]["endpoints"].clone();
            let routes = p["endpoint_search"]["routes"].clone();
            assert_eq!(
                ready_admitted_worlds(&snapshot, &p).unwrap()["endpoint_ids"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(next_phase(&snapshot, &mut p, 200, 1_660_000), "compose");
            assert_eq!(p["endpoint_search"]["endpoints"], originals);
            assert_eq!(p["endpoint_search"]["routes"], routes);
        }
    }

    #[test]
    fn admitted_work_capacity_spans_candidates_routes_and_novelty_without_reset() {
        let mut p = json!({"audit_policy_version":2,"world_search_contract":1,"stage":"exploration","transition_count":60,"reasoning_episode_durations_ms":{"backward":1260000}});
        assert!(core::backward_work_admission(&mut p, 16 * 60000));
        assert_eq!(p["admitted_work"]["evaluation_transition_capacity"], 256);
        p["admitted_work"]["status"] = json!("checking");
        for stage in ["exploration", "routes", "proposals"] {
            p["stage"] = json!(stage);
            assert_eq!(core::transition_limit(&p), 360);
            assert_eq!(core::time_limit(&p), 50*60_000);
        }
        let admitted = p["admitted_work"].clone();
        assert!(!core::backward_work_admission(&mut p, 38 * 60000));
        assert_eq!(p["admitted_work"], admitted);
        assert_eq!(p["backward_work_refusal"]["reason"], "time_budget");
        assert!(!core::backward_work_admission(&mut p, core::MAX_MS));
        p["transition_count"] = json!(480);
        assert!(!core::backward_work_admission(&mut p, 0));
        assert!(p.get("started_at_ms").is_none());
    }

    #[test]
    #[ignore = "uses the frozen pass15 music checkpoint supplied by FORESIGHT_MUSIC_CAPTURE"]
    fn captured_music_workload_counts_unfinished_candidates_and_routes_together() {
        let data: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_MUSIC_CAPTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let mut p: Value =
            serde_json::from_str(data["fields"]["program_json"].as_str().unwrap()).unwrap();
        let snapshot: Value =
            serde_json::from_str(data["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
        assert_eq!(p["stage"], "exploration");
        assert_eq!(p["cursor"], 92);
        p["audit_policy_version"] = json!(2);
        // Replay the work before the terminal admission refusal. The frozen
        // record has already rewritten these pending novelty statuses unresolved.
        p["deferred_novelty_recheck"] = Value::Null;
        for receipt in p["endpoint_novelty"].as_object_mut().unwrap().values_mut() {
            if receipt["reason"].as_str().is_some_and(|reason|reason.starts_with("No admitted comparison recheck")) {
                receipt["status"] = json!("provisional");
            }
        }
        let work = core::endpoints::pending_mandatory_work(&snapshot, &p).unwrap();
        assert!(!work["candidate_tasks"].as_array().unwrap().is_empty());
        assert!(!work["route_tasks"].as_array().unwrap().is_empty());
        assert_eq!(work["novelty_rechecks"], 5);
        eprintln!(
            "captured music candidate_tasks={} route_tasks={} packed={} contingent={} required={}",
            work["candidate_tasks"].as_array().unwrap().len(),
            work["route_tasks"].as_array().unwrap().len(),
            work["known_packed_http_requests"],
            work["contingent_questions"],
            work["required_transitions"]
        );
        // No claim this already exhausted historical run can now fit. The new
        // owner must count all of its work rather than stage-gate it away.
    }

    #[test]
    fn education_generation_episode_counts_corrective_children_before_admission() {
        // Captured education: final child started at53.13min, completed59.16;
        // preceding route assessments ended38.01. Simulate a21-minute episode.
        let mut p = json!({"stage":"exploration","world_search_contract":1});
        core::start_reasoning_timing(&mut p, "backward", 1000);
        core::finish_reasoning_timing(&mut p, "backward", 541000);
        p["response_correction"] = json!({"attempt":1});
        core::start_reasoning_timing(&mut p, "backward", 541000);
        core::finish_reasoning_timing(&mut p, "backward", 901000);
        p["response_correction"]["attempt"] = json!(2);
        core::start_reasoning_timing(&mut p, "backward", 901000);
        core::finish_reasoning_timing(&mut p, "backward", 1261000);
        assert_eq!(p["reasoning_durations_ms"]["backward"], 540000);
        assert_eq!(p["reasoning_episode_durations_ms"]["backward"], 1260000);
        assert_eq!(
            core::research_admission(&p, "backward", 38 * 60000)["admitted"],
            false
        );
        p["response_correction"] = Value::Null;
        core::start_reasoning_timing(&mut p, "backward", 2000000);
        assert_eq!(p["reasoning_timing"]["episode_started_at_ms"], 2000000);
        assert!(p.get("started_at_ms").is_none());
    }

    #[test]
    fn pass15_city_admitted_development_finishes_its_required_comparison() {
        // Exact admission/timing scalars from frozen pass15 city; payload retained
        // synthetically below to verify a refusal never removes the existing pool.
        let mut p = json!({"stage":"proposals","reasoning_durations_ms":{"explore":541525,"imagine":240950,"seed":120337},"proposal_pool":{"stage":"contrast","research_attempts":1,"development":{"status":"completed"},"development_admission":{"admitted":true,"remaining_transitions":431,"provisional_count":4},"candidates":[{"id":"preserved"}]}});
        let current = comparison_admission(&p, 417, 1_147_099);
        assert_eq!(current["admitted"], true);
        assert_eq!(current["optional_repair"], false);
        assert_eq!(
            core::optional_repair_time_admission(&p, 1_147_099)["admitted"],
            false
        );
        p["proposal_pool"]["novelty_repair"] = json!({"status":"pending"});
        let optional = comparison_admission(&p, 417, 1_147_099);
        assert_eq!(optional["admitted"], false);
        assert_eq!(optional["reason"], "time_budget");
        assert_eq!(optional["transition_admitted"], true);
        p["proposal_pool"]
            .as_object_mut()
            .unwrap()
            .remove("novelty_repair");
        assert_eq!(
            comparison_admission(&p, 1, 1_147_099)["reason"],
            "transition_budget"
        );
        assert_eq!(
            comparison_admission(&p, 417, core::MAX_MS)["reason"],
            "time_budget"
        );
        let original = p["proposal_pool"]["candidates"].clone();
        p["proposal_pool"]["stage"] = json!("enrich");
        assert!(!admit_development(&mut p, 906_149));
        assert_eq!(p["proposal_pool"]["candidates"], original);
        assert_eq!(
            p["proposal_pool"]["development_admission"]["reason"],
            "time_budget"
        );
        assert_eq!(p["endpoint_proposal_attempt"]["status"], "unresolved");
        assert!(admit_development(&mut p, 0));
        assert!(p.get("started_at_ms").is_none());
    }

    #[test]
    fn pass14_optional_repair_preserves_first_routes_and_writing_window() {
        // Recorded pass14 food first checks at20.09min, then39.29min after repair.
        let p = json!({"endpoint_proposal_contract":2,"stage":"proposals","reasoning_durations_ms":{"explore":663216,"imagine":180358,"seed":120247}});
        assert_eq!(
            core::research_admission(&p, "explore", 1_205_400)["admitted"],
            true
        );
        let admission = core::optional_repair_time_admission(&p, 1_205_400);
        assert_eq!(admission["admitted"], false);
        assert_eq!(admission["route_reserve_ms"], 1_800_000);
        assert_eq!(admission["finalization_reserve_ms"], 600_000);
        assert_eq!(
            core::optional_repair_time_admission(&p, 60_000)["admitted"],
            true
        );
        assert_eq!(
            core::optional_repair_time_admission(&p, core::MAX_MS)["admitted"],
            false
        );
        assert_eq!(
            core::research_admission(&p, "backward", 2_357_400)["predicted_generation_ms"],
            900_000
        );
        let merged = json!({"endpoint_proposal_contract":2,"scope_repair":{"status":"pending"}});
        assert!(!standalone_scope_repair(&merged));
        assert!(standalone_scope_repair(
            &json!({"scope_repair":{"status":"pending"}})
        ));
    }

    #[test]
    fn world_first_imagination_precedes_components_and_failed_routes_get_an_alternative() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario"}]});
        let mut program = json!({"world_search_contract":1,"baseline_status":"established","stage":"exploration","tasks":[],"cursor":0});
        assert_eq!(next_phase(&snapshot, &mut program, 0, 0), "imagine");
        program["endpoint_search"] = json!({"status":"imagined","endpoints":[{"id":"e","commitments":[{"id":"c"}]}],"routes":[],"amendments":[],"rounds":[]});
        assert_eq!(next_phase(&snapshot, &mut program, 0, 0), "backward");
        program["endpoint_search"]["rounds"] = json!([{"round":1}]);
        program["endpoint_search"]["routes"] =
            json!([{"id":"r","endpoint_id":"e","commitment_id":"c","status":"blocked"}]);
        program["stage"] = json!("routes");
        program["continue_exploring"] = json!(false);
        assert_eq!(next_phase(&snapshot, &mut program, 4, 1000), "backward");
        assert_eq!(program["stop_reason"], "backward_alternative_needed");
        assert!(!plan_combination_phase(&snapshot, &mut program, 4, 1000));
        program["transition_count"] = json!(core::MAX_APP_TRANSITIONS);
        assert_eq!(next_phase(&snapshot, &mut program, 4, 1000), "compose");
        assert_eq!(program["endpoint_search"]["routes"][0]["status"], "blocked");
        assert_eq!(program["stop_reason"], "backward_routes_unresolved");
    }

    #[test]
    fn hard_exploration_exit_replaces_prior_admission_without_inventing_cost() {
        let snapshot = json!({"nodes":[]});
        let mut program = json!({"stage":"exploration","transition_count":269,"stop_reason":"transition_budget","continue_exploring":true,"exploration_admission":{"admitted":true,"remaining_transitions":183,"required_transitions":96}});
        assert_eq!(next_phase(&snapshot, &mut program, 112, 1000), "compose");
        let receipt = program["exploration_admission"].clone();
        assert_eq!(receipt["admitted"], false);
        assert_eq!(receipt["reason"], "transition_budget");
        assert_eq!(receipt["remaining_transitions"], 0);
        assert!(receipt.get("required_transitions").is_none());
        assert_eq!(receipt["previous_admission"]["admitted"], true);
        program["stage"] = json!("combinations");
        program["stop_reason"] = json!("checking_combinations");
        next_phase(&snapshot, &mut program, 112, 1100);
        assert_eq!(program["exploration_admission"], receipt);
    }

    #[test]
    fn captured_food_route_tail_and_current_novelty_fit_reserved_finalization() {
        let capture: Value = serde_json::from_str(include_str!(
            "../../semantic_route_finalization_fixture.json"
        ))
        .unwrap();
        let snapshot = &capture["snapshot"];
        let mut program = capture["program"].clone();
        // Undo only the captured terminal decision, preserving actual route
        // tasks, cursor, source evidence, statements and previous check requests.
        program
            .as_object_mut()
            .unwrap()
            .remove("deferred_novelty_recheck");
        for receipt in program["endpoint_novelty"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            receipt["status"] = json!("passed");
            receipt.as_object_mut().unwrap().remove("reason");
        }
        let before = program.clone();
        assert_eq!(program["cursor"], 104);
        assert_eq!(program["tasks"].as_array().unwrap().len(), 116);
        assert!(admit_route_finalization(snapshot, &mut program, 357, 2_500_483).unwrap());
        let receipt = &program["route_finalization"];
        assert_eq!(receipt["estimated_batches"], 9);
        assert_eq!(receipt["novelty_transitions"], 4);
        assert_eq!(receipt["required_transitions"], 26);
        assert_eq!(receipt["remaining_transitions"], 32);
        assert_eq!(core::transition_limit(&program), 360);
        assert_eq!(
            core::research_admission(&program, "backward", 1000)["admitted"],
            false
        );
        assert_eq!(
            core::research_admission(&program, "explore", 1000)["admitted"],
            false
        );
        assert!(pending_assessment(
            &program, 104, 116, 357, 2_500_483, false
        ));
        assert_eq!(program["results"], before["results"]);
        assert_eq!(program["endpoint_novelty"], before["endpoint_novelty"]);
        assert_eq!(program["tasks"], before["tasks"]);
        assert_eq!(program["cursor"], before["cursor"]);
        let mut insufficient = before.clone();
        insufficient["transition_count"] = json!(335);
        assert!(!admit_route_finalization(snapshot, &mut insufficient, 357, 2_500_483).unwrap());
        assert_eq!(
            insufficient["route_finalization"]["remaining_transitions"],
            25
        );
        let mut expired = before.clone();
        assert!(!admit_route_finalization(snapshot, &mut expired, 357, core::MAX_MS).unwrap());
        let mut denied = before;
        denied["stop_reason"] = json!("provider_error");
        assert!(!admit_route_finalization(snapshot, &mut denied, 357, 2_500_483).unwrap());
    }

    #[test]
    fn observed_generation_duration_prevents_late_research_admission() {
        let mut p = json!({"world_search_contract":1,"stage":"exploration","started_at_ms":"1"});
        core::start_reasoning_timing(&mut p, "backward", 1_000_000);
        core::finish_reasoning_timing(&mut p, "backward", 1_900_000);
        assert_eq!(p["reasoning_durations_ms"]["backward"], 900_000);
        core::finish_reasoning_timing(&mut p, "backward", 2_900_000);
        assert_eq!(p["reasoning_durations_ms"]["backward"], 900_000, "Completed callbacks cannot inflate timing twice");
        assert_eq!(p["started_at_ms"], "1");
        let late = core::research_admission(&p, "backward", 47 * 60_000 + 59_000);
        assert_eq!(late["admitted"], false);
        assert_eq!(late["predicted_generation_ms"], 900_000);
        assert_eq!(late["basis"], "maximum_observed_generation_episode");
        assert_eq!(core::research_admission(&p, "backward", 30 * 60_000)["admitted"], true);
        assert_eq!(core::research_admission(&p, "explore", 48 * 60_000)["admitted"], false);
        core::start_reasoning_timing(&mut p, "backward", 3_000_000);
        core::finish_reasoning_timing(&mut p, "backward", 3_100_000);
        assert_eq!(p["reasoning_durations_ms"]["backward"], 900_000, "A faster follow-up cannot erase observed slow work");
    }

    #[test]
    fn admitted_endpoint_checks_drain_after_research_closes_without_new_research() {
        let program = json!({"world_search_contract":1,"stage":"exploration"});
        let minute = 60_000;
        assert_eq!(core::research_time_limit(&program), 50 * minute);
        // A newly accepted graph with cleared stale scores still has102 checks.
        // Exercise the actual dispatch predicate, not just a deadline constant.
        assert!(pending_assessment(&program, 0, 102, 168, 51 * minute, false));
        assert!(!pending_assessment(&program, 0, 102, 168, 52 * minute, false));
        assert!(!pending_assessment(&program, 0, 102, 168, 60 * minute, false));
        assert!(!pending_assessment(&program, 0, 102, 168, 51 * minute, true));
        assert!(!pending_assessment(&program, 102, 102, 168, 51 * minute, false));
        assert!(51 * minute >= core::research_time_limit(&program).saturating_sub(120_000));
        assert_eq!(core::time_limit(&program), 52 * minute);
        assert!(52 * minute >= core::time_limit(&program));
        let mut routes = program.clone();
        routes["stage"] = json!("routes");
        assert_eq!(core::time_limit(&routes), 52 * minute);
        routes["stage"] = json!("proposals");
        routes["endpoint_proposal_attempt"] = json!({"pool_stage":"deferred"});
        assert_eq!(core::time_limit(&routes), 52 * minute);
        routes["endpoint_proposal_attempt"]["pool_stage"] = json!("individual");
        assert_eq!(core::time_limit(&routes), 50 * minute);
        assert_eq!(core::time_limit(&json!({"stage":"exploration"})), 50 * minute);
        assert_eq!(core::time_limit(&json!({"stage":"worlds"})), 57 * minute);
        assert_eq!(core::MAX_MS, 60 * minute);
        let snapshot = json!({"nodes":[]});
        let mut no_routes = json!({"world_search_contract":1,"stage":"exploration","endpoint_search":{"endpoints":[],"routes":[],"rounds":[]}});
        assert_eq!(next_phase(&snapshot, &mut no_routes, 0, 34 * minute), "backward");
        assert_eq!(next_phase(&snapshot, &mut no_routes, 0, 48 * minute), "compose");
        assert_eq!(no_routes["stop_reason"], "time_budget");
    }

    #[test]
    fn endpoint_pending_evaluations_are_not_interrupted_by_legacy_challenge() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario","statement":"A future event","edges":"[]"}]});
        let program = json!({"world_search_contract":1,"stage":"exploration","baseline_status":"established","transition_count":252,"cursor":0,"tasks":[{"nodeId":"h","function":"classify_claim_role","depth":0}],"results":{},"evaluations":{}});
        assert!(program["cursor"].as_u64().unwrap() < program["tasks"].as_array().unwrap().len() as u64);
        assert_eq!(core::transition_limit(&program), 328);
        assert!(!challenge_due(&snapshot, &program, 0), "The legacy transition trigger must not preempt pending endpoint evaluations");
        assert!(!challenge_due(&snapshot, &program, core::REASONING_ADMISSION_RESERVE));
        assert!(core::request(&snapshot, &program).is_ok(), "The queued assessment remains executable");
        let mut legacy = program.clone();
        legacy.as_object_mut().unwrap().remove("world_search_contract");
        legacy["transition_count"] = json!(core::transition_limit(&legacy) - core::REASONING_ADMISSION_RESERVE - 32);
        assert!(challenge_due(&snapshot, &legacy, 0), "Legacy exploration retains its independent challenge");
    }

    #[test]
    fn independent_challenge_has_a_reserved_window_without_repeating_or_overrunning() {
        let limit = core::transition_limit(&json!({"stage":"exploration"}));
        let trigger = limit - core::REASONING_ADMISSION_RESERVE - 32;
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario"}]});
        let mut p = json!({"stage":"exploration","baseline_status":"established","transition_count":trigger-1});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["transition_count"] = json!(trigger - 6);
        assert!(challenge_due(
            &snapshot,
            &p,
            core::REASONING_ADMISSION_RESERVE
        ));
        p["transition_count"] = json!(trigger);
        assert!(challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = json!({"status":"pending"});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = json!({"status":"completed"});
        assert!(!challenge_due(&snapshot, &p, 0));
        p["independent_challenge"] = Value::Null;
        p["transition_count"] = json!(limit - core::REASONING_ADMISSION_RESERVE);
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
        assert_eq!(cost["current_graph_tasks"], 96);
        assert_eq!(cost["estimated_batches"], 6);
        assert_eq!(cost["current_graph_evaluation_transitions"], 12);
        assert_eq!(cost["required_transitions"], 88);
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
    #[ignore = "Private captured food snapshot/program supplied explicitly"]
    fn captured_food_complete_audits_keep_uncertainty_and_estimates() {
        let dir = std::env::var("FOOD_AUDIT_FIXTURE_DIR").unwrap();
        let snapshot: Value = serde_json::from_slice(
            &std::fs::read(format!("{dir}/food-a03-resumed-final-snapshot.json")).unwrap(),
        )
        .unwrap();
        let mut program: Value = serde_json::from_slice(
            &std::fs::read(format!("{dir}/food-a03-resumed-final-checkpoint.json")).unwrap(),
        )
        .unwrap();
        let before = program.clone();
        // Replay scheduling only, with captured judgments; no clock or native run is changed.
        assert_eq!(next_phase(&snapshot, &mut program, 0, 0), "synthesize");
        assert_eq!(program["stop_reason"], "world_audits_unresolved");
        assert_eq!(program["results"], before["results"]);
        assert_eq!(program["evaluations"], before["evaluations"]);
        assert_eq!(program["world_refinement"], before["world_refinement"]);
        assert_eq!(program["world_audits"], before["world_audits"]);
    }

    #[test]
    fn completed_audit_uncertainty_is_distinct_from_missing_checks() {
        for (label, expected) in [
            ("uncertain", "world_audits_unresolved"),
            ("conflict", "world_audits_unresolved"),
            ("compatible", "worlds_evaluated"),
        ] {
            let (snapshot, mut program) = evaluated_world(label);
            program["world_pass"] = json!(3);
            program["world_revision"] = json!(3);
            let results = program["results"].clone();
            assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "synthesize");
            assert_eq!(program["stop_reason"], expected);
            assert_eq!(program["results"], results);
            assert_eq!(
                program["world_audits"]["w"]["planned_checks"],
                program["world_audits"]["w"]["completed_checks"]
            );
        }
        let (snapshot, mut program) = evaluated_world("uncertain");
        program["world_pass"] = json!(3);
        program["results"]["w"]
            .as_object_mut()
            .unwrap()
            .remove("check_world_consistency");
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "synthesize");
        assert_eq!(program["stop_reason"], "world_audits_incomplete");
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
        for task in program["tasks"].as_array().unwrap().clone() {
            let id = core::field(&task, "nodeId");
            let function = core::field(&task, "function");
            let mut context = json!({"world_pass":1});
            if function != "estimate_likelihood" {
                let request = core::search::request(&snapshot, &program, &task).unwrap();
                context["audit_input_fingerprint"] = json!(core::search::audit_input_fingerprint(
                    &snapshot, &task, &request
                ));
            }
            program["evaluations"][id][function] = json!({"context":context});
        }
        let results = program["results"].clone();
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "refine");
        let first = program["world_refinement"]["w"]["rounds"][0].clone();
        program["results"] = results.clone();
        program["evaluations"]["w"]["estimate_likelihood"] =
            json!({"context":{"world_pass":2},"probability":0.42});
        assert_eq!(next_phase(&snapshot, &mut program, 40, 2000), "synthesize");
        assert_eq!(program["world_audits"]["w"]["status"], "uncertain");
        assert_eq!(program["stop_reason"], "world_audits_unresolved");
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

    // A valid set with complete, reusable audit provenance for each world.
    fn conflicting_world_set() -> (Value, Value) {
        let (mut snapshot, mut program) = evaluated_world("conflict");
        let mut second = snapshot["nodes"][3].clone();
        second["Id"] = json!("w2");
        second["statement"] = json!("A second joint future with the same tested components");
        snapshot["nodes"].as_array_mut().unwrap().push(second);
        program["active_world_ids"] = json!(["w", "w2"]);
        let tasks: Vec<_> = snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["kind"] == "world")
            .flat_map(core::search::world_tasks)
            .collect();
        program["tasks"] = json!(tasks);
        for task in tasks {
            let id = core::field(&task, "nodeId");
            let function = core::field(&task, "function");
            program["results"][id][function] = json!(match function {
                "estimate_likelihood" => "0.42",
                "check_world_consistency" => "conflict",
                _ => "compatible",
            });
            let mut context = json!({"world_pass":1});
            if function != "estimate_likelihood" {
                let request = core::search::request(&snapshot, &program, &task).unwrap();
                context["audit_input_fingerprint"] = json!(core::search::audit_input_fingerprint(
                    &snapshot, &task, &request
                ));
            }
            program["evaluations"][id][function] = json!({"context":context});
        }
        (snapshot, program)
    }

    fn finish_second_world_estimates(program: &mut Value) {
        assert_eq!(program["world_pass"], 2);
        assert_eq!(program["tasks"].as_array().unwrap().len(), 2);
        for id in ["w", "w2"] {
            program["results"][id]["estimate_likelihood"] = json!("0.42");
            program["evaluations"][id]["estimate_likelihood"] =
                json!({"context":{"world_pass":2},"probability":0.42});
        }
    }

    #[test]
    fn explicit_conflict_still_recomposes_after_completed_refinement() {
        let (snapshot, mut program) = conflicting_world_set();
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "refine");
        finish_second_world_estimates(&mut program);
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
        let (snapshot, mut program) = conflicting_world_set();
        assert_eq!(next_phase(&snapshot, &mut program, 20, 1000), "refine");
        finish_second_world_estimates(&mut program);
        assert_eq!(next_phase(&snapshot, &mut program, 40, 2000), "compose");
        assert_eq!(program["recomposition_admission"]["admitted"], true);
        assert_eq!(program["world_audits"]["w"]["status"], "conflicts_found");
        assert_eq!(program["world_audits"]["w2"]["status"], "conflicts_found");
        // Even a complete contradiction with budget remaining cannot rewrite forever.
        program["world_revision"] = json!(3);
        assert_eq!(next_phase(&snapshot, &mut program, 40, 2000), "synthesize");
        assert_eq!(program["stop_reason"], "world_audits_unresolved");
    }
}

#[cfg(test)]
mod proposal_stop_tests {
    use super::*;
    #[test]
    fn unevaluated_city_cutoff_reports_saved_work_and_actual_limit() {
        let mut p = json!({"stage":"exploration","cursor":0,"tasks":vec![json!({});93],"endpoint_search":{"routes":vec![json!({"status":"pending"});6]}});
        let message = composition_unavailable_message(&p, 0, 95, 3_000_000);
        assert!(message.contains("time_budget"));
        assert!(message.contains("93 candidate checks and 6 pending routes"));
        assert!(message.contains("Unevaluated work is not a rejected future"));
        assert!(!message.contains("at least three are required"));
        for reason in ["transition_budget", "call_budget", "trace_budget", "provider_error"] {
            p["stop_reason"] = json!(reason);
            assert!(composition_unavailable_message(&p, 0, 95, 1).contains(reason));
        }
        p["stop_reason"] = Value::Null;
        assert!(composition_unavailable_message(&p, 0, 95, 1).contains("at least three are required"));
    }

    #[test]
    fn reserved_deadline_is_not_a_novelty_judgment() {
        let mut p = json!({"stage":"proposals","endpoint_proposal_attempt":{"tasks":vec![json!({});33],"checks":vec![json!({"evaluation":null});33]}});
        assert_eq!(proposal_resource_reason(&p, 0, 2_999_999), None);
        assert_eq!(
            proposal_resource_reason(&p, 0, 3_000_000),
            Some("time_budget")
        );
        p["stop_reason"] = json!("time_budget");
        let message = proposal_terminal_message(&p);
        assert!(message.contains("0 of 33"));
        assert!(message.contains("time_budget"));
        assert!(!message.contains("insufficiently distinct"));
        assert!(!message.contains("did not produce sufficiently distinct"));
        for reason in [
            "call_budget",
            "transition_budget",
            "trace_budget",
            "provider_error",
        ] {
            p["stop_reason"] = json!(reason);
            assert!(proposal_terminal_message(&p).contains(reason));
        }
        p["stop_reason"] = json!("endpoint_proposal_quality");
        assert!(proposal_terminal_message(&p).contains("Endpoint proposal quality unresolved"));
    }
}
