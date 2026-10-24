use session_tree_lib::SessionTree;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    bounded_reads, create_content_file_ref, create_initial_session_entries, create_session_entry,
    is_session_entries_ref, runtime_headers, runtime_headers_for_workspace,
    session_id_from_entries_ref, timestamp_millis_string,
};

const DEFAULT_TOOLS_ENABLED: &str = "temper_create,temper_get,temper_list,temper_action,temper_patch,temper_submit_specs,temper_show_spec,temper_specs,temper_upload_wasm,temper_get_trajectories,temper_get_insights,temper_get_decisions,temper_poll_decision,temper_approve_decision,temper_deny_decision,temper_submit_policy,temper_list_policies,temper_get_policy,temper_update_policy,temper_delete_policy,temper_search_apps,temper_install_app,temper_publish_app,temper_update_app,temper_list_apps,temper_spawn_session,temper_list_sessions,temper_abort_session,temper_steer_session,temper_save_memory,temper_recall_memory,temper_write,temper_read,temper_run_coding_agent,temper_get_secret,temper_datadog_query,temper_railway,temper_vercel,temper_web_search,temper_web_fetch,temper_image_generate,read,write,edit,bash";
const PLAN_MODE_TOOLS: &str = "temper_create,temper_get,temper_list,temper_action,temper_specs,temper_show_spec,temper_save_memory,temper_recall_memory,temper_read,temper_write,temper_web_search,temper_web_fetch,temper_get_trajectories,temper_get_insights,read,bash";
const DEFAULT_WORKDIR: &str = "/workspace";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContinuationPreparedContextStorage {
    file_id: String,
    inline_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TraceContextFields {
    trace_id: String,
    span_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeliveryRouteSnapshot {
    channel_id: String,
    thread_id: String,
    channel_entity_id: Option<String>,
    channel_type: Option<String>,
    source: String,
}

impl AsRef<TraceContextFields> for TraceContextFields {
    fn as_ref(&self) -> &TraceContextFields {
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_get_context(_buf_ptr: i32, _buf_len: i32) -> i32 {
    -1
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_http_call(
    _method_ptr: i32,
    _method_len: i32,
    _url_ptr: i32,
    _url_len: i32,
    _headers_ptr: i32,
    _headers_len: i32,
    _body_ptr: i32,
    _body_len: i32,
    _result_buf_ptr: i32,
    _result_buf_len: i32,
) -> i32 {
    -1
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_http_call_batch(
    _requests_ptr: i32,
    _requests_len: i32,
    _result_buf_ptr: i32,
    _result_buf_len: i32,
) -> i32 {
    -1
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_log(_level_ptr: i32, _level_len: i32, _msg_ptr: i32, _msg_len: i32) {}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_set_result(_ptr: i32, _len: i32) {}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_get_time_millis() -> i64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let channel_id = str_field(&fields, &["channel_id", "ChannelId"]).unwrap_or("");
        let channel_type = str_field(&fields, &["channel_type", "ChannelType"]).unwrap_or("");
        let default_agent_config =
            str_field(&fields, &["default_agent_config", "DefaultAgentConfig"]).unwrap_or("{}");
        let thread_id = str_field(&fields, &["thread_id", "ThreadId"]).unwrap_or("");
        let author_id = str_field(&fields, &["author_id", "AuthorId"]).unwrap_or("");
        let content = str_field(&fields, &["content", "Content"]).unwrap_or("");
        // Read command from trigger_params (the current ReceiveMessage call), NOT
        // from entity_state.fields which persists the value from prior calls.
        // Regular messages don't include "command", so it must default to "" —
        // reading from entity state would re-use a stale "/plan" command forever.
        let command = ctx
            .trigger_params
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let trace_context = trace_context_from_trigger_params(&ctx.trigger_params);
        if channel_id.is_empty() || author_id.is_empty() {
            return Err("route_message: missing channel_id/author_id".to_string());
        }
        // For DMs without threads, default thread_id to channel_id
        let thread_id = if thread_id.is_empty() {
            channel_id
        } else {
            thread_id
        };
        let delivery_route = delivery_route_snapshot_from_channel_message(
            channel_id,
            thread_id,
            &ctx.entity_id,
            channel_type,
        );

        let existing_cs = find_active_session(
            &ctx,
            &temper_api_url,
            &ctx.tenant,
            channel_id,
            thread_id,
            author_id,
        )?;

        // /reset command: cancel active session, expire ChannelSession, start fresh (ADR-0025)
        if command == "reset" {
            let route = find_route(&ctx, &temper_api_url, &ctx.tenant, channel_id)?;
            let route_config = route
                .as_ref()
                .and_then(|value| nested_str_field(value, &["AgentConfig", "agent_config"]))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default_agent_config);
            let route_agent_id = route
                .as_ref()
                .and_then(|value| nested_str_field(value, &["AgentId", "agent_id"]))
                .unwrap_or("");

            // Clean up existing session/channel-session
            if let Some(ref cs) = existing_cs {
                let cs_id = cs
                    .get("entity_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| nested_str_field(cs, &["Id", "entity_id"]))
                    .unwrap_or_default();
                let session_entity_id =
                    nested_str_field(cs, &["SessionEntityId", "session_entity_id"])
                        .unwrap_or_default();
                if !session_entity_id.is_empty() {
                    cancel_session(&ctx, &temper_api_url, &ctx.tenant, session_entity_id).ok();
                }
                expire_session(&ctx, &temper_api_url, &ctx.tenant, cs_id).ok();
            }

            // Create fresh session with no inherited conversation tree
            let user_msg = if content.is_empty() {
                "[System: The user has reset the conversation. Start fresh.]"
            } else {
                content
            };
            let (new_agent_id, new_session_id) = create_session_for_agent(
                &ctx,
                &temper_api_url,
                &ctx.tenant,
                route_config,
                route_agent_id,
                user_msg,
                "",
                Some(&delivery_route),
                trace_context.as_ref(),
            )?;
            create_channel_session(
                &ctx,
                &temper_api_url,
                &ctx.tenant,
                channel_id,
                thread_id,
                author_id,
                &new_agent_id,
                &new_session_id,
            )?;
            set_success_result("MessageRouted", &json!({ "session_id": new_session_id }));
            return Ok(());
        }

        let session_id = if let Some(cs) = existing_cs {
            let cs_id = cs
                .get("entity_id")
                .and_then(|v| v.as_str())
                .or_else(|| nested_str_field(&cs, &["Id", "entity_id"]))
                .unwrap_or_default()
                .to_string();
            let agent_entity_id = nested_str_field(&cs, &["AgentEntityId", "agent_entity_id"])
                .unwrap_or_default()
                .to_string();
            let session_entity_id =
                nested_str_field(&cs, &["SessionEntityId", "session_entity_id"])
                    .unwrap_or_default()
                    .to_string();

            if !session_entity_id.is_empty() {
                // Fetch the current Session entity to check its status
                let session = match fetch_entity(
                    &ctx,
                    &session_entity_url(&temper_api_url, &session_entity_id),
                    &ctx.tenant,
                ) {
                    Ok(session) => session,
                    Err(err) => {
                        ctx.log(
                            "warn",
                            &format!(
                                "route_message: ChannelSession {cs_id} points at unreadable Session {session_entity_id}: {err}; starting a fresh session"
                            ),
                        );
                        let route = find_route(&ctx, &temper_api_url, &ctx.tenant, channel_id)?;
                        let route_config = route
                            .as_ref()
                            .and_then(|value| {
                                nested_str_field(value, &["AgentConfig", "agent_config"])
                            })
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or(default_agent_config);
                        let route_agent_id = route
                            .as_ref()
                            .and_then(|value| nested_str_field(value, &["AgentId", "agent_id"]))
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or(&agent_entity_id);
                        expire_session(&ctx, &temper_api_url, &ctx.tenant, &cs_id).ok();
                        let (new_agent_id, new_session_id) = create_session_for_agent(
                            &ctx,
                            &temper_api_url,
                            &ctx.tenant,
                            route_config,
                            route_agent_id,
                            content,
                            command,
                            Some(&delivery_route),
                            trace_context.as_ref(),
                        )?;
                        create_channel_session(
                            &ctx,
                            &temper_api_url,
                            &ctx.tenant,
                            channel_id,
                            thread_id,
                            author_id,
                            &new_agent_id,
                            &new_session_id,
                        )?;
                        ctx.log(
                            "info",
                            &format!(
                                "route_message: routed thread {thread_id} to fresh session {new_session_id} after stale binding"
                            ),
                        );
                        set_success_result(
                            "",
                            &json!({
                                "status": "routed",
                                "thread_id": thread_id,
                                "agent_entity_id": new_session_id,
                            }),
                        );
                        return Ok(());
                    }
                };
                let session_status =
                    nested_str_field(&session, &["Status", "status"]).unwrap_or("");

                if is_steerable_status(session_status) {
                    resume_session(&ctx, &temper_api_url, &ctx.tenant, &cs_id).ok();
                    // ADR-0039 Sub-Decision 1: if the Session hasn't made
                    // forward progress recently, dispatch the state-specific
                    // Resume* action to wake its driving integration. Cheap
                    // no-op for actively progressing sessions (is_session_stale
                    // returns false when last_progress_at is fresh).
                    if is_session_stale(
                        &session,
                        (timestamp_millis_string().parse::<i64>().unwrap_or(0)) / 1000,
                        RESUME_STALENESS_THRESHOLD_SECS,
                    ) && let Some(action_name) = resume_action_for_status(session_status)
                    {
                        if let Err(e) = wake_session(
                            &ctx,
                            &temper_api_url,
                            &ctx.tenant,
                            &session_entity_id,
                            action_name,
                        ) {
                            ctx.log(
                                "warn",
                                &format!(
                                    "route_message: wake-up failed for session {session_entity_id} (status={session_status}, action={action_name}): {e}"
                                ),
                            );
                        } else {
                            ctx.log(
                                "info",
                                &format!(
                                    "route_message: dispatched {action_name} on stale session {session_entity_id} (status={session_status})"
                                ),
                            );
                        }
                    }
                    if !command.is_empty() {
                        // Slash command: switch mode first, then steer
                        switch_mode_and_steer(
                            &ctx,
                            &temper_api_url,
                            &ctx.tenant,
                            &session_entity_id,
                            command,
                            content,
                        )?;
                        session_entity_id
                    } else if steer_session(
                        &ctx,
                        &temper_api_url,
                        &ctx.tenant,
                        &session_entity_id,
                        content,
                    )
                    .is_ok()
                    {
                        session_entity_id
                    } else {
                        // Steer failed — create a new session under the same agent
                        continue_with_new_session(
                            &ctx,
                            &temper_api_url,
                            &ctx.tenant,
                            &cs,
                            &cs_id,
                            &agent_entity_id,
                            &session,
                            &session_entity_id,
                            content,
                            command,
                            Some(&delivery_route),
                            trace_context.as_ref(),
                        )?
                    }
                } else if should_continue_with_new_session_for_status(session_status) {
                    // Terminal sessions are done. WaitingForApproval sessions are
                    // human-gated, but a fresh DM should interrupt that blocked
                    // turn so the channel thread stays responsive.
                    if session_status == "WaitingForApproval" {
                        cancel_session(&ctx, &temper_api_url, &ctx.tenant, &session_entity_id).ok();
                    }
                    continue_with_new_session(
                        &ctx,
                        &temper_api_url,
                        &ctx.tenant,
                        &cs,
                        &cs_id,
                        &agent_entity_id,
                        &session,
                        &session_entity_id,
                        content,
                        command,
                        Some(&delivery_route),
                        trace_context.as_ref(),
                    )?
                } else {
                    // Non-steerable, non-terminal bootstrap/recovery states.
                    // Just wait — don't expire or create a new session
                    session_entity_id
                }
            } else {
                // ChannelSession exists but has no session_entity_id — route fresh
                let route = find_route(&ctx, &temper_api_url, &ctx.tenant, channel_id)?;
                let route_config = route
                    .as_ref()
                    .and_then(|value| nested_str_field(value, &["AgentConfig", "agent_config"]))
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(default_agent_config);
                let route_agent_id = route
                    .as_ref()
                    .and_then(|value| nested_str_field(value, &["AgentId", "agent_id"]))
                    .unwrap_or("");
                expire_session(&ctx, &temper_api_url, &ctx.tenant, &cs_id).ok();
                let (new_agent_id, new_session_id) = create_session_for_agent(
                    &ctx,
                    &temper_api_url,
                    &ctx.tenant,
                    route_config,
                    route_agent_id,
                    content,
                    command,
                    Some(&delivery_route),
                    trace_context.as_ref(),
                )?;
                create_channel_session(
                    &ctx,
                    &temper_api_url,
                    &ctx.tenant,
                    channel_id,
                    thread_id,
                    author_id,
                    &new_agent_id,
                    &new_session_id,
                )?;
                new_session_id
            }
        } else {
            // No existing ChannelSession — route from scratch
            let route = find_route(&ctx, &temper_api_url, &ctx.tenant, channel_id)?;
            let route_config = route
                .as_ref()
                .and_then(|value| nested_str_field(value, &["AgentConfig", "agent_config"]))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default_agent_config);
            let route_agent_id = route
                .as_ref()
                .and_then(|value| nested_str_field(value, &["AgentId", "agent_id"]))
                .unwrap_or("");
            let (agent_id, new_session_id) = create_session_for_agent(
                &ctx,
                &temper_api_url,
                &ctx.tenant,
                route_config,
                route_agent_id,
                content,
                command,
                Some(&delivery_route),
                trace_context.as_ref(),
            )?;
            create_channel_session(
                &ctx,
                &temper_api_url,
                &ctx.tenant,
                channel_id,
                thread_id,
                author_id,
                &agent_id,
                &new_session_id,
            )?;
            new_session_id
        };

        ctx.log(
            "info",
            &format!("route_message: routed thread {thread_id} to session {session_id}"),
        );
        set_success_result(
            "",
            &json!({
                "status": "routed",
                "thread_id": thread_id,
                "agent_entity_id": session_id,
            }),
        );
        Ok(())
    })();

    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

fn resolve_temper_api_url(ctx: &Context, fields: &Value) -> String {
    fields
        .get("temper_api_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            ctx.config
                .get("temper_api_url")
                .filter(|s| !s.is_empty())
                .cloned()
        })
        .unwrap_or_else(|| "http://127.0.0.1:3467".to_string())
}

fn odata_headers(ctx: &Context, tenant: &str) -> Vec<(String, String)> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    runtime_headers(
        ctx,
        tenant,
        &fields,
        Some("application/json"),
        Some("application/json"),
    )
}

fn file_headers(
    ctx: &Context,
    tenant: &str,
    workspace_id: &str,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    if workspace_id.is_empty() {
        runtime_headers(ctx, tenant, &json!({}), content_type, accept)
    } else {
        runtime_headers_for_workspace(ctx, tenant, &json!({}), workspace_id, content_type, accept)
    }
}

fn trace_context_from_trigger_params(trigger_params: &Value) -> Option<TraceContextFields> {
    let trace_id = trigger_params
        .get("gen_ai_parent_trace_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let span_id = trigger_params
        .get("gen_ai_parent_span_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;

    Some(TraceContextFields {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
    })
}

fn apply_trace_context(configure_body: &mut Value, trace_context: &TraceContextFields) {
    let Some(object) = configure_body.as_object_mut() else {
        return;
    };

    object.insert(
        "gen_ai_parent_trace_id".into(),
        json!(trace_context.trace_id.clone()),
    );
    object.insert(
        "gen_ai_parent_span_id".into(),
        json!(trace_context.span_id.clone()),
    );
}

fn delivery_route_snapshot_from_channel_message(
    channel_id: &str,
    thread_id: &str,
    channel_entity_id: &str,
    channel_type: &str,
) -> DeliveryRouteSnapshot {
    DeliveryRouteSnapshot {
        channel_id: channel_id.to_string(),
        thread_id: thread_id.to_string(),
        channel_entity_id: if channel_entity_id.trim().is_empty() {
            None
        } else {
            Some(channel_entity_id.to_string())
        },
        channel_type: if channel_type.trim().is_empty() {
            None
        } else {
            Some(channel_type.to_string())
        },
        source: "channel_message".to_string(),
    }
}

#[cfg(test)]
fn delivery_route_snapshot_from_channel_session(value: &Value) -> Option<DeliveryRouteSnapshot> {
    let channel_id = nested_str_field(value, &["ChannelId", "channel_id"])
        .filter(|value| !value.trim().is_empty())?;
    let thread_id = nested_str_field(value, &["ThreadId", "thread_id"])
        .filter(|value| !value.trim().is_empty())?;
    Some(DeliveryRouteSnapshot {
        channel_id: channel_id.to_string(),
        thread_id: thread_id.to_string(),
        channel_entity_id: None,
        channel_type: None,
        source: "channel_session".to_string(),
    })
}

fn apply_delivery_route_snapshot(configure_body: &mut Value, route: &DeliveryRouteSnapshot) {
    let Some(object) = configure_body.as_object_mut() else {
        return;
    };
    object.insert("reply_channel_id".into(), json!(route.channel_id.as_str()));
    object.insert("reply_thread_id".into(), json!(route.thread_id.as_str()));
    object.insert("reply_route_source".into(), json!(route.source.as_str()));
    if let Some(channel_entity_id) = &route.channel_entity_id {
        object.insert("reply_channel_entity_id".into(), json!(channel_entity_id));
    }
    if let Some(channel_type) = &route.channel_type {
        object.insert("reply_channel_type".into(), json!(channel_type));
    }
}

fn list_entities(ctx: &Context, url: &str, tenant: &str) -> Result<Vec<Value>, String> {
    let resp = ctx.http_call("GET", url, &odata_headers(ctx, tenant), "")?;
    if resp.status != 200 {
        return Err(format!("GET {url} failed (HTTP {})", resp.status));
    }
    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({ "value": [] }));
    Ok(parsed
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn escape_odata_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn active_session_query(channel_id: &str, thread_id: &str, author_id: &str) -> String {
    let channel_id = escape_odata_literal(channel_id);
    let thread_id = escape_odata_literal(thread_id);
    let author_id = escape_odata_literal(author_id);
    format!(
        "$filter=Status eq 'Active' and channel_id eq '{channel_id}' and thread_id eq '{thread_id}' and author_id eq '{author_id}'&$top=1"
    )
}

fn find_active_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    channel_id: &str,
    thread_id: &str,
    author_id: &str,
) -> Result<Option<Value>, String> {
    let query = active_session_query(channel_id, thread_id, author_id);
    let sessions = list_entities(
        ctx,
        &format!("{temper_api_url}/tdata/ChannelSessions?{query}"),
        tenant,
    )?;
    Ok(sessions.into_iter().next())
}

fn resume_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/ChannelSessions('{session_id}')/Paw.Channel.Resume");
    let body = json!({ "last_message_at": timestamp_millis_string() });
    let _ = ctx.http_call("POST", &url, &odata_headers(ctx, tenant), &body.to_string())?;
    Ok(())
}

fn expire_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/ChannelSessions('{session_id}')/Paw.Channel.Expire");
    let _ = ctx.http_call("POST", &url, &odata_headers(ctx, tenant), "{}")?;
    Ok(())
}

fn cancel_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/Sessions('{session_id}')/TemperPaw.Cancel");
    let _ = ctx.http_call("POST", &url, &odata_headers(ctx, tenant), "{}")?;
    Ok(())
}

fn find_route(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    channel_id: &str,
) -> Result<Option<Value>, String> {
    for query in agent_route_queries(channel_id) {
        let routes = list_entities(
            ctx,
            &format!("{temper_api_url}/tdata/AgentRoutes?{query}"),
            tenant,
        )?;
        if let Some(route) = routes.into_iter().next() {
            return Ok(Some(route));
        }
    }
    Ok(None)
}

fn agent_route_queries(channel_id: &str) -> Vec<String> {
    let channel_id = escape_odata_literal(channel_id);
    vec![
        format!("$filter=Status eq 'Active' and channel_id eq '{channel_id}'&$top=1"),
        "$filter=Status eq 'Active' and channel_id eq ''&$top=1".to_string(),
    ]
}

/// Fetch the persistent Agent entity and extract its configuration.
fn fetch_agent_config(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<Value, String> {
    let url = format!("{temper_api_url}/tdata/Agents('{agent_id}')");
    fetch_entity(ctx, &url, tenant)
}

/// Create a new Session for a persistent Agent.
///
/// If `route_agent_id` is non-empty, fetches the Agent entity and uses its
/// config (soul_id, model, provider, tools_enabled, max_turns).  Otherwise
/// falls back to `route_config` JSON for backward-compatible ad-hoc routing.
///
/// Returns `(agent_entity_id, session_entity_id)`.
fn create_session_for_agent(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    route_config: &str,
    route_agent_id: &str,
    user_message: &str,
    command: &str,
    delivery_route: Option<&DeliveryRouteSnapshot>,
    trace_context: Option<&TraceContextFields>,
) -> Result<(String, String), String> {
    let config: Value = serde_json::from_str(route_config).unwrap_or_else(|_| json!({}));

    // If route points to a persistent Agent, fetch its config
    let (
        agent_id,
        agent_soul_id,
        agent_model,
        agent_provider,
        agent_provider_options_json,
        agent_tools,
        agent_max_turns,
    ) = if !route_agent_id.is_empty() {
            let agent = fetch_agent_config(ctx, temper_api_url, tenant, route_agent_id)?;
            let soul_id = nested_str_field(&agent, &["SoulId", "soul_id"])
                .unwrap_or("")
                .to_string();
            let model = nested_str_field(&agent, &["Model", "model"])
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("Agent {route_agent_id} has no configured model"))?
                .to_string();
            let provider = nested_str_field(&agent, &["Provider", "provider"])
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("Agent {route_agent_id} has no configured provider"))?
                .to_string();
            let tools = nested_str_field(&agent, &["ToolsEnabled", "tools_enabled"])
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_TOOLS_ENABLED)
                .to_string();
            let max_turns = nested_str_field(&agent, &["MaxTurns", "max_turns"])
                .unwrap_or("200")
                .to_string();
            let provider_options_json = config
                .get("provider_options_json")
                .or_else(|| config.get("providerOptionsJson"))
                .and_then(Value::as_str)
                .or_else(|| nested_str_field(&agent, &["ProviderOptionsJson", "provider_options_json"]))
                .unwrap_or("")
                .to_string();
            (
                route_agent_id.to_string(),
                soul_id,
                model,
                provider,
                provider_options_json,
                tools,
                max_turns,
            )
        } else {
            // Ad-hoc routing: no persistent Agent, use route_config JSON
            let soul_id = config
                .get("soul_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let model = config
                .get("model")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or("AgentRoute config requires model when agent_id is not set")?
                .to_string();
            let provider = config
                .get("provider")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or("AgentRoute config requires provider when agent_id is not set")?
                .to_string();
            let tools = config
                .get("tools_enabled")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_TOOLS_ENABLED)
                .to_string();
            let max_turns = config
                .get("max_turns")
                .and_then(Value::as_str)
                .unwrap_or("200")
                .to_string();
            let provider_options_json = config
                .get("provider_options_json")
                .or_else(|| config.get("providerOptionsJson"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (
                String::new(),
                soul_id,
                model,
                provider,
                provider_options_json,
                tools,
                max_turns,
            )
        };

    // Create blank Session entity
    let session_id = create_blank_session(ctx, temper_api_url, tenant)?;

    // Determine mode-specific tools and session_mode
    let (session_mode, tools_enabled, pre_plan_tools) = if command == "plan" {
        ("plan", PLAN_MODE_TOOLS.to_string(), agent_tools.clone())
    } else {
        ("execute", agent_tools.clone(), String::new())
    };

    let mut configure_body = json!({
        "system_prompt": config.get("system_prompt").and_then(Value::as_str).unwrap_or(""),
        "user_message": user_message,
        "model": agent_model,
        "provider": agent_provider,
        "provider_options_json": agent_provider_options_json,
        "tools_enabled": tools_enabled,
        "max_turns": agent_max_turns,
        "workdir": config.get("workdir").and_then(Value::as_str).unwrap_or(DEFAULT_WORKDIR),
        "sandbox_url": config.get("sandbox_url").and_then(Value::as_str).unwrap_or(""),
        "temper_api_url": config.get("temper_api_url").and_then(Value::as_str).unwrap_or(""),
        "soul_id": agent_soul_id,
        "agent_id": agent_id,
        "parent_session_id": config.get("parent_session_id").and_then(Value::as_str).unwrap_or(""),
        "session_depth": config.get("session_depth").and_then(Value::as_str).unwrap_or("0"),
        "max_follow_ups": config.get("max_follow_ups").and_then(Value::as_str).unwrap_or("5"),
        "hook_policy": config.get("hook_policy").and_then(Value::as_str).unwrap_or("none"),
        "reserve_tokens": config.get("reserve_tokens").and_then(Value::as_str).unwrap_or("20000"),
        "keep_recent_tokens": config.get("keep_recent_tokens").and_then(Value::as_str).unwrap_or("10000"),
        "compaction_model": config.get("compaction_model").and_then(Value::as_str).unwrap_or(""),
        "heartbeat_timeout_seconds": config.get("heartbeat_timeout_seconds").and_then(Value::as_str).unwrap_or("300"),
        "project_harness_id": config.get("project_harness_id").and_then(Value::as_str).unwrap_or(""),
        "project_id": config.get("project_id").and_then(Value::as_str).unwrap_or(""),
        "session_mode": session_mode,
        "pre_plan_tools_enabled": pre_plan_tools,
    });
    if let Some(trace_context) = trace_context {
        apply_trace_context(&mut configure_body, trace_context);
    }
    if let Some(delivery_route) = delivery_route {
        apply_delivery_route_snapshot(&mut configure_body, delivery_route);
    }
    let configure_url =
        format!("{temper_api_url}/tdata/Sessions('{session_id}')/TemperPaw.Configure");
    ctx.log(
        "info",
        &format!(
            "route_message: creating session {session_id} for agent {agent_id} via {configure_url}"
        ),
    );
    let configure_resp = ctx.http_call(
        "POST",
        &configure_url,
        &odata_headers(ctx, tenant),
        &configure_body.to_string(),
    )?;
    if !(200..300).contains(&configure_resp.status) {
        return Err(format!(
            "configure Session failed (HTTP {}): {}",
            configure_resp.status,
            truncate_error_body(&configure_resp.body)
        ));
    }

    // Provision is auto-scheduled by the Configure action's spec effect
    // (session.ioa.toml: effect = [{ type = "schedule", action = "Provision", delay_seconds = 0 }]).
    // No explicit Provision call needed — it would race with the scheduled one.
    Ok((agent_id, session_id))
}

/// Create a new Session when the prior Session has ended (terminal) or steer failed.
///
/// Reads the Agent ID from the ChannelSession (persistent Agent), fetches
/// the Agent entity for soul_id/config, carries forward session tree state
/// from the prior Session, and updates the ChannelSession to point to the
/// new Session via UpdateSession.
fn continue_with_new_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    _channel_session: &Value,
    cs_id: &str,
    agent_entity_id: &str,
    prior_session: &Value,
    prior_session_id: &str,
    user_message: &str,
    command: &str,
    delivery_route: Option<&DeliveryRouteSnapshot>,
    trace_context: Option<&TraceContextFields>,
) -> Result<String, String> {
    let fields = prior_session
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let session_file_id = str_field(&fields, &["session_file_id", "SessionFileId"]).unwrap_or("");
    let conversation_file_id =
        str_field(&fields, &["conversation_file_id", "ConversationFileId"]).unwrap_or("");
    let prior_leaf_id = str_field(&fields, &["session_leaf_id", "SessionLeafId"]).unwrap_or("");
    let workspace_id = resolve_workspace_id_for_session(
        ctx,
        temper_api_url,
        tenant,
        &fields,
        session_file_id,
        conversation_file_id,
    )?;

    let mut carry_prior_artifacts = true;
    if should_start_clean_continuation_from_prior(&fields) {
        carry_prior_artifacts = false;
        ctx.log(
            "warn",
            &format!(
                "route_message: starting clean continuation after prior session {prior_session_id} failed in context/compaction path"
            ),
        );
    }

    let new_leaf_id = if carry_prior_artifacts && !session_file_id.is_empty() {
        match append_user_message_to_session(
            ctx,
            temper_api_url,
            tenant,
            &fields,
            &workspace_id,
            session_file_id,
            prior_leaf_id,
            user_message,
        ) {
            Ok(leaf_id) => Some(leaf_id),
            Err(err) if should_start_fresh_after_session_append_failure(&err) => {
                carry_prior_artifacts = false;
                ctx.log(
                    "warn",
                    &format!(
                        "route_message: starting clean continuation after session tree append failed for prior session {prior_session_id}: {err}"
                    ),
                );
                None
            }
            Err(err) => return Err(err),
        }
    } else {
        None
    };

    if carry_prior_artifacts && new_leaf_id.is_none() && !conversation_file_id.is_empty() {
        append_user_message_to_conversation(
            ctx,
            temper_api_url,
            tenant,
            &workspace_id,
            conversation_file_id,
            user_message,
        )?;
    }

    // Prepend a session-continuation notice so the agent naturally acknowledges the new session.
    let continuation_notice = if carry_prior_artifacts {
        "A new session has started. Your previous conversation context and memories are preserved."
    } else {
        "A new session has started. The prior conversation tree was unhealthy, so continue from the user's latest message and ask for any missing context."
    };
    let user_message_with_notice = format!("[System: {continuation_notice}]\n\n{user_message}");

    let new_session_id = create_blank_session(ctx, temper_api_url, tenant)?;

    // Fetch Agent entity to get soul_id and config (if we have a persistent Agent)
    let (
        soul_id,
        agent_model,
        agent_provider,
        agent_provider_options_json,
        agent_tools,
        agent_max_turns,
    ) =
        if !agent_entity_id.is_empty() {
            match fetch_agent_config(ctx, temper_api_url, tenant, agent_entity_id) {
                Ok(agent) => {
                    let sid = nested_str_field(&agent, &["SoulId", "soul_id"])
                        .unwrap_or("")
                        .to_string();
                    let model = nested_str_field(&agent, &["Model", "model"])
                        .unwrap_or("")
                        .to_string();
                    let provider = nested_str_field(&agent, &["Provider", "provider"])
                        .unwrap_or("")
                        .to_string();
                    let provider_options_json =
                        nested_str_field(&agent, &["ProviderOptionsJson", "provider_options_json"])
                            .unwrap_or("")
                            .to_string();
                    let tools = nested_str_field(&agent, &["ToolsEnabled", "tools_enabled"])
                        .unwrap_or("")
                        .to_string();
                    let max_turns = nested_str_field(&agent, &["MaxTurns", "max_turns"])
                        .unwrap_or("")
                        .to_string();
                    (sid, model, provider, provider_options_json, tools, max_turns)
                }
                Err(_) => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ),
            }
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };

    // Use Agent entity values when available, otherwise fall back to prior session fields
    let effective_soul_id = if !soul_id.is_empty() {
        &soul_id
    } else {
        str_field(&fields, &["soul_id", "SoulId"]).unwrap_or("")
    };
    let effective_model = if !agent_model.is_empty() {
        &agent_model
    } else {
        str_field(&fields, &["model", "Model"])
            .filter(|value| !value.trim().is_empty())
            .ok_or("route_message requires a configured model from AgentRoute, Agent, or prior Session")?
    };
    let effective_provider = if !agent_provider.is_empty() {
        &agent_provider
    } else {
        str_field(&fields, &["provider", "Provider"])
            .filter(|value| !value.trim().is_empty())
            .ok_or("route_message requires a configured provider from AgentRoute, Agent, or prior Session")?
    };
    let effective_provider_options_json = if !agent_provider_options_json.is_empty() {
        agent_provider_options_json.as_str()
    } else {
        str_field(&fields, &["provider_options_json", "ProviderOptionsJson"]).unwrap_or("")
    };
    let effective_tools = if !agent_tools.is_empty() {
        &agent_tools
    } else {
        // If prior session was in plan mode, its tools_enabled is the restricted
        // PLAN_MODE_TOOLS set. Use the stashed pre_plan_tools_enabled instead so
        // the continuation session starts with the full tool set.
        let prior_mode = str_field(&fields, &["session_mode", "SessionMode"]).unwrap_or("execute");
        if prior_mode == "plan" {
            str_field(&fields, &["pre_plan_tools_enabled", "PrePlanToolsEnabled"])
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_TOOLS_ENABLED)
        } else {
            str_field(&fields, &["tools_enabled", "ToolsEnabled"]).unwrap_or(DEFAULT_TOOLS_ENABLED)
        }
    };
    let effective_max_turns = if !agent_max_turns.is_empty() {
        &agent_max_turns
    } else {
        str_field(&fields, &["max_turns", "MaxTurns"]).unwrap_or("200")
    };

    configure_session_from_prior(
        ctx,
        temper_api_url,
        tenant,
        &new_session_id,
        &fields,
        &user_message_with_notice,
        prior_session_id,
        agent_entity_id,
        effective_soul_id,
        effective_model,
        effective_provider,
        effective_provider_options_json,
        effective_tools,
        effective_max_turns,
        &workspace_id,
        if carry_prior_artifacts {
            new_leaf_id.as_deref().unwrap_or(prior_leaf_id)
        } else {
            ""
        },
        carry_prior_artifacts,
        command,
        delivery_route,
        trace_context,
    )?;

    // Update ChannelSession: agent_entity_id stays the same, only session_entity_id changes
    update_session_binding(ctx, temper_api_url, tenant, cs_id, &new_session_id)?;
    Ok(new_session_id)
}

