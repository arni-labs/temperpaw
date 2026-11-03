//! Patrol Request Router - turn submitted work intent into Patrol work.
//!
//! Triggered by `WorkRequest.Submit` and the legacy `PatrolRequest.Submit`.
//! The trigger boundary creates only the intake entity; this WASM integration
//! performs the Temper-native follow-on transitions: PM issue, FactoryCase,
//! WorkCycle, and queued WorkerRun.

use temper_wasm_sdk::prelude::*;

const PATROL_PROJECT_ID: &str = "temperpaw-dark-factory";
const ISSUES_PATH: &str = "/tdata/Issues";
const FACTORY_CASES_PATH: &str = "/tdata/FactoryCases";
const WORK_CYCLES_PATH: &str = "/tdata/WorkCycles";
const WORKER_RUNS_PATH: &str = "/tdata/WorkerRuns";
const PM_SET_DESCRIPTION: &str = "TemperPaw.PM.SetDescription";
const PM_SET_PRIORITY: &str = "TemperPaw.PM.SetPriority";
const PM_MOVE_TO_TRIAGE: &str = "TemperPaw.PM.MoveToTriage";
const PATROL_TRIAGE: &str = "TemperPaw.Patrol.Triage";
const PATROL_ACCEPT_AS_CASE: &str = "TemperPaw.Patrol.AcceptAsCase";
const PATROL_LINK_PM_ISSUE: &str = "TemperPaw.Patrol.LinkPmIssue";
const PATROL_OPEN: &str = "TemperPaw.Patrol.Open";
const PATROL_SET_RISK_FLOOR: &str = "TemperPaw.Patrol.SetRiskFloor";
const PATROL_OPEN_WORK_CYCLE: &str = "TemperPaw.Patrol.OpenWorkCycle";
const PATROL_QUEUE_WORK: &str = "TemperPaw.Patrol.QueueWork";
const PATROL_CONFIGURE: &str = "TemperPaw.Patrol.Configure";
const PATROL_WRITE_PLAN: &str = "TemperPaw.Patrol.WritePlan";
const PATROL_REQUEST_HUMAN_START_APPROVAL: &str = "TemperPaw.Patrol.RequestHumanStartApproval";
const PATROL_START_WORK: &str = "TemperPaw.Patrol.StartWork";
const PATROL_ATTACH_WORKER_RUN: &str = "TemperPaw.Patrol.AttachWorkerRun";

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        ctx.log("info", "patrol_request_router: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let request_id = entity_id(&ctx);
        let source = string_param(&ctx, &fields, "source", "Source");
        let request_text = string_param(&ctx, &fields, "request_text", "RequestText");
        let requester_id = string_param(&ctx, &fields, "requester_id", "RequesterId");
        let signal_id = string_field(&fields, "signal_id", "SignalId");

        if request_text.trim().is_empty() {
            return Err("patrol_request_router: request_text is required".to_string());
        }

        let temper_api_url = resolve_api_url(&ctx);
        let headers = odata_headers(&ctx);
        let summary = summarize_request(&source, &request_text);
        let risk = initial_risk(&source, &request_text);
        let intake_set = intake_entity_set(&ctx);
        let (patrol_request_id, work_request_id) = intake_case_links(&ctx, &request_id);
        let risk_evidence = json!({
            "source": &source,
            "requester_id": &requester_id,
            "intake_entity_type": &ctx.entity_type,
            "intake_entity_id": &request_id,
            "patrol_request_id": &patrol_request_id,
            "work_request_id": &work_request_id,
            "matched_evidence": &risk.evidence,
            "rule_scope": "initial_intake_only"
        })
        .to_string();

        let issue_id = create_entity(&ctx, &temper_api_url, &headers, ISSUES_PATH)?;
        let case_id = create_entity(&ctx, &temper_api_url, &headers, FACTORY_CASES_PATH)?;
        let work_cycle_id = create_entity(&ctx, &temper_api_url, &headers, WORK_CYCLES_PATH)?;

        let branch_name = format!("codex/paw-patrol-{}", short_id(&request_id));
        let worktree_path = worktree_path(&ctx, &branch_name);
        let worker_task = worker_task(
            &request_id,
            &case_id,
            &work_cycle_id,
            &summary,
            &request_text,
        );
        let plan_summary = work_cycle_plan(&summary, &request_text, risk.lane);
        let allowed_worker_id = configured_local_worker_id(&ctx);
        let start_approval_required = requires_human_start_approval(risk.lane);
        let worker_run_id = if start_approval_required {
            "queued after human start approval".to_string()
        } else {
            create_entity(&ctx, &temper_api_url, &headers, WORKER_RUNS_PATH)?
        };
        let triage_summary = format!(
            "Patrol accepted this request as FactoryCase {case_id}, WorkCycle {work_cycle_id}, WorkerRun {worker_run_id}, with initial risk floor {} from {}.",
            risk.lane, risk.source
        );

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Issues",
            &issue_id,
            PM_SET_DESCRIPTION,
            &json!({
                "description": issue_description(
                    &request_id,
                    &case_id,
                    &work_cycle_id,
                    &worker_run_id,
                    &source,
                    &request_text,
                    risk.lane
                )
            }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Issues",
            &issue_id,
            PM_SET_PRIORITY,
            &json!({ "level": priority_for_lane(risk.lane) }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Issues",
            &issue_id,
            PM_MOVE_TO_TRIAGE,
            &json!({ "project_id": PATROL_PROJECT_ID }),
        )?;

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "FactoryCases",
            &case_id,
            PATROL_OPEN,
            &json!({
                "summary": &summary,
                "signal_id": &signal_id,
                "patrol_request_id": &patrol_request_id,
                "work_request_id": &work_request_id
            }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "FactoryCases",
            &case_id,
            PATROL_SET_RISK_FLOOR,
            &json!({
                "minimum_risk_lane": risk.lane,
                "risk_floor_source": risk.source,
                "risk_evidence": risk_evidence
            }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "FactoryCases",
            &case_id,
            PATROL_LINK_PM_ISSUE,
            &json!({ "pm_issue_id": &issue_id }),
        )?;

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "WorkCycles",
            &work_cycle_id,
            PATROL_CONFIGURE,
            &json!({
                "factory_case_id": &case_id,
                "pm_issue_id": &issue_id,
                "task_summary": &summary,
                "task_detail": &worker_task,
                "risk_lane": risk.lane
            }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "WorkCycles",
            &work_cycle_id,
            PATROL_WRITE_PLAN,
            &json!({ "plan_summary": &plan_summary }),
        )?;
        if requires_human_start_approval(risk.lane) {
            post_action(
                &ctx,
                &temper_api_url,
                &headers,
                "WorkCycles",
                &work_cycle_id,
                PATROL_REQUEST_HUMAN_START_APPROVAL,
                &json!({
                    "approval_summary": format!(
                        "Risk lane {} requires human approval before any local Codex WorkerRun is queued.",
                        risk.lane
                    )
                }),
            )?;
        } else {
            post_action(
                &ctx,
                &temper_api_url,
                &headers,
                "WorkCycles",
                &work_cycle_id,
                PATROL_START_WORK,
                &json!({}),
            )?;

            post_action(
                &ctx,
                &temper_api_url,
                &headers,
                "WorkerRuns",
                &worker_run_id,
                PATROL_CONFIGURE,
                &json!({
                    "work_cycle_id": &work_cycle_id,
                    "factory_case_id": &case_id,
                    "risk_lane": risk.lane,
                    "task": &worker_task,
                    "branch_name": &branch_name,
                    "worktree_path": &worktree_path,
                    "runner_kind": "local_codex",
                    "allowed_worker_id": &allowed_worker_id,
                    "provider_id": "local-codex",
                    "required_capabilities": "local_codex,repo_write"
                }),
            )?;
            post_action(
                &ctx,
                &temper_api_url,
                &headers,
                "WorkCycles",
                &work_cycle_id,
                PATROL_ATTACH_WORKER_RUN,
                &json!({ "implementer_worker_run_id": &worker_run_id }),
            )?;
        }
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "FactoryCases",
            &case_id,
            PATROL_OPEN_WORK_CYCLE,
            &json!({ "work_cycle_id": &work_cycle_id }),
        )?;
        if !start_approval_required {
            post_action(
                &ctx,
                &temper_api_url,
                &headers,
                "FactoryCases",
                &case_id,
                PATROL_QUEUE_WORK,
                &json!({}),
            )?;
        }

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            intake_set,
            &request_id,
            PATROL_TRIAGE,
            &json!({ "triage_summary": &triage_summary }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            intake_set,
            &request_id,
            PATROL_ACCEPT_AS_CASE,
            &json!({ "factory_case_id": &case_id }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            intake_set,
            &request_id,
            PATROL_LINK_PM_ISSUE,
            &json!({ "pm_issue_id": &issue_id }),
        )?;

        ctx.log(
            "info",
            &format!(
                "patrol_request_router: queued paw-codex-worker WorkerRun {worker_run_id} for WorkCycle {work_cycle_id}"
            ),
        );
        set_success_result("", &json!({ "worker_run_id": &worker_run_id }));
        Ok(())
    })();

    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

struct Risk<'a> {
    lane: &'a str,
    source: &'a str,
    evidence: Vec<&'a str>,
}

