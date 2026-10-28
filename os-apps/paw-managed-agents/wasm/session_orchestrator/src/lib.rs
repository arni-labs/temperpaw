#[path = "../../common.rs"]
mod common;

use common::{
    create_entity, create_session_event, entity_id, escape_odata_string, field_i64, field_string,
    finish_agent_session_span, get_entity, is_terminal_status, log_managed_session_event,
    managed_agent_provider, managed_agent_provider_options_json, managed_environment_sandbox_params,
    managed_session_event_context, managed_tools_enabled, next_session_event_sequence,
    pending_user_prompt, post_absolute_action, post_action, start_agent_session_span, status_of,
    system_json_headers, with_session_event_context,
};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::resolve_temper_api_url;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let base_url = resolve_temper_api_url(&ctx, &fields);
        let headers = system_json_headers(&ctx, &ctx.tenant, &fields);

        match ctx
            .config
            .get("mode")
            .map(String::as_str)
            .unwrap_or("start")
        {
            "check" => check_inner_session(&ctx, &fields, &base_url, &headers),
            "resume" => start_or_resume(&ctx, &fields, &base_url, &headers, true),
            _ => start_or_resume(&ctx, &fields, &base_url, &headers, false),
        }
    })();

    if let Err(error) = result {
        temper_wasm_sdk::set_error_result(&error);
    }
    0
}