fn create_blank_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
) -> Result<String, String> {
    let create_body = "{ }".to_string();
    ctx.log(
        "info",
        &format!(
            "route_message: creating session via {temper_api_url}/tdata/Sessions with {} bytes",
            create_body.len()
        ),
    );
    let create_resp = ctx.http_call(
        "POST",
        &format!("{temper_api_url}/tdata/Sessions"),
        &odata_headers(ctx, tenant),
        &create_body,
    )?;
    if !(200..300).contains(&create_resp.status) {
        return Err(format!(
            "create Session failed via {temper_api_url} (HTTP {}): {}",
            create_resp.status,
            truncate_error_body(&create_resp.body)
        ));
    }
    let parsed: Value = serde_json::from_str(&create_resp.body).unwrap_or_else(|_| json!({}));
    let session_id = parsed
        .get("entity_id")
        .or_else(|| parsed.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() {
        return Err("created Session missing entity_id".to_string());
    }
    Ok(session_id)
}

fn configure_session_from_prior(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
    fields: &Value,
    user_message: &str,
    prior_session_id: &str,
    agent_entity_id: &str,
    soul_id: &str,
    model: &str,
    provider: &str,
    provider_options_json: &str,
    tools_enabled: &str,
    max_turns: &str,
    workspace_id: &str,
    session_leaf_id: &str,
    carry_prior_artifacts: bool,
    command: &str,
    delivery_route: Option<&DeliveryRouteSnapshot>,
    trace_context: Option<&TraceContextFields>,
) -> Result<(), String> {
    // Determine mode-specific tools and session_mode
    let (session_mode, effective_tools, pre_plan_tools) = if command == "plan" {
        ("plan", PLAN_MODE_TOOLS, tools_enabled.to_string())
    } else {
        ("execute", tools_enabled, String::new())
    };

    // Include resume-specific fields (workspace, conversation, session tree) in Configure
    // so they're stored as session fields before auto-Provision fires. The provision_sandbox
    // integration checks these to decide whether to restore an existing workspace or provision new.
    let effective_workspace_id = if !carry_prior_artifacts {
        ""
    } else if workspace_id.is_empty() {
        str_field(fields, &["workspace_id", "WorkspaceId"]).unwrap_or("")
    } else {
        workspace_id
    };
    let prepared_context_storage =
        continuation_prepared_context_storage(carry_prior_artifacts, fields);

    let mut configure_body = json!({
        "system_prompt": str_field(fields, &["system_prompt", "SystemPrompt"]).unwrap_or(""),
        "user_message": user_message,
        "model": model,
        "provider": provider,
        "provider_options_json": provider_options_json,
        "tools_enabled": effective_tools,
        "max_turns": max_turns,
        "workdir": str_field(fields, &["workdir", "Workdir"]).unwrap_or(DEFAULT_WORKDIR),
        // Don't carry forward sandbox_url/sandbox_id from the prior session —
        // Tensorlake sandboxes expire and stale URLs cause infinite retry loops.
        // Let provision_sandbox create a fresh sandbox for each continuation.
        "sandbox_url": "",
        "sandbox_id": "",
        "temper_api_url": str_field(fields, &["temper_api_url", "TemperApiUrl"]).unwrap_or(""),
        "soul_id": soul_id,
        "agent_id": agent_entity_id,
        "parent_session_id": prior_session_id,
        "session_depth": str_field(fields, &["session_depth", "SessionDepth"]).unwrap_or("0"),
        "max_follow_ups": str_field(fields, &["max_follow_ups", "MaxFollowUps"]).unwrap_or("5"),
        "hook_policy": str_field(fields, &["hook_policy", "HookPolicy"]).unwrap_or("none"),
        "reserve_tokens": str_field(fields, &["reserve_tokens", "ReserveTokens"]).unwrap_or("20000"),
        "keep_recent_tokens": str_field(fields, &["keep_recent_tokens", "KeepRecentTokens"]).unwrap_or("10000"),
        "compaction_model": str_field(fields, &["compaction_model", "CompactionModel"]).unwrap_or(""),
        "heartbeat_timeout_seconds": str_field(fields, &["heartbeat_timeout_seconds", "HeartbeatTimeoutSeconds"]).unwrap_or("300"),
        // Resume fields — folded into Configure so auto-Provision can restore prior state.
        "workspace_id": effective_workspace_id,
        "conversation_file_id": carried_prior_field(carry_prior_artifacts, fields, &["conversation_file_id", "ConversationFileId"]),
        "file_manifest_id": carried_prior_field(carry_prior_artifacts, fields, &["file_manifest_id", "FileManifestId"]),
        "session_file_id": carried_prior_field(carry_prior_artifacts, fields, &["session_file_id", "SessionFileId"]),
        "session_leaf_id": session_leaf_id,
        "prepared_context_file_id": prepared_context_storage.file_id,
        "prepared_context_inline_json": prepared_context_storage.inline_json,
        "system_prompt_hash": str_field(fields, &["system_prompt_hash", "SystemPromptHash"]).unwrap_or(""),
        "system_prompt_file_id": str_field(fields, &["system_prompt_file_id", "SystemPromptFileId"]).unwrap_or(""),
        "project_harness_id": str_field(fields, &["project_harness_id", "ProjectHarnessId"]).unwrap_or(""),
        "project_id": str_field(fields, &["project_id", "ProjectId"]).unwrap_or(""),
        "session_mode": session_mode,
        "pre_plan_tools_enabled": pre_plan_tools,
    });
    if let Some(trace_context) = trace_context {
        apply_trace_context(&mut configure_body, trace_context);
    }
    if let Some(delivery_route) = delivery_route {
        apply_delivery_route_snapshot(&mut configure_body, delivery_route);
    }
    let configure_url =
        format!("{temper_api_url}/tdata/Sessions('{session_id}')/TemperPaw.Configure");
    let configure_resp = ctx.http_call(
        "POST",
        &configure_url,
        &odata_headers(ctx, tenant),
        &configure_body.to_string(),
    )?;
    if !(200..300).contains(&configure_resp.status) {
        return Err(format!(
            "configure continuation Session failed (HTTP {}): {}",
            configure_resp.status,
            truncate_error_body(&configure_resp.body)
        ));
    }
    // Provision is auto-scheduled by Configure's spec effect. No explicit Resume needed —
    // the resume fields are already stored and provision_sandbox will restore the workspace.
    Ok(())
}

fn carried_prior_field<'a>(
    carry_prior_artifacts: bool,
    fields: &'a Value,
    keys: &[&str],
) -> &'a str {
    if carry_prior_artifacts {
        str_field(fields, keys).unwrap_or("")
    } else {
        ""
    }
}