fn entity_id(ctx: &Context) -> String {
    ctx.entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .unwrap_or(&ctx.entity_id)
        .to_string()
}

fn intake_entity_set(ctx: &Context) -> &'static str {
    if is_entity_type(ctx, "WorkRequest") {
        "WorkRequests"
    } else {
        "PatrolRequests"
    }
}

fn intake_case_links(ctx: &Context, request_id: &str) -> (String, String) {
    if is_entity_type(ctx, "WorkRequest") {
        ("".to_string(), request_id.to_string())
    } else {
        (request_id.to_string(), "".to_string())
    }
}

fn is_entity_type(ctx: &Context, expected: &str) -> bool {
    ctx.entity_type.eq_ignore_ascii_case(expected)
        || ctx
            .entity_type
            .eq_ignore_ascii_case(&format!("{expected}s"))
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

fn summarize_request(source: &str, request_text: &str) -> String {
    let compact = request_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut summary: String = compact.chars().take(120).collect();
    if compact.chars().count() > 120 {
        summary.push_str("...");
    }
    if source.trim().is_empty() {
        summary
    } else {
        format!("{source}: {summary}")
    }
}

fn initial_risk<'a>(source: &'a str, request_text: &'a str) -> Risk<'a> {
    let evidence = sensitive_intake_evidence(&format!("{source} {request_text}"));
    if evidence.is_empty() {
        Risk {
            lane: "L1",
            source: "patrol_request_router:ordinary_initial_intake",
            evidence: vec!["ordinary maintenance request"],
        }
    } else {
        Risk {
            lane: "L3",
            source: "patrol_request_router:sensitive_initial_intake",
            evidence,
        }
    }
}

fn priority_for_lane(lane: &str) -> &str {
    match lane {
        "L3" => "1",
        "L2" => "2",
        "L1" => "3",
        _ => "4",
    }
}

fn requires_human_start_approval(lane: &str) -> bool {
    lane.eq_ignore_ascii_case("L3")
}

fn sensitive_intake_evidence(input: &str) -> Vec<&'static str> {
    let normalized = input.to_ascii_lowercase().replace('_', "-");
    let words = normalized
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let mut evidence = Vec::new();

    if has_word(&words, &["production", "prod"]) {
        push_evidence(&mut evidence, "sensitive_intake:production");
    }
    if words
        .iter()
        .any(|word| word.starts_with("deploy") || word.starts_with("release"))
    {
        push_evidence(&mut evidence, "sensitive_intake:deploy");
    }
    if has_word(
        &words,
        &["secret", "secrets", "credential", "credentials", "token", "tokens"],
    ) {
        push_evidence(&mut evidence, "sensitive_intake:secret");
    }
    if has_word(&words, &["migration", "migrations", "database", "schema"]) {
        push_evidence(&mut evidence, "sensitive_intake:migration");
    }
    if has_word(&words, &["security", "auth"]) {
        push_evidence(&mut evidence, "sensitive_intake:security");
    }
    if has_word(&words, &["policy", "policies"]) {
        push_evidence(&mut evidence, "sensitive_intake:policy");
    }
    if has_word(&words, &["cedar"]) {
        push_evidence(&mut evidence, "sensitive_intake:cedar");
    }
    if has_word(
        &words,
        &[
            "slack",
            "channel",
            "channels",
            "transport",
            "transports",
            "webhook",
            "webhooks",
            "user-facing",
        ],
    ) {
        push_evidence(&mut evidence, "sensitive_intake:user_facing");
    }
    if has_word(&words, &["discord"]) {
        push_evidence(&mut evidence, "sensitive_intake:discord");
    }
    if has_word(&words, &["railway"]) {
        push_evidence(&mut evidence, "sensitive_intake:deploy");
    }

    evidence
}

