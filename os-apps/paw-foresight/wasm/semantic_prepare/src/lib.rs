use temper_wasm_sdk::prelude::*;
// Preparation uses only the snapshot planner from the shared evaluator contract.
#[allow(dead_code, unused_imports)]
mod core {
    include!("../../semantic_core.rs");
}
fn read(ctx: &Context, path: &str) -> Result<Value, String> {
    read_bounded(ctx, path, 2_000_000)
}
fn read_bounded(ctx: &Context, path: &str, max_bytes: usize) -> Result<Value, String> {
    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("Missing Temper URL")?;
    let url = format!("{api}/tdata/{path}");
    let headers = [
        ("x-tenant-id".into(), ctx.tenant.clone()),
        ("x-temper-principal-kind".into(), "agent".into()),
        ("x-temper-principal-id".into(), ctx.entity_id.clone()),
        ("x-temper-agent-type".into(), "system".into()),
    ];
    let response = if max_bytes < temper_wasm_sdk::host::HTTP_BUF_LEN {
        ctx.http_call("GET", &url, &headers, "")?
    } else {
        read_checkpoint_response(&url, &headers, max_bytes)?
    };
    if response.status != 200 {
        return Err(format!("Snapshot read HTTP {}", response.status));
    }
    if response.body.len() > max_bytes {
        return Err(format!(
            "Snapshot read exceeds {max_bytes} bytes; refusing truncation"
        ));
    }
    core::parse(&response.body)
}

/// The persisted trace can exceed the SDK's general-purpose 4 MiB buffer.
/// Use the same governed HTTP host operation with an explicitly bounded buffer;
/// never omit or truncate prior evaluations to make a checkpoint fit.
fn read_checkpoint_response(
    url: &str,
    headers: &[(String, String)],
    max_bytes: usize,
) -> Result<HttpResponse, String> {
    let headers = serde_json::to_string(headers).map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; max_bytes + 4]; // three-digit status plus newline
    // SAFETY: all input slices and the writable output allocation live through
    // the synchronous host call. The returned length is checked before slicing.
    let len = unsafe {
        temper_wasm_sdk::host::host_http_call(
            b"GET".as_ptr() as i32,
            3,
            url.as_ptr() as i32,
            url.len() as i32,
            headers.as_ptr() as i32,
            headers.len() as i32,
            b"".as_ptr() as i32,
            0,
            buffer.as_mut_ptr() as i32,
            buffer.len() as i32,
        )
    };
    if len == -2 {
        return Err(format!(
            "Checkpoint exceeds {max_bytes} bytes; refusing truncation"
        ));
    }
    if len < 0 || len as usize > buffer.len() {
        return Err(format!("Checkpoint HTTP read failed with code {len}"));
    }
    buffer.truncate(len as usize);
    let mut response = String::from_utf8(buffer).map_err(|_| "Invalid checkpoint encoding")?;
    let split = response
        .find('\n')
        .ok_or("Missing checkpoint HTTP status")?;
    let status = response[..split]
        .parse()
        .map_err(|_| "Invalid checkpoint HTTP status")?;
    response.drain(..=split);
    Ok(HttpResponse {
        status,
        body: response,
    })
}
fn snapshot_node(n: &Value) -> Result<Value, String> {
    let mut safe = json!({});
    for k in [
        "Id",
        "statement",
        "edges",
        "source_refs",
        "resolve_by",
        "provenance",
        "probability",
        "resolution",
        "Status",
    ] {
        safe[k] = json!(core::field(n, k));
    }
    // EventNode's empty default means it declares no dependency edges.
    if core::field(&safe, "edges").trim().is_empty() {
        safe["edges"] = json!("[]");
    }
    safe["evidence_metadata"] = core::evidence::parse(core::field(n, "evidence_json"))?;
    if safe["evidence_metadata"]["kind"] != "legacy_unverified" {
        core::evidence::single_source(&safe["source_refs"])?;
    }
    safe["kind"] = json!(if core::field(n, "provenance") == "hypothesis" {
        "scenario"
    } else {
        "evidence"
    });
    Ok(safe)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn exploration_interrupted(snapshot: &Value, program: &Value, trace: &Value) -> bool {
    let world_ids: std::collections::BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["kind"] == "world")
        .filter_map(|n| n["Id"].as_str())
        .collect();
    program["stop_reason"] == "provider_error"
        && (program["stage"] == "exploration"
            || program["exploration_stop_reason"] == "provider_error")
        && !trace.as_array().into_iter().flatten().any(|item| {
            item["task"]["world_id"].as_str().is_some()
                || (item["function"] == "estimate_likelihood"
                    && item["nodeId"]
                        .as_str()
                        .is_some_and(|id| world_ids.contains(id)))
        })
}