fn continuation_prepared_context_storage(
    carry_prior_artifacts: bool,
    fields: &Value,
) -> ContinuationPreparedContextStorage {
    if !carry_prior_artifacts {
        return ContinuationPreparedContextStorage {
            file_id: String::new(),
            inline_json: String::new(),
        };
    }

    ContinuationPreparedContextStorage {
        file_id: str_field(
            fields,
            &["prepared_context_file_id", "PreparedContextFileId"],
        )
        .unwrap_or("")
        .to_string(),
        inline_json: String::new(),
    }
}

fn should_start_fresh_after_session_append_failure(error: &str) -> bool {
    error.contains("HTTP 423")
        || error.contains("VerificationRequired")
        || error.contains("SessionEntry creation failed")
        || error.contains("session entries continuation missing parent leaf")
        || error.contains("session_leaf_id is missing; starting clean continuation")
}

fn should_start_clean_continuation_from_prior(fields: &Value) -> bool {
    if str_field(fields, &["Status", "status"]) != Some("Failed") {
        return false;
    }

    if i64_field(fields, &["compaction_count", "CompactionCount"]).unwrap_or(0) >= 3 {
        return true;
    }

    let failure_text = [
        str_field(fields, &["error", "Error"]),
        str_field(fields, &["error_message", "ErrorMessage"]),
        str_field(fields, &["compaction_skipped_reason", "CompactionSkippedReason"]),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
    .to_ascii_lowercase();

    [
        "session-tree walk",
        "no walkable context leaf",
        "context preparation did not complete",
        "fuel exhausted",
        "large_session_tree_guard",
        "repeated_compaction_guard",
        "no_valid_cut_point",
    ]
    .iter()
    .any(|needle| failure_text.contains(needle))
}

/// Update a ChannelSession to point to a new Session via the UpdateSession action.
/// The agent_entity_id stays the same (persistent Agent); only session_entity_id changes.
fn update_session_binding(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    cs_id: &str,
    new_session_id: &str,
) -> Result<(), String> {
    let url =
        format!("{temper_api_url}/tdata/ChannelSessions('{cs_id}')/Paw.Channel.UpdateSession");
    let body = json!({
        "session_entity_id": new_session_id,
        "last_message_at": timestamp_millis_string(),
    });
    // Use the normal runtime headers so production bearer auth and the invoking
    // agent identity are forwarded consistently with the other ChannelSession actions.
    let headers = odata_headers(ctx, tenant);
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "ChannelSession.UpdateSession failed (HTTP {})",
            resp.status
        ));
    }
    Ok(())
}

