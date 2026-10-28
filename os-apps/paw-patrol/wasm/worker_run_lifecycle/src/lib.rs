//! WorkerRun Lifecycle — fan out local Codex results into review/evaluation.
//!
//! Triggered by WorkerRun.StartLocal, WorkerRun.ReportDone, and
//! WorkerRun.ReportFailed. This keeps the Dark Factory loop visible as
//! Temper state transitions: cases begin work, worker completion requests an
//! independent reviewer, evaluation gates are queued, and a visual ProofPacket
//! draft is attached before any human review.

use temper_wasm_sdk::prelude::*;

const FACTORY_CASES_PATH: &str = "/tdata/FactoryCases";
const WORK_CYCLES_PATH: &str = "/tdata/WorkCycles";
const REVIEW_RUNS_PATH: &str = "/tdata/ReviewRuns";
const EVALUATION_RUNS_PATH: &str = "/tdata/EvaluationRuns";
const PROOF_PACKETS_PATH: &str = "/tdata/ProofPackets";

const PATROL_BEGIN_WORK: &str = "TemperPaw.Patrol.BeginWork";
const PATROL_BEGIN_REVIEW: &str = "TemperPaw.Patrol.BeginReview";
const PATROL_ESCALATE: &str = "TemperPaw.Patrol.Escalate";
const PATROL_WORKER_DONE: &str = "TemperPaw.Patrol.WorkerDone";
const PATROL_SUBMIT_FOR_REVIEW: &str = "TemperPaw.Patrol.SubmitForReview";
const PATROL_ATTACH_REVIEW_RUN: &str = "TemperPaw.Patrol.AttachReviewRun";
const PATROL_ATTACH_EVALUATION_RUN: &str = "TemperPaw.Patrol.AttachEvaluationRun";
const PATROL_FAIL: &str = "TemperPaw.Patrol.Fail";
const PATROL_REQUEST_REVIEW: &str = "TemperPaw.Patrol.Request";
const PATROL_QUEUE_EVALUATION: &str = "TemperPaw.Patrol.Queue";
const PATROL_ATTACH_DRAFT: &str = "TemperPaw.Patrol.AttachDraft";

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let base_url = resolve_api_url(&ctx);
        let headers = odata_headers(&ctx);
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        match ctx.trigger_action.as_str() {
            "StartLocal" => handle_started(&ctx, &base_url, &headers, &fields),
            "ReportDone" => handle_done(&ctx, &base_url, &headers, &fields),
            "ReportFailed" => handle_failed(&ctx, &base_url, &headers, &fields),
            other => Err(format!("worker_run_lifecycle: unsupported trigger {other}")),
        }?;

        set_success_result("", &json!({ "status": "worker_run_lifecycle_complete" }));
        Ok(())
    })();

    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

fn handle_started(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    fields: &Value,
) -> Result<(), String> {
    let case_id = string_field(fields, "factory_case_id", "FactoryCaseId");
    if case_id.is_empty() {
        return Ok(());
    }

    begin_case_work_if_needed(ctx, base_url, headers, &case_id)
}