fn has_word(words: &[&str], needles: &[&str]) -> bool {
    words.iter().any(|word| needles.contains(word))
}

fn push_evidence(evidence: &mut Vec<&'static str>, value: &'static str) {
    if !evidence.contains(&value) {
        evidence.push(value);
    }
}

fn issue_description(
    request_id: &str,
    case_id: &str,
    work_cycle_id: &str,
    worker_run_id: &str,
    source: &str,
    request_text: &str,
    risk_lane: &str,
) -> String {
    format!(
        "Patrol intake created from WorkRequest or legacy PatrolRequest {request_id}.\n\nSource: {source}\nRisk lane: {risk_lane}\nFactoryCase: {case_id}\nWorkCycle: {work_cycle_id}\nWorkerRun: {worker_run_id}\n\nRequest:\n{request_text}\n\nExecutor: paw-codex-worker on the registered local Mac mini worker."
    )
}

fn work_cycle_plan(summary: &str, request_text: &str, risk_lane: &str) -> String {
    format!(
        "# WorkCycle Plan\n\n## Context\nPatrol accepted a WorkRequest or legacy PatrolRequest as implementation work.\n\nSummary: {summary}\nRisk lane: {risk_lane}\n\nRequest evidence:\n{request_text}\n\n## Codex Plan Mode\nBefore any file mutation, paw-codex-worker must run Codex with a read-only sandbox to inspect the assigned worktree and revise this WorkCycle plan with concrete file/test findings.\n\n## Approach\n1. Trace the user-facing behavior from ingress through Temper entities, WASM integrations, Cedar policy, transport/runtime code, and dashboard surfaces.\n2. Add the red test that demonstrates the requested behavior or regression.\n3. Make the smallest Temper-native change: state belongs in entities, orchestration belongs in WASM transitions, and authorization belongs in Cedar.\n4. Keep unrelated cleanup out of the branch and preserve the assigned worktree/branch boundary.\n\n## File Manifest\n- `os-apps/*/specs/`: inspect or change only if the state machine contract must move.\n- `os-apps/*/wasm/`: inspect or change business logic that reacts to entity actions.\n- `crates/*`: inspect or change trigger/runtime/worker edges only when the trigger boundary or platform support requires it.\n- `dashboard/`: inspect or change only for user-visible Patrol/dashboard behavior.\n- `.proofs/` or `docs/proofs/`: record evidence from tests and live checks.\n\n## Verification Plan\nRun the focused failing test first, then the minimal green test set for the touched crate/module. Build the affected WASM or Rust crate, exercise the OData/trigger/dashboard flow, query WorkCycle/WorkerRun/Review/Evaluation/Proof state, and capture proof evidence.\n\n## Risks\n- L2/L3 or production-impacting changes need human approval before unsafe mutation.\n- Discord, Datadog, GitHub, deployment, secret, migration, and Cedar changes can have broad blast radius; escalate if evidence is incomplete.\n- If the read-only plan finds the initial risk lane too low, stop and report the approval needed.\n\n## Open Questions\nThe Codex Plan Mode pass must verify exact files, tests, and live checks before implementation."
    )
}