fn append_user_message_to_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    workspace_id: &str,
    session_file_id: &str,
    session_leaf_id: &str,
    user_message: &str,
) -> Result<String, String> {
    let entity_backed_session = is_session_entries_ref(session_file_id);
    if entity_backed_session {
        return append_user_message_to_session_entries(
            ctx,
            temper_api_url,
            tenant,
            fields,
            session_file_id,
            session_leaf_id,
            user_message,
        );
    }

    let session_jsonl =
        read_file_value(ctx, temper_api_url, tenant, workspace_id, session_file_id)?;
    let mut tree = SessionTree::from_jsonl(&session_jsonl);
    let mut parent_id = if !session_leaf_id.is_empty() {
        session_leaf_id.to_string()
    } else {
        tree.last_entry_id()
            .map(|value| value.to_string())
            .ok_or("session tree is empty")?
    };
    if let Some(interrupted_results) = tree.interrupted_tool_results_for_leaf(&parent_id) {
        let note = "Tool execution was interrupted because the previous agent run ended before returning results.";
        let tokens = estimate_tokens(note);
        let (tool_result_id, _) =
            tree.append_tool_results(&parent_id, &interrupted_results, tokens);
        parent_id = tool_result_id;
    }
    let tokens = estimate_tokens(user_message);
    let (new_leaf_id, _) = if workspace_id.is_empty() {
        tree.append_user_message(&parent_id, user_message, tokens)
    } else {
        let file_name = format!("session-user-{}.txt", tree.len());
        match create_content_file_ref(
            ctx,
            temper_api_url,
            tenant,
            workspace_id,
            &file_name,
            user_message,
        ) {
            Ok(content_ref) => append_externalized_user_message(
                &mut tree,
                &parent_id,
                &content_ref.file_id,
                Some(content_ref.file_version_id.as_str()),
                tokens,
            ),
            Err(err) => {
                ctx.log(
                    "warn",
                    &format!(
                        "route_message: failed to externalize continued user message to TemperFS: {err}"
                    ),
                );
                tree.append_user_message(&parent_id, user_message, tokens)
            }
        }
    };
    write_file_value(
        ctx,
        temper_api_url,
        tenant,
        workspace_id,
        session_file_id,
        &tree.to_jsonl(),
    )?;
    Ok(new_leaf_id)
}