fn start_or_resume(
    ctx: &Context,
    fields: &Value,
    base_url: &str,
    headers: &[(String, String)],
    is_resume: bool,
) -> Result<(), String> {
    let session_id = ctx.entity_id.as_str();
    let managed_agent_id = field_string(fields, &["AgentId", "agent_id"]);
    let parent_session_id = field_string(fields, &["ParentSessionId", "parent_session_id"]);
    let environment_id = field_string(fields, &["EnvironmentId", "environment_id"]);
    if managed_agent_id.is_empty() || environment_id.is_empty() {
        temper_wasm_sdk::set_success_result(
            "InnerSessionFailed",
            &json!({
                "ErrorMessage": "ManagedSession requires AgentId and EnvironmentId.",
                "TerminationReason": "error",
            }),
        );
        return Ok(());
    }

    let managed_agent = get_entity(ctx, base_url, headers, "ManagedAgents", &managed_agent_id)?;
    let managed_environment = get_entity(
        ctx,
        base_url,
        headers,
        "ManagedEnvironments",
        &environment_id,
    )?;
    let environment_package_rows = common::list_entities(
        ctx,
        base_url,
        headers,
        &format!(
            "/tdata/EnvironmentPackages?$filter=EnvironmentId%20eq%20'{}'&$orderby=Name%20asc",
            escape_odata_string(&environment_id)
        ),
    )?;

    let tool_rows = common::list_entities(
        ctx,
        base_url,
        headers,
        &format!(
            "/tdata/AgentTools?$filter=AgentId%20eq%20'{}'&$orderby=Name%20asc",
            escape_odata_string(&managed_agent_id)
        ),
    )?;
    let tool_ids = tool_rows.iter().filter_map(entity_id).collect::<Vec<_>>();
    let tool_config_rows = if tool_ids.is_empty() {
        Vec::new()
    } else {
        let filter = tool_ids
            .iter()
            .map(|tool_id| format!("ToolId%20eq%20'{}'", escape_odata_string(tool_id)))
            .collect::<Vec<_>>()
            .join("%20or%20");
        common::list_entities(
            ctx,
            base_url,
            headers,
            &format!("/tdata/AgentToolConfigs?$filter={filter}&$orderby=ToolName%20asc"),
        )?
    };

    let inner_agent_id = ensure_inner_agent(
        ctx,
        base_url,
        headers,
        &managed_agent,
        &managed_agent_id,
        &tool_rows,
        &tool_config_rows,
    )?;

    let last_consumed = field_i64(
        fields,
        &["LastConsumedUserSequence", "last_consumed_user_sequence"],
    );
    let pending_events = common::list_entities(
        ctx,
        base_url,
        headers,
        &format!(
            "/tdata/SessionEvents?$filter=SessionId%20eq%20'{}'%20and%20Sequence%20gt%20{}&$orderby=Sequence%20asc",
            escape_odata_string(session_id),
            last_consumed
        ),
    )?;
    let (prompt, last_sequence) = pending_user_prompt(&pending_events, last_consumed);
    if prompt.trim().is_empty() {
        temper_wasm_sdk::set_success_result(
            "IdleSession",
            &json!({
                "StopReason": "user_input_required",
            }),
        );
        return Ok(());
    }

    let existing_inner_session_id = field_string(fields, &["InnerSessionId", "inner_session_id"]);
    if is_resume && !existing_inner_session_id.is_empty() {
        if let Ok(existing_inner) = get_entity(
            ctx,
            base_url,
            headers,
            "Sessions",
            &existing_inner_session_id,
        ) {
            if !is_terminal_status(&status_of(&existing_inner)) {
                let action_name = "ManagedAgents.ResumeSession";
                let mut session_span = start_agent_session_span(
                    ctx,
                    session_id,
                    &existing_inner_session_id,
                    &managed_agent_id,
                    &environment_id,
                    &parent_session_id,
                    action_name,
                );
                let steer_result = post_absolute_action(
                    ctx,
                    headers,
                    &format!(
                        "{base_url}/tdata/Sessions('{existing_inner_session_id}')/TemperPaw.Steer"
                    ),
                    &json!({
                        "steering_messages": serde_json::to_string(&vec![json!({ "content": prompt })])
                            .unwrap_or_else(|_| "[]".to_string())
                    }),
                    "steer inner session",
                );
                finish_agent_session_span(&mut session_span, &steer_result);
                let _ = steer_result?;
                record_running_event(
                    ctx,
                    fields,
                    base_url,
                    headers,
                    session_id,
                    &existing_inner_session_id,
                    &inner_agent_id,
                    &managed_agent_id,
                    &parent_session_id,
                    &environment_id,
                    "ManagedAgents.ResumeSession",
                )?;
                temper_wasm_sdk::set_success_result(
                    "InnerSessionReady",
                    &json!({
                        "InnerSessionId": existing_inner_session_id,
                        "InnerAgentId": inner_agent_id,
                        "LastConsumedUserSequence": last_sequence,
                        "InnerSessionCheckCount": 0,
                    }),
                );
                return Ok(());
            }
        }
    }

    let workspace_id = existing_inner_session_id
        .as_str()
        .pipe(|inner_id| {
            if inner_id.is_empty() {
                Ok(String::new())
            } else {
                get_entity(ctx, base_url, headers, "Sessions", inner_id)
                    .map(|entity| field_string(&entity, &["WorkspaceId", "workspace_id"]))
            }
        })
        .unwrap_or_default();

    let created = create_entity(ctx, base_url, headers, "Sessions", &json!({}))?;
    let inner_session_id =
        entity_id(&created).ok_or("create Sessions did not return an entity id")?;

    let model_id = field_string(&managed_agent, &["ModelId", "model_id"]);
    if model_id.is_empty() {
        temper_wasm_sdk::set_success_result(
            "InnerSessionFailed",
            &json!({
                "ErrorMessage": "ManagedAgent requires ModelId before starting a session.",
                "TerminationReason": "error",
            }),
        );
        return Ok(());
    }
    let provider = managed_agent_provider(&managed_agent);
    if provider.is_empty() {
        temper_wasm_sdk::set_success_result(
            "InnerSessionFailed",
            &json!({
                "ErrorMessage": "ManagedAgent requires Provider before starting a session.",
                "TerminationReason": "error",
            }),
        );
        return Ok(());
    }
    let provider_options_json = managed_agent_provider_options_json(&managed_agent);
    let system_prompt = {
        let value = field_string(&managed_agent, &["System", "system"]);
        if value.is_empty() {
            "You are a helpful managed agent.".to_string()
        } else {
            value
        }
    };
    let tools_enabled = managed_tools_enabled(&tool_rows, &tool_config_rows);
    let max_turns = "60";

    let mut configure_body = json!({
        "system_prompt": system_prompt,
        "user_message": prompt,
        "model": model_id,
        "provider": provider,
        "provider_options_json": provider_options_json,
        "tools_enabled": tools_enabled,
        "max_turns": max_turns,
        "temper_api_url": base_url,
        "agent_id": inner_agent_id,
        "parent_session_id": parent_session_id,
    });
    if let Some(configure_object) = configure_body.as_object_mut() {
        let sandbox_params =
            managed_environment_sandbox_params(&managed_environment, &environment_package_rows);
        if let Some(sandbox_object) = sandbox_params.as_object() {
            for (key, value) in sandbox_object {
                configure_object.insert(key.clone(), value.clone());
            }
        }
    }
    if !workspace_id.is_empty() {
        configure_body["workspace_id"] = json!(workspace_id);
    }

    let action_name = if is_resume {
        "ManagedAgents.ResumeSession"
    } else {
        "ManagedAgents.StartSession"
    };
    let mut session_span = start_agent_session_span(
        ctx,
        session_id,
        &inner_session_id,
        &managed_agent_id,
        &environment_id,
        &parent_session_id,
        action_name,
    );
    let configure_result = post_absolute_action(
        ctx,
        headers,
        &format!("{base_url}/tdata/Sessions('{inner_session_id}')/TemperPaw.Configure"),
        &configure_body,
        "configure inner session",
    );
    finish_agent_session_span(&mut session_span, &configure_result);
    let _ = configure_result?;

    record_running_event(
        ctx,
        fields,
        base_url,
        headers,
        session_id,
        &inner_session_id,
        &inner_agent_id,
        &managed_agent_id,
        &parent_session_id,
        &environment_id,
        if is_resume {
            "ManagedAgents.ResumeSession"
        } else {
            "ManagedAgents.StartSession"
        },
    )?;
    temper_wasm_sdk::set_success_result(
        "InnerSessionReady",
        &json!({
            "InnerSessionId": inner_session_id,
            "InnerAgentId": inner_agent_id,
            "LastConsumedUserSequence": last_sequence,
            "InnerSessionCheckCount": 0,
        }),
    );
    Ok(())
}

