//! notify_approval_needed WASM — sends approval controls when an agent
//! is paused for Cedar approval.
//!
//! Triggered by Agent.PauseForApproval. Reads the pending_decision_id
//! from the agent's state, registers the GovernanceDecision callback,
//! and then tries to notify the human through the bound channel session.
//!
//! Callback registration and notification failures preserve the human approval pause.
//! A session without a channel binding must still be able to wait for
//! approval via the dashboard or API.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    entity_field_str, find_connected_channel_by_external_id, list_entities, resolve_temper_api_url,
};

#[derive(Clone, Copy)]
struct ApprovalObservability<'a> {
    operation: &'a str,
    outcome: &'a str,
    delivery: &'a str,
    reason: &'a str,
    decision_id: &'a str,
    session_id: &'a str,
    agent_id: &'a str,
    parent_session_id: &'a str,
    active_plan_id: &'a str,
    bound_agent_id: &'a str,
    channel_id: &'a str,
    thread_id: &'a str,
    action: &'a str,
    http_status: Option<u16>,
}

impl<'a> Default for ApprovalObservability<'a> {
    fn default() -> Self {
        Self {
            operation: "",
            outcome: "",
            delivery: "",
            reason: "",
            decision_id: "",
            session_id: "",
            agent_id: "",
            parent_session_id: "",
            active_plan_id: "",
            bound_agent_id: "",
            channel_id: "",
            thread_id: "",
            action: "",
            http_status: None,
        }
    }
}

fn approval_observability_fields(event: ApprovalObservability<'_>) -> Value {
    json!({
        "observability_event": "temperpaw.approval",
        "decision_id": event.decision_id,
        "session_id": event.session_id,
        "agent_id": event.agent_id,
        "parent_session_id": event.parent_session_id,
        "active_plan_id": event.active_plan_id,
        "approval": {
            "operation": event.operation,
            "outcome": event.outcome,
            "delivery": event.delivery,
            "reason": event.reason,
            "action": event.action,
            "bound_agent_id": event.bound_agent_id,
            "channel_id": event.channel_id,
            "thread_id": event.thread_id,
            "http_status": event.http_status.unwrap_or_default(),
        }
    })
}