fn restore_exploration(snapshot: &Value, program: &Value) -> Result<(Value, Value), String> {
    let nodes: Vec<_> = snapshot["nodes"]
        .as_array()
        .ok_or("Missing resume nodes")?
        .iter()
        .filter(|n| n["kind"] != "world")
        .cloned()
        .collect();
    let mut planned = core::plan(&nodes)?;
    let mut restored = program.clone();
    restored["claim_role_contract"] = planned["claim_role_contract"].clone();
    restored["event_dependencies"] = planned["event_dependencies"].clone();
    restored["stage"] = json!("exploration");
    core::clear_ineligible_forecasts(&mut restored);
    planned["tasks"].as_array_mut().unwrap().retain(|task| {
        restored["results"][core::field(task, "nodeId")][core::field(task, "function")].is_null()
    });
    restored["tasks"] = planned["tasks"].clone();
    core::defer_recorded_rankings(
        &mut restored,
        &json!({"historical_search_guidance":program["historical_search_guidance"]}),
    );
    restored["cursor"] = json!(0);
    restored["stage"] = json!("exploration");
    restored["continue_exploring"] = json!(true);
    restored["stop_reason"] = json!("resumed_after_provider_error");
    restored["resume_mode"] = json!("unfinished_exploration");
    restored["active_world_ids"] = json!([]);
    let mut snapshot = snapshot.clone();
    for node in snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|n| n["kind"] == "world")
    {
        node["archived"] = json!(true);
    }
    Ok((snapshot, restored))
}

