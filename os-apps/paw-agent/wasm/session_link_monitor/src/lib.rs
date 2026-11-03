use temper_wasm_sdk::prelude::*;
use wasm_helpers::{entity_field_str, resolve_temper_api_url, runtime_headers_as};

const TERMINAL_STATUSES: &[&str] = &[
    "Completed",
    "Failed",
    "Cancelled",
    "Archived",
    "Destroyed",
    "Terminated",
];

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let api_url = resolve_temper_api_url(&ctx, &fields);
        let headers = runtime_headers_as(
            &ctx,
            &ctx.tenant,
            &fields,
            "system",
            Some("application/json"),
            Some("application/json"),
        );

        let link = SessionLinkFields::from_entity(&fields)?;
        let parent = get_entity(
            &ctx,
            &api_url,
            &headers,
            &link.parent_entity_set,
            &link.parent_entity_id,
        )?;
        let parent_status = status_of(&parent);
        if is_terminal_status(&parent_status) {
            set_success_result(
                "ParentNotified",
                &json!({ "LastChildStatus": format!("parent:{parent_status}") }),
            );
            return Ok(());
        }

        let child = get_entity(&ctx, &api_url, &headers, "Sessions", &link.child_session_id)?;
        let child_status = status_of(&child);
        match child_status.as_str() {
            "Completed" => {
                dispatch_parent_completed(&ctx, &api_url, &headers, &link, &child)?;
                set_success_result(
                    "ParentNotified",
                    &json!({ "LastChildStatus": child_status }),
                );
            }
            "Failed" | "Cancelled" => {
                dispatch_parent_failed(&ctx, &api_url, &headers, &link, &child, &child_status)?;
                set_success_result(
                    "ParentNotified",
                    &json!({ "LastChildStatus": child_status }),
                );
            }
            _ => {
                let check_count = field_i64(&fields, &["CheckCount", "check_count"]);
                if check_count >= link.max_checks {
                    let error = format!(
                        "Child Session {} did not reach a terminal state after {} checks.",
                        link.child_session_id, link.max_checks
                    );
                    dispatch_parent_error(&ctx, &api_url, &headers, &link, "TimedOut", &error)?;
                    set_success_result("ParentNotified", &json!({ "LastChildStatus": "TimedOut" }));
                } else {
                    set_success_result("ChildPending", &json!({ "LastChildStatus": child_status }));
                }
            }
        }

        Ok(())
    })();

    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

struct SessionLinkFields {
    parent_entity_set: String,
    parent_entity_id: String,
    parent_action_namespace: String,
    child_session_id: String,
    on_completed_action: String,
    on_failure_action: String,
    max_checks: i64,
}

impl SessionLinkFields {
    fn from_entity(fields: &Value) -> Result<Self, String> {
        let parent_entity_set = require_field(fields, &["ParentEntitySet", "parent_entity_set"])?;
        let parent_entity_id = require_field(fields, &["ParentEntityId", "parent_entity_id"])?;
        let child_session_id = require_field(fields, &["ChildSessionId", "child_session_id"])?;
        let parent_action_namespace = non_empty_field(
            fields,
            &["ParentActionNamespace", "parent_action_namespace"],
        )
        .unwrap_or_else(|| "TemperPaw".to_string());
        let on_completed_action =
            non_empty_field(fields, &["OnCompletedAction", "on_completed_action"])
                .unwrap_or_default();
        let on_failure_action = require_field(fields, &["OnFailureAction", "on_failure_action"])?;
        let max_checks = non_empty_field(fields, &["MaxChecks", "max_checks"])
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(80)
            .max(1);

        Ok(Self {
            parent_entity_set,
            parent_entity_id,
            parent_action_namespace,
            child_session_id,
            on_completed_action,
            on_failure_action,
            max_checks,
        })
    }
}

fn get_entity(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    entity_set: &str,
    entity_id: &str,
) -> Result<Value, String> {
    let url = format!(
        "{api_url}/tdata/{entity_set}('{}')",
        escape_odata_string(entity_id)
    );
    let response = ctx.http_call("GET", &url, headers, "")?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "GET {entity_set}('{entity_id}') failed: HTTP {}: {}",
            response.status,
            response.body.chars().take(500).collect::<String>()
        ));
    }
    serde_json::from_str(&response.body)
        .map_err(|err| format!("parse {entity_set}('{entity_id}') response: {err}"))
}

fn dispatch_parent_completed(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    link: &SessionLinkFields,
    child: &Value,
) -> Result<(), String> {
    if link.on_completed_action.trim().is_empty() {
        return Ok(());
    }
    let result = field_string(child, &["Result", "result"]);
    dispatch_parent_action(
        ctx,
        api_url,
        headers,
        link,
        &link.on_completed_action,
        &json!({
            "output": if result.is_empty() { "{}" } else { result.as_str() },
            "child_session_id": link.child_session_id,
            "ChildSessionId": link.child_session_id,
            "child_status": "Completed",
            "ChildStatus": "Completed",
        }),
    )
}

fn dispatch_parent_failed(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    link: &SessionLinkFields,
    child: &Value,
    child_status: &str,
) -> Result<(), String> {
    let error = field_string(child, &["ErrorMessage", "error_message"]);
    let result = field_string(child, &["Result", "result"]);
    let message = if !error.is_empty() {
        error
    } else if !result.is_empty() {
        result
    } else {
        format!(
            "Child Session {} ended with status {child_status}.",
            link.child_session_id
        )
    };
    dispatch_parent_error(ctx, api_url, headers, link, child_status, &message)
}

fn dispatch_parent_error(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    link: &SessionLinkFields,
    child_status: &str,
    error_message: &str,
) -> Result<(), String> {
    dispatch_parent_action(
        ctx,
        api_url,
        headers,
        link,
        &link.on_failure_action,
        &json!({
            "error_message": error_message,
            "ErrorMessage": error_message,
            "child_session_id": link.child_session_id,
            "ChildSessionId": link.child_session_id,
            "child_status": child_status,
            "ChildStatus": child_status,
        }),
    )
}

fn dispatch_parent_action(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    link: &SessionLinkFields,
    action: &str,
    body: &Value,
) -> Result<(), String> {
    let url = format!(
        "{api_url}/tdata/{entity_set}('{entity_id}')/{namespace}.{action}",
        entity_set = link.parent_entity_set,
        entity_id = escape_odata_string(&link.parent_entity_id),
        namespace = link.parent_action_namespace,
    );
    let response = ctx.http_call("POST", &url, headers, &body.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "parent action {}.{} failed for {}('{}'): HTTP {}: {}",
            link.parent_action_namespace,
            action,
            link.parent_entity_set,
            link.parent_entity_id,
            response.status,
            response.body.chars().take(500).collect::<String>()
        ));
    }
    Ok(())
}

fn is_terminal_status(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

fn status_of(value: &Value) -> String {
    field_string(value, &["Status", "status"])
}

fn field_i64(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn field_string(value: &Value, keys: &[&str]) -> String {
    entity_field_str(value, keys).unwrap_or("").to_string()
}

fn require_field(value: &Value, keys: &[&str]) -> Result<String, String> {
    non_empty_field(value, keys).ok_or_else(|| format!("missing required field {}", keys[0]))
}

fn non_empty_field(value: &Value, keys: &[&str]) -> Option<String> {
    entity_field_str(value, keys)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn escape_odata_string(value: &str) -> String {
    value.replace('\'', "''")
}