fn log_approval_event(ctx: &Context, level: &str, message: &str, event: ApprovalObservability<'_>) {
    let fields = approval_observability_fields(event);
    let _ = ctx.log_structured(level, message, &fields);
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let mut pending_approval: Option<(String, Option<String>)> = None;
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let tenant = &ctx.tenant;
        // Use the persistent Agent entity ID from Session fields, not the Session's own ID.
        // ChannelSessions store agent_entity_id = aj-..., not ss-...
        let session_id = ctx.entity_id.as_str();
        let agent_id = entity_field_str(&fields, &["agent_id", "AgentId"]).unwrap_or(session_id);
        let parent_session_id =
            entity_field_str(&fields, &["parent_session_id", "ParentSessionId"]).unwrap_or("");
        let active_plan_id =
            entity_field_str(&fields, &["active_plan_id", "ActivePlanId"]).unwrap_or("");

        // Read decision context from agent state
        let decision_id =
            entity_field_str(&fields, &["pending_decision_id", "PendingDecisionId"]).unwrap_or("");
        let tool_context_str =
            entity_field_str(&fields, &["pending_tool_context", "PendingToolContext"])
                .unwrap_or("{}");
        let tool_context: Value =
            serde_json::from_str(tool_context_str).unwrap_or_else(|_| json!({}));

        // The pending_tool_context has shape:
        // { "tool_context": { "method": "Session.SwitchMode", "args": {...} }, ... }
        let inner_ctx = tool_context.get("tool_context").unwrap_or(&tool_context);
        let action_method = inner_ctx
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown action");
        let action_desc = action_method;

        if decision_id.is_empty() {
            if !active_plan_id.is_empty() {
                log_approval_event(
                    &ctx,
                    "info",
                    "temperpaw.approval skipped",
                    ApprovalObservability {
                        operation: "resolve_context",
                        outcome: "skipped",
                        delivery: "not_applicable",
                        reason: "plan_review_notification_not_handled_here",
                        decision_id,
                        session_id,
                        agent_id,
                        parent_session_id,
                        active_plan_id,
                        action: action_desc,
                        ..ApprovalObservability::default()
                    },
                );
                ctx.log(
                    "info",
                    &format!(
                        "notify_approval: active_plan_id={active_plan_id} with no pending_decision_id; skipping governance callback registration"
                    ),
                );
                set_success_result(
                    "",
                    &json!({
                        "status": "skipped",
                        "reason": "plan_review_notification_not_handled_here",
                        "active_plan_id": active_plan_id,
                    }),
                );
                return Ok(());
            }

            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval missing decision",
                ApprovalObservability {
                    operation: "resolve_context",
                    outcome: "error",
                    delivery: "not_applicable",
                    reason: "missing_pending_decision_id",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            return Err("notify_approval: missing pending_decision_id".to_string());
        }

        // Register callback before notifying humans so an approval click cannot
        // outpace callback wiring and strand the waiting session.
        let callback_error =
            match register_gd_callback(&ctx, &temper_api_url, tenant, session_id, decision_id) {
                Ok(()) => {
                    log_approval_event(
                        &ctx,
                        "info",
                        "temperpaw.approval callback registered",
                        ApprovalObservability {
                            operation: "register_callback",
                            outcome: "success",
                            delivery: "not_applicable",
                            decision_id,
                            session_id,
                            agent_id,
                            parent_session_id,
                            active_plan_id,
                            action: action_desc,
                            ..ApprovalObservability::default()
                        },
                    );
                    None
                }
                Err(error) => {
                    log_approval_event(
                        &ctx,
                        "warn",
                        "temperpaw.approval callback registration failed",
                        ApprovalObservability {
                            operation: "register_callback",
                            outcome: "error",
                            delivery: "not_applicable",
                            reason: "register_callback_failed",
                            decision_id,
                            session_id,
                            agent_id,
                            parent_session_id,
                            active_plan_id,
                            action: action_desc,
                            ..ApprovalObservability::default()
                        },
                    );
                    ctx.log("warn", &error);
                    Some(error)
                }
            };
        pending_approval = Some((decision_id.to_string(), callback_error.clone()));
        let set_success_result = |action: &str, body: &Value| {
            set_success_result(
                action,
                &approval_result(body.clone(), callback_error.as_deref()),
            );
        };

        // Find the ChannelSession for this agent. Sessions without a channel
        // binding can still be approved via dashboard/API, so do not fail the
        // paused session if transport delivery is unavailable.
        let session = find_session_by_agent(
            &ctx,
            &temper_api_url,
            tenant,
            session_id,
            agent_id,
            parent_session_id,
        )?;
        let Some((session, bound_agent_id)) = session else {
            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval delivery skipped",
                ApprovalObservability {
                    operation: "notify_human",
                    outcome: "skipped",
                    delivery: "skipped",
                    reason: "no_channel_session",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            ctx.log(
                "warn",
                &format!(
                    "notify_approval: no channel session for agent {agent_id}; decision {decision_id} awaiting out-of-band approval"
                ),
            );
            set_success_result(
                "",
                &json!({
                    "status": "waiting_for_out_of_band_approval",
                    "decision_id": decision_id,
                    "delivery": "skipped",
                    "reason": "no_channel_session",
                }),
            );
            return Ok(());
        };
        if bound_agent_id != agent_id {
            log_approval_event(
                &ctx,
                "info",
                "temperpaw.approval using parent channel session",
                ApprovalObservability {
                    operation: "resolve_channel_session",
                    outcome: "success",
                    delivery: "not_applicable",
                    reason: "using_parent_channel_session",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    bound_agent_id: &bound_agent_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            ctx.log(
                "info",
                &format!(
                    "notify_approval: using parent channel session {bound_agent_id} for agent {agent_id}"
                ),
            );
        }

        let channel_id = entity_field_str(&session, &["ChannelId", "channel_id"]).unwrap_or("");
        let thread_id = entity_field_str(&session, &["ThreadId", "thread_id"]).unwrap_or("");
        if channel_id.is_empty() || thread_id.is_empty() {
            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval delivery skipped",
                ApprovalObservability {
                    operation: "notify_human",
                    outcome: "skipped",
                    delivery: "skipped",
                    reason: "missing_channel_binding",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    bound_agent_id: &bound_agent_id,
                    channel_id,
                    thread_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            ctx.log(
                "warn",
                &format!(
                    "notify_approval: session missing channel_id or thread_id for agent {agent_id}; decision {decision_id} awaiting out-of-band approval"
                ),
            );
            set_success_result(
                "",
                &json!({
                    "status": "waiting_for_out_of_band_approval",
                    "decision_id": decision_id,
                    "delivery": "skipped",
                    "reason": "missing_channel_binding",
                }),
            );
            return Ok(());
        }

        // Find the Channel entity to get the webhook_url
        let Some(channel) =
            find_connected_channel_by_external_id(&ctx, &temper_api_url, tenant, channel_id)?
        else {
            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval delivery skipped",
                ApprovalObservability {
                    operation: "notify_human",
                    outcome: "skipped",
                    delivery: "skipped",
                    reason: "channel_not_connected",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    bound_agent_id: &bound_agent_id,
                    channel_id,
                    thread_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            ctx.log(
                "warn",
                &format!(
                    "notify_approval: no connected Channel for channel_id={channel_id}; decision {decision_id} awaiting out-of-band approval"
                ),
            );
            set_success_result(
                "",
                &json!({
                    "status": "waiting_for_out_of_band_approval",
                    "decision_id": decision_id,
                    "delivery": "skipped",
                    "reason": "channel_not_connected",
                }),
            );
            return Ok(());
        };
        let webhook_url = entity_field_str(&channel, &["webhook_url", "WebhookUrl"]).unwrap_or("");
        if webhook_url.is_empty() {
            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval delivery skipped",
                ApprovalObservability {
                    operation: "notify_human",
                    outcome: "skipped",
                    delivery: "skipped",
                    reason: "missing_webhook_url",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    bound_agent_id: &bound_agent_id,
                    channel_id,
                    thread_id,
                    action: action_desc,
                    ..ApprovalObservability::default()
                },
            );
            ctx.log(
                "warn",
                &format!(
                    "notify_approval: Channel has no webhook_url for channel_id={channel_id}; decision {decision_id} awaiting out-of-band approval"
                ),
            );
            set_success_result(
                "",
                &json!({
                    "status": "waiting_for_out_of_band_approval",
                    "decision_id": decision_id,
                    "delivery": "skipped",
                    "reason": "missing_webhook_url",
                }),
            );
            return Ok(());
        }

        let pending_decision = match fetch_pending_decision(
            &ctx,
            &temper_api_url,
            tenant,
            decision_id,
        ) {
            Ok(decision) => decision,
            Err(error) => {
                log_approval_event(
                    &ctx,
                    "warn",
                    "temperpaw.approval decision lookup failed",
                    ApprovalObservability {
                        operation: "lookup_decision",
                        outcome: "error",
                        delivery: "not_applicable",
                        reason: "decision_lookup_failed",
                        decision_id,
                        session_id,
                        agent_id,
                        parent_session_id,
                        active_plan_id,
                        bound_agent_id: &bound_agent_id,
                        channel_id,
                        thread_id,
                        action: action_desc,
                        ..ApprovalObservability::default()
                    },
                );
                ctx.log(
                    "warn",
                    &format!(
                        "notify_approval: decision detail lookup failed for {decision_id}: {error}"
                    ),
                );
                None
            }
        };

        // Build the approval message with scoped buttons.
        // custom_id uses the platform's decision ID (PD-xxx).
        let mut content = format_approval_content(
            decision_id,
            agent_id,
            action_desc,
            inner_ctx,
            pending_decision.as_ref(),
        );

        if let Some(error) = &callback_error {
            content.push_str(&format!("\n\nAutomatic session resumption is unavailable: {error}. Your decision still requires human approval; after deciding, authorized callback recovery is needed to resume this paused session."));
        }
        let body = json!({
            "thread_id": thread_id,
            "content": content,
            "agent_entity_id": agent_id,
            "components": [{
                "type": 1,
                "components": [
                    {
                        "type": 2,
                        "style": 3,
                        "label": "Allow Always",
                        "custom_id": format!("approve_always:{decision_id}")
                    },
                    {
                        "type": 2,
                        "style": 1,
                        "label": "Allow Session",
                        "custom_id": format!("approve_session:{decision_id}")
                    },
                    {
                        "type": 2,
                        "style": 1,
                        "label": "Allow Once",
                        "custom_id": format!("approve_once:{decision_id}")
                    },
                    {
                        "type": 2,
                        "style": 4,
                        "label": "Deny",
                        "custom_id": format!("deny:{decision_id}")
                    }
                ]
            }],
            "blocks": [
                {
                    "type": "section",
                    "text": {
                        "type": "mrkdwn",
                        "text": content,
                    }
                },
                {
                    "type": "actions",
                    "block_id": format!("decision_{decision_id}"),
                    "elements": [
                        {
                            "type": "button",
                            "text": {
                                "type": "plain_text",
                                "text": "Allow Always"
                            },
                            "action_id": format!("approve_always:{decision_id}"),
                            "style": "primary"
                        },
                        {
                            "type": "button",
                            "text": {
                                "type": "plain_text",
                                "text": "Allow Session"
                            },
                            "action_id": format!("approve_session:{decision_id}")
                        },
                        {
                            "type": "button",
                            "text": {
                                "type": "plain_text",
                                "text": "Allow Once"
                            },
                            "action_id": format!("approve_once:{decision_id}")
                        },
                        {
                            "type": "button",
                            "text": {
                                "type": "plain_text",
                                "text": "Deny"
                            },
                            "action_id": format!("deny:{decision_id}"),
                            "style": "danger"
                        }
                    ]
                }
            ]
        });

        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-tenant-id".to_string(), tenant.to_string()),
        ];

        let resp = ctx.http_call("POST", webhook_url, &headers, &body.to_string())?;
        if !(200..300).contains(&resp.status) {
            let failure = format!(
                "notify_approval: webhook POST failed (HTTP {}): {}",
                resp.status,
                &resp.body[..resp.body.len().min(200)]
            );
            log_approval_event(
                &ctx,
                "warn",
                "temperpaw.approval delivery failed",
                ApprovalObservability {
                    operation: "notify_human",
                    outcome: "error",
                    delivery: "failed",
                    reason: "webhook_http_error",
                    decision_id,
                    session_id,
                    agent_id,
                    parent_session_id,
                    active_plan_id,
                    bound_agent_id: &bound_agent_id,
                    channel_id,
                    thread_id,
                    action: action_desc,
                    http_status: Some(resp.status),
                },
            );
            ctx.log("warn", &failure);
            set_success_result(
                "",
                &json!({
                    "status": "waiting_for_out_of_band_approval",
                    "decision_id": decision_id,
                    "delivery": "failed",
                    "error": failure,
                }),
            );
            return Ok(());
        }

        log_approval_event(
            &ctx,
            "info",
            "temperpaw.approval delivery notified",
            ApprovalObservability {
                operation: "notify_human",
                outcome: "success",
                delivery: "notified",
                decision_id,
                session_id,
                agent_id,
                parent_session_id,
                active_plan_id,
                bound_agent_id: &bound_agent_id,
                channel_id,
                thread_id,
                action: action_desc,
                http_status: Some(resp.status),
                ..ApprovalObservability::default()
            },
        );
        ctx.log(
            "info",
            &format!(
                "notify_approval: sent approval buttons for {decision_id} (agent {agent_id}) to thread {thread_id}"
            ),
        );

        set_success_result(
            "",
            &json!({"status": "notified", "decision_id": decision_id}),
        );
        Ok(())
    })();

    if let Err(error) = result {
        if let Some((decision_id, callback_error)) = pending_approval {
            set_success_result(
                "",
                &approval_result(
                    json!({
                        "status":"waiting_for_out_of_band_approval",
                        "decision_id":decision_id,
                        "delivery":"failed",
                        "notification_error":error
                    }),
                    callback_error.as_deref(),
                ),
            );
        } else {
            set_error_result(&error);
        }
    }
    0
}

fn approval_result(mut body: Value, callback_error: Option<&str>) -> Value {
    body["callback_registered"] = json!(callback_error.is_none());
    if let Some(error) = callback_error {
        body["status"] = json!("waiting_for_out_of_band_approval");
        body["callback_error"] = json!(error);
        body["recovery_required"] = json!(
            "Human decision and authorized callback recovery; no automatic resume is available."
        );
    }
    body
}

/// Query the GovernanceDecision entity by pending_decision_id in temper-system
/// and dispatch RegisterCallback so approval/denial routes back to this Session.
fn register_gd_callback(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
    decision_id: &str,
) -> Result<(), String> {
    // Query GD by pending_decision_id in temper-system tenant.
    let escaped = decision_id.replace('\'', "''");
    let gd_filter = format!("$filter=pending_decision_id eq '{escaped}'&$top=1");
    let gd_url = format!("{temper_api_url}/tdata/GovernanceDecisions?{gd_filter}");
    // Use "admin" principal — "system" is blocked from HTTP headers to prevent
    // privilege escalation. The temper-system tenant has a Cedar policy permitting
    // Admin principals to manage GovernanceDecision entities.
    let system_headers = vec![
        ("accept".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), "temper-system".to_string()),
        ("x-temper-principal-kind".to_string(), "admin".to_string()),
        (
            "x-temper-principal-id".to_string(),
            "request-approval-wasm".to_string(),
        ),
    ];

    let resp = ctx.http_call("GET", &gd_url, &system_headers, "")?;
    if resp.status != 200 {
        return Err(format!("GD query failed (HTTP {})", resp.status));
    }
    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({"value": []}));
    let gd = parsed
        .get("value")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first());
    let Some(gd) = gd else {
        return Err(format!(
            "notify_approval: no GovernanceDecision found for pending_decision_id={decision_id}"
        ));
    };

    let gd_id = gd
        .get("entity_id")
        .or_else(|| gd.get("fields").and_then(|f| f.get("Id")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if gd_id.is_empty() {
        return Err("GovernanceDecision has no entity_id".to_string());
    }

    // Dispatch RegisterCallback on the GovernanceDecision.
    let callback_url = format!(
        "{temper_api_url}/tdata/GovernanceDecisions('{gd_id}')/temper-system.RegisterCallback"
    );
    let callback_body = json!({
        "callback_tenant": tenant,
        "callback_entity_set": "Sessions",
        "callback_entity_id": session_id,
        "callback_on_approve": "ResumeAfterApproval",
        "callback_on_deny": "Fail"
    });
    let post_headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), "temper-system".to_string()),
        ("x-temper-principal-kind".to_string(), "admin".to_string()),
        (
            "x-temper-principal-id".to_string(),
            "request-approval-wasm".to_string(),
        ),
    ];

    let resp = ctx.http_call(
        "POST",
        &callback_url,
        &post_headers,
        &callback_body.to_string(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "RegisterCallback failed (HTTP {}): {}",
            resp.status,
            &resp.body[..resp.body.len().min(200)]
        ));
    }

    ctx.log(
        "info",
        &format!("notify_approval: registered callback on GD {gd_id} → Sessions('{session_id}')"),
    );
    Ok(())
}