fn append_user_message_to_session_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_file_id: &str,
    session_leaf_id: &str,
    user_message: &str,
) -> Result<String, String> {
    let session_id = session_id_from_entries_ref(session_file_id)
        .ok_or("session entries reference missing Session id")?;
    let latest = match unwritten_first_turn(fields, session_id, session_leaf_id) {
        Some(first_message) => Some(write_unwritten_first_turn(
            ctx,
            temper_api_url,
            tenant,
            fields,
            session_id,
            session_leaf_id,
            first_message,
        )?),
        None => latest_session_entry(ctx, temper_api_url, tenant, session_id, session_leaf_id)?,
    };
    // Trust the database, not the entity field. The Session.session_leaf_id
    // field can drift past the actual db tip when an upstream writer
    // advances the field but its SessionEntry POST didn't durably land
    // (e.g., the orphan-chain case we hit on ss-019de892-8be2 where
    // session_leaf_id="t-8" pointed at an entry that was never created).
    // Using the field as parent in that state produces u-N rows with
    // dangling parent_id, which break every subsequent context_preparer
    // walk forever. Always use the latest verified entry as parent and
    // log when we override a non-matching field hint.
    let parent_entry_id = match latest.as_ref() {
        Some((entry_id, _)) => {
            if !session_leaf_id.is_empty() && session_leaf_id != entry_id {
                ctx.log(
                    "warn",
                    &format!(
                        "append_user_message: session_leaf_id field={} disagrees with db latest={} for SessionId={session_id} — using db latest as parent",
                        session_leaf_id, entry_id
                    ),
                );
            }
            entry_id.clone()
        }
        None => {
            return Err("session entries continuation missing parent leaf".to_string());
        }
    };
    let sequence = latest
        .as_ref()
        .map(|(_, sequence)| sequence.saturating_add(1))
        .unwrap_or(0);
    let entry_id = format!("u-{sequence}");
    let content = json!(user_message);
    let created = create_session_entry(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        &entry_id,
        Some(&parent_entry_id),
        sequence,
        "message",
        Some("user"),
        Some(&content),
        None,
        None,
        None,
        estimate_tokens(user_message),
    )?;
    Ok(created.entry_id)
}

/// The prior Session's first message when its first turn never wrote its
/// SessionEntries, else None. A first turn writes them when its provider
/// response is applied, so one that failed before then (on an expired
/// provider login, say) leaves `session_entries_materialized` "false", its
/// leaf at the virtual `u-<session>-0` and no entries. Continuing from that
/// leaf would find no parent and start the thread over.
fn unwritten_first_turn<'a>(
    fields: &'a Value,
    session_id: &str,
    session_leaf_id: &str,
) -> Option<&'a str> {
    let unwritten = str_field(
        fields,
        &["session_entries_materialized", "SessionEntriesMaterialized"],
    )
    .is_some_and(|_| false);
    if !unwritten || session_leaf_id != format!("u-{session_id}-0") {
        return None;
    }
    str_field(fields, &["user_message", "UserMessage"]).filter(|message| !message.trim().is_empty())
}