fn check_inner_session(
    ctx: &Context,
    fields: &Value,
    base_url: &str,
    headers: &[(String, String)],
) -> Result<(), String> {
    let inner_session_id = field_string(fields, &["InnerSessionId", "inner_session_id"]);
    if inner_session_id.is_empty() {
        temper_wasm_sdk::set_success_result(
            "InnerSessionFailed",
            &json!({
                "ErrorMessage": "ManagedSession has no inner session to monitor.",
                "TerminationReason": "error",
            }),
        );
        return Ok(());
    }

    let check_count = field_i64(
        fields,
        &["InnerSessionCheckCount", "inner_session_check_count"],
    );
    let max_checks = field_string(
        fields,
        &["MaxInnerSessionChecks", "max_inner_session_checks"],
    )
    .parse::<i64>()
    .unwrap_or(180);
    if check_count >= max_checks {
        temper_wasm_sdk::set_success_result(
            "InnerSessionFailed",
            &json!({
                "ErrorMessage": format!(
                    "ManagedSession exceeded {} inner-session checks without reaching a terminal state.",
                    max_checks
                ),
                "TerminationReason": "error",
            }),
        );
        return Ok(());
    }

    let inner_session = get_entity(ctx, base_url, headers, "Sessions", &inner_session_id)?;
    let status = status_of(&inner_session);
    match status.as_str() {
        "Completed" => temper_wasm_sdk::set_success_result(
            "IdleSession",
            &json!({
                "StopReason": "user_input_required",
            }),
        ),
        "Failed" | "Cancelled" => {
            let error_message = {
                let error = field_string(&inner_session, &["ErrorMessage", "error_message"]);
                if error.is_empty() {
                    let result = field_string(&inner_session, &["Result", "result"]);
                    if result.is_empty() {
                        format!("Inner session ended with status {status}.")
                    } else {
                        result
                    }
                } else {
                    error
                }
            };
            temper_wasm_sdk::set_success_result(
                "InnerSessionFailed",
                &json!({
                    "ErrorMessage": error_message,
                    "TerminationReason": if status == "Cancelled" {
                        "cancelled"
                    } else {
                        "error"
                    },
                }),
            );
        }
        _ => temper_wasm_sdk::set_success_result("InnerSessionPending", &json!({})),
    }
    Ok(())
}

fn ensure_inner_agent(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    managed_agent: &Value,
    managed_agent_id: &str,
    tool_rows: &[Value],
    tool_config_rows: &[Value],
) -> Result<String, String> {
    let existing = field_string(managed_agent, &["InnerAgentId", "inner_agent_id"]);
    let name = {
        let value = field_string(managed_agent, &["Name", "name"]);
        if value.is_empty() {
            format!("managed-agent-{managed_agent_id}")
        } else {
            value
        }
    };
    let description = field_string(managed_agent, &["Description", "description"]);
    let model_id = field_string(managed_agent, &["ModelId", "model_id"]);
    if model_id.is_empty() {
        return Err("ManagedAgent requires ModelId before syncing inner Agent".into());
    }
    let tools_enabled = managed_tools_enabled(tool_rows, tool_config_rows);
    let provider = managed_agent_provider(managed_agent);
    if provider.is_empty() {
        return Err("ManagedAgent requires Provider before syncing inner Agent".into());
    }
    let provider_options_json = managed_agent_provider_options_json(managed_agent);

    if existing.is_empty() {
        let created = create_entity(ctx, base_url, headers, "Agents", &json!({}))?;
        let inner_agent_id =
            entity_id(&created).ok_or("create Agents did not return an entity id")?;
        let _ = post_absolute_action(
            ctx,
            headers,
            &format!("{base_url}/tdata/Agents('{inner_agent_id}')/TemperPaw.Configure"),
            &json!({
                "name": name,
                "role": "managed-agent",
                "description": description,
                "source_app_id": "paw-managed-agents",
                "model": model_id,
                "provider": provider,
                "provider_options_json": provider_options_json,
                "tools_enabled": tools_enabled,
                "max_turns": "60",
            }),
            "configure inner agent",
        )?;
        let _ = post_action(
            ctx,
            base_url,
            headers,
            "ManagedAgents",
            managed_agent_id,
            "BindInnerAgent",
            &json!({ "InnerAgentId": inner_agent_id }),
            false,
        )?;
        Ok(inner_agent_id)
    } else {
        let _ = post_absolute_action(
            ctx,
            headers,
            &format!("{base_url}/tdata/Agents('{existing}')/TemperPaw.Update"),
            &json!({
                "description": description,
                "model": model_id,
                "provider": provider,
                "provider_options_json": provider_options_json,
                "tools_enabled": tools_enabled,
                "max_turns": "60",
            }),
            "update inner agent",
        )?;
        Ok(existing)
    }
}

