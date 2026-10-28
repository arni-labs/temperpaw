//! Signal Router - turn observed failures into Patrol work.
//!
//! Triggered by `Signal.Ingest`. External sources such as Datadog, Discord,
//! GitHub, and schedules create or ingest one Signal, then this integration
//! normalizes the observation, archives obvious noise, or routes actionable
//! failures into FactoryCase, paw-pm Issue, WorkCycle, and local Codex
//! WorkerRun entities.

use temper_wasm_sdk::prelude::*;

const PATROL_PROJECT_ID: &str = "temperpaw-dark-factory";
const ISSUES_PATH: &str = "/tdata/Issues";
const FACTORY_CASES_PATH: &str = "/tdata/FactoryCases";
const WORK_CYCLES_PATH: &str = "/tdata/WorkCycles";
const WORKER_RUNS_PATH: &str = "/tdata/WorkerRuns";

const PM_SET_DESCRIPTION: &str = "TemperPaw.PM.SetDescription";
const PM_SET_PRIORITY: &str = "TemperPaw.PM.SetPriority";
const PM_MOVE_TO_TRIAGE: &str = "TemperPaw.PM.MoveToTriage";

const PATROL_NORMALIZE: &str = "TemperPaw.Patrol.Normalize";
const PATROL_TRIAGE: &str = "TemperPaw.Patrol.Triage";
const PATROL_ATTACH_CASE: &str = "TemperPaw.Patrol.AttachCase";
const PATROL_ARCHIVE: &str = "TemperPaw.Patrol.Archive";
const PATROL_OPEN: &str = "TemperPaw.Patrol.Open";
const PATROL_SET_RISK_FLOOR: &str = "TemperPaw.Patrol.SetRiskFloor";
const PATROL_LINK_PM_ISSUE: &str = "TemperPaw.Patrol.LinkPmIssue";
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
        ctx.log("info", "signal_router: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let signal_id = entity_id(&ctx);
        let source = string_param(&ctx, &fields, "source", "Source");
        let payload = string_param(&ctx, &fields, "payload", "Payload");
        let source_url = string_param(&ctx, &fields, "source_url", "SourceUrl");
        let severity = string_param(&ctx, &fields, "severity", "Severity");

        if payload.trim().is_empty() {
            return archive_signal(
                &ctx,
                &resolve_api_url(&ctx),
                &odata_headers(&ctx),
                &signal_id,
                "Signal had an empty payload and was archived as noise.",
            );
        }

        let temper_api_url = resolve_api_url(&ctx);
        let headers = odata_headers(&ctx);
        let summary = summarize_signal(&source, &payload);

        if is_noise_signal(&source, &payload, &severity) {
            archive_signal(
                &ctx,
                &temper_api_url,
                &headers,
                &signal_id,
                &format!("Archived non-actionable signal: {summary}"),
            )?;
            set_success_result("", &json!({ "status": "signal_archived" }));
            return Ok(());
        }

        let risk = initial_risk(&source, &payload, &severity);
        let risk_evidence = json!({
            "source": &source,
            "severity": &severity,
            "source_url": &source_url,
            "signal_id": &signal_id,
            "matched_evidence": &risk.evidence,
            "rule_scope": "initial_signal_intake"
        })
        .to_string();

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Signals",
            &signal_id,
            PATROL_NORMALIZE,
            &json!({
                "summary": &summary,
                "severity": normalized_severity(&severity, risk.lane)
            }),
        )?;
        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Signals",
            &signal_id,
            PATROL_TRIAGE,
            &json!({
                "summary": format!(
                    "Actionable {} signal routed by Patrol with risk floor {}.",
                    signal_source_label(&source),
                    risk.lane
                )
            }),
        )?;

        let issue_id = create_entity(&ctx, &temper_api_url, &headers, ISSUES_PATH)?;
        let case_id = create_entity(&ctx, &temper_api_url, &headers, FACTORY_CASES_PATH)?;
        let work_cycle_id = create_entity(&ctx, &temper_api_url, &headers, WORK_CYCLES_PATH)?;

        let branch_name = format!("codex/paw-signal-{}", short_id(&signal_id));
        let worktree_path = worktree_path(&ctx, &branch_name);
        let worker_task = worker_task(
            &signal_id,
            &case_id,
            &work_cycle_id,
            &source,
            &source_url,
            &summary,
            &payload,
        );
        let plan_summary = work_cycle_plan(&summary, &source, &source_url, &payload, risk.lane);
        let allowed_worker_id = configured_local_worker_id(&ctx);
        let start_approval_required = requires_human_start_approval(risk.lane);
        let worker_run_id = if start_approval_required {
            "queued after human start approval".to_string()
        } else {
            create_entity(&ctx, &temper_api_url, &headers, WORKER_RUNS_PATH)?
        };

        post_action(
            &ctx,
            &temper_api_url,
            &headers,
            "Issues",
            &issue_id,
            PM_SET_DESCRIPTION,
            &json!({
                "description": issue_description(
                    &signal_id,
                    &case_id,
                    &work_cycle_id,
                    &worker_run_id,
                    &source,
                    &source_url,
                    &payload,
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
                "patrol_request_id": "",
                "work_request_id": ""
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
            "Signals",
            &signal_id,
            PATROL_ATTACH_CASE,
            &json!({ "factory_case_id": &case_id }),
        )?;

        ctx.log(
            "info",
            &format!(
                "signal_router: routed {} Signal {signal_id} into FactoryCase {case_id}, WorkCycle {work_cycle_id}, WorkerRun {worker_run_id}",
                signal_source_label(&source)
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

fn archive_signal(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    signal_id: &str,
    summary: &str,
) -> Result<(), String> {
    post_action(
        ctx,
        base_url,
        headers,
        "Signals",
        signal_id,
        PATROL_ARCHIVE,
        &json!({
            "summary": summary,
            "error_message": "",
            "integration": "signal_router"
        }),
    )?;
    Ok(())
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

fn summarize_signal(source: &str, payload: &str) -> String {
    let compact = payload.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut summary: String = compact.chars().take(140).collect();
    if compact.chars().count() > 140 {
        summary.push_str("...");
    }
    let source = signal_source_label(source);
    if source.is_empty() {
        summary
    } else {
        format!("{source}: {summary}")
    }
}

fn signal_source_label(source: &str) -> String {
    match source.trim().to_ascii_lowercase().as_str() {
        "datadog" | "dd" => "Datadog".to_string(),
        "discord" | "discord-dm" | "dm" => "Discord".to_string(),
        "github" | "gh" => "GitHub".to_string(),
        "schedule" | "cron" => "schedule".to_string(),
        "repo-sweep" | "sweep" => "repo sweep".to_string(),
        other => other.to_string(),
    }
}

fn is_noise_signal(source: &str, payload: &str, severity: &str) -> bool {
    let haystack = format!("{source} {payload} {severity}").to_ascii_lowercase();
    let severity = severity.trim().to_ascii_lowercase();
    matches!(severity.as_str(), "debug" | "trace" | "noise" | "ok")
        || contains_any(
            &haystack,
            &["heartbeat ok", "resolved without action", "no-op"],
        )
}

fn initial_risk<'a>(source: &'a str, payload: &'a str, severity: &'a str) -> Risk<'a> {
    let evidence = sensitive_intake_evidence(&format!("{source} {payload} {severity}"));
    if evidence.is_empty() {
        Risk {
            lane: "L1",
            source: "signal_router:ordinary_initial_signal_intake",
            evidence: vec!["ordinary maintenance signal"],
        }
    } else {
        Risk {
            lane: "L3",
            source: "signal_router:sensitive_initial_signal_intake",
            evidence,
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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

fn normalized_severity(severity: &str, lane: &str) -> String {
    let severity = severity.trim();
    if !severity.is_empty() {
        severity.to_string()
    } else {
        lane.to_string()
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

fn issue_description(
    signal_id: &str,
    case_id: &str,
    work_cycle_id: &str,
    worker_run_id: &str,
    source: &str,
    source_url: &str,
    payload: &str,
    risk_lane: &str,
) -> String {
    format!(
        "Patrol intake created from Signal {signal_id}.\n\nSource: {source}\nSource URL: {source_url}\nRisk lane: {risk_lane}\nFactoryCase: {case_id}\nWorkCycle: {work_cycle_id}\nWorkerRun: {worker_run_id}\n\nPayload:\n{payload}\n\nExecutor: paw-codex-worker on the registered local Mac mini worker."
    )
}

fn work_cycle_plan(
    summary: &str,
    source: &str,
    source_url: &str,
    payload: &str,
    risk_lane: &str,
) -> String {
    format!(
        "# WorkCycle Plan\n\n## Context\nPatrol routed an observed signal into implementation work.\n\nSummary: {summary}\nSource: {source}\nSource URL: {source_url}\nRisk lane: {risk_lane}\n\nSignal payload:\n{payload}\n\n## Codex Plan Mode\nBefore any mutation, paw-codex-worker must run Codex in a read-only sandbox, inspect the signal path, and revise this WorkCycle plan with concrete source files, tests, and live checks.\n\n## Approach\n1. Reproduce or explain the signal from evidence first; avoid keyword-only risk decisions.\n2. Trace the signal through trigger intake, entity transitions, WASM integrations, Cedar authorization, runtime code, and dashboard visibility.\n3. Write the red test or proof harness for the observed failure before implementation.\n4. Apply the smallest Temper-native correction and keep unrelated state out of the branch.\n\n## File Manifest\n- `crates/paw-transport/` or trigger crates when the signal enters through Discord, Slack, webhooks, or runtime transport.\n- `os-apps/*/specs/`, `os-apps/*/wasm/`, and `os-apps/*/policies/` when entity flow or authorization changes.\n- `dashboard/` when the signal is user-visible in Paw Patrol views.\n- `.proofs/` or `docs/proofs/` for the state-transition and live evidence packet.\n\n## Verification Plan\nRun the focused failing test, then the touched Rust/WASM/dashboard checks. Exercise the original signal path or an equivalent webhook/transport flow, query WorkCycle/WorkerRun/Review/Evaluation/Proof state over OData, and record the live evidence.\n\n## Risks\n- Production, secrets, deployment, policy, migration, and external API changes require L2/L3 caution and possible human approval.\n- Signal evidence may be stale or incomplete; stop and escalate if the read-only plan cannot verify the failure.\n- Long-running transport fixes need liveness metrics so a passing unit test does not hide runtime stalls.\n\n## Open Questions\nThe Codex Plan Mode pass must identify exact reproduction steps and whether additional human approval is required."
    )
}

fn worker_task(
    signal_id: &str,
    case_id: &str,
    work_cycle_id: &str,
    source: &str,
    source_url: &str,
    summary: &str,
    payload: &str,
) -> String {
    format!(
        "You are the local Codex implementer for an observed TemperPaw signal.\n\nSignal: {signal_id}\nFactoryCase: {case_id}\nWorkCycle: {work_cycle_id}\nSource: {source}\nSource URL: {source_url}\nSummary: {summary}\n\nPayload:\n{payload}\n\nRequired loop:\n1. Work in the assigned git worktree and branch.\n2. Reproduce or explain the observed failure from the signal evidence.\n3. First perform agentic risk triage from actual evidence, not keyword matching. If the work is production-impacting, security/policy-sensitive, deploy/secrets/data-migration related, or otherwise needs human approval, do not make risky changes; report the approval needed and the evidence.\n4. Follow red-green TDD before implementation when implementation is safe to start.\n5. Keep orchestration Temper-native: entity specs, WASM integrations, and Cedar policies.\n6. Run focused tests and relevant live/E2E verification for the observed failure.\n7. Produce a visual ProofPacket with changed-files map, state diagram, tests, E2E evidence, risk notes, and OData links.\n8. Finish normally. The paw-codex-worker will report WorkerRun.ReportDone or WorkerRun.ReportFailed to Temper after the local Codex process exits."
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
        format!("{}[truncated]", input.chars().take(max).collect::<String>())
    }
}