/// Writes the header and first user message a continuation builds on, unless
/// they landed after all. Returns the user entry's id and sequence.
fn write_unwritten_first_turn(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    session_leaf_id: &str,
    first_message: &str,
) -> Result<(String, i64), String> {
    let leaf_url = session_entry_lookup_url(temper_api_url, session_id, session_leaf_id);
    if let Some(written) = list_entities(ctx, &leaf_url, tenant)?
        .first()
        .and_then(session_entry_identity)
    {
        return Ok(written);
    }
    let (_header, user) = create_initial_session_entries(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        first_message,
    )?;
    ctx.log(
        "info",
        &format!(
            "append_user_message: wrote the unwritten first turn of SessionId={session_id} before continuing it"
        ),
    );
    // create_initial_session_entries writes the user entry at sequence 1.
    Ok((user.entry_id, 1))
}

fn latest_session_entry(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
    session_leaf_id: &str,
) -> Result<Option<(String, i64)>, String> {
    if !session_leaf_id.is_empty() {
        let leaf_url = session_entry_lookup_url(temper_api_url, session_id, session_leaf_id);
        if let Some(entry) = list_entities(ctx, &leaf_url, tenant)?.into_iter().next() {
            return Ok(session_entry_identity(&entry));
        }
        return Err(format!(
            "session entries continuation missing parent leaf: Session.session_leaf_id={session_leaf_id} missing for SessionId={session_id}"
        ));
    }

    Err(format!(
        "session_leaf_id is missing; starting clean continuation for SessionId={session_id}"
    ))
}

fn session_entry_identity(entry: &Value) -> Option<(String, i64)> {
    let entry_id = nested_str_field(entry, &["EntryId", "entry_id"])?.to_string();
    let sequence = nested_i64_field(entry, &["Sequence", "sequence"]).unwrap_or(0);
    Some((entry_id, sequence))
}

fn session_entry_lookup_url(temper_api_url: &str, session_id: &str, entry_id: &str) -> String {
    bounded_reads::composite_entity_url(
        temper_api_url,
        "SessionEntries",
        &[("SessionId", session_id), ("EntryId", entry_id)],
    )
}

fn append_externalized_user_message(
    tree: &mut SessionTree,
    parent_id: &str,
    content_file_id: &str,
    content_file_version_id: Option<&str>,
    tokens: usize,
) -> (String, String) {
    tree.append_user_message_file(parent_id, content_file_id, content_file_version_id, tokens)
}

fn append_user_message_to_conversation(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    conversation_file_id: &str,
    user_message: &str,
) -> Result<(), String> {
    let raw = read_file_value(
        ctx,
        temper_api_url,
        tenant,
        workspace_id,
        conversation_file_id,
    )?;
    let parsed: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "messages": [] }));
    let mut messages = parsed
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    messages.push(json!({ "role": "user", "content": user_message }));
    let updated = json!({ "messages": messages }).to_string();
    write_file_value(
        ctx,
        temper_api_url,
        tenant,
        workspace_id,
        conversation_file_id,
        &updated,
    )
}

fn resolve_workspace_id_for_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_file_id: &str,
    conversation_file_id: &str,
) -> Result<String, String> {
    if let Some(workspace_id) = str_field(fields, &["workspace_id", "WorkspaceId"])
        && !workspace_id.is_empty()
    {
        return Ok(workspace_id.to_string());
    }
    if !conversation_file_id.is_empty() {
        match fetch_entity(
            ctx,
            &format!("{temper_api_url}/tdata/Files('{conversation_file_id}')"),
            tenant,
        ) {
            Ok(conversation_file) => {
                if let Some(workspace_id) =
                    nested_str_field(&conversation_file, &["workspace_id", "WorkspaceId"])
                        .filter(|value| !value.is_empty())
                {
                    return Ok(workspace_id.to_string());
                }
            }
            Err(err) => {
                ctx.log(
                    "warn",
                    &format!(
                        "route_message: failed to resolve workspace from conversation file {conversation_file_id}: {err}"
                    ),
                );
            }
        }
    }
    if session_file_id.is_empty() {
        return Ok(String::new());
    }
    if is_session_entries_ref(session_file_id) {
        return Ok(String::new());
    }
    let session_file = fetch_entity(
        ctx,
        &format!("{temper_api_url}/tdata/Files('{session_file_id}')"),
        tenant,
    )?;
    Ok(
        nested_str_field(&session_file, &["workspace_id", "WorkspaceId"])
            .unwrap_or("")
            .to_string(),
    )
}

fn read_file_value(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_id: &str,
) -> Result<String, String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = file_headers(ctx, tenant, workspace_id, None, None);
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status == 200 {
        Ok(resp.body)
    } else {
        Err(format!("GET {url} failed (HTTP {})", resp.status))
    }
}

fn write_file_value(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_id: &str,
    body: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = file_headers(ctx, tenant, workspace_id, Some("text/plain"), None);
    let resp = ctx.http_call("PUT", &url, &headers, body)?;
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!("PUT {url} failed (HTTP {})", resp.status))
    }
}

fn fetch_entity(ctx: &Context, url: &str, tenant: &str) -> Result<Value, String> {
    let resp = ctx.http_call("GET", url, &odata_headers(ctx, tenant), "")?;
    if resp.status != 200 {
        return Err(format!("GET {url} failed (HTTP {})", resp.status));
    }
    serde_json::from_str(&resp.body).map_err(|e| format!("failed to parse entity JSON: {e}"))
}

fn truncate_error_body(body: &str) -> String {
    const LIMIT: usize = 240;
    if body.len() <= LIMIT {
        body.to_string()
    } else {
        format!("{}...", &body[..LIMIT])
    }
}

fn session_entity_url(temper_api_url: &str, session_id: &str) -> String {
    format!("{temper_api_url}/tdata/Sessions('{session_id}')")
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "Completed" | "Failed" | "Cancelled")
}

fn should_continue_with_new_session_for_status(status: &str) -> bool {
    is_terminal_status(status) || status == "WaitingForApproval"
}

fn is_steerable_status(status: &str) -> bool {
    matches!(
        status,
        "PreparingContext"
            | "CallingProvider"
            | "Thinking"
            | "Executing"
            | "Steering"
            | "Compacting"
    )
}

/// Maximum staleness, in seconds, before `route_message` preemptively
/// dispatches a Resume* action on a session before Steer. The session's
/// state_timeouts are typically 300-600s; a 60s staleness threshold is
/// well below any of those, so Resume fires before the state_timeout
/// TimeoutFail would have triggered, giving the user a fast response
/// path on DM arrival instead of waiting for the safety net.
const RESUME_STALENESS_THRESHOLD_SECS: i64 = 60;

/// Maps a Session.status to the Resume* action name that wakes that state's
/// driving integration. Per ADR-0039 Sub-Decision 1. Returns `None` for
/// terminal states (where wake-up is meaningless) and for intermediate
/// states with no declared driver (currently none).
///
/// States that don't get a Resume* action:
/// - Created, Provisioning (bootstrap states — different wake-up mechanism)
/// - ApplyingProviderResponse (brief glue state; state_timeout=60s is enough)
/// - WaitingForApproval (human-gated by design, ADR-0005)
/// - Recovering (already a recovery state; Recover* actions cover it)
/// - Completed, Failed, Cancelled (terminal)
fn resume_action_for_status(status: &str) -> Option<&'static str> {
    match status {
        "Executing" => Some("ResumeTools"),
        "CallingProvider" => Some("ResumeProvider"),
        "PreparingContext" => Some("ResumeContext"),
        "Thinking" => Some("ResumeThinking"),
        "Compacting" => Some("ResumeCompacting"),
        "Steering" => Some("ResumeSteering"),
        _ => None,
    }
}

/// True if the session's `last_progress_at` is older than `threshold_secs`
/// relative to `now_secs`. Missing or unparseable `last_progress_at`
/// returns true (safe default — prefer to wake than to silently stall).
fn is_session_stale(session: &Value, now_secs: i64, threshold_secs: i64) -> bool {
    let last_str = session
        .pointer("/fields/last_progress_at")
        .and_then(|v| v.as_str())
        .or_else(|| nested_str_field(session, &["LastProgressAt", "last_progress_at"]));
    let Some(s) = last_str else {
        return true; // field missing (old snapshot) — wake-up is safe.
    };
    let Some(last_secs) = parse_iso8601_secs(s) else {
        return true;
    };
    now_secs.saturating_sub(last_secs) > threshold_secs
}

/// Parse an ISO-8601 / RFC-3339 timestamp string into Unix seconds.
/// Supports the `YYYY-MM-DDTHH:MM:SS[.fff]Z` shape temper-observe emits.
/// Returns `None` on any parse failure.
fn parse_iso8601_secs(s: &str) -> Option<i64> {
    // Minimal parser: strip trailing Z, split T, parse date and time.
    // Avoids pulling chrono into this crate just for one parse path.
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let date = &s[..10];
    let time_start = 11;
    let time_end = if let Some(z) = s.find(['Z', '+', '.']) {
        z
    } else {
        s.len()
    };
    if time_end <= time_start + 7 {
        return None;
    }
    let time = &s[time_start..time_start + 8]; // HH:MM:SS

    let (y, m, d) = {
        let mut parts = date.split('-');
        (
            parts.next()?.parse::<i32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
        )
    };
    let (hh, mm, ss) = {
        let mut parts = time.split(':');
        (
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
        )
    };

    // Days-from-civil (Hinnant), avoids chrono dep in this WASM crate.
    let y_adj = y - if m <= 2 { 1 } else { 0 };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = (y_adj - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_since_epoch = era as i64 * 146097 + doe as i64 - 719468;
    Some(days_since_epoch * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64)
}

/// Dispatch the matching Resume* action on the Session entity to wake its
/// driving integration. ADR-0039 Sub-Decision 1.
fn wake_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_entity_id: &str,
    action_name: &str,
) -> Result<(), String> {
    let url =
        format!("{temper_api_url}/tdata/Sessions('{session_entity_id}')/TemperPaw.{action_name}");
    let _ = ctx.http_call("POST", &url, &odata_headers(ctx, tenant), "{}")?;
    Ok(())
}

fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

fn create_channel_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    channel_id: &str,
    thread_id: &str,
    author_id: &str,
    agent_entity_id: &str,
    session_entity_id: &str,
) -> Result<(), String> {
    let create_resp = ctx.http_call(
        "POST",
        &format!("{temper_api_url}/tdata/ChannelSessions"),
        &odata_headers(ctx, tenant),
        "{}",
    )?;
    if !(200..300).contains(&create_resp.status) {
        return Err(format!(
            "create ChannelSession failed (HTTP {})",
            create_resp.status
        ));
    }
    let parsed: Value = serde_json::from_str(&create_resp.body).unwrap_or_else(|_| json!({}));
    let cs_id = parsed
        .get("entity_id")
        .or_else(|| parsed.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if cs_id.is_empty() {
        return Err("ChannelSession creation missing entity_id".to_string());
    }
    let create_url =
        format!("{temper_api_url}/tdata/ChannelSessions('{cs_id}')/Paw.Channel.Create");
    let body = json!({
        "channel_id": channel_id,
        "thread_id": thread_id,
        "author_id": author_id,
        "agent_entity_id": agent_entity_id,
        "session_entity_id": session_entity_id,
        "last_message_at": timestamp_millis_string(),
    });
    let resp = ctx.http_call(
        "POST",
        &create_url,
        &odata_headers(ctx, tenant),
        &body.to_string(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "ChannelSession.Create failed (HTTP {})",
            resp.status
        ));
    }
    Ok(())
}

fn switch_mode_and_steer(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
    command: &str,
    content: &str,
) -> Result<(), String> {
    // 1. Fetch the Session entity to read current tools_enabled
    let session_url = format!("{temper_api_url}/tdata/Sessions('{session_id}')");
    let session_resp = ctx.http_call("GET", &session_url, &odata_headers(ctx, tenant), "")?;
    let session: Value = if session_resp.status == 200 {
        serde_json::from_str(&session_resp.body).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let current_tools = nested_str_field(&session, &["ToolsEnabled", "tools_enabled"])
        .unwrap_or(DEFAULT_TOOLS_ENABLED);

    // 2. Build SwitchMode body
    let mut body = serde_json::Map::new();
    body.insert("session_mode".into(), json!(command));

    if command == "plan" {
        body.insert("pre_plan_tools_enabled".into(), json!(current_tools));
        body.insert("tools_enabled".into(), json!(PLAN_MODE_TOOLS));
    } else {
        // "execute" — restore stashed tools
        let stashed =
            nested_str_field(&session, &["PrePlanToolsEnabled", "pre_plan_tools_enabled"])
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_TOOLS_ENABLED);
        body.insert("tools_enabled".into(), json!(stashed));
        body.insert("pre_plan_tools_enabled".into(), json!(""));
    }

    // 3. Dispatch SwitchMode
    let switch_url =
        format!("{temper_api_url}/tdata/Sessions('{session_id}')/TemperPaw.SwitchMode");
    let resp = ctx.http_call(
        "POST",
        &switch_url,
        &odata_headers(ctx, tenant),
        &json!(body).to_string(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "SwitchMode failed (HTTP {}): {}",
            resp.status,
            truncate_error_body(&resp.body)
        ));
    }

    // 4. Steer with the task text (if any)
    if !content.is_empty() {
        steer_session(ctx, temper_api_url, tenant, session_id, content)?;
    }

    Ok(())
}

fn steer_session(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    session_id: &str,
    message: &str,
) -> Result<(), String> {
    let session_url = format!("{temper_api_url}/tdata/Sessions('{session_id}')");
    let session_resp = ctx.http_call("GET", &session_url, &odata_headers(ctx, tenant), "")?;
    let mut queue = if session_resp.status == 200 {
        let parsed: Value = serde_json::from_str(&session_resp.body).unwrap_or_else(|_| json!({}));
        serde_json::from_str::<Vec<Value>>(
            nested_str_field(&parsed, &["SteeringMessages", "steering_messages"]).unwrap_or("[]"),
        )
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    queue.push(json!({ "content": message }));
    let steer_url = format!("{temper_api_url}/tdata/Sessions('{session_id}')/TemperPaw.Steer");
    let body = json!({
        "steering_messages": serde_json::to_string(&queue).unwrap_or_else(|_| "[]".to_string()),
    });
    let resp = ctx.http_call(
        "POST",
        &steer_url,
        &odata_headers(ctx, tenant),
        &body.to_string(),
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!("steer session failed (HTTP {})", resp.status));
    }
    Ok(())
}

fn str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn nested_str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    str_field(value, keys).or_else(|| {
        value
            .get("fields")
            .and_then(|fields| str_field(fields, keys))
    })
}

fn i64_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = value.get(*key)?;
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
            .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
    })
}