fn record_running_event(
    ctx: &Context,
    fields: &Value,
    base_url: &str,
    headers: &[(String, String)],
    session_id: &str,
    inner_session_id: &str,
    inner_agent_id: &str,
    managed_agent_id: &str,
    parent_session_id: &str,
    environment_id: &str,
    action_name: &str,
) -> Result<(), String> {
    let sequence = next_session_event_sequence(ctx, base_url, headers, session_id)?;
    let event_fields = running_event_content(
        fields,
        session_id,
        inner_session_id,
        inner_agent_id,
        managed_agent_id,
        parent_session_id,
        environment_id,
        action_name,
    );
    let _ = create_session_event(
        ctx,
        base_url,
        headers,
        session_id,
        sequence,
        "session.status_running",
        event_fields.clone(),
    )?;
    log_managed_session_event(ctx, &event_fields, "session.status_running", sequence, &event_fields);
    Ok(())
}

fn running_event_content(
    fields: &Value,
    managed_session_id: &str,
    inner_session_id: &str,
    inner_agent_id: &str,
    managed_agent_id: &str,
    parent_session_id: &str,
    environment_id: &str,
    action_name: &str,
) -> Value {
    let context = managed_session_event_context(
        fields,
        managed_session_id,
        inner_session_id,
        inner_agent_id,
        managed_agent_id,
        parent_session_id,
        environment_id,
        action_name,
    );
    with_session_event_context(&context, json!({
        "Content": serde_json::to_string(&json!({
            "observability_event": "temperpaw.agent.session",
            "managed_session_id": managed_session_id,
            "inner_session_id": inner_session_id,
            "inner_agent_id": inner_agent_id,
            "managed_agent_id": managed_agent_id,
            "parent_session_id": parent_session_id,
            "environment_id": environment_id,
            "action_name": action_name,
        })).unwrap_or_else(|_| "{}".to_string()),
    }))
}

trait Pipe: Sized {
    fn pipe<T, F: FnOnce(Self) -> T>(self, func: F) -> T {
        func(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_event_content_records_bridge_observability_context() {
        let content = running_event_content(
            &json!({ "ParentSessionId": "parent-session-1" }),
            "managed-session-1",
            "inner-session-1",
            "inner-agent-1",
            "managed-agent-1",
            "parent-session-1",
            "environment-1",
            "ManagedAgents.StartSession",
        );
        let raw = content["Content"].as_str().expect("content should be JSON");
        let parsed: Value = serde_json::from_str(raw).expect("content JSON should parse");

        assert_eq!(
            content["ObservabilityEvent"],
            "temperpaw.agent.session"
        );
        assert_eq!(content["ManagedSessionId"], "managed-session-1");
        assert_eq!(content["InnerSessionId"], "inner-session-1");
        assert_eq!(content["InnerAgentId"], "inner-agent-1");
        assert_eq!(content["ManagedAgentId"], "managed-agent-1");
        assert_eq!(content["ParentSessionId"], "parent-session-1");
        assert_eq!(content["EnvironmentId"], "environment-1");
        assert_eq!(content["ActionName"], "ManagedAgents.StartSession");
        assert_eq!(
            parsed["observability_event"],
            "temperpaw.agent.session"
        );
        assert_eq!(parsed["managed_session_id"], "managed-session-1");
        assert_eq!(parsed["inner_session_id"], "inner-session-1");
        assert_eq!(parsed["inner_agent_id"], "inner-agent-1");
        assert_eq!(parsed["managed_agent_id"], "managed-agent-1");
        assert_eq!(parsed["parent_session_id"], "parent-session-1");
        assert_eq!(parsed["environment_id"], "environment-1");
        assert_eq!(parsed["action_name"], "ManagedAgents.StartSession");
    }
}