fn handle_done(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    fields: &Value,
) -> Result<(), String> {
    let worker_run_id = entity_id(ctx);
    let work_cycle_id = string_field(fields, "work_cycle_id", "WorkCycleId");
    let case_id = string_field(fields, "factory_case_id", "FactoryCaseId");
    let risk_lane = string_field(fields, "risk_lane", "RiskLane");
    let evaluator_id = string_field(fields, "worker_id", "WorkerId");
    let result_summary = string_param(ctx, fields, "result_summary", "ResultSummary");
    let branch_name = string_param(ctx, fields, "branch_name", "BranchName");
    let mut proof_packet_id = string_param(ctx, fields, "proof_packet_id", "ProofPacketId");

    if work_cycle_id.is_empty() {
        return Err("worker_run_lifecycle: work_cycle_id is required".to_string());
    }

    if proof_packet_id.is_empty() {
        proof_packet_id = create_entity(ctx, base_url, headers, PROOF_PACKETS_PATH)?;
    }

    let review_run_id = create_entity(ctx, base_url, headers, REVIEW_RUNS_PATH)?;
    let evaluation_run_id = create_entity(ctx, base_url, headers, EVALUATION_RUNS_PATH)?;
    let required_checks = required_checks_for_risk(&risk_lane);

    post_action(
        ctx,
        base_url,
        headers,
        "ProofPackets",
        &proof_packet_id,
        PATROL_ATTACH_DRAFT,
        &json!({
            "work_cycle_id": &work_cycle_id,
            "worker_run_id": &worker_run_id,
            "review_run_id": &review_run_id,
            "evaluation_run_id": &evaluation_run_id,
            "summary_markdown": proof_summary_markdown(&worker_run_id, &branch_name, &result_summary),
            "proof_json": proof_json(
                &worker_run_id,
                &work_cycle_id,
                &review_run_id,
                &evaluation_run_id,
                &required_checks,
                &result_summary
            ),
            "visual_summary_url": visual_summary_svg(&worker_run_id, &branch_name, &risk_lane),
            "state_diagram_mermaid": state_diagram_mermaid(),
            "changed_files_map": changed_files_map(&branch_name, &result_summary),
            "reviewer_verdict": "pending independent reviewer",
            "residual_risks": "Reviewer and automated evaluation have not passed yet."
        }),
    )?;

    post_action(
        ctx,
        base_url,
        headers,
        "ReviewRuns",
        &review_run_id,
        PATROL_REQUEST_REVIEW,
        &json!({
            "work_cycle_id": &work_cycle_id,
            "worker_run_id": &worker_run_id,
            "proof_packet_id": &proof_packet_id
        }),
    )?;
    post_action(
        ctx,
        base_url,
        headers,
        "EvaluationRuns",
        &evaluation_run_id,
        PATROL_QUEUE_EVALUATION,
        &json!({
            "work_cycle_id": &work_cycle_id,
            "required_checks": &required_checks,
            "evaluator_id": &evaluator_id
        }),
    )?;

    advance_work_cycle_to_review(
        ctx,
        base_url,
        headers,
        &work_cycle_id,
        &worker_run_id,
        &review_run_id,
        &evaluation_run_id,
    )?;

    if !case_id.is_empty() {
        begin_case_work_if_needed(ctx, base_url, headers, &case_id)?;
        begin_case_review_if_needed(ctx, base_url, headers, &case_id)?;
    }

    ctx.log(
        "info",
        &format!(
            "worker_run_lifecycle: queued independent reviewer {review_run_id}, evaluation {evaluation_run_id}, and visual ProofPacket {proof_packet_id}"
        ),
    );

    Ok(())
}

fn handle_failed(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    fields: &Value,
) -> Result<(), String> {
    let work_cycle_id = string_field(fields, "work_cycle_id", "WorkCycleId");
    let case_id = string_field(fields, "factory_case_id", "FactoryCaseId");
    let error_message = string_param(ctx, fields, "error_message", "ErrorMessage");
    let error_message = if error_message.trim().is_empty() {
        "WorkerRun failed without a detailed error.".to_string()
    } else {
        error_message
    };

    if !work_cycle_id.is_empty() {
        let status = get_status(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            &work_cycle_id,
        )?;
        if matches!(
            status.as_str(),
            "Planning" | "Planned" | "InProgress" | "Testing" | "Reviewing" | "Proving"
        ) {
            post_action(
                ctx,
                base_url,
                headers,
                entity_set(WORK_CYCLES_PATH),
                &work_cycle_id,
                PATROL_FAIL,
                &json!({ "error_message": &error_message }),
            )?;
        }
    }

    if !case_id.is_empty() {
        let status = get_status(
            ctx,
            base_url,
            headers,
            entity_set(FACTORY_CASES_PATH),
            &case_id,
        )?;
        if matches!(
            status.as_str(),
            "Triaging" | "Scoped" | "Queued" | "InProgress" | "Reviewing" | "Proving"
        ) {
            post_action(
                ctx,
                base_url,
                headers,
                entity_set(FACTORY_CASES_PATH),
                &case_id,
                PATROL_ESCALATE,
                &json!({ "escalation_reason": &error_message }),
            )?;
        }
    }

    Ok(())
}