fn nested_i64_field(value: &Value, keys: &[&str]) -> Option<i64> {
    i64_field(value, keys).or_else(|| {
        value
            .get("fields")
            .and_then(|fields| i64_field(fields, keys))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Continuing a first turn that never wrote its entries ------------

    #[test]
    fn a_continuation_writes_the_first_turn_a_failed_session_left_unwritten() {
        let prior = json!({
            "user_message": "What changed in last night's deploy?",
            "session_entries_materialized": "false",
        });
        assert_eq!(
            unwritten_first_turn(&prior, "ss-1", "u-ss-1-0"),
            Some("What changed in last night's deploy?")
        );
        let odata_spelling = json!({
            "UserMessage": "What changed?",
            "SessionEntriesMaterialized": "FALSE",
        });
        assert_eq!(
            unwritten_first_turn(&odata_spelling, "ss-1", "u-ss-1-0"),
            Some("What changed?")
        );
    }

    #[test]
    fn a_continuation_writes_no_first_turn_unless_the_session_says_it_never_did() {
        let message = "What changed in last night's deploy?";
        for prior in [
            json!({"user_message": message, "session_entries_materialized": "true"}),
            json!({"user_message": message, "session_entries_materialized": ""}),
            json!({"user_message": message}),
            json!({"user_message": " ", "session_entries_materialized": "false"}),
        ] {
            assert_eq!(
                unwritten_first_turn(&prior, "ss-1", "u-ss-1-0"),
                None,
                "{prior}"
            );
        }
        let past_first_turn = json!({
            "user_message": message,
            "session_entries_materialized": "false",
        });
        assert_eq!(unwritten_first_turn(&past_first_turn, "ss-1", "a-3"), None);
    }

    // --- Stale-session wake-up (ADR-0039) -------------------------------

    #[test]
    fn resume_action_for_status_covers_all_driving_states() {
        assert_eq!(resume_action_for_status("Executing"), Some("ResumeTools"));
        assert_eq!(
            resume_action_for_status("CallingProvider"),
            Some("ResumeProvider")
        );
        assert_eq!(
            resume_action_for_status("PreparingContext"),
            Some("ResumeContext")
        );
        assert_eq!(resume_action_for_status("Thinking"), Some("ResumeThinking"));
        assert_eq!(
            resume_action_for_status("Compacting"),
            Some("ResumeCompacting")
        );
        assert_eq!(resume_action_for_status("Steering"), Some("ResumeSteering"));
    }

    #[test]
    fn active_provider_and_context_states_are_steerable() {
        for status in ["PreparingContext", "CallingProvider"] {
            assert!(
                is_steerable_status(status),
                "{status} must accept user steering and stale-session wake-up"
            );
        }
    }

    #[test]
    fn resume_action_for_status_returns_none_for_non_driving_states() {
        for status in [
            "Created",
            "Provisioning",
            "ApplyingProviderResponse",
            "WaitingForApproval",
            "Recovering",
            "Completed",
            "Failed",
            "Cancelled",
        ] {
            assert_eq!(
                resume_action_for_status(status),
                None,
                "status {status} must not produce a Resume* action"
            );
        }
    }

    #[test]
    fn waiting_for_approval_should_not_swallow_follow_up_messages() {
        assert!(
            should_continue_with_new_session_for_status("WaitingForApproval"),
            "normal DMs to a human-gated session must start a fresh continuation"
        );
        assert!(
            should_continue_with_new_session_for_status("Completed"),
            "terminal sessions already continue in a new session"
        );
        assert!(
            !should_continue_with_new_session_for_status("Provisioning"),
            "bootstrap states should keep waiting instead of spawning duplicate sessions"
        );
    }

    #[test]
    fn continuation_drops_inline_prepared_context() {
        let fields = json!({
            "prepared_context_file_id": "fl-prepared",
            "prepared_context_inline_json": "large inline artifact"
        });

        let storage = continuation_prepared_context_storage(true, &fields);

        assert_eq!(storage.file_id, "fl-prepared");
        assert_eq!(storage.inline_json, "");
    }

    #[test]
    fn continuation_drops_prior_artifacts_when_session_tree_append_is_unrecoverable() {
        let fields = json!({
            "prepared_context_file_id": "fl-prepared",
            "prepared_context_inline_json": "large inline artifact"
        });

        let storage = continuation_prepared_context_storage(false, &fields);

        assert_eq!(storage.file_id, "");
        assert_eq!(storage.inline_json, "");
        assert_eq!(
            carried_prior_field(false, &fields, &["prepared_context_file_id"]),
            ""
        );
    }

    #[test]
    fn session_entry_verification_failure_starts_clean_continuation() {
        assert!(should_start_fresh_after_session_append_failure(
            "SessionEntry creation failed (HTTP 423): VerificationRequired"
        ));
        assert!(should_start_fresh_after_session_append_failure(
            "session entries continuation missing parent leaf"
        ));
        assert!(!should_start_fresh_after_session_append_failure(
            "create Session failed via http://127.0.0.1"
        ));
    }

    #[test]
    fn failed_context_compaction_sessions_start_clean_continuations() {
        let fields = json!({
            "Status": "Failed",
            "error": "fuel exhausted -- module exceeded instruction budget",
            "error_message": "fuel exhausted -- module exceeded instruction budget",
            "compaction_count": 2328,
            "compaction_skipped_reason": "no_valid_cut_point",
        });

        assert!(should_start_clean_continuation_from_prior(&fields));
    }

    #[test]
    fn failed_session_tree_walks_start_clean_continuations() {
        let fields = json!({
            "Status": "Failed",
            "error_message": "session-tree walk from leaf 'u-1002' produced no entries against 999-entry tree; refusing to feed empty conversation to LLM",
        });

        assert!(should_start_clean_continuation_from_prior(&fields));
    }

    #[test]
    fn non_failed_prior_sessions_do_not_start_clean_by_default() {
        let fields = json!({
            "Status": "Completed",
            "compaction_count": 10,
            "error_message": "fuel exhausted",
        });

        assert!(!should_start_clean_continuation_from_prior(&fields));
    }

    #[test]
    fn parse_iso8601_secs_handles_utc_z_suffix() {
        // 2026-04-21T14:30:00Z → Unix seconds
        // Hand-computed: (2026-1970)*365.25 ~ 56*365.25 = 20_454 days + some leap days.
        // Easier to use a round-trip check against a known anchor.
        let anchor = parse_iso8601_secs("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(anchor, 0);
        let day_later = parse_iso8601_secs("1970-01-02T00:00:00Z").unwrap();
        assert_eq!(day_later, 86_400);
        let hour_later = parse_iso8601_secs("1970-01-01T01:00:00Z").unwrap();
        assert_eq!(hour_later, 3_600);
    }

    #[test]
    fn parse_iso8601_secs_handles_fractional_and_offset() {
        // Fractional seconds suffix (".123Z") truncates at the dot.
        let parsed = parse_iso8601_secs("2026-04-21T14:30:00.123Z").unwrap();
        // Same whole-second base whether fractional present or not.
        let baseline = parse_iso8601_secs("2026-04-21T14:30:00Z").unwrap();
        assert_eq!(parsed, baseline);
    }

    #[test]
    fn parse_iso8601_secs_returns_none_for_garbage() {
        assert_eq!(parse_iso8601_secs("not-a-timestamp"), None);
        assert_eq!(parse_iso8601_secs(""), None);
        assert_eq!(parse_iso8601_secs("short"), None);
    }

    #[test]
    fn is_session_stale_returns_true_when_last_progress_older_than_threshold() {
        // Anchor on the actual parsed timestamp to avoid off-by-wall-clock
        // errors. last_progress_at is a fixed instant; we probe staleness
        // at different "now" offsets relative to it.
        let ts = "2026-04-21T14:30:00Z";
        let ts_secs = parse_iso8601_secs(ts).expect("anchor parses");
        let session = json!({ "fields": { "last_progress_at": ts } });

        // "now" 20s after last_progress_at, threshold 60s → not stale.
        assert!(!is_session_stale(&session, ts_secs + 20, 60));
        // "now" 20s after, threshold 10s → stale.
        assert!(is_session_stale(&session, ts_secs + 20, 10));
        // "now" 120s after, threshold 60s → stale.
        assert!(is_session_stale(&session, ts_secs + 120, 60));
    }

    #[test]
    fn is_session_stale_returns_true_when_last_progress_missing() {
        // Safe default: old snapshots pre-PR#98 don't have last_progress_at.
        // Better to wake-up unnecessarily than to silently stall.
        let session = json!({ "fields": {} });
        assert!(is_session_stale(&session, 1_000_000, 60));
    }

    #[test]
    fn is_session_stale_returns_true_on_unparseable_timestamp() {
        let session = json!({ "fields": { "last_progress_at": "not-a-date" } });
        assert!(is_session_stale(&session, 1_000_000, 60));
    }

    #[test]
    fn trace_context_is_read_from_receive_message_trigger_params() {
        let trace = trace_context_from_trigger_params(&json!({
            "gen_ai_parent_trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
            "gen_ai_parent_span_id": "00f067aa0ba902b7",
        }))
        .expect("trace context should be present");

        assert_eq!(trace.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(trace.span_id, "00f067aa0ba902b7");
    }

    #[test]
    fn configure_body_carries_trace_context_when_available() {
        let trace = TraceContextFields {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            span_id: "00f067aa0ba902b7".to_string(),
        };
        let mut configure_body = json!({
            "user_message": "hello",
            "session_mode": "execute",
        });

        apply_trace_context(&mut configure_body, trace.as_ref());

        assert_eq!(
            configure_body
                .get("gen_ai_parent_trace_id")
                .and_then(Value::as_str),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(
            configure_body
                .get("gen_ai_parent_span_id")
                .and_then(Value::as_str),
            Some("00f067aa0ba902b7")
        );
    }

    #[test]
    fn configure_body_carries_reply_route_when_channel_routed() {
        let mut configure_body = json!({
            "user_message": "hello",
            "session_mode": "execute",
        });
        let route = DeliveryRouteSnapshot {
            channel_id: "discord-channel-1".to_string(),
            thread_id: "discord-thread-1".to_string(),
            channel_entity_id: Some("ch-entity-1".to_string()),
            channel_type: Some("cli".to_string()),
            source: "channel_session".to_string(),
        };

        apply_delivery_route_snapshot(&mut configure_body, &route);

        assert_eq!(
            configure_body
                .get("reply_channel_id")
                .and_then(Value::as_str),
            Some("discord-channel-1")
        );
        assert_eq!(
            configure_body
                .get("reply_thread_id")
                .and_then(Value::as_str),
            Some("discord-thread-1")
        );
        assert_eq!(
            configure_body
                .get("reply_channel_entity_id")
                .and_then(Value::as_str),
            Some("ch-entity-1")
        );
        assert_eq!(
            configure_body
                .get("reply_channel_type")
                .and_then(Value::as_str),
            Some("cli")
        );
        assert_eq!(
            configure_body
                .get("reply_route_source")
                .and_then(Value::as_str),
            Some("channel_session")
        );
    }

    #[test]
    fn reply_route_snapshot_from_channel_session_uses_stored_channel_thread() {
        let fields = json!({
            "channel_id": "discord-channel-1",
            "thread_id": "discord-thread-1",
        });

        let route = delivery_route_snapshot_from_channel_session(&fields)
            .expect("channel session route should be complete");

        assert_eq!(route.channel_id, "discord-channel-1");
        assert_eq!(route.thread_id, "discord-thread-1");
        assert_eq!(route.channel_entity_id, None);
        assert_eq!(route.channel_type, None);
        assert_eq!(route.source, "channel_session");
    }

    #[test]
    fn reply_route_snapshot_from_channel_message_includes_channel_entity_id_and_type() {
        let route = delivery_route_snapshot_from_channel_message(
            "discord-channel-1",
            "discord-thread-1",
            "ch-entity-1",
            "cli",
        );

        assert_eq!(route.channel_id, "discord-channel-1");
        assert_eq!(route.thread_id, "discord-thread-1");
        assert_eq!(route.channel_entity_id.as_deref(), Some("ch-entity-1"));
        assert_eq!(route.channel_type.as_deref(), Some("cli"));
        assert_eq!(route.source, "channel_message");
    }

    #[test]
    fn active_session_lookup_is_bounded_and_escapes_literals() {
        assert_eq!(
            active_session_query("discord-gateway", "thread'one", "author'one"),
            "$filter=Status eq 'Active' and channel_id eq 'discord-gateway' and thread_id eq 'thread''one' and author_id eq 'author''one'&$top=1"
        );
    }

    #[test]
    fn agent_route_lookup_prefers_specific_route_then_global_fallback() {
        assert_eq!(
            agent_route_queries("discord'gateway"),
            vec![
                "$filter=Status eq 'Active' and channel_id eq 'discord''gateway'&$top=1",
                "$filter=Status eq 'Active' and channel_id eq ''&$top=1",
            ]
        );
    }

    #[test]
    fn continued_externalized_user_messages_preserve_file_version() {
        let mut tree = SessionTree::new("route-message-file-ref");
        let parent_id = tree.last_entry_id().unwrap().to_string();

        let (leaf_id, _) = append_externalized_user_message(
            &mut tree,
            &parent_id,
            "file-123",
            Some("ver-456"),
            17,
        );

        let refs = tree.build_context_refs(&leaf_id);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].content_file_id.as_deref(), Some("file-123"));
        assert_eq!(refs[0].content_file_version_id.as_deref(), Some("ver-456"));
    }

    #[test]
    fn session_entry_lookup_uses_leaf_without_ordered_scan() {
        let url = session_entry_lookup_url("http://127.0.0.1:8080", "ss-1", "a-2");

        assert_eq!(
            url,
            "http://127.0.0.1:8080/tdata/SessionEntries(SessionId='ss-1',EntryId='a-2')"
        );
        assert!(
            !url.contains("$filter") && !url.contains("$orderby"),
            "route_message should use direct composite-key SessionEntry reads"
        );
    }
}