fn fetch_pending_decision(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    decision_id: &str,
) -> Result<Option<Value>, String> {
    let url = format!("{temper_api_url}/api/tenants/{tenant}/decisions?status=pending");
    let headers = admin_json_headers(tenant, "request-approval-wasm");
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status == 404 {
        return Ok(None);
    }
    if resp.status != 200 {
        return Err(format!(
            "pending decision query failed (HTTP {}): {}",
            resp.status,
            &resp.body[..resp.body.len().min(200)]
        ));
    }
    let parsed: Value =
        serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({ "decisions": [] }));
    let decisions = parsed
        .get("decisions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(decisions.into_iter().find(|decision| {
        entity_field_str(decision, &["id", "Id"]).is_some_and(|id| id == decision_id)
    }))
}

fn admin_json_headers(tenant: &str, principal_id: &str) -> Vec<(String, String)> {
    vec![
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), tenant.to_string()),
        ("x-temper-principal-kind".to_string(), "admin".to_string()),
        (
            "x-temper-principal-id".to_string(),
            principal_id.to_string(),
        ),
    ]
}

fn format_approval_content(
    decision_id: &str,
    agent_id: &str,
    fallback_action: &str,
    tool_context: &Value,
    decision: Option<&Value>,
) -> String {
    let action = decision
        .and_then(|value| entity_field_str(value, &["action", "Action", "action_name"]))
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_action);
    let resource_type = decision
        .and_then(|value| entity_field_str(value, &["resource_type", "ResourceType"]))
        .unwrap_or("");
    let resource_id = decision
        .and_then(|value| entity_field_str(value, &["resource_id", "ResourceId"]))
        .unwrap_or("");
    let reason = decision
        .and_then(|value| entity_field_str(value, &["denial_reason", "DenialReason"]))
        .unwrap_or("");
    let module_name = decision
        .and_then(|value| entity_field_str(value, &["module_name", "ModuleName"]))
        .unwrap_or("");
    let session_id = decision
        .and_then(|value| entity_field_str(value, &["session_id", "SessionId"]))
        .unwrap_or("");

    let mut lines = vec![
        "**Permission Required**".to_string(),
        format!("Agent: `{agent_id}`"),
        format!("Action: `{action}`"),
    ];

    if !resource_type.is_empty() || !resource_id.is_empty() {
        lines.push(format!("Resource: `{resource_type}/{resource_id}`"));
    }
    if !module_name.is_empty() {
        lines.push(format!("Module: `{module_name}`"));
    }
    if !session_id.is_empty() {
        lines.push(format!("Session: `{session_id}`"));
    }
    if !reason.is_empty() {
        lines.push(format!("Reason: {reason}"));
    }

    let args_preview = format_tool_args_preview(tool_context);
    if !args_preview.is_empty() {
        lines.push(format!("Request: `{args_preview}`"));
    }

    lines.push(format!("Decision: `{decision_id}`"));
    lines.push(String::new());
    lines.push("Allow Always: this agent, this action, any resource of this type.".to_string());
    lines.push("Allow Session: same scope, limited to this session.".to_string());
    lines.push("Allow Once: this exact resource, limited to this session.".to_string());
    lines.push("Deny: reject this attempt.".to_string());

    lines.join("\n")
}