fn begin_case_work_if_needed(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    case_id: &str,
) -> Result<(), String> {
    let status = get_status(
        ctx,
        base_url,
        headers,
        entity_set(FACTORY_CASES_PATH),
        case_id,
    )?;
    if status == "Queued" {
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(FACTORY_CASES_PATH),
            case_id,
            PATROL_BEGIN_WORK,
            &json!({}),
        )?;
    }
    Ok(())
}

fn begin_case_review_if_needed(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    case_id: &str,
) -> Result<(), String> {
    let status = get_status(
        ctx,
        base_url,
        headers,
        entity_set(FACTORY_CASES_PATH),
        case_id,
    )?;
    if status == "InProgress" {
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(FACTORY_CASES_PATH),
            case_id,
            PATROL_BEGIN_REVIEW,
            &json!({}),
        )?;
    }
    Ok(())
}

fn advance_work_cycle_to_review(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    work_cycle_id: &str,
    worker_run_id: &str,
    review_run_id: &str,
    evaluation_run_id: &str,
) -> Result<(), String> {
    let mut status = get_status(
        ctx,
        base_url,
        headers,
        entity_set(WORK_CYCLES_PATH),
        work_cycle_id,
    )?;
    if status == "InProgress" {
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            PATROL_WORKER_DONE,
            &json!({ "implementer_worker_run_id": worker_run_id }),
        )?;
        status = wait_for_status(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            "Testing",
            &["Reviewing"],
        )?;
    }

    if status == "Testing" {
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            PATROL_SUBMIT_FOR_REVIEW,
            &json!({}),
        )?;
        status = wait_for_status(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            "Reviewing",
            &[],
        )?;
    }

    if status == "Reviewing" {
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            PATROL_ATTACH_REVIEW_RUN,
            &json!({ "reviewer_run_id": review_run_id }),
        )?;
        post_action(
            ctx,
            base_url,
            headers,
            entity_set(WORK_CYCLES_PATH),
            work_cycle_id,
            PATROL_ATTACH_EVALUATION_RUN,
            &json!({ "evaluation_run_id": evaluation_run_id }),
        )?;
    }

    Ok(())
}

fn wait_for_status(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    entity_set: &str,
    entity_id: &str,
    expected_status: &str,
    acceptable_later_statuses: &[&str],
) -> Result<String, String> {
    let mut last_status = String::new();
    for _ in 0..8 {
        last_status = get_status(ctx, base_url, headers, entity_set, entity_id)?;
        if last_status == expected_status
            || acceptable_later_statuses
                .iter()
                .any(|status| *status == last_status)
        {
            return Ok(last_status);
        }
    }

    Err(format!(
        "worker_run_lifecycle: WorkCycle {entity_id} did not reach {expected_status}; last observed status was {last_status}"
    ))
}

fn entity_id(ctx: &Context) -> String {
    ctx.entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.entity_id)
        .to_string()
}

fn string_param(ctx: &Context, fields: &Value, snake: &str, pascal: &str) -> String {
    ctx.trigger_params
        .get(snake)
        .and_then(Value::as_str)
        .or_else(|| ctx.trigger_params.get(pascal).and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| string_field(fields, snake, pascal))
}