/// Resume only trusted persisted state; no caller-provided graph or evaluations.
fn resume_checkpoint(record: &Value, world_id: &str, now_ms: u64) -> Result<Value, String> {
    if core::field(record, "world_id") != world_id {
        return Err("Resume world mismatch".into());
    }
    let snapshot_raw = core::field(record, "snapshot_json");
    let program_raw = core::field(record, "program_json");
    let trace_raw = core::field(record, "trace_json");
    let snapshot = core::parse(snapshot_raw)?;
    core::branches::validate(&snapshot)?;
    for node in snapshot["nodes"].as_array().into_iter().flatten() {
        if !core::field(node, "branch_id").is_empty()
            && node["branch_state"]
                != core::branches::state(
                    &snapshot,
                    core::field(node, "branch_id"),
                    Some(core::field(node, "Id")),
                )?
        {
            return Err("Checkpoint branch history differs from canonical conditions".into());
        }
    }
    let mut program = core::parse(program_raw)?;
    let trace = core::parse(trace_raw)?;
    if core::field(&snapshot["world"], "Id") != world_id {
        return Err("Resume snapshot world mismatch".into());
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing resume nodes")?;
    core::plan(nodes)?;
    if program["schema"] != "foresight-open-semantic-v2" {
        return Err("Unsupported resume program".into());
    }
    let tasks = program["tasks"].as_array().ok_or("Missing resume tasks")?;
    let cursor = program["cursor"]
        .as_u64()
        .filter(|n| *n <= tasks.len() as u64)
        .ok_or("Invalid resume cursor")?;
    let interrupted_exploration = exploration_interrupted(&snapshot, &program, &trace);
    let partial_answer = (core::field(record, "Status") == "Completed"
        && interrupted_exploration
        && program["exploration_stop_reason"] == "provider_error")
        || (core::field(record, "Status") == "Completed"
            && program["stage"] == "worlds"
            && program["stop_reason"] == "provider_error"
            && cursor < tasks.len() as u64
            && program["world_refinement"]
                .as_object()
                .is_some_and(|worlds| {
                    worlds.values().any(|world| {
                        world["rounds"].as_array().is_some_and(|rounds| {
                            rounds.iter().any(|round| round["complete"] == false)
                        })
                    })
                }));
    if !matches!(core::field(record, "Status"), "Failed" | "Cancelled") && !partial_answer {
        return Err(
            "Only stopped exploration or a provider-interrupted partial answer can be resumed"
                .into(),
        );
    }
    if !program["results"].is_object()
        || !program["evaluations"].is_object()
        || !program["rounds"].is_array()
    {
        return Err("Invalid resume evaluations or history".into());
    }
    let ids: std::collections::BTreeSet<_> = nodes.iter().map(|n| core::field(n, "Id")).collect();
    let mut validated = std::collections::BTreeSet::new();
    let mut validate_structural = |task: &Value| -> Result<(), String> {
        let identity = task.to_string();
        if !validated.contains(&identity) {
            core::search::validate_task(&snapshot, task)?;
            validated.insert(identity);
        }
        Ok(())
    };
    for task in tasks {
        if core::search::is_structural(task) {
            validate_structural(task)?;
        } else if !ids.contains(core::field(task, "nodeId")) {
            return Err("Resume task references missing node".into());
        }
    }
    let traces = trace.as_array().ok_or("Invalid resume trace")?;
    if traces.len() > core::MAX_CALLS || trace_raw.len() > core::MAX_TRACE_BYTES {
        return Err("Resume trace exceeds budget".into());
    }
    for (index, item) in traces.iter().enumerate() {
        let structural = core::search::is_structural(&item["task"]);
        let valid_subject = if structural {
            item["task"]["nodeId"] == item["nodeId"]
                && item["task"]["function"] == item["function"]
                && validate_structural(&item["task"]).is_ok()
        } else {
            ids.contains(core::field(item, "nodeId"))
        };
        if item["index"].as_u64() != Some(index as u64)
            || !valid_subject
            || !((item["response"].is_object() && item["request"].is_object())
                || (item["requestFormat"] == "failed-attempt-hash-only"
                    && item["error"].as_str().is_some_and(|s| !s.is_empty())
                    && item["requestHash"].as_str().is_some_and(|s| {
                        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
                    })))
        {
            return Err("Corrupt resume trace".into());
        }
    }
    let started_raw = core::field(record, "started_at_ms");
    let started = started_raw
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= now_ms)
        .ok_or("Invalid resume start time")?;
    let phase = core::field(record, "phase");
    if !matches!(
        phase,
        "seed" | "explore" | "challenge" | "compose" | "synthesize"
    ) {
        return Err("Invalid resume phase".into());
    }
    let has_worlds = nodes.iter().any(|n| core::field(n, "kind") == "world");
    let has_hypotheses = nodes
        .iter()
        .filter(|n| matches!(core::field(n, "kind"), "scenario" | "revision"))
        .count()
        >= 2;
    let exhausted = now_ms - started >= core::MAX_MS
        || traces.len() >= core::MAX_CALLS
        || nodes.len() >= core::MAX_NODES
        || program["round"].as_u64().unwrap_or(0) >= core::MAX_ROUNDS;
    let pending_world_checks = !exhausted
        && has_worlds
        && cursor < tasks.len() as u64
        && program["stop_reason"] == "provider_error";
    let phase = if pending_world_checks || (phase == "synthesize" && !has_worlds && has_hypotheses)
    {
        "compose"
    } else if exhausted {
        if has_worlds {
            "synthesize"
        } else if has_hypotheses {
            "compose"
        } else {
            return Err(
                "Exhausted checkpoint has insufficient hypotheses to compose worlds".into(),
            );
        }
    } else {
        phase
    };
    if phase == "synthesize" && !has_worlds && !has_hypotheses {
        return Err("Resume synthesis needs persisted hypotheses".into());
    }
    let agent_id = core::field(record, "agent_id");
    let model = core::field(record, "model");
    let provider = core::field(record, "provider");
    if !valid_id(agent_id) || model.trim().is_empty() || provider.trim().is_empty() {
        return Err("Missing resume agent or provider metadata".into());
    }
    if !exhausted && interrupted_exploration {
        let (snapshot, program) = restore_exploration(&snapshot, &program)?;
        return Ok(
            json!({"world_id":world_id,"agent_id":agent_id,"model":model,"provider":provider,
            "snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":trace_raw,
            "started_at_ms":started_raw,"phase":"explore","reasoning_phase_polls":0}),
        );
    }
    // Keep the program exactly unless a spent budget needs its honest stop reason.
    let pending_tasks = cursor < tasks.len() as u64;
    let program_raw = if exhausted {
        program["stop_reason"] = json!(if now_ms - started >= core::MAX_MS {
            "time_budget"
        } else if traces.len() >= core::MAX_CALLS {
            "call_budget"
        } else if nodes.len() >= core::MAX_NODES {
            "node_budget"
        } else {
            "round_budget"
        });
        program.to_string()
    } else if pending_tasks && program["stop_reason"] == "provider_error" {
        // The failed attempt remains in trace and consumes budget; only its
        // transient stop marker clears so the exact unfinished task can retry.
        program["stop_reason"] = json!("resumed_after_provider_error");
        program.to_string()
    } else {
        program_raw.to_owned()
    };
    Ok(
        json!({"world_id":world_id,"agent_id":agent_id,"model":model,"provider":provider,
        "snapshot_json":snapshot_raw,"program_json":program_raw,"trace_json":trace_raw,
        "started_at_ms":started_raw,"phase":phase,"reasoning_phase_polls":0}),
    )
}