fn format_tool_args_preview(tool_context: &Value) -> String {
    let args = tool_context.get("args").unwrap_or(&Value::Null);
    if args.is_null() {
        return String::new();
    }
    let raw = if let Some(text) = args.as_str() {
        text.to_string()
    } else {
        args.to_string()
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "[]" || trimmed == "{}" {
        return String::new();
    }
    truncate_preview(trimmed, 240)
}

fn truncate_preview(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let boundary = value.floor_char_boundary(limit);
    format!("{}...", value[..boundary].trim_end())
}

fn find_session_by_agent(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Result<Option<(Value, String)>, String> {
    for candidate in
        channel_session_lookup_candidates(current_session_id, agent_id, parent_session_id)
    {
        let url = format!(
            "{temper_api_url}/tdata/ChannelSessions?{}",
            candidate.filter
        );
        if let Some(session) = list_entities(ctx, &url, tenant)?.into_iter().next() {
            return Ok(Some((session, candidate.bound_id)));
        }
    }

    Ok(None)
}

fn escape_odata(value: &str) -> String {
    value.replace('\'', "''")
}

struct ChannelSessionLookup {
    filter: String,
    bound_id: String,
}

fn channel_session_lookup_candidates(
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Vec<ChannelSessionLookup> {
    let mut candidates = Vec::new();
    let current_session_id = current_session_id.trim();
    let parent_session_id = parent_session_id.trim();
    let agent_id = agent_id.trim();

    if !current_session_id.is_empty() {
        let escaped = escape_odata(current_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: agent_id.to_string(),
        });
    }

    if !parent_session_id.is_empty() && parent_session_id != current_session_id {
        let escaped = escape_odata(parent_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: parent_session_id.to_string(),
        });
    }

    if !agent_id.is_empty() {
        let escaped = escape_odata(agent_id);
        candidates.push(ChannelSessionLookup {
            filter: format!("$filter=Status eq 'Active' and agent_entity_id eq '{escaped}'&$top=1"),
            bound_id: agent_id.to_string(),
        });
        candidates.push(ChannelSessionLookup {
            filter: format!("$filter=agent_entity_id eq '{escaped}'&$top=1"),
            bound_id: agent_id.to_string(),
        });
    }

    candidates
}

#[cfg(test)]
fn channel_session_lookup_filters(
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Vec<String> {
    channel_session_lookup_candidates(current_session_id, agent_id, parent_session_id)
        .into_iter()
        .map(|candidate| candidate.filter)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_session_lookup_prefers_current_session_then_parent_then_agent_binding() {
        let filters = channel_session_lookup_filters("ss-current", "aj-agent", "ss-parent");

        assert_eq!(
            filters,
            vec![
                "$filter=Status eq 'Active' and session_entity_id eq 'ss-current'&$top=1",
                "$filter=Status eq 'Active' and session_entity_id eq 'ss-parent'&$top=1",
                "$filter=Status eq 'Active' and agent_entity_id eq 'aj-agent'&$top=1",
                "$filter=agent_entity_id eq 'aj-agent'&$top=1",
            ]
        );
    }

    #[test]
    fn channel_session_lookup_deduplicates_resumed_parent_session() {
        let filters = channel_session_lookup_filters("ss-current", "aj-agent", "ss-current");

        assert_eq!(
            filters,
            vec![
                "$filter=Status eq 'Active' and session_entity_id eq 'ss-current'&$top=1",
                "$filter=Status eq 'Active' and agent_entity_id eq 'aj-agent'&$top=1",
                "$filter=agent_entity_id eq 'aj-agent'&$top=1",
            ]
        );
    }

    #[test]
    fn channel_session_lookup_escapes_odata_values() {
        let filters = channel_session_lookup_filters("ss'oops", "aj-agent", "");

        assert_eq!(
            filters[0],
            "$filter=Status eq 'Active' and session_entity_id eq 'ss''oops'&$top=1"
        );
    }

    #[test]
    fn approval_observability_fields_are_structured_and_do_not_include_message_body() {
        let fields = approval_observability_fields(ApprovalObservability {
            operation: "notify_human",
            outcome: "error",
            delivery: "failed",
            reason: "webhook_http_error",
            decision_id: "pd-123",
            session_id: "ss-456",
            agent_id: "aj-789",
            parent_session_id: "ss-parent",
            active_plan_id: "pl-111",
            bound_agent_id: "aj-bound",
            channel_id: "ch-slack",
            thread_id: "thread-1",
            action: "temper.write",
            http_status: Some(500),
        });

        assert_eq!(fields["observability_event"], "temperpaw.approval");
        assert_eq!(fields["decision_id"], "pd-123");
        assert_eq!(fields["session_id"], "ss-456");
        assert_eq!(fields["agent_id"], "aj-789");
        assert_eq!(fields["approval"]["operation"], "notify_human");
        assert_eq!(fields["approval"]["outcome"], "error");
        assert_eq!(fields["approval"]["delivery"], "failed");
        assert_eq!(fields["approval"]["http_status"], 500);
        assert!(
            !fields.to_string().contains("Permission Required"),
            "approval observability must not duplicate human notification bodies"
        );
    }
}