fn string_field(fields: &Value, snake: &str, pascal: &str) -> String {
    fields
        .get(snake)
        .and_then(Value::as_str)
        .or_else(|| fields.get(pascal).and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn required_checks_for_risk(risk_lane: &str) -> String {
    let mut checks = vec![
        "red-green TDD evidence",
        "cargo fmt --check for touched Rust crates",
        "focused cargo tests for touched specs/WASM/Rust",
        "targeted live/E2E verification when behavior touches Discord, Temper, Railway, or user-visible flows",
        "human-readable visual ProofPacket",
    ];
    match risk_lane {
        "L3" => {
            checks
                .push("manual approval for deploy, secrets, billing, database, or migration risk");
            checks.push("security review with affected-path evidence");
        }
        "L2" => {
            checks.push("independent reviewer executes relevant code or live/E2E checks");
            checks.push("Cedar/WASM/state-machine regression check when affected");
        }
        _ => {}
    }
    json!(checks).to_string()
}

fn proof_summary_markdown(worker_run_id: &str, branch_name: &str, result_summary: &str) -> String {
    format!(
        "# Worker Proof Draft\n\nWorkerRun: {worker_run_id}\nBranch: {branch_name}\n\nResult:\n{result_summary}\n\nReview status: pending independent reviewer.\nEvaluation status: queued.\n"
    )
}

fn proof_json(
    worker_run_id: &str,
    work_cycle_id: &str,
    review_run_id: &str,
    evaluation_run_id: &str,
    required_checks: &str,
    result_summary: &str,
) -> String {
    json!({
        "worker_run_id": worker_run_id,
        "work_cycle_id": work_cycle_id,
        "review_run_id": review_run_id,
        "evaluation_run_id": evaluation_run_id,
        "required_checks": required_checks,
        "result_summary": result_summary,
        "human_review_position": "after independent reviewer and evaluator"
    })
    .to_string()
}

fn state_diagram_mermaid() -> &'static str {
    "stateDiagram-v2\n  WorkerRun --> ReviewRun: ReportDone creates independent reviewer\n  WorkerRun --> EvaluationRun: ReportDone queues claimed gates\n  WorkerRun --> ProofPacket: ReportDone attaches visual ProofPacket draft\n  EvaluationRun --> WorkCycle: Pass records ReportE2e\n  WorkCycle --> Reviewing: WorkerDone + SubmitForReview\n"
}

fn changed_files_map(branch_name: &str, result_summary: &str) -> String {
    let changed_files = extract_git_status_changed_files(result_summary);
    if changed_files.is_empty() {
        json!({
            "branch_name": branch_name,
            "changed_files": "pending reviewer inspection",
            "dependency_map": "pending evaluator snapshot"
        })
        .to_string()
    } else {
        json!({
            "branch_name": branch_name,
            "changed_files": changed_files,
            "evidence_source": "WorkerRun result_summary git-status block",
            "dependency_map": "pending evaluator snapshot"
        })
        .to_string()
    }
}

fn extract_git_status_changed_files(result_summary: &str) -> Vec<String> {
    let Some(block) = fenced_block(result_summary, "git-status") else {
        return Vec::new();
    };

    block
        .lines()
        .filter_map(parse_git_status_path)
        .collect()
}

fn fenced_block(input: &str, language: &str) -> Option<String> {
    let fence = format!("```{language}");
    let start = input.find(&fence)?;
    let after_start = &input[start + fence.len()..];
    let after_newline = after_start.strip_prefix('\n').unwrap_or(after_start);
    let end = after_newline.find("```")?;
    Some(after_newline[..end].trim_end().to_string())
}

fn parse_git_status_path(line: &str) -> Option<String> {
    let line = line.trim_end();
    if line.len() < 3 || line == "(clean worktree)" {
        return None;
    }
    let status = &line.as_bytes()[..2];
    let known_status = matches!(
        status,
        b" M" | b"M " | b"MM" | b"A " | b" A" | b"D " | b" D" | b"R " | b" R" | b"C " | b" C"
            | b"??"
            | b"!!"
            | b"UU"
    );
    if !known_status {
        return None;
    }
    let path = line[2..].trim();
    let path = path
        .rsplit_once(" -> ")
        .map(|(_, renamed)| renamed)
        .unwrap_or(path)
        .trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn visual_summary_svg(worker_run_id: &str, branch_name: &str, risk_lane: &str) -> String {
    let title = format!("WorkerRun {worker_run_id}");
    let branch = if branch_name.trim().is_empty() {
        "branch pending".to_string()
    } else {
        branch_name.to_string()
    };
    let risk = if risk_lane.trim().is_empty() {
        "risk pending"
    } else {
        risk_lane
    };
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='1200' height='720' viewBox='0 0 1200 720'><rect width='1200' height='720' fill='#f7f4ed'/><rect x='56' y='56' width='1088' height='608' rx='18' fill='#ffffff' stroke='#1f2937' stroke-width='3'/><text x='96' y='132' font-family='Inter, Arial, sans-serif' font-size='44' font-weight='700' fill='#111827'>Patrol Proof Draft</text><text x='96' y='190' font-family='Inter, Arial, sans-serif' font-size='24' fill='#374151'>{}</text><text x='96' y='236' font-family='Inter, Arial, sans-serif' font-size='22' fill='#4b5563'>Branch: {}</text><g font-family='Inter, Arial, sans-serif' font-size='22' font-weight='700'><rect x='96' y='310' width='210' height='86' rx='12' fill='#dbeafe' stroke='#2563eb'/><text x='128' y='363' fill='#1e3a8a'>Worker done</text><rect x='380' y='310' width='230' height='86' rx='12' fill='#fef3c7' stroke='#d97706'/><text x='414' y='363' fill='#92400e'>Review pending</text><rect x='684' y='310' width='250' height='86' rx='12' fill='#dcfce7' stroke='#16a34a'/><text x='718' y='363' fill='#166534'>Evaluation queued</text></g><path d='M306 353 L380 353' stroke='#374151' stroke-width='4'/><path d='M610 353 L684 353' stroke='#374151' stroke-width='4'/><text x='96' y='492' font-family='Inter, Arial, sans-serif' font-size='24' fill='#111827'>Risk lane: {}</text><text x='96' y='542' font-family='Inter, Arial, sans-serif' font-size='20' fill='#4b5563'>Human review waits until reviewer and evaluator pass, unless the risk lane requires escalation.</text></svg>",
        escape_xml(&title),
        escape_xml(&branch),
        escape_xml(risk)
    );
    format!("data:image/svg+xml,{}", percent_encode_svg(&svg))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn percent_encode_svg(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push_str("%20"),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn resolve_api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|value| !value.is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn odata_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn create_entity(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    path: &str,
) -> Result<String, String> {
    let url = format!("{base_url}{path}");
    let entity_set = path.rsplit('/').next().unwrap_or(path);
    let resp = ctx.http_call("POST", &url, headers, "{}")?;
    let body = parse_json_response(resp, &format!("create {entity_set}"))?;
    entity_id_from_response(&body).ok_or_else(|| format!("create {entity_set}: missing entity_id"))
}

fn entity_set(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn get_status(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    entity_set: &str,
    entity_id: &str,
) -> Result<String, String> {
    let url = format!("{base_url}/tdata/{entity_set}('{entity_id}')");
    let resp = ctx.http_call("GET", &url, headers, "")?;
    let body = parse_json_response(resp, &format!("get {entity_set}('{entity_id}')"))?;
    Ok(status_from_response(&body))
}

fn post_action(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    entity_set: &str,
    entity_id: &str,
    action_path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{base_url}/tdata/{entity_set}('{entity_id}')/{action_path}");
    let resp = ctx.http_call("POST", &url, headers, &body.to_string())?;
    parse_json_response(
        resp,
        &format!("{action_path} on {entity_set}('{entity_id}')"),
    )
}

fn parse_json_response(resp: HttpResponse, label: &str) -> Result<Value, String> {
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "{label} failed with HTTP {}: {}",
            resp.status,
            truncate(&resp.body, 500)
        ));
    }
    if resp.body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&resp.body).map_err(|err| format!("{label}: parse response: {err}"))
}

fn entity_id_from_response(value: &Value) -> Option<String> {
    value
        .get("entity_id")
        .or_else(|| value.get("id"))
        .or_else(|| value.get("Id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn status_from_response(value: &Value) -> String {
    value
        .get("Status")
        .or_else(|| value.get("status"))
        .or_else(|| value.pointer("/state/status"))
        .or_else(|| value.pointer("/fields/Status"))
        .or_else(|| value.pointer("/fields/status"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        format!("{}[truncated]", &input[..max])
    }
}