fn resume_transition(prepared: &Value) -> Result<&'static str, String> {
    let program = core::parse(core::field(prepared, "program_json"))?;
    let cursor = program["cursor"].as_u64().ok_or("Missing resume cursor")?;
    let count = program["tasks"]
        .as_array()
        .ok_or("Missing resume tasks")?
        .len() as u64;
    Ok(
        if core::field(prepared, "phase") != "synthesize"
            && (core::field(prepared, "phase") != "compose" || program["stage"] == "worlds")
            && (cursor < count
                || program["resume_mode"] == "unfinished_exploration"
                || (core::field(prepared, "phase") == "compose" && program["stage"] == "worlds"))
        {
            "ResumePrepared"
        } else {
            "Prepared"
        },
    )
}

fn run_inner(ctx: &Context) -> Result<(), String> {
    let direct = core::field(&ctx.entity_state, "world_id");
    let id = if direct.is_empty() {
        core::field(&ctx.entity_state, "parent_id")
    } else {
        direct
    };
    if !valid_id(id) {
        return Err("Invalid world identity".into());
    }
    let resume_id = core::field(&ctx.entity_state, "resume_run_id");
    if !resume_id.is_empty() {
        if !valid_id(resume_id) || resume_id == ctx.entity_id {
            return Err("Invalid resume identity".into());
        }
        // Read only the trusted checkpoint and launch metadata. JSON escaping
        // of the bounded 24 MiB trace plus graph/program needs a larger envelope.
        let path = format!(
            "SemanticRuns('{resume_id}')?$select=Id,Status,world_id,agent_id,model,provider,snapshot_json,program_json,trace_json,started_at_ms,phase"
        );
        let record = read_bounded(ctx, &path, 64 * 1024 * 1024)?;
        let prepared = resume_checkpoint(&record, id, Context::get_time_millis() as u64)?;
        set_success_result(resume_transition(&prepared)?, &prepared);
        return Ok(());
    }
    let world = read(ctx, &format!("Worlds('{id}')"))?;
    if core::field(&world, "Status") != "Active" {
        return Err("Semantic exploration needs an active world".into());
    }
    let rows = read(
        ctx,
        &format!("EventNodes?$filter=world_id%20eq%20'{id}'&$top=513"),
    )?;
    if rows["value"].as_array().ok_or("Missing event list")?.len() > 512 {
        return Err(
            "World exceeds the 512-event snapshot limit; refusing silent truncation".into(),
        );
    }
    let mut nodes = vec![];
    for n in rows["value"].as_array().ok_or("Missing event list")? {
        if core::field(n, "world_id") != id {
            return Err("Cross-world snapshot rejected".into());
        }
        let safe = snapshot_node(n)?;
        nodes.push(safe);
    }
    let program = core::plan(&nodes)?;
    let session_id = core::field(&world, "research_session_id");
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("World has no verified research session".into());
    }
    let session = read(ctx, &format!("Sessions('{session_id}')"))?;
    let agent_id = core::field(&session, "agent_id");
    if agent_id.is_empty() {
        return Err("Research agent is unavailable".into());
    }
    let mut safe_world = json!({"evidence_contract":"v1"});
    for k in [
        "Id",
        "name",
        "domain",
        "description",
        "last_ingest_date",
        "target_date",
        "learning_mode",
        "hindcast_mode",
    ] {
        safe_world[k] = json!(core::field(&world, k));
    }
    let snapshot = json!({"world":safe_world,"nodes":nodes});
    if snapshot.to_string().len() > 2 * 1024 * 1024 {
        return Err("World snapshot exceeds 2 MB".into());
    }
    set_success_result(
        "Prepared",
        &json!({"world_id":id,"agent_id":agent_id,"model":core::field(&world,"agent_model"),"provider":core::field(&world,"agent_provider"),"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","started_at_ms":Context::get_time_millis().to_string(),"phase":"seed","reasoning_phase_polls":0}),
    );
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host().and_then(|ctx| run_inner(&ctx)) {
        Ok(()) => (),
        Err(e) => set_success_result("Fail", &json!({"error_message":e})),
    };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint() -> Value {
        let snapshot = json!({"world":{"Id":"w"},"nodes":[{"Id":"h","kind":"scenario","statement":"Future","edges":"[]"}]});
        let program = json!({"schema":"foresight-open-semantic-v2","tasks":[],"cursor":0,"results":{"h":{"estimate_likelihood":"0.37"}},"evaluations":{},"rounds":[],"round":2});
        json!({"Status":"Failed","world_id":"w","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","started_at_ms":"1000","phase":"explore","agent_id":"agent-a","model":"model-a","provider":"provider-a"})
    }
    #[test]
    fn legacy_resume_requeues_odds_removed_by_new_admission() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario","edges":"[]"}]});
        let old = json!({"baseline_status":"established","results":{"h":{"classify_temporal":"uncertain","estimate_likelihood":"0.61","estimate_conditional":"0.73"}},"evaluations":{"h":{"estimate_likelihood":{"probability":0.61}}}});
        let (_, mut restored) = restore_exploration(&snapshot, &old).unwrap();
        assert_eq!(restored["claim_role_contract"], 1);
        assert!(restored["results"]["h"]["estimate_likelihood"].is_null());
        assert!(restored["results"]["h"]["estimate_conditional"].is_null());
        for function in ["classify_claim_role", "estimate_likelihood"] {
            assert!(
                restored["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["function"] == function)
            );
        }
        restored["results"]["h"]["classify_claim_role"] = json!("event");
        assert!(core::forecast_allows(&restored, "h"));
        assert_eq!(old["results"]["h"]["estimate_likelihood"], "0.61");
    }

    #[test]
    fn resumed_exploration_does_not_reschedule_historical_rankings() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario","edges":"[]"}]});
        let old = json!({"results":{},"evaluations":{},"historical_search_guidance":{"h":{"evaluate_novelty":{"result":"2","current":false},"decision_value":{"result":"3","current":false}}}});
        let (_, p) = restore_exploration(&snapshot, &old).unwrap();
        assert_eq!(
            p["historical_search_guidance"],
            old["historical_search_guidance"]
        );
        assert!(p["tasks"].as_array().unwrap().iter().all(|t| !matches!(
            core::field(t, "function"),
            "evaluate_novelty" | "decision_value"
        )));
        assert_eq!(p["tasks"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn completed_world_composition_checkpoint_returns_to_native_decision() {
        let program = json!({"stage":"worlds","cursor":0,"tasks":[]});
        assert_eq!(
            resume_transition(&json!({"phase":"compose","program_json":program.to_string()}))
                .unwrap(),
            "ResumePrepared"
        );
        assert_eq!(
            resume_transition(&json!({"phase":"synthesize","program_json":program.to_string()}))
                .unwrap(),
            "Prepared"
        );
        let exploration = json!({"stage":"exploration","cursor":0,"tasks":[]});
        assert_eq!(
            resume_transition(&json!({"phase":"compose","program_json":exploration.to_string()}))
                .unwrap(),
            "Prepared"
        );
    }

    #[test]
    #[ignore = "Requires explicit local failed composed-world checkpoint"]
    fn saved_completed_world_passes_resume_without_changing_history() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_WORLD_COMPOSE_FIXTURE").unwrap())
                .unwrap(),
        )
        .unwrap();
        let record = &raw["fields"];
        let started = core::field(record, "started_at_ms").parse::<u64>().unwrap();
        let prepared =
            resume_checkpoint(record, core::field(record, "world_id"), started + 1000).unwrap();
        for key in [
            "snapshot_json",
            "program_json",
            "trace_json",
            "started_at_ms",
        ] {
            assert_eq!(prepared[key], record[key]);
        }
        assert_eq!(prepared["phase"], "compose");
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        let exhausted = resume_checkpoint(
            record,
            core::field(record, "world_id"),
            started + core::MAX_MS,
        )
        .unwrap();
        assert_eq!(exhausted["phase"], "synthesize");
        assert_eq!(resume_transition(&exhausted).unwrap(), "Prepared");
    }

    #[test]
    fn archived_world_classification_failure_resumes_even_when_real_tasks_are_complete() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario","edges":"[]"},{"Id":"old-world","kind":"world","archived":true,"edges":"[]"}]});
        let mut old = json!({"stage":"exploration","stop_reason":"provider_error","results":{}});
        let plan = core::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        for task in plan["tasks"].as_array().unwrap() {
            old["results"][core::field(task, "nodeId")][core::field(task, "function")] =
                json!(if task["function"] == "classify_claim_role" {
                    "event"
                } else {
                    "recorded"
                });
        }
        let trace =
            json!([{"nodeId":"old-world","function":"classify_gap","error":"max_tokens_exceeded"}]);
        assert!(exploration_interrupted(&snapshot, &old, &trace));
        let (_, restored) = restore_exploration(&snapshot, &old).unwrap();
        assert!(restored["tasks"].as_array().unwrap().is_empty());
        assert_eq!(restored["continue_exploring"], true);
        assert_eq!(
            resume_transition(&json!({"phase":"explore","program_json":restored.to_string()}))
                .unwrap(),
            "ResumePrepared"
        );
        assert!(!exploration_interrupted(
            &snapshot,
            &old,
            &json!([{"nodeId":"old-world","function":"estimate_likelihood"}])
        ));
    }

    #[test]
    fn qualitative_answer_can_resume_unfinished_exploration_without_rewinding_world_checks() {
        let mut record = checkpoint();
        let mut snapshot = core::parse(core::field(&record, "snapshot_json")).unwrap();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"qualitative","kind":"world","edges":"[]"}));
        let mut program = core::parse(core::field(&record, "program_json")).unwrap();
        program["stage"] = json!("worlds");
        program["stop_reason"] = json!("provider_error");
        program["exploration_stop_reason"] = json!("provider_error");
        program["world_revision"] = json!(1);
        program["tasks"] = json!([]);
        program["cursor"] = json!(0);
        program["http_calls"] = json!(17);
        record["Status"] = json!("Completed");
        record["phase"] = json!("synthesize");
        record["snapshot_json"] = json!(snapshot.to_string());
        record["program_json"] = json!(program.to_string());
        let prepared = resume_checkpoint(&record, "w", 2000).unwrap();
        let restored = core::parse(core::field(&prepared, "program_json")).unwrap();
        assert_eq!(prepared["phase"], "explore");
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        assert_eq!(restored["resume_mode"], "unfinished_exploration");
        for key in ["http_calls", "world_revision"] {
            assert_eq!(restored[key], program[key]);
        }
        assert!(restored["results"]["h"]["estimate_likelihood"].is_null());
        assert!(
            restored["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "estimate_likelihood")
        );
        assert_eq!(prepared["trace_json"], record["trace_json"]);
        assert_eq!(prepared["started_at_ms"], record["started_at_ms"]);
        let restored_snapshot = core::parse(core::field(&prepared, "snapshot_json")).unwrap();
        assert_eq!(
            restored_snapshot["nodes"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["archived"],
            true
        );
        assert!(
            restored["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["nodeId"] != "qualitative" && t["function"] != "choose_next_operation")
        );
        assert!(!exploration_interrupted(
            &snapshot,
            &program,
            &json!([{"task":{"world_id":"qualitative"}}])
        ));
        assert!(!exploration_interrupted(
            &snapshot,
            &program,
            &json!([{"nodeId":"qualitative","function":"estimate_likelihood"}])
        ));
        program["exploration_stop_reason"] = json!("exploration_converged");
        record["program_json"] = json!(program.to_string());
        assert!(resume_checkpoint(&record, "w", 2000).is_err());
    }

    #[test]
    #[ignore = "Requires explicit local completed exploration fixture"]
    fn saved_completed_exploration_recovers_all_recorded_checks() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_EXPLORATION_FIXTURE").unwrap())
                .unwrap(),
        )
        .unwrap();
        let record = &raw["fields"];
        let now = core::field(record, "started_at_ms").parse::<u64>().unwrap() + 1000;
        let prepared = resume_checkpoint(record, core::field(record, "world_id"), now).unwrap();
        assert_eq!(prepared["trace_json"], record["trace_json"]);
        assert_eq!(prepared["started_at_ms"], record["started_at_ms"]);
        let old = core::parse(core::field(record, "program_json")).unwrap();
        let restored = core::parse(core::field(&prepared, "program_json")).unwrap();
        for key in ["results", "evaluations", "http_calls"] {
            assert_eq!(restored[key], old[key]);
        }
        assert_eq!(prepared["phase"], "explore");
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        assert_eq!(restored["resume_mode"], "unfinished_exploration");
        assert!(restored["tasks"].as_array().unwrap().iter().all(|task| {
            old["results"][core::field(task, "nodeId")][core::field(task, "function")].is_null()
        }));
    }

    #[test]
    fn resume_preserves_checkpoint_and_rejects_running_cross_world_or_corrupt_data() {
        let record = checkpoint();
        let prepared = resume_checkpoint(&record, "w", 2000).unwrap();
        for key in [
            "snapshot_json",
            "program_json",
            "trace_json",
            "started_at_ms",
            "agent_id",
            "model",
            "provider",
            "phase",
        ] {
            assert_eq!(prepared[key], record[key]);
        }
        assert!(resume_checkpoint(&record, "another-world", 2000).is_err());
        for status in ["Created", "Reasoning", "Completed"] {
            let mut bad = record.clone();
            bad["Status"] = json!(status);
            assert!(resume_checkpoint(&bad, "w", 2000).is_err());
        }
        let mut bad = record.clone();
        bad["snapshot_json"] = json!("{}");
        assert!(resume_checkpoint(&bad, "w", 2000).is_err());
        bad = record.clone();
        bad["trace_json"] = json!("not JSON");
        assert!(resume_checkpoint(&bad, "w", 2000).is_err());
        bad = record.clone();
        bad["trace_json"] = json!("[{\"index\":99}]");
        assert!(resume_checkpoint(&bad, "w", 2000).is_err());
    }
    #[test]
    fn unfinished_evaluations_resume_at_existing_cursor_without_losing_failed_attempt() {
        let mut record = checkpoint();
        let mut program: Value =
            serde_json::from_str(record["program_json"].as_str().unwrap()).unwrap();
        program["tasks"] = json!([{"nodeId":"h","function":"estimate_likelihood"}]);
        program["stop_reason"] = json!("provider_error");
        record["program_json"] = json!(program.to_string());
        record["trace_json"]=json!(json!([{"index":0,"nodeId":"h","requestFormat":"failed-attempt-hash-only","requestHash":"a".repeat(64),"error":"HTTP503"}]).to_string());
        let prepared = resume_checkpoint(&record, "w", 2000).unwrap();
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        assert_eq!(prepared["trace_json"], record["trace_json"]);
        let resumed: Value =
            serde_json::from_str(prepared["program_json"].as_str().unwrap()).unwrap();
        assert_eq!(resumed["cursor"], 0);
        assert_eq!(resumed["tasks"], program["tasks"]);
        assert_eq!(resumed["results"], program["results"]);
        assert_eq!(resumed["stop_reason"], "resumed_after_provider_error");
        let completed = resume_checkpoint(&checkpoint(), "w", 2000).unwrap();
        assert_eq!(resume_transition(&completed).unwrap(), "Prepared");
    }

    #[test]
    fn resume_checks_structural_subjects_even_when_trace_node_id_exists() {
        let mut record = checkpoint();
        let mut snapshot = core::parse(record["snapshot_json"].as_str().unwrap()).unwrap();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"h2","kind":"scenario","statement":"Second future","edges":"[]"}));
        let mut program = core::parse(record["program_json"].as_str().unwrap()).unwrap();
        assert!(core::search::plan_combinations(&snapshot, &mut program, 10));
        record["snapshot_json"] = json!(snapshot.to_string());
        record["program_json"] = json!(program.to_string());
        let task = program["tasks"][0].clone();
        let mut trace = json!([{"index":0,"nodeId":task["nodeId"],"function":"check_pair","task":task,"request":{},"response":{}}]);
        record["trace_json"] = json!(trace.to_string());
        assert!(resume_checkpoint(&record, "w", 2000).is_ok());
        trace[0]["nodeId"] = json!("h");
        trace[0]["task"]["nodeId"] = json!("h");
        trace[0]["task"]["pair_ids"] = json!(["h", "missing"]);
        record["trace_json"] = json!(trace.to_string());
        assert!(resume_checkpoint(&record, "w", 2000).is_err());
        record["trace_json"] = json!("[]");
        program["tasks"][0]["pair_ids"] = json!(["h", "h"]);
        record["program_json"] = json!(program.to_string());
        assert!(resume_checkpoint(&record, "w", 2000).is_err());
    }

    #[test]
    fn partial_completed_answer_resumes_unfinished_checks_but_complete_answers_do_not() {
        let mut record = checkpoint();
        record["Status"] = json!("Completed");
        record["phase"] = json!("synthesize");
        let mut snapshot = core::parse(core::field(&record, "snapshot_json")).unwrap();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"world-a","kind":"world","statement":"Joint future","edges":"[]"}));
        record["snapshot_json"] = json!(snapshot.to_string());
        let mut program = core::parse(core::field(&record, "program_json")).unwrap();
        program["stage"] = json!("worlds");
        program["tasks"] = json!([{"nodeId":"h","function":"estimate_likelihood"}]);
        program["stop_reason"] = json!("provider_error");
        program["world_refinement"] =
            json!({"world-a":{"rounds":[{"round":1,"complete":false,"probability":null}]}});
        record["program_json"] = json!(program.to_string());
        let prepared = resume_checkpoint(&record, "w", 2000).unwrap();
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        assert_eq!(prepared["snapshot_json"], record["snapshot_json"]);
        assert_eq!(prepared["trace_json"], record["trace_json"]);
        assert_eq!(prepared["started_at_ms"], record["started_at_ms"]);
        let resumed = core::parse(core::field(&prepared, "program_json")).unwrap();
        assert_eq!(resumed["world_refinement"], program["world_refinement"]);
        assert_eq!(resumed["cursor"], 0);
        assert_eq!(resumed["stop_reason"], "resumed_after_provider_error");
        let exhausted = resume_checkpoint(&record, "w", core::MAX_MS + 1000).unwrap();
        assert_eq!(resume_transition(&exhausted).unwrap(), "Prepared");
        assert_eq!(exhausted["phase"], "synthesize");
        for reason in ["judgments_stable", "max_rounds", "time_budget", ""] {
            program["stop_reason"] = json!(reason);
            record["program_json"] = json!(program.to_string());
            assert!(resume_checkpoint(&record, "w", 2000).is_err());
        }
        program["stop_reason"] = json!("provider_error");
        program["cursor"] = json!(1);
        record["program_json"] = json!(program.to_string());
        assert!(resume_checkpoint(&record, "w", 2000).is_err());
    }

    #[test]
    fn independent_challenge_resumes_in_its_original_phase() {
        let mut record = checkpoint();
        record["phase"] = json!("challenge");
        let prepared = resume_checkpoint(&record, "w", 2000).unwrap();
        assert_eq!(prepared["phase"], "challenge");
        assert_eq!(prepared["snapshot_json"], record["snapshot_json"]);
        assert_eq!(prepared["trace_json"], record["trace_json"]);
        assert_eq!(resume_transition(&prepared).unwrap(), "Prepared");
    }

    #[test]
    fn interrupted_world_evaluation_resumes_tasks_instead_of_recomposing() {
        let prepared = json!({"phase":"compose","program_json":json!({"stage":"worlds","cursor":1,"tasks":[{},{}]}).to_string()});
        assert_eq!(resume_transition(&prepared).unwrap(), "ResumePrepared");
        let qualitative = json!({"phase":"compose","program_json":json!({"stage":"exploration","cursor":1,"tasks":[{},{}]}).to_string()});
        assert_eq!(resume_transition(&qualitative).unwrap(), "Prepared");
    }
    #[test]
    fn elapsed_budget_composes_drafts_without_component_estimates() {
        let mut record = checkpoint();
        let mut snapshot = core::parse(record["snapshot_json"].as_str().unwrap()).unwrap();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"h2","kind":"scenario","statement":"Second future","edges":"[]"}));
        record["snapshot_json"] = json!(snapshot.to_string());
        let mut program = core::parse(record["program_json"].as_str().unwrap()).unwrap();
        program["results"] = json!({});
        record["program_json"] = json!(program.to_string());
        record["phase"] = json!("synthesize");
        assert_eq!(
            resume_checkpoint(&record, "w", 2000).unwrap()["phase"],
            "compose"
        );
        let prepared = resume_checkpoint(&record, "w", 1000 + core::MAX_MS).unwrap();
        assert_eq!(prepared["phase"], "compose");
        assert_eq!(prepared["started_at_ms"], "1000");
        assert_eq!(resume_transition(&prepared).unwrap(), "Prepared");
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"w1","kind":"world","edges":"[]"}));
        record["snapshot_json"] = json!(snapshot.to_string());
        assert_eq!(
            resume_checkpoint(&record, "w", 1000 + core::MAX_MS).unwrap()["phase"],
            "synthesize"
        );
    }
    #[test]
    #[ignore = "Requires explicit local saved checkpoint fixture"]
    fn saved_252_call_checkpoint_is_preserved_exactly() {
        let path = std::env::var("FORESIGHT_RESUME_FIXTURE").expect("explicit fixture path");
        let record: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let now = core::field(&record, "started_at_ms")
            .parse::<u64>()
            .unwrap()
            + 1000;
        let prepared = resume_checkpoint(&record, core::field(&record, "world_id"), now).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(prepared["trace_json"].as_str().unwrap())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            252
        );
        for key in [
            "snapshot_json",
            "program_json",
            "trace_json",
            "started_at_ms",
        ] {
            assert_eq!(prepared[key], core::field(&record, key));
        }
    }

    #[test]
    fn researched_hypotheses_remain_hypotheses_in_the_evaluation_graph() {
        let h = snapshot_node(
            &json!({"Id":"h","provenance":"hypothesis","statement":"An inferred future"}),
        )
        .unwrap();
        let e = snapshot_node(
            &json!({"Id":"e","provenance":"contested","statement":"A disputed source claim"}),
        )
        .unwrap();
        assert_eq!(h["kind"], "scenario");
        assert_eq!(e["kind"], "evidence");
        assert_eq!(e["provenance"], "contested");
        let plan = core::plan(&[h, e]).unwrap();
        assert_eq!(
            plan["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| t["nodeId"] == "h")
                .count(),
            6
        );
    }
}