fn worker_task(
    request_id: &str,
    case_id: &str,
    work_cycle_id: &str,
    summary: &str,
    request_text: &str,
) -> String {
    format!(
        "You are the local Codex implementer for TemperPaw paw-patrol.\n\nWorkRequest or legacy PatrolRequest: {request_id}\nFactoryCase: {case_id}\nWorkCycle: {work_cycle_id}\nSummary: {summary}\n\nRequest:\n{request_text}\n\nRequired loop:\n1. Work in the assigned git worktree and branch.\n2. First perform agentic risk triage from actual evidence, not keyword matching. If the work is production-impacting, security/policy-sensitive, deploy/secrets/data-migration related, or otherwise needs human approval, do not make risky changes; report the approval needed and the evidence.\n3. Follow red-green TDD before implementation when implementation is safe to start.\n4. Keep orchestration Temper-native: entity specs, WASM integrations, and Cedar policies.\n5. Run focused tests and relevant live/E2E verification for touched behavior.\n6. Produce a visual ProofPacket with changed-files map, state diagram, tests, E2E evidence, risk notes, and OData links.\n7. Finish normally. The paw-codex-worker will report WorkerRun.ReportDone or WorkerRun.ReportFailed to Temper after the local Codex process exits."
    )
}

fn short_id(entity_id: &str) -> String {
    let tail: String = entity_id
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    tail.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
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

fn configured_local_worker_id(ctx: &Context) -> String {
    ctx.config
        .get("local_codex_worker_id")
        .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "mac-mini-codex-prod".to_string())
}

fn configured_local_worktree_root(ctx: &Context) -> String {
    ctx.config
        .get("local_codex_worktree_root")
        .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "/Users/openclaw/Development/temperpaw-worktrees".to_string())
}

fn worktree_path(ctx: &Context, branch_name: &str) -> String {
    format!(
        "{}/{}",
        configured_local_worktree_root(ctx).trim_end_matches('/'),
        branch_name.replace('/', "-")
    )
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

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        format!("{}[truncated]", &input[..max])
    }
}
