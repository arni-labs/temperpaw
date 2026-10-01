//! Dispatch `temper.*` and `sandbox.*` method calls to HTTP endpoints.
//!
//! When Monty code calls `temper.create("Issues", {...})`, the Monty
//! interpreter pauses and yields a `FunctionCall`. This module handles
//! that call by making an HTTP request via the WASM host function
//! `ctx.http_call()` and returning the result as JSON.
//!
//! Mirrors the method signatures from `temper-sandbox/src/dispatch.rs`
//! so agents see the exact same Python interface as `mcp__temper__execute`.

use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeSet};
use temper_wasm_sdk::context::{Context, HttpRequest, HttpResponse};
use tool_catalog::{DEFAULT_TOOLS_ENABLED, enabled_tool_set};
use wasm_helpers::{entity_field_str, read_session_from_temperfs};

const MAX_INLINE_SANDBOX_IMAGE_BASE64_CHARS: usize = 16 * 1024;

/// Tools available in plan mode (ADR-004). Blocks sandbox mutation (write, edit)
/// and governance writes. Allows read ops, research, memory, Plan CRUD, and
/// TemperFS writes (for plan documents).
pub const PLAN_MODE_TOOLS: &str = "temper_create,temper_get,temper_list,temper_action,temper_specs,temper_show_spec,temper_save_memory,temper_recall_memory,temper_read,temper_write,temper_write_many,temper_ls,temper_grep,temper_glob,temper_search_history,temper_web_search,temper_web_fetch,temper_get_trajectories,temper_get_insights,read,bash";

// Thread-local storage for the done signal. When an agent calls
// temper.done(result), the result is stored here. After all tool
// calls finish, lib.rs checks this and returns RecordResult instead
// of HandleToolResults, completing the session.
thread_local! {
    static DONE_RESULT: RefCell<Option<String>> = RefCell::new(None);
}

// Thread-local storage for lazily provisioned sandbox (ADR-0022).
// When a sandbox tool is called and no sandbox exists, we provision
// one on-demand and cache (sandbox_url, sandbox_id, provider) here.
// lib.rs reads this after tool execution to persist via HandleToolResults.
thread_local! {
    static LAZY_SANDBOX: RefCell<Option<(String, String, String)>> = RefCell::new(None);
}

// Thread-local storage for Cedar denial. When dispatch detects a
// CEDAR_DENIED_CTX error, the full JSON context is stored here so
// lib.rs can read it after drive_repl_loop returns (even if Monty
// Python code catches the exception).
thread_local! {
    static CEDAR_DENIAL: RefCell<Option<String>> = RefCell::new(None);
    // Dispatch output: when a tool produces important output that the LLM
    // must see (even if the Python code assigns the result to a variable
    // instead of printing it), store it here. The REPL appends it to the
    // tool result if the expression was null/None.
    static DISPATCH_OUTPUT: RefCell<Option<String>> = RefCell::new(None);
    // Image results produced by dispatch calls in the current Monty snippet.
    // The REPL consumes these after execution so a generated PawFS image is
    // still available for Discord attachment extraction even if the Python
    // code prints a summary instead of returning the image object.
    static DISPATCH_IMAGE_RESULTS: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    // Current tool being dispatched (ADR-0037). Set by ToolScope at
    // dispatch() entry, read by internal_headers() to emit
    // X-Temper-Span-* hint headers so the host wraps each outgoing
    // HTTP call in a `tool.<name>` span.
    static CURRENT_TOOL_NAME: RefCell<Option<String>> = RefCell::new(None);
    // The LLM-issued tool_use.id for the tool currently dispatching
    // (ADR-0037). Exposed as `tool.call_id` span attribute so multiple
    // retries of the same tool stay disambiguated in the trace tree.
    static CURRENT_TOOL_CALL_ID: RefCell<Option<String>> = RefCell::new(None);
}

/// RAII guard that sets the active tool metadata on entry and clears on drop.
/// Instantiated once at the top of `dispatch()`.
struct ToolScope;

impl ToolScope {
    fn new(tool_name: String, tool_call_id: Option<String>) -> Self {
        CURRENT_TOOL_NAME.with(|cell| *cell.borrow_mut() = Some(tool_name));
        CURRENT_TOOL_CALL_ID.with(|cell| *cell.borrow_mut() = tool_call_id);
        ToolScope
    }
}

impl Drop for ToolScope {
    fn drop(&mut self) {
        CURRENT_TOOL_NAME.with(|cell| *cell.borrow_mut() = None);
        CURRENT_TOOL_CALL_ID.with(|cell| *cell.borrow_mut() = None);
    }
}

/// Build the span-hint headers for the current tool dispatch, if any.
/// Consumed and stripped by the host's `split_span_hint_headers` (see
/// temper-wasm) so the resulting `wasm.host.http_call` span is renamed
/// `tool.<tool_name>` with queryable `tool.name` / `tool.call_id`
/// attributes.
#[allow(dead_code)]
fn tool_span_hint_headers() -> Vec<(String, String)> {
    let tool_name = CURRENT_TOOL_NAME.with(|cell| cell.borrow().clone());
    let tool_call_id = CURRENT_TOOL_CALL_ID.with(|cell| cell.borrow().clone());
    tool_span_hint_headers_for(tool_name.as_deref(), tool_call_id.as_deref())
}

fn tool_span_hint_headers_for(
    tool_name: Option<&str>,
    tool_call_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(name) = tool_name.filter(|name| !name.is_empty()) {
        headers.push(("X-Temper-Span-Name".to_string(), format!("tool.{name}")));
        headers.push((
            "X-Temper-Span-Attr-gen_ai.operation.name".to_string(),
            "execute_tool".to_string(),
        ));
        headers.push(("X-Temper-Span-Attr-tool.name".to_string(), name.to_string()));
    }
    if let Some(id) = tool_call_id.filter(|id| !id.is_empty()) {
        headers.push((
            "X-Temper-Span-Attr-tool.call_id".to_string(),
            id.to_string(),
        ));
    }
    headers
}

pub(crate) fn internal_headers_for_tool(
    tool_name: &str,
    tool_call_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    headers.extend(tool_span_hint_headers_for(Some(tool_name), tool_call_id));
    headers
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchableToolPlan {
    pub tool_name: String,
    pub kind: BatchableToolPlanKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BatchableToolPlanKind {
    DirectGet {
        path: String,
        unwrap_value_array: bool,
    },
    WebQuerySearch {
        query: String,
    },
    WebQueryFetch {
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchableToolCall {
    pub tool_call_id: String,
    pub plan: BatchableToolPlan,
}

pub(crate) fn batchable_tool_plan_from_code(code: &str) -> Option<BatchableToolPlan> {
    let (object, method, args) = parse_batchable_tool_call(code)?;
    if object != "temper" {
        return None;
    }

    let kind = match method.as_str() {
        "web_search" if args.len() == 1 => BatchableToolPlanKind::WebQuerySearch {
            query: args[0].clone(),
        },
        "web_fetch" if args.len() == 1 => BatchableToolPlanKind::WebQueryFetch {
            url: args[0].clone(),
        },
        _ => {
            let (path, unwrap_value_array) = batchable_direct_get_path(&method, &args)?;
            BatchableToolPlanKind::DirectGet {
                path,
                unwrap_value_array,
            }
        }
    };

    Some(BatchableToolPlan {
        tool_name: format!("{object}.{method}"),
        kind,
    })
}

pub(crate) fn execute_batchable_tool_calls<F>(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    calls: &[BatchableToolCall],
    mut emit_progress: F,
) -> Vec<Result<Value, String>>
where
    F: FnMut(),
{
    let mut results: Vec<Option<Result<Value, String>>> = vec![None; calls.len()];
    let mut direct_calls: Vec<(usize, &BatchableToolCall, String, bool)> = Vec::new();
    let mut web_calls: Vec<(usize, &BatchableToolCall, String, String)> = Vec::new();

    for (index, call) in calls.iter().enumerate() {
        match &call.plan.kind {
            BatchableToolPlanKind::DirectGet {
                path,
                unwrap_value_array,
            } => direct_calls.push((index, call, path.clone(), *unwrap_value_array)),
            BatchableToolPlanKind::WebQuerySearch { query } => {
                web_calls.push((index, call, "search".to_string(), query.clone()))
            }
            BatchableToolPlanKind::WebQueryFetch { url } => {
                web_calls.push((index, call, "fetch".to_string(), url.clone()))
            }
        }
    }

    if !direct_calls.is_empty() {
        emit_progress();
        let requests: Vec<HttpRequest> = direct_calls
            .iter()
            .map(|(_, call, path, _)| HttpRequest {
                method: "GET".to_string(),
                url: format!("{api_url}{}", path.replace("{tenant}", tenant)),
                headers: internal_headers_for_tool(
                    &call.plan.tool_name,
                    Some(call.tool_call_id.as_str()),
                ),
                body: String::new(),
            })
            .collect();

        match ctx.http_call_batch(&requests) {
            Ok(responses) => {
                for ((index, _, path, unwrap_value_array), response) in
                    direct_calls.iter().zip(responses.into_iter())
                {
                    results[*index] = Some(interpret_batch_json_response(
                        path,
                        response,
                        *unwrap_value_array,
                        tenant,
                    ));
                }
            }
            Err(error) => {
                for (index, _, _, _) in &direct_calls {
                    results[*index] = Some(Err(error.clone()));
                }
            }
        }
    }

    if !web_calls.is_empty() {
        execute_batchable_web_query_calls(
            ctx,
            api_url,
            tenant,
            &web_calls,
            &mut results,
            &mut emit_progress,
        );
    }

    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Err("batchable tool result missing".to_string())))
        .collect()
}

/// Take the done result (if set). Clears it after reading.
pub fn take_done_result() -> Option<String> {
    DONE_RESULT.with(|cell| cell.borrow_mut().take())
}

/// Take the lazily provisioned sandbox details (if set). Clears after reading.
/// Returns (sandbox_url, sandbox_id, provider).
pub fn take_lazy_sandbox() -> Option<(String, String, String)> {
    LAZY_SANDBOX.with(|cell| cell.borrow_mut().take())
}

// The approval and completion paths keep their existing control flow. Never
// checkpoint while either signal is pending, even if Python caught a denial.
pub fn continuation_allowed() -> bool {
    CEDAR_DENIAL.with(|v| v.borrow().is_none()) && DONE_RESULT.with(|v| v.borrow().is_none())
}

pub fn continuation_state() -> Value {
    json!({"output":DISPATCH_OUTPUT.with(|v|v.borrow().clone()),
        "images":DISPATCH_IMAGE_RESULTS.with(|v|v.borrow().clone()),
        "sandbox":LAZY_SANDBOX.with(|v|v.borrow().clone())})
}

pub fn restore_continuation_state(state: &Value) -> Result<(), String> {
    let output: Option<String> = serde_json::from_value(state["output"].clone())
        .map_err(|e| format!("invalid continuation output: {e}"))?;
    let images: Vec<Value> = serde_json::from_value(state["images"].clone())
        .map_err(|e| format!("invalid continuation images: {e}"))?;
    let sandbox: Option<(String, String, String)> =
        serde_json::from_value(state["sandbox"].clone())
            .map_err(|e| format!("invalid continuation sandbox: {e}"))?;
    DISPATCH_OUTPUT.with(|v| *v.borrow_mut() = output);
    DISPATCH_IMAGE_RESULTS.with(|v| *v.borrow_mut() = images);
    LAZY_SANDBOX.with(|v| *v.borrow_mut() = sandbox);
    Ok(())
}

/// Take the Cedar denial context (if set). Clears after reading.
pub fn take_cedar_denial() -> Option<String> {
    CEDAR_DENIAL.with(|cell| cell.borrow_mut().take())
}

/// Take the dispatch output (if set). Clears after reading.
pub fn take_dispatch_output() -> Option<String> {
    DISPATCH_OUTPUT.with(|cell| cell.borrow_mut().take())
}

/// Store a message that should be shown to the LLM as tool output.
pub fn set_dispatch_output(msg: &str) {
    DISPATCH_OUTPUT.with(|cell| *cell.borrow_mut() = Some(msg.to_string()));
}

/// Take image results captured during dispatch. Clears after reading.
pub fn take_dispatch_image_results() -> Vec<Value> {
    DISPATCH_IMAGE_RESULTS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Store a generated image result so the REPL can surface it as an attachment.
fn record_dispatch_image_result(result: &Value) {
    if result
        .get("__temperpaw_image")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        DISPATCH_IMAGE_RESULTS.with(|cell| cell.borrow_mut().push(result.clone()));
    }
}

/// Peek at the lazily provisioned sandbox URL without consuming it.
pub fn peek_lazy_sandbox_url() -> Option<String> {
    LAZY_SANDBOX.with(|cell| cell.borrow().as_ref().map(|(url, _, _)| url.clone()))
}

/// Peek at the lazily provisioned sandbox provider without consuming it.
pub fn peek_lazy_sandbox_provider() -> Option<String> {
    LAZY_SANDBOX.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|(_, _, provider)| provider.clone())
    })
}

/// Peek at the lazily provisioned sandbox ID without consuming it.
pub fn peek_lazy_sandbox_id() -> Option<String> {
    LAZY_SANDBOX.with(|cell| cell.borrow().as_ref().map(|(_, id, _)| id.clone()))
}

fn sandbox_identity_from_fields(fields: &Value) -> (Option<String>, Option<String>) {
    let lazy = LAZY_SANDBOX.with(|cell| cell.borrow().clone());

    let sandbox_id = fields
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| lazy.as_ref().map(|(_, id, _)| id.clone()));

    let provider = fields
        .get("sandbox_provider")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| lazy.as_ref().map(|(_, _, provider)| provider.clone()));

    (sandbox_id, provider)
}

/// Dispatch a `temper.<method>()` or `sandbox.<method>()` call.
///
/// Called by the Monty event loop when user code invokes a method on a
/// dataclass object. `obj_name` is `"temper"` or `"sandbox"`, `method`
/// is the method name, and `args` are the JSON-converted positional args.
pub fn dispatch(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    workdir: &str,
    obj_name: &str,
    method: &str,
    tool_call_id: Option<&str>,
    args: &[Value],
    kwargs: &[(Value, Value)],
) -> Result<Value, String> {
    // Activate the tool-scope guard so every ctx.http_call made through
    // internal_headers() during this dispatch carries span-hint headers
    // identifying the caller (ADR-0037). Cleared on drop at function exit.
    let _tool_scope = ToolScope::new(
        format!("{obj_name}.{method}"),
        tool_call_id.map(str::to_string),
    );

    // Lazy sandbox provisioning (ADR-0022): if this tool needs a sandbox and
    // none is attached, provision one on-demand instead of failing.
    let needs_sandbox = obj_name == "sandbox"
        || (obj_name == "temper"
            && matches!(method, "run_coding_agent" | "publish_app" | "update_app"));

    let effective_sandbox_url = if needs_sandbox && sandbox_url.is_empty() {
        // Check thread-local cache first (already provisioned this invocation)
        if let Some(url) = peek_lazy_sandbox_url() {
            url
        } else {
            // Lazy provision
            match lazy_provision_sandbox(ctx, temper_api_url, tenant) {
                Ok(url) => url,
                Err(e) => {
                    return Err(format!(
                        "This tool requires a code execution sandbox, but sandbox provisioning failed: {e}. \
                         You can still use non-sandbox tools (temper.create, temper.list, temper.web_search, etc.) \
                         to help the user."
                    ));
                }
            }
        }
    } else {
        sandbox_url.to_string()
    };

    ensure_method_enabled(ctx, obj_name, method, &effective_sandbox_url)?;
    let fields_for_sandbox = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let (sandbox_id, cached_provider) = sandbox_identity_from_fields(&fields_for_sandbox);
    let provider = cached_provider.unwrap_or_else(|| {
        wasm_helpers::sandbox::resolve_sandbox_provider(ctx, &fields_for_sandbox)
            .unwrap_or_else(|_| "tensorlake".to_string())
    });
    let result = match obj_name {
        "temper" => {
            // Coalesce kwargs into args[0] when caller used keyword form
            // (`temper.list_sessions(filter="x", top=10)`). Most entity-ops
            // methods accept an input dict as the first positional arg, so
            // this unblocks the natural Python-style call shape without
            // changing any per-method signature. Methods that don't expect
            // a dict will surface their own clearer error.
            let temper_args: Vec<Value> = if args.is_empty() && !kwargs.is_empty() {
                let mut obj = serde_json::Map::new();
                for (k, v) in kwargs {
                    if let Some(key) = k.as_str() {
                        obj.insert(key.to_string(), v.clone());
                    }
                }
                vec![Value::Object(obj)]
            } else if !args.is_empty() && !kwargs.is_empty() {
                return Err(format!(
                    "temper.{method}() does not accept mixed positional and keyword arguments — pass either a single dict or kwargs"
                ));
            } else {
                args.to_vec()
            };
            dispatch_temper(
                ctx,
                temper_api_url,
                tenant,
                &effective_sandbox_url,
                sandbox_id.as_deref().unwrap_or(""),
                provider.as_str(),
                workdir,
                method,
                &temper_args,
            )
        }
        "sandbox" => {
            let sandbox_args = coalesce_sandbox_args(method, args, kwargs)?;
            dispatch_sandbox(
                ctx,
                &effective_sandbox_url,
                sandbox_id.as_deref().unwrap_or(""),
                &provider,
                workdir,
                method,
                &sandbox_args,
            )
        }
        "json" => dispatch_json(method, args, kwargs),
        _ => Err(format!("unknown object: {obj_name}")),
    };

    // Enrich Cedar denial errors with tool context so the REPL loop can
    // build a complete pause payload (decision ID + what was being attempted).
    if let Err(ref e) = result {
        if let Some(rest) = e.strip_prefix("CEDAR_DENIED:") {
            // rest = "{decision_id}:{body}" — split on first ':'
            let (decision_id, body) = rest.split_once(':').unwrap_or((rest, ""));
            let ctx_json = json!({
                "decision_id": decision_id,
                "method": format!("{obj_name}.{method}"),
                "args": args,
                "body": body,
            });
            // Store in thread-local so lib.rs can detect the denial even if
            // Monty Python code catches the RuntimeError exception.
            let ctx_str = ctx_json.to_string();
            CEDAR_DENIAL.with(|cell| {
                *cell.borrow_mut() = Some(ctx_str.clone());
            });
            return Err(format!("CEDAR_DENIED_CTX:{ctx_str}"));
        }
    }

    result
}

fn ensure_method_enabled(
    ctx: &Context,
    obj_name: &str,
    method: &str,
    sandbox_url: &str,
) -> Result<(), String> {
    let enabled = enabled_tools(ctx);

    match obj_name {
        "sandbox" => {
            if sandbox_url.is_empty() {
                return Err("sandbox is not configured for this session".to_string());
            }

            let Some(token) = sandbox_method_token(method) else {
                return Ok(());
            };
            if enabled.contains(token) {
                return Ok(());
            }

            Err(format!(
                "sandbox.{method}() is not enabled for this session. Enabled tools: {}",
                format_enabled_tools(&enabled)
            ))
        }
        "temper" => {
            let Some(token) = temper_method_token(method) else {
                return Ok(());
            };
            if enabled.contains(token) {
                return Ok(());
            }

            Err(format!(
                "temper.{method}() is not enabled for this session. Enabled tools: {}",
                format_enabled_tools(&enabled)
            ))
        }
        "json" => {
            if method == "dumps" || method == "loads" {
                Ok(())
            } else {
                Err(format!(
                    "json.{method}() is not available. Available: json.dumps, json.loads"
                ))
            }
        }
        _ => Ok(()),
    }
}

fn reject_kwargs(obj_name: &str, method: &str, kwargs: &[(Value, Value)]) -> Result<(), String> {
    if kwargs.is_empty() {
        return Ok(());
    }

    let names = kwargs
        .iter()
        .filter_map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{obj_name}.{method}() does not support keyword arguments{}",
        if names.is_empty() {
            ".".to_string()
        } else {
            format!(": {names}")
        }
    ))
}

fn coalesce_sandbox_args(
    method: &str,
    args: &[Value],
    kwargs: &[(Value, Value)],
) -> Result<Vec<Value>, String> {
    if kwargs.is_empty() {
        return Ok(args.to_vec());
    }

    if method == "read" && args.len() == 1 {
        let mut opts = serde_json::Map::new();
        for (key, value) in kwargs {
            if let Some(key) = key.as_str() {
                opts.insert(key.to_string(), value.clone());
            }
        }
        return Ok(vec![args[0].clone(), Value::Object(opts)]);
    }

    reject_kwargs("sandbox", method, kwargs)?;
    Ok(args.to_vec())
}

fn dispatch_json(method: &str, args: &[Value], kwargs: &[(Value, Value)]) -> Result<Value, String> {
    match method {
        "dumps" => json_dumps(args, kwargs),
        "loads" => json_loads(args, kwargs),
        _ => Err(format!(
            "unknown json method '{method}'. Available: dumps, loads"
        )),
    }
}

fn json_dumps(args: &[Value], kwargs: &[(Value, Value)]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("json.dumps(): missing required argument 'obj'".to_string());
    }
    if args.len() > 1 {
        return Err("json.dumps(): only the object positional argument is supported".to_string());
    }

    let mut pretty = false;
    for (key, value) in kwargs {
        let Some(name) = key.as_str() else {
            return Err("json.dumps(): keyword names must be strings".to_string());
        };
        match name {
            "ensure_ascii" | "sort_keys" => {
                // serde_json already emits valid UTF-8 JSON and object order is stable for
                // serde_json::Map in this path, so these flags are accepted for prompt
                // compatibility without changing semantics.
                if !value.is_boolean() {
                    return Err(format!("json.dumps(): {name} must be bool"));
                }
            }
            "indent" => {
                if value.is_null() {
                    pretty = false;
                } else if value.as_i64().is_some() {
                    pretty = true;
                } else {
                    return Err("json.dumps(): indent must be an int or None".to_string());
                }
            }
            "separators" => {
                // Compact/default separators are equivalent for the entity-field use cases.
            }
            other => {
                return Err(format!(
                    "json.dumps(): keyword argument '{other}' is not supported"
                ));
            }
        }
    }

    let serialized = if pretty {
        serde_json::to_string_pretty(&args[0])
    } else {
        serde_json::to_string(&args[0])
    }
    .map_err(|e| format!("json.dumps(): failed to serialize value: {e}"))?;

    Ok(Value::String(serialized))
}

fn json_loads(args: &[Value], kwargs: &[(Value, Value)]) -> Result<Value, String> {
    if !kwargs.is_empty() {
        return Err("json.loads(): keyword arguments are not supported".to_string());
    }
    if args.len() != 1 {
        return Err("json.loads(): expected exactly one string argument".to_string());
    }
    let raw = args[0]
        .as_str()
        .ok_or_else(|| "json.loads(): expected a string argument".to_string())?;
    serde_json::from_str(raw).map_err(|e| format!("json.loads(): invalid JSON: {e}"))
}

fn enabled_tools(ctx: &Context) -> BTreeSet<String> {
    enabled_tool_set(
        ctx.entity_state
            .get("fields")
            .and_then(|fields| fields.get("tools_enabled"))
            .and_then(|value| value.as_str())
            .unwrap_or(DEFAULT_TOOLS_ENABLED),
    )
}

fn format_enabled_tools(enabled: &BTreeSet<String>) -> String {
    if enabled.is_empty() {
        "(none)".to_string()
    } else {
        enabled.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn sandbox_method_token(method: &str) -> Option<&'static str> {
    match method {
        "bash" => Some("bash"),
        "read" => Some("read"),
        "write" => Some("write"),
        "edit" => Some("edit"),
        _ => None,
    }
}

fn temper_method_token(method: &str) -> Option<&'static str> {
    match method {
        "get" => Some("temper_get"),
        "list" => Some("temper_list"),
        "create" => Some("temper_create"),
        "action" => Some("temper_action"),
        "patch" => Some("temper_patch"),
        "submit_specs" => Some("temper_submit_specs"),
        "show_spec" | "spec_detail" => Some("temper_show_spec"),
        "specs" => Some("temper_specs"),
        "upload_wasm" => Some("temper_upload_wasm"),
        "get_trajectories" => Some("temper_get_trajectories"),
        "get_insights" => Some("temper_get_insights"),
        "get_decisions" => Some("temper_get_decisions"),
        "poll_decision" => Some("temper_poll_decision"),
        "approve_decision" => Some("temper_approve_decision"),
        "deny_decision" => Some("temper_deny_decision"),
        "submit_policy" => Some("temper_submit_policy"),
        "list_policies" => Some("temper_list_policies"),
        "get_policy" => Some("temper_get_policy"),
        "update_policy" => Some("temper_update_policy"),
        "delete_policy" => Some("temper_delete_policy"),
        "search_apps" => Some("temper_search_apps"),
        "install_app" => Some("temper_install_app"),
        "publish_app" => Some("temper_publish_app"),
        "update_app" => Some("temper_update_app"),
        "list_apps" => Some("temper_list_apps"),
        "spawn_session" => Some("temper_spawn_session"),
        "list_sessions" => Some("temper_list_sessions"),
        "abort_session" => Some("temper_abort_session"),
        "steer_session" => Some("temper_steer_session"),
        "save_memory" => Some("temper_save_memory"),
        "recall_memory" => Some("temper_recall_memory"),
        "write" => Some("temper_write"),
        "write_many" => Some("temper_write_many"),
        "read" => Some("temper_read"),
        "ls" => Some("temper_ls"),
        "grep" => Some("temper_grep"),
        "glob" => Some("temper_glob"),
        "edit" => Some("temper_edit"),
        "rename" => Some("temper_rename"),
        "search_history" => Some("temper_search_history"),
        "run_coding_agent" => Some("temper_run_coding_agent"),
        "get_secret" => Some("temper_get_secret"),
        "datadog_query" => Some("temper_datadog_query"),
        "railway" => Some("temper_railway"),
        "vercel" => Some("temper_vercel"),
        "web_search" => Some("temper_web_search"),
        "web_fetch" => Some("temper_web_fetch"),
        "image_generate" => Some("temper_image_generate"),
        "done" | "get_agent_id" | "get_session_id" | "switch_provider" | "switch_mode" => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Temper dispatch
// ---------------------------------------------------------------------------

fn dispatch_temper(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    sandbox_id: &str,
    sandbox_provider: &str,
    workdir: &str,
    method: &str,
    args: &[Value],
) -> Result<Value, String> {
    match method {
        // Entity CRUD
        "list" => temper_list(ctx, api_url, tenant, args),
        "get" => temper_get(ctx, api_url, tenant, args),
        "create" => temper_create(ctx, api_url, tenant, args),
        "action" => temper_action(ctx, api_url, tenant, args),
        "patch" => temper_patch(ctx, api_url, tenant, args),

        // Specs
        "submit_specs" => temper_submit_specs(ctx, api_url, tenant, args),
        "show_spec" | "spec_detail" => temper_show_spec(ctx, api_url, tenant, args),
        "specs" => temper_specs(ctx, api_url, tenant),

        // WASM
        "upload_wasm" => temper_upload_wasm(ctx, api_url, tenant, args),

        // Evolution
        "get_trajectories" => temper_get_trajectories(ctx, api_url, tenant, args),
        "get_insights" => temper_get_insights(ctx, api_url, tenant),

        // Governance — decisions
        "get_decisions" => temper_get_decisions(ctx, api_url, tenant),
        "poll_decision" => temper_poll_decision(ctx, api_url, tenant, args),
        "approve_decision" => temper_approve_decision(ctx, api_url, tenant, args),
        "deny_decision" => temper_deny_decision(ctx, api_url, tenant, args),

        // Governance — Cedar policies (all Cedar-gated by platform)
        "submit_policy" => temper_submit_policy(ctx, api_url, tenant, args),
        "list_policies" => temper_list_policies(ctx, api_url, tenant),
        "get_policy" => temper_get_policy(ctx, api_url, tenant, args),
        "update_policy" => temper_update_policy(ctx, api_url, tenant, args),
        "delete_policy" => temper_delete_policy(ctx, api_url, tenant, args),

        // Apps
        "install_app" => temper_install_app(ctx, api_url, tenant, args),
        "search_apps" => temper_search_apps(ctx, args),
        "publish_app" => temper_publish_app(
            ctx,
            sandbox_url,
            sandbox_id,
            sandbox_provider,
            workdir,
            args,
        ),
        "update_app" => temper_update_app(
            ctx,
            sandbox_url,
            sandbox_id,
            sandbox_provider,
            workdir,
            args,
        ),
        "list_apps" => temper_list_apps(ctx, api_url, tenant),

        // Agent identity
        "get_agent_id" => {
            let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
            let agent_id = fields
                .get("agent_id")
                .or_else(|| fields.get("AgentId"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!(agent_id))
        }
        "get_session_id" => {
            let session_id = ctx
                .entity_state
                .get("entity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!(session_id))
        }

        // Switch own LLM provider/model mid-session
        "switch_provider" => {
            let input = args
                .first()
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let model = input.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let provider = input.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let provider_options_json = input
                .get("provider_options_json")
                .or_else(|| input.get("providerOptionsJson"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if model.is_empty() && provider.is_empty() && provider_options_json.is_empty() {
                return Err(
                    "switch_provider requires at least one of: model, provider, provider_options_json"
                        .into(),
                );
            }
            let agent_id = ctx
                .entity_state
                .get("entity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut body = serde_json::Map::new();
            if !model.is_empty() {
                body.insert("model".into(), json!(model));
            }
            if !provider.is_empty() {
                body.insert("provider".into(), json!(provider));
            }
            if !provider_options_json.is_empty() {
                body.insert("provider_options_json".into(), json!(provider_options_json));
            }
            let url = format!("{api_url}/tdata/Sessions('{agent_id}')/TemperPaw.SwitchProvider");
            let headers = internal_headers();
            let resp = ctx.http_call("POST", &url, &headers, &json!(body).to_string())?;
            if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
                return Err(denial);
            }
            if resp.status >= 200 && resp.status < 300 {
                Ok(json!({
                    "switched": true,
                    "model": if model.is_empty() { "unchanged" } else { model },
                    "provider": if provider.is_empty() { "unchanged" } else { provider },
                    "provider_options_json": if provider_options_json.is_empty() { "unchanged" } else { provider_options_json },
                }))
            } else {
                Err(format!(
                    "SwitchProvider failed (HTTP {}): {}",
                    resp.status,
                    &resp.body[..resp.body.len().min(200)]
                ))
            }
        }

        // Switch session mode between plan and execute (ADR-004)
        "switch_mode" => {
            let input = args
                .first()
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let target_mode = input.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            if target_mode != "plan" && target_mode != "execute" {
                return Err("switch_mode requires mode='plan' or mode='execute'".into());
            }
            let agent_id = ctx
                .entity_state
                .get("entity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
            let current_tools = fields
                .get("tools_enabled")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_TOOLS_ENABLED);

            let mut body = serde_json::Map::new();
            body.insert("session_mode".into(), json!(target_mode));

            if target_mode == "plan" {
                // Stash current tools so they can be restored on switch to execute
                body.insert("pre_plan_tools_enabled".into(), json!(current_tools));
                body.insert("tools_enabled".into(), json!(PLAN_MODE_TOOLS));
            } else {
                // Restore stashed tools
                let stashed = fields
                    .get("pre_plan_tools_enabled")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(DEFAULT_TOOLS_ENABLED);
                body.insert("tools_enabled".into(), json!(stashed));
                body.insert("pre_plan_tools_enabled".into(), json!(""));
            }

            // Carry active_plan_id if provided
            if let Some(plan_id) = input.get("plan_id").and_then(|v| v.as_str()) {
                body.insert("active_plan_id".into(), json!(plan_id));
            }

            let url = format!("{api_url}/tdata/Sessions('{agent_id}')/TemperPaw.SwitchMode");
            let headers = internal_headers();
            let resp = ctx.http_call("POST", &url, &headers, &json!(body).to_string())?;
            if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
                return Err(denial);
            }
            if resp.status >= 200 && resp.status < 300 {
                Ok(json!({
                    "switched": true,
                    "mode": target_mode,
                }))
            } else {
                Err(format!(
                    "SwitchMode failed (HTTP {}): {}",
                    resp.status,
                    &resp.body[..resp.body.len().min(200)]
                ))
            }
        }

        // Entity operations (ported from tool_runner)
        "spawn_session" => {
            super::entity_ops::spawn_session(ctx, api_url, tenant, sandbox_url, workdir, args)
        }
        "list_sessions" => super::entity_ops::list_sessions(ctx, api_url, tenant, args),
        "abort_session" => super::entity_ops::abort_session(ctx, api_url, tenant, args),
        "steer_session" => super::entity_ops::steer_session(ctx, api_url, tenant, args),
        "save_memory" => super::entity_ops::save_memory(ctx, api_url, tenant, args),
        "recall_memory" => super::entity_ops::recall_memory(ctx, api_url, tenant, args),
        "write" => super::entity_ops::write_with_sandbox(ctx, api_url, tenant, sandbox_url, args),
        "write_many" => {
            super::entity_ops::write_many_with_sandbox(ctx, api_url, tenant, sandbox_url, args)
        }
        "read" => super::entity_ops::read(ctx, api_url, tenant, args),
        "ls" => super::entity_ops::ls(ctx, api_url, tenant, args),
        "grep" => super::entity_ops::grep(ctx, api_url, tenant, args),
        "glob" => super::entity_ops::glob_files(ctx, api_url, tenant, args),
        "edit" => super::entity_ops::edit(ctx, api_url, tenant, args),
        "rename" => super::entity_ops::rename(ctx, api_url, tenant, args),
        "search_history" => super::entity_ops::search_history(ctx, api_url, tenant, args),
        "run_coding_agent" => {
            super::entity_ops::run_coding_agent(ctx, api_url, tenant, sandbox_url, workdir, args)
        }

        // Secrets (Cedar-gated via access_secret on Secret resource)
        "get_secret" => temper_get_secret(ctx, args),

        // Session completion — agent signals "I'm done"
        "done" => temper_done(args),

        // External service integrations (ported from tool_runner)
        "datadog_query" => super::datadog::datadog_query(ctx, args),
        "railway" => super::railway::railway(ctx, args),
        "vercel" => super::vercel::vercel(ctx, args),

        // Web research (backed by standalone WASM modules via WebQuery entity)
        "web_search" => temper_web_search(ctx, api_url, tenant, args),
        "web_fetch" => temper_web_fetch(ctx, api_url, tenant, args),
        "image_generate" => temper_image_generate(ctx, api_url, tenant, args),

        _ => Err(format!(
            "unknown temper method '{method}'. Available: \
             list, get, create, action, patch, submit_specs, show_spec, \
             upload_wasm, get_trajectories, get_insights, \
             get_decisions, poll_decision, approve_decision, deny_decision, \
             submit_policy, list_policies, get_policy, update_policy, delete_policy, \
             get_secret, done, install_app, search_apps, publish_app, update_app, list_apps, get_agent_id, get_session_id, \
             spawn_session, list_sessions, abort_session, steer_session, \
             save_memory, recall_memory, write, read, ls, grep, glob, edit, rename, \
             search_history, run_coding_agent, datadog_query, railway, vercel, \
             web_search, web_fetch, image_generate"
        )),
    }
}

// --- Entity CRUD ---

fn temper_list(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_set = str_arg(args, 0, "entity_set", "list")?;
    let query = opt_str_arg(args, 1).and_then(|arg| normalize_odata_query_arg(&arg));
    let path = match query {
        Some(ODataQueryArg::Filter(filter)) => {
            let encoded = encode_odata_query_value(&filter);
            format!("/tdata/{entity_set}?$filter={encoded}")
        }
        Some(ODataQueryArg::Raw(raw_query)) => {
            let encoded = encode_odata_query_value(&raw_query);
            format!("/tdata/{entity_set}?{encoded}")
        }
        None => format!("/tdata/{entity_set}"),
    };
    let resp = http_get(ctx, api_url, tenant, &path)?;
    Ok(resp.get("value").cloned().unwrap_or(resp))
}

fn temper_get(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let entity_set = str_arg(args, 0, "entity_set", "get")?;
    let entity_id = str_arg(args, 1, "entity_id", "get")?;
    let key = escape_odata_key(&entity_id);
    http_get(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/{entity_set}('{key}')"),
    )
}

#[derive(Debug, PartialEq, Eq)]
enum ODataQueryArg {
    Filter(String),
    Raw(String),
}

fn normalize_odata_query_arg(arg: &str) -> Option<ODataQueryArg> {
    let trimmed = arg.trim().trim_start_matches('?');
    if trimmed.is_empty() {
        return None;
    }
    if let Some(filter) = trimmed.strip_prefix("$filter=") {
        let filter = filter.trim();
        if filter.is_empty() {
            None
        } else {
            Some(ODataQueryArg::Filter(filter.to_string()))
        }
    } else if trimmed.starts_with('$') {
        Some(ODataQueryArg::Raw(trimmed.to_string()))
    } else {
        Some(ODataQueryArg::Filter(trimmed.to_string()))
    }
}

fn encode_odata_query_value(value: &str) -> String {
    value.replace(' ', "%20").replace('\'', "%27")
}

fn temper_create(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_set = str_arg(args, 0, "entity_set", "create")?;
    let body =
        with_session_parent_provenance(ctx, &entity_set, obj_arg(args, 1, "fields", "create")?);
    http_post(ctx, api_url, tenant, &format!("/tdata/{entity_set}"), &body)
}

fn with_session_parent_provenance(ctx: &Context, entity_set: &str, mut body: Value) -> Value {
    if matches!(entity_set, "ManagedSessions" | "CurationJobs") {
        let has_parent = entity_field_str(&body, &["ParentSessionId", "parent_session_id"])
            .is_some_and(|value| !value.trim().is_empty());
        let parent_session_id = if entity_set == "CurationJobs" {
            approval_route_parent_session_id(ctx)
        } else {
            current_session_id(ctx)
        };
        if !has_parent
            && let Some(parent_session_id) = parent_session_id
            && let Some(object) = body.as_object_mut()
        {
            object.insert("ParentSessionId".to_string(), json!(parent_session_id));
        }
    }
    body
}

fn with_curation_job_action_parent(
    ctx: &Context,
    entity_set: &str,
    action_name: &str,
    mut body: Value,
) -> Value {
    if entity_set == "CurationJobs" && matches!(action_name, "Configure" | "ConfigureAndSubmit") {
        let has_parent = entity_field_str(&body, &["parent_session_id", "ParentSessionId"])
            .is_some_and(|value| !value.trim().is_empty());
        if !has_parent
            && let Some(parent_session_id) = approval_route_parent_session_id(ctx)
            && let Some(object) = body.as_object_mut()
        {
            object.insert("parent_session_id".to_string(), json!(parent_session_id));
        }
    }
    body
}

fn current_session_id(ctx: &Context) -> Option<String> {
    ctx.entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("ss-"))
        .map(str::to_string)
}

fn approval_route_parent_session_id(ctx: &Context) -> Option<String> {
    ctx.entity_state
        .get("fields")
        .and_then(|fields| entity_field_str(fields, &["parent_session_id", "ParentSessionId"]))
        .filter(|value| value.starts_with("ss-"))
        .map(str::to_string)
        .or_else(|| current_session_id(ctx))
}

fn temper_action(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_set = str_arg(args, 0, "entity_set", "action")?;
    let entity_id = str_arg(args, 1, "entity_id", "action")?;
    let action_name = str_arg(args, 2, "action_name", "action")?;
    let body =
        with_curation_job_action_parent(ctx, &entity_set, &action_name, obj_arg_or_empty(args, 3));
    let key = escape_odata_key(&entity_id);
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/{entity_set}('{key}')/Temper.{action_name}"),
        &body,
    )
}

fn temper_patch(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_set = str_arg(args, 0, "entity_set", "patch")?;
    let entity_id = str_arg(args, 1, "entity_id", "patch")?;
    let body = obj_arg(args, 2, "fields", "patch")?;
    let key = escape_odata_key(&entity_id);
    http_patch(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/{entity_set}('{key}')"),
        &body,
    )
}

// --- Specs ---

fn temper_submit_specs(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let specs = obj_arg(args, 0, "specs", "submit_specs")?;
    let spec_names: Vec<String> = specs
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let has_model = has_model_csdl(&spec_names);
    if !has_model {
        return Err(
            "temper.submit_specs requires a spec bundle containing model.csdl.xml. \
             Include model.csdl.xml plus one or more *.ioa.toml files; nested paths are allowed."
                .to_string(),
        );
    }
    // Validate all spec values are strings (LoadInlineRequest expects BTreeMap<String, String>).
    if let Some(obj) = specs.as_object() {
        for (key, val) in obj {
            if !val.is_string() {
                let vtype = if val.is_object() {
                    "object"
                } else if val.is_array() {
                    "array"
                } else if val.is_number() {
                    "number"
                } else if val.is_boolean() {
                    "boolean"
                } else {
                    "non-string"
                };
                return Err(format!(
                    "temper.submit_specs(): spec value for '{}' must be a TOML/XML/JSON string, \
                     not a {} value. Wrap the content in a string.",
                    key, vtype
                ));
            }
        }
    }
    let body = json!({ "tenant": tenant, "specs": specs });
    let result = http_post(ctx, api_url, tenant, "/api/specs/load-inline", &body)?;
    // Surface the outcome via dispatch output so the LLM sees it even
    // if the Python code assigns the return value to a variable.
    let msg = if result.get("ok").and_then(|v| v.as_bool()) == Some(true) || result.is_null() {
        format!("Specs loaded successfully: {}", spec_names.join(", "))
    } else {
        format!("submit_specs response: {result}")
    };
    ctx.log("info", &format!("submit_specs: {msg}"));
    set_dispatch_output(&msg);
    Ok(json!({
        "ok": true,
        "message": msg,
        "specs_submitted": spec_names,
    }))
}

fn has_model_csdl(spec_names: &[String]) -> bool {
    spec_names
        .iter()
        .any(|name| name == "model.csdl.xml" || name.ends_with("/model.csdl.xml"))
}

fn temper_show_spec(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_type = str_arg(args, 0, "entity_type", "show_spec")?;
    http_get(
        ctx,
        api_url,
        tenant,
        &format!("/observe/specs/{entity_type}"),
    )
}

fn temper_specs(ctx: &Context, api_url: &str, tenant: &str) -> Result<Value, String> {
    http_get(ctx, api_url, tenant, "/observe/specs")
}

// --- WASM ---

fn temper_upload_wasm(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let module_name = str_arg(args, 0, "module_name", "upload_wasm")?;
    let wasm_base64 = str_arg(args, 1, "wasm_base64", "upload_wasm")?;
    let body = json!({ "wasm_base64": wasm_base64 });
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/api/wasm/modules/{module_name}"),
        &body,
    )
}

// --- Evolution ---

fn temper_get_trajectories(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let entity_type = opt_str_arg(args, 0);
    let failed_only = args.get(1).and_then(|v| v.as_bool()).unwrap_or(false);
    let limit = args.get(2).and_then(|v| v.as_u64()).unwrap_or(50);
    let mut path = format!("/api/evolution/trajectories?limit={limit}");
    if let Some(et) = entity_type {
        path.push_str(&format!("&entity_type={et}"));
    }
    if failed_only {
        path.push_str("&failed_only=true");
    }
    http_get(ctx, api_url, tenant, &path)
}

fn temper_get_insights(ctx: &Context, api_url: &str, tenant: &str) -> Result<Value, String> {
    http_get(ctx, api_url, tenant, "/api/evolution/insights")
}

// --- Governance ---

fn tenant_decisions_path(tenant: &str) -> String {
    format!("/api/tenants/{tenant}/decisions?status=pending")
}

fn tenant_decision_path(tenant: &str, decision_id: &str) -> String {
    format!("/api/tenants/{tenant}/decisions/{decision_id}")
}

fn temper_get_decisions(ctx: &Context, api_url: &str, tenant: &str) -> Result<Value, String> {
    http_get(ctx, api_url, tenant, &tenant_decisions_path(tenant))
}

fn temper_poll_decision(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let decision_id = str_arg(args, 0, "decision_id", "poll_decision")?;
    // Poll once — the agent can call repeatedly if needed.
    // Full blocking poll would exceed WASM execution budget.
    http_get(
        ctx,
        api_url,
        tenant,
        &tenant_decision_path(tenant, &decision_id),
    )
}

// --- Web research (dispatch wrappers for standalone WASM modules) ---

/// Search the web via WebQuery entity + web_search WASM module.
/// Creates a WebQuery, dispatches ExecuteSearch, reads results.
fn temper_web_search(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let query = str_arg(args, 0, "query", "web_search")?;
    let primary_result = web_query_dispatch(ctx, api_url, tenant, "search", &query, "")?;
    if !web_search_results_empty(&primary_result) {
        return Ok(primary_result);
    }

    if let Some(retry_query) =
        fallback_web_search_query(&query, &recent_user_messages(ctx, api_url, tenant, 8))
    {
        ctx.log(
            "info",
            &format!("web_search: retrying vague zero-result query '{query}' as '{retry_query}'"),
        );

        let retry_result = web_query_dispatch(ctx, api_url, tenant, "search", &retry_query, "")?;
        if !web_search_results_empty(&retry_result) {
            return Ok(retry_result);
        }
    }

    Ok(primary_result)
}

/// Fetch a URL via WebQuery entity + web_fetch WASM module.
/// Creates a WebQuery, dispatches ExecuteFetch, reads results.
fn temper_web_fetch(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let url = str_arg(args, 0, "url", "web_fetch")?;
    let result = web_query_dispatch(ctx, api_url, tenant, "fetch", "", &url)?;
    if web_search_results_empty(&result) {
        return Err(format!(
            "web_fetch: fetched no readable content from {url}; try a more specific page or search first"
        ));
    }
    Ok(result)
}

/// Generate an image via MediaGenerationRequest entity + openai_codex_image_generate WASM.
/// Creates a MediaGenerationRequest, dispatches Generate, reads result metadata.
fn temper_image_generate(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = image_generate_input(ctx, api_url, tenant, args)?;
    let body = media_generation_fields(&input);
    let entity = http_post(
        ctx,
        api_url,
        tenant,
        "/tdata/MediaGenerationRequests",
        &body,
    )?;
    let entity_id = entity
        .get("entity_id")
        .or_else(|| entity.get("EntityId"))
        .or_else(|| entity.get("Id"))
        .and_then(Value::as_str)
        .or_else(|| entity_field_str(&entity, &["Id", "id"]))
        .ok_or_else(|| {
            "image_generate: failed to get entity_id from created MediaGenerationRequest"
                .to_string()
        })?;

    let key = escape_odata_key(entity_id);
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/MediaGenerationRequests('{key}')/Temper.Generate?await_integration=true"),
        &json!({
            "prompt": input.prompt,
            "media_type": input.media_type,
            "operation": input.operation,
            "provider": input.provider,
            "model": input.model,
            "size": input.size,
            "quality": input.quality,
            "output_format": input.output_format,
            "background": input.background,
            "workspace_id": input.workspace_id,
            "output_path": input.output_path,
        }),
    )?;

    let result = http_get(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/MediaGenerationRequests('{key}')"),
    )?;
    let rendered = render_media_generation_result(entity_id, &result, input.include_base64)?;
    record_dispatch_image_result(&rendered);
    Ok(rendered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageGenerateInput {
    prompt: String,
    media_type: String,
    operation: String,
    provider: String,
    model: String,
    size: String,
    quality: String,
    output_format: String,
    background: String,
    workspace_id: String,
    output_path: String,
    include_base64: bool,
}

fn image_generate_input(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<ImageGenerateInput, String> {
    let (prompt, opts) = match args.first() {
        Some(Value::Object(input)) => {
            let prompt = input
                .get("prompt")
                .or_else(|| input.get("Prompt"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    "temper.image_generate(): object input must include non-empty prompt"
                        .to_string()
                })?
                .trim()
                .to_string();
            let opts = input
                .get("opts")
                .or_else(|| input.get("options"))
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| Value::Object(input.clone()));
            (prompt, opts)
        }
        _ => {
            let prompt = str_arg(args, 0, "prompt", "image_generate")?;
            (prompt.trim().to_string(), obj_arg_or_empty(args, 1))
        }
    };

    if prompt.is_empty() {
        return Err("temper.image_generate(): prompt cannot be empty".to_string());
    }

    let workspace_id = resolve_image_workspace_id(ctx, api_url, tenant, &opts)?;

    let provider = normalize_image_provider(
        &opts_string(&opts, &["provider", "Provider"])
            .unwrap_or_else(|| "openai_codex".to_string()),
    );
    let model = normalize_image_model_for_provider(
        &provider,
        &opts_string(&opts, &["model", "Model"]).unwrap_or_default(),
    );

    Ok(ImageGenerateInput {
        prompt,
        media_type: opts_string(&opts, &["media_type", "MediaType"])
            .unwrap_or_else(|| "image".to_string()),
        operation: opts_string(&opts, &["operation", "Operation"])
            .unwrap_or_else(|| "generate".to_string()),
        provider,
        model,
        size: opts_string(&opts, &["size", "Size"]).unwrap_or_else(|| "1024x1024".to_string()),
        quality: opts_string(&opts, &["quality", "Quality"]).unwrap_or_else(|| "low".to_string()),
        output_format: opts_string(&opts, &["output_format", "OutputFormat"])
            .or_else(|| opts_string(&opts, &["format", "Format"]))
            .unwrap_or_else(|| "png".to_string()),
        background: opts_string(&opts, &["background", "Background"])
            .unwrap_or_else(|| "auto".to_string()),
        workspace_id,
        output_path: opts_string(&opts, &["output_path", "OutputPath"])
            .or_else(|| opts_string(&opts, &["path", "Path"]))
            .unwrap_or_default(),
        include_base64: opts
            .get("inline")
            .or_else(|| opts.get("include_base64"))
            .or_else(|| opts.get("base64"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

fn resolve_image_workspace_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    opts: &Value,
) -> Result<String, String> {
    if let Some(workspace_id) = opts_string(opts, &["workspace_id", "WorkspaceId"]) {
        return Ok(workspace_id);
    }

    let session_fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    if let Some(workspace_id) = entity_field_str(&session_fields, &["workspace_id", "WorkspaceId"])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(workspace_id.to_string());
    }

    let agent_id = entity_field_str(&session_fields, &["agent_id", "AgentId"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ctx.entity_id.as_str());
    ensure_image_workspace(ctx, api_url, tenant, agent_id)
}

fn ensure_image_workspace(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<String, String> {
    let workspace_id = format!("agent-{agent_id}");
    let lookup_path = format!(
        "/tdata/Workspaces?$filter=WorkspaceId eq '{}'&$top=1",
        encode_odata_filter_literal(&workspace_id)
    );
    if let Ok(result) = http_get(ctx, api_url, tenant, &lookup_path)
        && let Some(found_id) = result
            .get("value")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|entity| {
                entity
                    .get("entity_id")
                    .or_else(|| entity.get("Id"))
                    .and_then(Value::as_str)
                    .or_else(|| entity_field_str(entity, &["WorkspaceId", "workspace_id"]))
            })
            .filter(|value| !value.trim().is_empty())
    {
        return Ok(found_id.to_string());
    }

    let body = json!({
        "WorkspaceId": workspace_id,
        "name": format!("Agent {agent_id} Workspace"),
        "owner_id": agent_id,
        "quota_bytes": "104857600"
    });
    match http_post(ctx, api_url, tenant, "/tdata/Workspaces", &body) {
        Ok(entity) => entity
            .get("entity_id")
            .or_else(|| entity.get("Id"))
            .and_then(Value::as_str)
            .or_else(|| entity_field_str(&entity, &["WorkspaceId", "workspace_id"]))
            .map(ToOwned::to_owned)
            .ok_or_else(|| "image_generate: created Workspace without entity id".to_string()),
        Err(err) => {
            let retry = http_get(ctx, api_url, tenant, &lookup_path)?;
            retry
                .get("value")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|entity| {
                    entity
                        .get("entity_id")
                        .or_else(|| entity.get("Id"))
                        .and_then(Value::as_str)
                        .or_else(|| entity_field_str(entity, &["WorkspaceId", "workspace_id"]))
                })
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("image_generate: ensure workspace failed: {err}"))
        }
    }
}

fn media_generation_fields(input: &ImageGenerateInput) -> Value {
    json!({
        "Prompt": input.prompt,
        "MediaType": input.media_type,
        "Operation": input.operation,
        "Provider": input.provider,
        "Model": input.model,
        "Size": input.size,
        "Quality": input.quality,
        "OutputFormat": input.output_format,
        "Background": input.background,
        "WorkspaceId": input.workspace_id,
        "OutputPath": input.output_path,
    })
}

fn render_media_generation_result(
    media_generation_id: &str,
    entity: &Value,
    include_base64: bool,
) -> Result<Value, String> {
    let fields = entity.get("fields").unwrap_or(entity);
    let status = entity_field_str(entity, &["Status", "status"]).unwrap_or("Created");
    if status != "Complete" {
        let error = entity_field_str(fields, &["Error", "error"])
            .or_else(|| entity_field_str(fields, &["LastError", "last_error"]))
            .unwrap_or("");
        return Err(format!(
            "image_generate: generation did not complete (status={status}){}",
            if error.is_empty() {
                String::new()
            } else {
                format!(": {error}")
            }
        ));
    }

    let mime_type = entity_field_str(fields, &["MimeType", "mime_type"]).unwrap_or("image/png");
    let base64_data =
        entity_field_str(fields, &["ResultImageBase64", "result_image_base64"]).unwrap_or("");
    let file_id = entity_field_str(fields, &["ResultFileId", "result_file_id"]).unwrap_or("");
    let file_version_id =
        entity_field_str(fields, &["ResultFileVersionId", "result_file_version_id"]).unwrap_or("");
    let path = entity_field_str(fields, &["ResultPath", "result_path"]).unwrap_or("");
    if file_id.is_empty() && base64_data.is_empty() && path.is_empty() {
        return Err(format!(
            "image_generate: generation completed without an image artifact (media_generation_id={media_generation_id})"
        ));
    }

    let byte_count = base64_data.len().saturating_mul(3) / 4;
    let mut result = json!({
        "__temperpaw_image": true,
        "media_generation_id": media_generation_id,
        "media_type": mime_type,
        "mime_type": mime_type,
        "file_id": file_id,
        "file_version_id": file_version_id,
        "path": path,
        "source_path": path,
        "prompt": entity_field_str(fields, &["Prompt", "prompt"]).unwrap_or(""),
        "revised_prompt": entity_field_str(fields, &["RevisedPrompt", "revised_prompt"]).unwrap_or(""),
        "provider": entity_field_str(fields, &["Provider", "provider"]).unwrap_or("openai_codex"),
        "model": entity_field_str(fields, &["Model", "model"]).unwrap_or(""),
        "provider_response_id": entity_field_str(fields, &["ProviderResponseId", "provider_response_id"]).unwrap_or(""),
        "byte_count": byte_count,
    });
    if include_base64 && !base64_data.is_empty() {
        result["base64_data"] = json!(base64_data);
    } else {
        result["content_ref"] = json!("pawfs_file");
    }
    Ok(result)
}

fn opts_string(opts: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| opts.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_image_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "" => "openai_codex".to_string(),
        other => other.to_string(),
    }
}

fn normalize_image_model_for_provider(provider: &str, model: &str) -> String {
    let trimmed = model.trim();
    if provider == "openai_codex" && is_public_openai_image_model_name(trimmed) {
        String::new()
    } else {
        trimmed.to_string()
    }
}

fn is_public_openai_image_model_name(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized == "gpt-image"
        || normalized.starts_with("gpt-image-")
        || normalized == "dall-e"
        || normalized.starts_with("dall-e-")
}

/// Shared implementation: create WebQuery entity, dispatch action, return results.
fn web_query_dispatch(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    query_type: &str,
    query: &str,
    url: &str,
) -> Result<Value, String> {
    if let Some(cached) = lookup_completed_web_query(ctx, api_url, tenant, query_type, query, url)?
    {
        ctx.log(
            "info",
            &format!("web_query: reusing completed cached {query_type} result"),
        );
        return Ok(cached);
    }

    // 1. Create WebQuery entity
    let body = json!({
        "QueryType": query_type,
        "Query": query,
        "Url": url,
    });
    let entity = http_post(ctx, api_url, tenant, "/tdata/WebQueries", &body)?;

    let entity_id = entity
        .get("entity_id")
        .or_else(|| entity.get("EntityId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "web_query: failed to get entity_id from created WebQuery".to_string())?;

    // 2. Dispatch the appropriate Execute action
    let action_name = if query_type == "search" {
        "ExecuteSearch"
    } else {
        "ExecuteFetch"
    };
    let action_params = if query_type == "search" {
        json!({"query": query})
    } else {
        json!({"url": url})
    };
    let key = escape_odata_key(entity_id);
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/WebQueries('{key}')/Temper.{action_name}?await_integration=true"),
        &action_params,
    )?;
    wait_for_web_query_completion(ctx, api_url, tenant, entity_id, query_type);

    // 3. Read the entity back — WASM integration has run by this point.
    // OData GET hydrates blob refs transparently (temper ADR-0040), so the
    // `results` field arrives fully resolved regardless of size. The prior
    // TemperFS File workaround (result_file_id + delete-after-read) is
    // retired — see temper ADR-0045 / ADR-0046 and temperpaw ADR-0033.
    let result = http_get(ctx, api_url, tenant, &format!("/tdata/WebQueries('{key}')"))?;
    let result_fields = result.get("fields").cloned().unwrap_or(result.clone());
    let (status, results_raw) = interpret_web_query_entity_result(query_type, &result_fields)?;

    // Try to parse as JSON array; if not, return as plain text
    match serde_json::from_str::<Value>(results_raw) {
        Ok(parsed) => {
            ctx.log(
                "info",
                &format!("web_query: {query_type} completed with status {status}"),
            );
            Ok(parsed)
        }
        Err(_) => {
            ctx.log(
                "info",
                &format!("web_query: {query_type} completed with status {status}"),
            );
            Ok(json!(results_raw))
        }
    }
}

fn lookup_completed_web_query(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    query_type: &str,
    query: &str,
    url: &str,
) -> Result<Option<Value>, String> {
    let path = web_query_cache_lookup_path(query_type, query, url);
    let lookup = match http_get(ctx, api_url, tenant, &path) {
        Ok(value) => value,
        Err(error) => {
            ctx.log(
                "warn",
                &format!("web_query: cache lookup failed, falling back to fresh query: {error}"),
            );
            return Ok(None);
        }
    };

    interpret_cached_web_query_result(query_type, &lookup)
}

fn web_query_cache_lookup_path(query_type: &str, query: &str, url: &str) -> String {
    let (field_name, raw_value) = if query_type == "search" {
        ("Query", query)
    } else {
        ("Url", url)
    };
    let escaped_value = encode_odata_filter_literal(raw_value);
    format!(
        "/tdata/WebQueries?$filter=Status%20eq%20'Complete'%20and%20QueryType%20eq%20'{query_type}'%20and%20{field_name}%20eq%20'{escaped_value}'&$top=1"
    )
}

fn interpret_cached_web_query_result(
    query_type: &str,
    lookup: &Value,
) -> Result<Option<Value>, String> {
    let entity = lookup
        .get("value")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned();
    let Some(entity) = entity else {
        return Ok(None);
    };

    let result_fields = entity.get("fields").cloned().unwrap_or(entity);
    let (status, results_raw) = interpret_web_query_entity_result(query_type, &result_fields)?;

    let parsed = match serde_json::from_str::<Value>(results_raw) {
        Ok(parsed) => parsed,
        Err(_) => json!(results_raw),
    };

    let _ = status;
    Ok(Some(parsed))
}

const GENERIC_SEARCH_TOKENS: &[&str] = &[
    "a",
    "an",
    "and",
    "as",
    "before",
    "can",
    "could",
    "do",
    "does",
    "easily",
    "famous",
    "find",
    "for",
    "from",
    "github",
    "it",
    "just",
    "look",
    "of",
    "open",
    "please",
    "repo",
    "repository",
    "same",
    "search",
    "super",
    "that",
    "the",
    "this",
    "up",
    "use",
    "web",
    "you",
    "your",
];

const SUBJECT_CONTEXT_STOPWORDS: &[&str] = &[
    "asked", "clone", "cloned", "don't", "dont", "find", "look", "remember", "search", "searched",
    "use", "using", "web",
];

fn web_search_results_empty(result: &Value) -> bool {
    match result {
        Value::Array(items) => items.is_empty(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return true;
            }
            serde_json::from_str::<Value>(trimmed)
                .ok()
                .is_some_and(|parsed| matches!(parsed, Value::Array(ref items) if items.is_empty()))
        }
        _ => false,
    }
}

fn interpret_web_query_entity_result<'a>(
    query_type: &str,
    fields: &'a Value,
) -> Result<(&'a str, &'a str), String> {
    let status = fields
        .get("Status")
        .or_else(|| fields.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match status {
        "Complete" => {}
        "Failed" => {
            let error = fields
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("web_{query_type}: {error}"));
        }
        other => {
            return Err(format!(
                "web_{query_type}: query never completed (status={})",
                if other.is_empty() { "unknown" } else { other }
            ));
        }
    }

    let results_raw = fields
        .get("results")
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    Ok((status, results_raw))
}

fn fallback_web_search_query(query: &str, recent_user_messages: &[String]) -> Option<String> {
    if !is_vague_web_search_query(query) {
        return None;
    }

    let query_lower = query.to_lowercase();
    let explicit_subject = recent_user_messages
        .iter()
        .rev()
        .find_map(|message| extract_explicit_repo_subject(message));

    let fallback_subject = explicit_subject.or_else(|| {
        recent_user_messages
            .iter()
            .rev()
            .find_map(|message| extract_search_subject(message))
    })?;

    if query_lower.contains(&fallback_subject.to_lowercase()) {
        return None;
    }

    Some(format!("{fallback_subject} github repo"))
}

fn is_vague_web_search_query(query: &str) -> bool {
    let tokens = search_tokens(query);
    !tokens.iter().any(|token| !is_generic_search_token(token))
}

fn extract_search_subject(message: &str) -> Option<String> {
    extract_explicit_repo_subject(message).or_else(|| {
        search_tokens(message).into_iter().rev().find(|token| {
            !is_generic_search_token(token) && !SUBJECT_CONTEXT_STOPWORDS.contains(&token.as_str())
        })
    })
}

fn extract_explicit_repo_subject(message: &str) -> Option<String> {
    if let Some(repo) = extract_github_repo(message) {
        return Some(repo);
    }

    let tokens = search_tokens(message);
    if tokens.is_empty() {
        return None;
    }

    for window in tokens.windows(2) {
        if matches!(window[1].as_str(), "repo" | "repository")
            && !is_generic_search_token(&window[0])
            && !SUBJECT_CONTEXT_STOPWORDS.contains(&window[0].as_str())
        {
            return Some(window[0].clone());
        }
    }

    None
}

fn extract_github_repo(message: &str) -> Option<String> {
    let marker = "github.com/";
    let start = message.find(marker)?;
    let remainder = &message[start + marker.len()..];
    let repo = remainder
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '?' | '#' | ')' | ']' | '"' | '\''))
        .next()?
        .trim_matches('/');
    if repo.is_empty() {
        return None;
    }

    let repo_name = repo.rsplit('/').next()?.trim();
    if repo_name.is_empty() {
        return None;
    }
    Some(repo_name.to_string())
}

fn search_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(clean_search_token)
        .filter(|token| !token.is_empty())
        .collect()
}

fn clean_search_token(raw: &str) -> String {
    raw.trim_matches(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.'))
    })
    .to_lowercase()
}

fn is_generic_search_token(token: &str) -> bool {
    token.len() < 3 || GENERIC_SEARCH_TOKENS.contains(&token)
}

fn recent_user_messages(ctx: &Context, api_url: &str, tenant: &str, limit: usize) -> Vec<String> {
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let session_file_id = fields
        .get("session_file_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    let mut messages = Vec::new();
    if let Some(current_user_message) = fields.get("user_message").and_then(Value::as_str) {
        if !current_user_message.trim().is_empty() {
            messages.push(current_user_message.trim().to_string());
        }
    }

    if session_file_id.is_empty() {
        return messages;
    }

    let Ok(session_jsonl) =
        read_session_from_temperfs(ctx, api_url, tenant, &fields, session_file_id)
    else {
        return messages;
    };

    let tree = session_tree_lib::SessionTree::from_jsonl(&session_jsonl);
    for entry_id in tree.entry_ids() {
        let Some(entry) = tree.get(entry_id) else {
            continue;
        };
        if entry.data.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let content = extract_search_entry_text(&entry.data);
        if !content.trim().is_empty() {
            messages.push(content.trim().to_string());
        }
    }

    if messages.len() > limit {
        messages.drain(0..messages.len().saturating_sub(limit));
    }
    messages
}

fn extract_search_entry_text(data: &Value) -> String {
    if let Some(text) = data.get("content").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(arr) = data.get("content").and_then(Value::as_array) {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    if let Some(summary) = data.get("summary").and_then(Value::as_str) {
        return summary.to_string();
    }
    String::new()
}

// --- Session completion ---

fn temper_done(args: &[Value]) -> Result<Value, String> {
    let result = args
        .first()
        .and_then(|v| v.as_str())
        .unwrap_or("(done)")
        .to_string();
    DONE_RESULT.with(|cell| {
        *cell.borrow_mut() = Some(result);
    });
    Ok(json!({"done": true}))
}

// --- Secrets (Cedar-gated via access_secret on Secret resource) ---

fn temper_get_secret(ctx: &Context, args: &[Value]) -> Result<Value, String> {
    let key = str_arg(args, 0, "key", "get_secret")?;
    let value = ctx.get_secret(&key)?;
    Ok(json!(value))
}

// --- Cedar Policy Management (all Cedar-gated by platform) ---

fn temper_submit_policy(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let policy_id = str_arg(args, 0, "policy_id", "submit_policy")?;
    let cedar_text = str_arg(args, 1, "cedar_text", "submit_policy")?;
    let body = json!({ "policy_id": policy_id, "cedar_text": cedar_text });
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/policies/create"),
        &body,
    )
}

fn temper_list_policies(ctx: &Context, api_url: &str, tenant: &str) -> Result<Value, String> {
    http_get(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/policies/list"),
    )
}

fn temper_get_policy(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let policy_id = str_arg(args, 0, "policy_id", "get_policy")?;
    // List all and filter — no single-policy GET endpoint
    let all = http_get(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/policies/list"),
    )?;
    let policies = all.get("policies").and_then(|v| v.as_array());
    if let Some(list) = policies {
        for p in list {
            if p.get("policy_id").and_then(|v| v.as_str()) == Some(&policy_id) {
                return Ok(p.clone());
            }
        }
    }
    Err(format!("policy '{policy_id}' not found"))
}

fn temper_update_policy(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let policy_id = str_arg(args, 0, "policy_id", "update_policy")?;
    let cedar_text = str_arg(args, 1, "cedar_text", "update_policy")?;
    let body = json!({ "cedar_text": cedar_text });
    http_patch(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/policies/entry/{policy_id}"),
        &body,
    )
}

fn temper_delete_policy(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let policy_id = str_arg(args, 0, "policy_id", "delete_policy")?;
    http_delete(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/policies/entry/{policy_id}"),
    )
}

// --- Decision Management (Cedar-gated by platform) ---

fn temper_approve_decision(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let decision_id = str_arg(args, 0, "decision_id", "approve_decision")?;
    let scope = obj_arg(args, 1, "scope", "approve_decision")?;
    let agent_id = ctx
        .entity_state
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    let body = json!({ "scope": scope, "decided_by": format!("agent:{agent_id}") });
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/decisions/{decision_id}/approve"),
        &body,
    )
}

fn temper_deny_decision(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let decision_id = str_arg(args, 0, "decision_id", "deny_decision")?;
    let agent_id = ctx
        .entity_state
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("agent");
    let body = json!({ "decided_by": format!("agent:{agent_id}") });
    http_post(
        ctx,
        api_url,
        tenant,
        &format!("/api/tenants/{tenant}/decisions/{decision_id}/deny"),
        &body,
    )
}

// --- Apps ---

fn temper_install_app(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = args
        .first()
        .ok_or_else(|| {
            "temper.install_app() requires {app_ref:'owner/name@hash', registry_url?, registry_tenant?, tenant?, follow_policy?}"
                .to_string()
        })?;
    let app_ref = if let Some(obj) = input.as_object() {
        obj.get("app_ref")
            .or_else(|| obj.get("AppRef"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        input.as_str().unwrap_or("").trim().to_string()
    };
    if !is_pinned_app_ref(&app_ref) {
        return Err(
            "temper.install_app() only installs pinned Genesis refs: owner/name@hash. Local app-name installs are legacy/admin-only."
                .to_string(),
        );
    }
    let input_obj = input.as_object();
    let target_tenant = input_obj
        .and_then(|obj| obj.get("tenant").or_else(|| obj.get("TargetTenant")))
        .and_then(Value::as_str)
        .unwrap_or(tenant)
        .to_string();
    let registry_url = genesis_registry_url(ctx, input_obj)?;
    let registry_tenant = input_obj
        .and_then(|obj| {
            obj.get("registry_tenant")
                .or_else(|| obj.get("RegistryTenant"))
        })
        .and_then(Value::as_str)
        .unwrap_or("default");
    let follow_policy = input_obj
        .and_then(|obj| {
            obj.get("follow_policy")
                .or_else(|| obj.get("FollowPolicy"))
                .and_then(Value::as_str)
        })
        .unwrap_or("pinned");
    let body = json!({
        "tenant": target_tenant,
        "app_ref": app_ref,
        "registry_url": registry_url,
        "registry_tenant": registry_tenant,
        "follow_policy": follow_policy,
    });
    http_post(ctx, api_url, tenant, "/api/genesis/apps/install", &body)
}

fn temper_search_apps(ctx: &Context, args: &[Value]) -> Result<Value, String> {
    let input = args.first().filter(|value| value.is_object());
    let input_obj = input.and_then(Value::as_object);
    let registry_url = genesis_registry_url(ctx, input_obj)?;
    let registry_tenant = input_obj
        .and_then(|obj| {
            obj.get("registry_tenant")
                .or_else(|| obj.get("RegistryTenant"))
        })
        .and_then(Value::as_str)
        .unwrap_or("default");
    let query = input_obj
        .and_then(|obj| obj.get("query").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();
    let owner = input_obj
        .and_then(|obj| obj.get("owner").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();
    let status = input_obj
        .and_then(|obj| obj.get("status").and_then(Value::as_str))
        .unwrap_or("Active")
        .to_ascii_lowercase();

    let url = format!("{}/tdata/Apps", registry_url.trim_end_matches('/'));
    let mut headers = internal_headers();
    headers.push(("X-Tenant-Id".to_string(), registry_tenant.to_string()));
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "Genesis app search failed: HTTP {} {}",
            resp.status, resp.body
        ));
    }
    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|e| format!("Genesis app search returned invalid JSON: {e}"))?;
    let values = parsed
        .get("value")
        .and_then(Value::as_array)
        .ok_or_else(|| "Genesis app search expected OData JSON with a value array".to_string())?;
    let mut apps = Vec::new();
    for app in values {
        let fields = app.get("fields").unwrap_or(app);
        let app_status = app
            .get("Status")
            .or_else(|| app.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("Active")
            .to_ascii_lowercase();
        if !status.is_empty() && app_status != status.to_ascii_lowercase() {
            continue;
        }
        let app_owner = field_str(fields, &["OwnerId", "owner_id", "owner"]).to_ascii_lowercase();
        if !owner.is_empty() && app_owner != owner {
            continue;
        }
        let name = field_str(fields, &["Name", "name"]);
        let desc = field_str(fields, &["Description", "description"]);
        let hash = field_str(fields, &["LatestVersionHash", "latest_version_hash"]);
        let haystack = format!(
            "{} {} {}",
            app_owner,
            name.to_ascii_lowercase(),
            desc.to_ascii_lowercase()
        );
        if !query.is_empty() && !haystack.contains(&query) {
            continue;
        }
        apps.push(json!({
            "owner": app_owner,
            "name": name,
            "description": desc,
            "latest_hash": hash,
            "app_ref": if hash.is_empty() { String::new() } else { format!("{}/{}@{}", app_owner, name, hash.trim_start_matches('@')) },
        }));
    }
    Ok(json!({
        "registry_url": registry_url,
        "registry_tenant": registry_tenant,
        "apps": apps,
    }))
}

fn temper_publish_app(
    ctx: &Context,
    sandbox_url: &str,
    sandbox_id: &str,
    sandbox_provider: &str,
    workdir: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = args.first().and_then(Value::as_object).ok_or_else(|| {
        "temper.publish_app() requires {path, owner, name, registry_url?, message?}".to_string()
    })?;
    let path = required_obj_str(input, "path", "publish_app")?;
    let owner = required_obj_str(input, "owner", "publish_app")?;
    let name = required_obj_str(input, "name", "publish_app")?;
    let registry_url = genesis_registry_url(ctx, Some(input))?;
    let registry_tenant = genesis_registry_tenant(input);
    let message = input
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Publish Genesis app");
    publish_or_update_app_via_git(
        ctx,
        sandbox_url,
        sandbox_id,
        sandbox_provider,
        workdir,
        &path,
        &owner,
        &name,
        &registry_url,
        &registry_tenant,
        message,
    )
}

fn temper_update_app(
    ctx: &Context,
    sandbox_url: &str,
    sandbox_id: &str,
    sandbox_provider: &str,
    workdir: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = args.first().and_then(Value::as_object).ok_or_else(|| {
        "temper.update_app() requires {path, app_ref_or_name, registry_url?, message?}".to_string()
    })?;
    let path = required_obj_str(input, "path", "update_app")?;
    let app_ref_or_name = required_obj_str(input, "app_ref_or_name", "update_app")?;
    let (owner, name) = owner_name_from_ref_or_name(&app_ref_or_name)?;
    let registry_url = genesis_registry_url(ctx, Some(input))?;
    let registry_tenant = genesis_registry_tenant(input);
    let message = input
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Update Genesis app");
    publish_or_update_app_via_git(
        ctx,
        sandbox_url,
        sandbox_id,
        sandbox_provider,
        workdir,
        &path,
        &owner,
        &name,
        &registry_url,
        &registry_tenant,
        message,
    )
}

fn publish_or_update_app_via_git(
    ctx: &Context,
    sandbox_url: &str,
    sandbox_id: &str,
    sandbox_provider: &str,
    workdir: &str,
    path: &str,
    owner: &str,
    name: &str,
    registry_url: &str,
    registry_tenant: &str,
    message: &str,
) -> Result<Value, String> {
    if sandbox_url.is_empty() {
        return Err("temper.publish_app()/update_app() requires an attached sandbox".to_string());
    }
    let repository_id =
        ensure_genesis_repository(ctx, registry_url, registry_tenant, owner, name, message)?;
    let handle = wasm_helpers::sandbox::SandboxHandle {
        sandbox_url: sandbox_url.to_string(),
        sandbox_id: sandbox_id.to_string(),
        provider: if sandbox_provider.is_empty() {
            "tensorlake".to_string()
        } else {
            sandbox_provider.to_string()
        },
    };
    let remote = format!(
        "{}/{}/{}.git",
        registry_url.trim_end_matches('/'),
        owner,
        name
    );
    let app_url = format!(
        "{}/tdata/Apps('{}')",
        registry_url.trim_end_matches('/'),
        escape_odata_key(&app_id_for(owner, name))
    );
    let headers = genesis_registry_headers(registry_tenant);
    let existing_latest_hash =
        fetch_genesis_latest_hash(ctx, &app_url, &headers, owner, name).unwrap_or_default();
    let git_header_key = format!("http.{}/.extraHeader", registry_url.trim_end_matches('/'));
    let git_tenant_header = format!("X-Tenant-Id: {registry_tenant}");
    let command = format!(
        "set -euo pipefail\n\
         source_dir={}\n\
         publish_dir=$(mktemp -d)\n\
         cleanup() {{ rm -rf \"$publish_dir\"; }}\n\
         trap cleanup EXIT\n\
         if ! git -c {}={} -c protocol.version=0 clone {} \"$publish_dir\" >/dev/null 2>&1; then\n\
           git init -b main \"$publish_dir\" >/dev/null 2>&1 || (git init \"$publish_dir\" >/dev/null && git -C \"$publish_dir\" checkout -B main >/dev/null)\n\
           git -C \"$publish_dir\" remote add origin {}\n\
         fi\n\
         find \"$publish_dir\" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {{}} +\n\
         (cd \"$source_dir\" && tar --exclude=.git --exclude=target --exclude=__pycache__ --exclude='*.py[co]' --exclude=.pytest_cache -cf - .) | (cd \"$publish_dir\" && tar -xf -)\n\
         find \"$publish_dir\" -type d \\( -name target -o -name __pycache__ -o -name .pytest_cache \\) -prune -exec rm -rf {{}} +\n\
         find \"$publish_dir\" -type f \\( -name '*.pyc' -o -name '*.pyo' \\) -delete\n\
         cd \"$publish_dir\"\n\
         git config --unset-all {} >/dev/null 2>&1 || true\n\
         git config --add {} {}\n\
         git config protocol.version 0\n\
         git add .\n\
         if ! git diff --cached --quiet; then git -c user.name={} -c user.email={} commit -m {} >/dev/null; fi\n\
         if ! git push {} HEAD:main; then\n\
           existing_hash={}\n\
           if [ -n \"$existing_hash\" ] && ! git rev-parse --verify refs/remotes/origin/main >/dev/null 2>&1; then\n\
             git push --force-with-lease=refs/heads/main:$existing_hash {} HEAD:main\n\
           else\n\
             exit 1\n\
           fi\n\
         fi\n\
         git rev-parse HEAD",
        shell_quote(path),
        shell_quote(&git_header_key),
        shell_quote(&git_tenant_header),
        shell_quote(&remote),
        shell_quote(&remote),
        shell_quote(&git_header_key),
        shell_quote(&git_header_key),
        shell_quote(&git_tenant_header),
        shell_quote("TemperPaw Agent"),
        shell_quote("agent@temperpaw.local"),
        shell_quote(message),
        shell_quote(&remote),
        shell_quote(&existing_latest_hash),
        shell_quote(&remote),
    );
    let result = wasm_helpers::sandbox::sandbox_exec(ctx, &handle, &command, workdir)?;
    if result.exit_code != 0 {
        return Err(format!(
            "Genesis app publish failed with exit {}: {}{}{}",
            result.exit_code,
            result.stderr,
            if result.stderr.is_empty() || result.stdout.is_empty() {
                ""
            } else {
                "\n"
            },
            result.stdout
        ));
    }
    let hash = result.stdout.lines().last().unwrap_or("").trim();
    if hash.is_empty() {
        return Err("Genesis app publish succeeded but did not return a commit hash".to_string());
    }
    let registry_action = publish_genesis_app_version(
        ctx,
        registry_url,
        registry_tenant,
        owner,
        name,
        &repository_id,
        hash,
        message,
    )?;
    Ok(json!({
        "app_ref": format!("{owner}/{name}@{hash}"),
        "owner": owner,
        "name": name,
        "hash": hash,
        "registry_url": registry_url,
        "registry_tenant": registry_tenant,
        "remote": remote,
        "registry_action": registry_action,
    }))
}

fn ensure_genesis_repository(
    ctx: &Context,
    registry_url: &str,
    registry_tenant: &str,
    owner: &str,
    name: &str,
    description: &str,
) -> Result<String, String> {
    let repository_id = repository_id_for(owner, name);
    let url = format!(
        "{}/tdata/Repositories('{}')",
        registry_url.trim_end_matches('/'),
        escape_odata_key(&repository_id)
    );
    let headers = genesis_registry_headers(registry_tenant);
    let existing = ctx.http_call("GET", &url, &headers, "")?;
    if existing.status < 400 {
        return Ok(repository_id);
    }
    if existing.status != 404 {
        return Err(format!(
            "Genesis repository lookup failed for {owner}/{name}: HTTP {} {}",
            existing.status, existing.body
        ));
    }

    let create_url = format!(
        "{}/tdata/Repositories?await_integration=true",
        registry_url.trim_end_matches('/')
    );
    let body = json!({
        "Id": repository_id,
        "OwnerAccountId": owner,
        "Name": name,
        "Description": description,
        "DefaultBranch": "main",
        "Visibility": "public",
    });
    let created = ctx.http_call("POST", &create_url, &headers, &body.to_string())?;
    if created.status >= 400
        && !created.body.contains("already")
        && !created.body.contains("exists")
    {
        return Err(format!(
            "Genesis repository create failed for {owner}/{name}: HTTP {} {}",
            created.status, created.body
        ));
    }
    Ok(repository_id)
}

fn publish_genesis_app_version(
    ctx: &Context,
    registry_url: &str,
    registry_tenant: &str,
    owner: &str,
    name: &str,
    repository_id: &str,
    hash: &str,
    message: &str,
) -> Result<Value, String> {
    let app_id = app_id_for(owner, name);
    let app_url = format!(
        "{}/tdata/Apps('{}')",
        registry_url.trim_end_matches('/'),
        escape_odata_key(&app_id)
    );
    let headers = genesis_registry_headers(registry_tenant);
    let existing = ctx.http_call("GET", &app_url, &headers, "")?;
    if existing.status == 404 {
        let register_url = format!("{app_url}/Temper.Git.RegisterNewApp?await_integration=true");
        let body = json!({
            "Name": name,
            "RepositoryId": repository_id,
            "Description": message,
            "Exports": "{}",
            "Visibility": "public",
        });
        let registered = ctx.http_call("POST", &register_url, &headers, &body.to_string())?;
        if registered.status >= 400 {
            return Err(format!(
                "Genesis Temper.Git.RegisterNewApp failed for {owner}/{name}: HTTP {} {}",
                registered.status, registered.body
            ));
        }
        let latest_hash = verify_genesis_latest_hash(ctx, &app_url, &headers, owner, name, hash)?;
        return Ok(json!({
            "kind": "registered",
            "app_id": app_id,
            "canonical_action": "Temper.Git.RegisterNewApp",
            "latest_hash": latest_hash,
            "verified_latest": true,
            "response": parse_json_or_text(&registered.body),
        }));
    }
    if existing.status >= 400 {
        return Err(format!(
            "Genesis app lookup failed for {owner}/{name}: HTTP {} {}",
            existing.status, existing.body
        ));
    }

    let parsed = serde_json::from_str::<Value>(&existing.body).unwrap_or(Value::Null);
    let fields = parsed.get("fields").unwrap_or(&parsed);
    let latest_hash = field_str(fields, &["LatestVersionHash", "latest_version_hash"]);
    if latest_hash.trim_start_matches('@') == hash {
        return Ok(json!({
            "kind": "already_current",
            "app_id": app_id,
            "canonical_action": "Temper.Git.PublishNewVersion",
            "latest_hash": hash,
            "verified_latest": true,
        }));
    }

    let publish_url = format!("{app_url}/Temper.Git.PublishNewVersion?await_integration=true");
    let body = json!({
        "NewHash": hash,
        "RefName": "main",
    });
    let published = ctx.http_call("POST", &publish_url, &headers, &body.to_string())?;
    if published.status >= 400 {
        if let Ok(latest_hash) = fetch_genesis_latest_hash(ctx, &app_url, &headers, owner, name) {
            if latest_hash.trim_start_matches('@') == hash {
                return Ok(json!({
                    "kind": "already_current",
                    "app_id": app_id,
                    "canonical_action": "Temper.Git.PublishNewVersion",
                    "latest_hash": latest_hash,
                    "verified_latest": true,
                    "response": parse_json_or_text(&published.body),
                }));
            }
        }
        return Err(format!(
            "Genesis Temper.Git.PublishNewVersion failed for {owner}/{name}: HTTP {} {}",
            published.status, published.body
        ));
    }
    let latest_hash = verify_genesis_latest_hash(ctx, &app_url, &headers, owner, name, hash)?;
    Ok(json!({
        "kind": "published_version",
        "app_id": app_id,
        "canonical_action": "Temper.Git.PublishNewVersion",
        "latest_hash": latest_hash,
        "verified_latest": true,
        "response": parse_json_or_text(&published.body),
    }))
}

fn fetch_genesis_latest_hash(
    ctx: &Context,
    app_url: &str,
    headers: &[(String, String)],
    owner: &str,
    name: &str,
) -> Result<String, String> {
    let current = ctx.http_call("GET", app_url, headers, "")?;
    if current.status >= 400 {
        return Err(format!(
            "Genesis app latest verification lookup failed for {owner}/{name}: HTTP {} {}",
            current.status, current.body
        ));
    }
    let parsed = serde_json::from_str::<Value>(&current.body).map_err(|error| {
        format!("Genesis app latest verification returned invalid JSON: {error}")
    })?;
    let fields = parsed.get("fields").unwrap_or(&parsed);
    let latest_hash = field_str(fields, &["LatestVersionHash", "latest_version_hash"]);
    if latest_hash.trim().is_empty() {
        return Err(format!(
            "Genesis app latest verification for {owner}/{name} returned no LatestVersionHash"
        ));
    }
    Ok(latest_hash.trim_start_matches('@').to_string())
}

fn verify_genesis_latest_hash(
    ctx: &Context,
    app_url: &str,
    headers: &[(String, String)],
    owner: &str,
    name: &str,
    expected_hash: &str,
) -> Result<String, String> {
    let latest_hash = fetch_genesis_latest_hash(ctx, app_url, headers, owner, name)?;
    if latest_hash.trim_start_matches('@') != expected_hash.trim_start_matches('@') {
        return Err(format!(
            "Genesis latest verification failed for {owner}/{name}: expected {}, got {}",
            expected_hash.trim_start_matches('@'),
            latest_hash
        ));
    }
    Ok(latest_hash)
}

fn genesis_registry_url(
    ctx: &Context,
    input: Option<&serde_json::Map<String, Value>>,
) -> Result<String, String> {
    let configured = input
        .and_then(|obj| {
            obj.get("registry_url")
                .or_else(|| obj.get("url"))
                .or_else(|| obj.get("RegistryUrl"))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| ctx.config.get("genesis_registry_url").cloned())
        .or_else(|| ctx.config.get("GENESIS_REGISTRY_URL").cloned())
        .unwrap_or_else(|| "https://genesis-production-164d.up.railway.app".to_string());
    let trimmed = configured.trim().trim_end_matches('/').to_string();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("Genesis registry URL must start with http:// or https://".to_string());
    }
    Ok(trimmed)
}

fn genesis_registry_tenant(input: &serde_json::Map<String, Value>) -> String {
    input
        .get("registry_tenant")
        .or_else(|| input.get("RegistryTenant"))
        .and_then(Value::as_str)
        .filter(|tenant| !tenant.trim().is_empty())
        .unwrap_or("default")
        .trim()
        .to_string()
}

fn genesis_registry_headers(registry_tenant: &str) -> Vec<(String, String)> {
    let mut headers = internal_headers();
    headers.push(("X-Tenant-Id".to_string(), registry_tenant.to_string()));
    headers
}

fn parse_json_or_text(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| json!({ "body": body }))
}

fn is_pinned_app_ref(app_ref: &str) -> bool {
    let Some((owner_name, hash)) = app_ref.split_once('@') else {
        return false;
    };
    let Some((owner, name)) = owner_name.split_once('/') else {
        return false;
    };
    !owner.trim().is_empty() && !name.trim().is_empty() && !hash.trim().is_empty()
}

fn owner_name_from_ref_or_name(value: &str) -> Result<(String, String), String> {
    let left = value.split_once('@').map(|(left, _)| left).unwrap_or(value);
    let (owner, name) = left
        .split_once('/')
        .ok_or_else(|| "app_ref_or_name must be owner/name or owner/name@hash".to_string())?;
    Ok((owner.to_string(), name.to_string()))
}

fn repository_id_for(owner: &str, name: &str) -> String {
    format!(
        "rp-{}-{}",
        sanitize_genesis_id_component(owner),
        sanitize_genesis_id_component(name)
    )
}

fn app_id_for(owner: &str, name: &str) -> String {
    format!(
        "app-{}-{}",
        sanitize_genesis_id_component(owner),
        sanitize_genesis_id_component(name)
    )
}

fn sanitize_genesis_id_component(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed
    }
}

fn required_obj_str(
    input: &serde_json::Map<String, Value>,
    key: &str,
    method: &str,
) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .ok_or_else(|| format!("temper.{method}() requires string field '{key}'"))
}

fn field_str(fields: &Value, names: &[&str]) -> String {
    for name in names {
        if let Some(value) = fields.get(*name).and_then(Value::as_str) {
            return value.to_string();
        }
    }
    String::new()
}

fn temper_list_apps(ctx: &Context, api_url: &str, tenant: &str) -> Result<Value, String> {
    let _ = (api_url, tenant);
    temper_search_apps(ctx, &[])
}

// ---------------------------------------------------------------------------
// Lazy sandbox provisioning (ADR-0022)
// ---------------------------------------------------------------------------

/// Provision a sandbox on-demand via the provider abstraction. Called when a
/// sandbox tool is invoked but no sandbox_url is set on the session.
///
/// Returns the sandbox_url on success. Caches (sandbox_url, sandbox_id, provider)
/// in the LAZY_SANDBOX thread-local so subsequent tool calls in the same
/// invocation reuse it, and lib.rs persists it via HandleToolResults params.
fn lazy_provision_sandbox(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
) -> Result<String, String> {
    use wasm_helpers::sandbox::{self, sandbox_config_from_fields};

    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let provider = sandbox::resolve_sandbox_provider(ctx, &fields)?;

    ctx.log(
        "info",
        &format!("lazy_provision_sandbox: provisioning via {provider} provider"),
    );

    // Send heartbeat so the user sees a typing indicator
    super::session::send_heartbeat(ctx, temper_api_url, tenant);

    // Create sandbox
    let config = sandbox_config_from_fields(&fields);
    let handle = sandbox::sandbox_create(ctx, &provider, &config)?;

    // Poll for readiness (max 12 retries = ~60s)
    let max_checks = 12;
    for attempt in 0..max_checks {
        match sandbox::sandbox_health_check(ctx, &handle) {
            Ok(true) => {
                ctx.log(
                    "info",
                    &format!(
                        "lazy_provision_sandbox: sandbox ready after {} checks: id={}, provider={}",
                        attempt + 1,
                        handle.sandbox_id,
                        handle.provider
                    ),
                );

                // Run post-provisioning setup (gh CLI etc.) — non-fatal
                sandbox::sandbox_setup(ctx, &handle);

                // Cache in thread-local
                LAZY_SANDBOX.with(|cell| {
                    *cell.borrow_mut() = Some((
                        handle.sandbox_url.clone(),
                        handle.sandbox_id.clone(),
                        handle.provider.clone(),
                    ));
                });

                return Ok(handle.sandbox_url);
            }
            Ok(false) => {
                ctx.log(
                    "info",
                    &format!(
                        "lazy_provision_sandbox: sandbox not ready, check {}/{}",
                        attempt + 1,
                        max_checks
                    ),
                );
            }
            Err(err) => {
                ctx.log(
                    "info",
                    &format!(
                        "lazy_provision_sandbox: readiness check failed ({}), check {}/{}",
                        err,
                        attempt + 1,
                        max_checks
                    ),
                );
            }
        }

        // Send heartbeat between retries (typing indicator)
        if attempt % 3 == 2 {
            super::session::send_heartbeat(ctx, temper_api_url, tenant);
        }
    }

    Err(format!(
        "sandbox {} did not become ready within {max_checks} readiness checks. \
         The sandbox may still be booting — try again in a moment.",
        handle.sandbox_id
    ))
}

// ---------------------------------------------------------------------------
// Sandbox dispatch (via provider abstraction)
// ---------------------------------------------------------------------------

fn dispatch_sandbox(
    ctx: &Context,
    sandbox_url: &str,
    sandbox_id: &str,
    provider: &str,
    workdir: &str,
    method: &str,
    args: &[Value],
) -> Result<Value, String> {
    use wasm_helpers::sandbox::{self, SandboxHandle};

    if sandbox_url.is_empty() {
        return Err(format!("sandbox.{method}(): no sandbox attached"));
    }

    let handle = SandboxHandle {
        sandbox_url: sandbox_url.to_string(),
        sandbox_id: sandbox_id.to_string(),
        provider: provider.to_string(),
    };

    match method {
        "read" => {
            let path = str_arg(args, 0, "path", "read")?;
            let opts = obj_arg_or_empty(args, 1);
            if is_image_extension(&path) {
                // Binary-safe read: base64-encode in sandbox to avoid UTF-8 corruption
                let b64_cmd = format!(
                    "base64 -w0 {} 2>/dev/null || base64 {}",
                    shell_quote(&path),
                    shell_quote(&path)
                );
                let result = sandbox::sandbox_exec(ctx, &handle, &b64_cmd, "/")?;
                if result.exit_code != 0 {
                    return Err(format!(
                        "sandbox.read({path}): failed to read image (exit {}): {}",
                        result.exit_code, result.stderr
                    ));
                }
                let b64_data = result.stdout.trim().to_string();
                if b64_data.is_empty() {
                    return Err(format!("sandbox.read({path}): image file is empty"));
                }
                let media_type = media_type_from_extension(&path);
                Ok(sandbox_image_read_result(
                    &path,
                    &media_type,
                    b64_data,
                    &opts,
                ))
            } else {
                let content = sandbox::sandbox_file_read(ctx, &handle, &path)?;
                Ok(json!(content))
            }
        }
        "write" => {
            let path = str_arg(args, 0, "path", "write")?;
            let content = str_arg(args, 1, "content", "write")?;
            sandbox::sandbox_file_write(ctx, &handle, &path, &content)?;
            Ok(json!({"ok": true}))
        }
        "edit" => sandbox_edit(ctx, &handle, args),
        "bash" => {
            let command = str_arg(args, 0, "command", "bash")?;
            let result = sandbox::sandbox_exec(ctx, &handle, &command, workdir)?;
            let mut output = String::new();
            if !result.stdout.is_empty() {
                output.push_str(&result.stdout);
            }
            if !result.stderr.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("STDERR: ");
                output.push_str(&result.stderr);
            }
            output.push_str(&format!("\n[exit code: {}]", result.exit_code));
            Ok(json!(output))
        }
        _ => Err(format!(
            "unknown sandbox method '{method}'. Available: read, write, edit, bash"
        )),
    }
}

fn sandbox_image_read_result(
    path: &str,
    media_type: &str,
    base64_data: String,
    opts: &Value,
) -> Value {
    let include_base64 = opts
        .get("inline")
        .or_else(|| opts.get("include_base64"))
        .or_else(|| opts.get("base64"))
        .and_then(Value::as_bool)
        .unwrap_or(base64_data.len() <= MAX_INLINE_SANDBOX_IMAGE_BASE64_CHARS);
    let approx_size_bytes = base64_data.len().saturating_mul(3) / 4;
    let mut result = json!({
        "__temperpaw_image": true,
        "media_type": media_type,
        "source_path": path,
        "byte_count": approx_size_bytes,
    });
    if include_base64 {
        result["base64_data"] = json!(base64_data);
    } else {
        result["content_ref"] = json!("sandbox_file");
    }
    result
}

/// Edit a file via read-modify-write (consumer-level operation, not provider-level).
fn sandbox_edit(
    ctx: &Context,
    handle: &wasm_helpers::sandbox::SandboxHandle,
    args: &[Value],
) -> Result<Value, String> {
    let path = str_arg(args, 0, "path", "edit")?;
    let old_string = str_arg(args, 1, "old_string", "edit")?;
    let new_string = str_arg(args, 2, "new_string", "edit")?;

    let content = wasm_helpers::sandbox::sandbox_file_read(ctx, handle, &path)?;
    if !content.contains(&old_string) {
        return Err(format!(
            "sandbox.edit({path}): old_string not found in file"
        ));
    }
    let new_content = content.replacen(&old_string, &new_string, 1);
    wasm_helpers::sandbox::sandbox_file_write(ctx, handle, &path, &new_content)?;
    Ok(json!({"ok": true}))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_batchable_tool_call(code: &str) -> Option<(String, String, Vec<String>)> {
    let trimmed = code.trim();
    let open_paren = trimmed.find('(')?;
    let close_paren = trimmed.rfind(')')?;
    if close_paren != trimmed.len().checked_sub(1)? {
        return None;
    }

    let receiver = trimmed[..open_paren].trim();
    if receiver.split('.').count() != 2 {
        return None;
    }
    let (object, method) = receiver.split_once('.')?;
    if object.is_empty() || method.is_empty() {
        return None;
    }

    let args = parse_batchable_string_arguments(&trimmed[open_paren + 1..close_paren])?;
    Some((object.to_string(), method.to_string(), args))
}

fn parse_batchable_string_arguments(args_src: &str) -> Option<Vec<String>> {
    let bytes = args_src.as_bytes();
    let mut index = 0usize;
    let mut args = Vec::new();

    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            return Some(args);
        }

        let quote = *bytes.get(index)?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        index += 1;

        let mut value = String::new();
        let mut closed = false;
        while index < bytes.len() {
            let ch = bytes[index];
            index += 1;
            if ch == quote {
                closed = true;
                break;
            }
            if ch == b'\\' {
                let escaped = *bytes.get(index)?;
                index += 1;
                value.push(match escaped {
                    b'\\' => '\\',
                    b'\'' => '\'',
                    b'"' => '"',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    other => other as char,
                });
                continue;
            }
            value.push(ch as char);
        }
        if !closed {
            return None;
        }

        args.push(value);

        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            return Some(args);
        }
        if bytes[index] != b',' {
            return None;
        }
        index += 1;
    }
}

fn batchable_direct_get_path(method: &str, args: &[String]) -> Option<(String, bool)> {
    match method {
        "show_spec" | "spec_detail" if args.len() == 1 => {
            Some((format!("/observe/specs/{}", args[0]), false))
        }
        "specs" if args.is_empty() => Some(("/observe/specs".to_string(), false)),
        "get_insights" if args.is_empty() => Some(("/api/evolution/insights".to_string(), false)),
        "get_decisions" if args.is_empty() => Some((tenant_decisions_path("{tenant}"), false)),
        "list_policies" if args.is_empty() => {
            Some(("/api/tenants/{tenant}/policies/list".to_string(), false))
        }
        _ => None,
    }
}

fn interpret_batch_json_response(
    path: &str,
    response: HttpResponse,
    unwrap_value_array: bool,
    tenant: &str,
) -> Result<Value, String> {
    let resolved_path = path.replace("{tenant}", tenant);
    if let Some(denial) = check_cedar_denial(response.status, &response.body) {
        return Err(denial);
    }
    if response.status >= 400 {
        return Err(format!(
            "HTTP GET {resolved_path}: {} {}",
            response.status, response.body
        ));
    }
    let parsed: Value = serde_json::from_str(&response.body)
        .map_err(|e| format!("failed to parse response from {resolved_path}: {e}"))?;
    if unwrap_value_array {
        Ok(parsed.get("value").cloned().unwrap_or(parsed))
    } else {
        Ok(parsed)
    }
}

fn execute_batchable_web_query_calls<F>(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    web_calls: &[(usize, &BatchableToolCall, String, String)],
    results: &mut [Option<Result<Value, String>>],
    emit_progress: &mut F,
) where
    F: FnMut(),
{
    let lookup_requests: Vec<HttpRequest> = web_calls
        .iter()
        .map(|(_, call, query_type, raw_value)| {
            let (query, url) = if query_type == "search" {
                (raw_value.as_str(), "")
            } else {
                ("", raw_value.as_str())
            };
            HttpRequest {
                method: "GET".to_string(),
                url: format!(
                    "{api_url}{}",
                    web_query_cache_lookup_path(query_type, query, url)
                ),
                headers: internal_headers_for_tool(
                    &call.plan.tool_name,
                    Some(call.tool_call_id.as_str()),
                ),
                body: String::new(),
            }
        })
        .collect();

    emit_progress();
    let lookup_responses = match ctx.http_call_batch(&lookup_requests) {
        Ok(responses) => responses,
        Err(error) => {
            for (index, _, _, _) in web_calls {
                results[*index] = Some(Err(error.clone()));
            }
            return;
        }
    };

    #[derive(Clone)]
    struct PendingWebQuery<'a> {
        index: usize,
        call: &'a BatchableToolCall,
        query_type: String,
        raw_value: String,
    }

    let mut pending = Vec::<PendingWebQuery<'_>>::new();
    for ((index, call, query_type, raw_value), response) in
        web_calls.iter().zip(lookup_responses.into_iter())
    {
        let lookup_path = if query_type == "search" {
            web_query_cache_lookup_path(query_type, raw_value, "")
        } else {
            web_query_cache_lookup_path(query_type, "", raw_value)
        };
        let lookup_json = match interpret_batch_json_response(&lookup_path, response, false, tenant)
        {
            Ok(value) => value,
            Err(error) => {
                ctx.log(
                    "warn",
                    &format!(
                        "web_query: cache lookup failed, falling back to fresh query: {error}"
                    ),
                );
                pending.push(PendingWebQuery {
                    index: *index,
                    call,
                    query_type: query_type.clone(),
                    raw_value: raw_value.clone(),
                });
                continue;
            }
        };

        match interpret_cached_web_query_result(query_type, &lookup_json) {
            Ok(Some(value)) => results[*index] = Some(Ok(value)),
            Ok(None) => pending.push(PendingWebQuery {
                index: *index,
                call,
                query_type: query_type.clone(),
                raw_value: raw_value.clone(),
            }),
            Err(error) => results[*index] = Some(Err(error)),
        }
    }

    if pending.is_empty() {
        return;
    }

    emit_progress();
    let create_requests: Vec<HttpRequest> = pending
        .iter()
        .map(|item| {
            let (query, url) = if item.query_type == "search" {
                (item.raw_value.clone(), String::new())
            } else {
                (String::new(), item.raw_value.clone())
            };
            HttpRequest {
                method: "POST".to_string(),
                url: format!("{api_url}/tdata/WebQueries"),
                headers: internal_headers_for_tool(
                    &item.call.plan.tool_name,
                    Some(item.call.tool_call_id.as_str()),
                ),
                body: json!({
                    "QueryType": item.query_type,
                    "Query": query,
                    "Url": url,
                })
                .to_string(),
            }
        })
        .collect();

    let create_responses = match ctx.http_call_batch(&create_requests) {
        Ok(responses) => responses,
        Err(error) => {
            for item in &pending {
                results[item.index] = Some(Err(error.clone()));
            }
            return;
        }
    };

    #[derive(Clone)]
    struct CreatedWebQuery<'a> {
        index: usize,
        call: &'a BatchableToolCall,
        query_type: String,
        raw_value: String,
        entity_id: String,
    }

    let mut created = Vec::<CreatedWebQuery<'_>>::new();
    for (item, response) in pending.iter().zip(create_responses.into_iter()) {
        match interpret_batch_json_response("/tdata/WebQueries", response, false, tenant) {
            Ok(entity) => {
                let entity_id = entity
                    .get("entity_id")
                    .or_else(|| entity.get("EntityId"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match entity_id {
                    Some(entity_id) => created.push(CreatedWebQuery {
                        index: item.index,
                        call: item.call,
                        query_type: item.query_type.clone(),
                        raw_value: item.raw_value.clone(),
                        entity_id,
                    }),
                    None => {
                        results[item.index] = Some(Err(
                            "web_query: failed to get entity_id from created WebQuery".to_string(),
                        ))
                    }
                }
            }
            Err(error) => results[item.index] = Some(Err(error)),
        }
    }

    if created.is_empty() {
        return;
    }

    emit_progress();
    let action_requests: Vec<HttpRequest> = created
        .iter()
        .map(|item| {
            let action_name = if item.query_type == "search" {
                "ExecuteSearch"
            } else {
                "ExecuteFetch"
            };
            let action_params = if item.query_type == "search" {
                json!({ "query": item.raw_value })
            } else {
                json!({ "url": item.raw_value })
            };
            let key = escape_odata_key(&item.entity_id);
            HttpRequest {
                method: "POST".to_string(),
                url: format!(
                    "{api_url}/tdata/WebQueries('{key}')/Temper.{action_name}?await_integration=true"
                ),
                headers: internal_headers_for_tool(
                    &item.call.plan.tool_name,
                    Some(item.call.tool_call_id.as_str()),
                ),
                body: action_params.to_string(),
            }
        })
        .collect();

    let action_responses = match ctx.http_call_batch(&action_requests) {
        Ok(responses) => responses,
        Err(error) => {
            for item in &created {
                results[item.index] = Some(Err(error.clone()));
            }
            return;
        }
    };

    let mut completed = Vec::<CreatedWebQuery<'_>>::new();
    for (item, response) in created.iter().zip(action_responses.into_iter()) {
        let action_name = if item.query_type == "search" {
            "ExecuteSearch"
        } else {
            "ExecuteFetch"
        };
        let key = escape_odata_key(&item.entity_id);
        let action_path =
            format!("/tdata/WebQueries('{key}')/Temper.{action_name}?await_integration=true");
        match interpret_batch_json_response(&action_path, response, false, tenant) {
            Ok(_) => completed.push(item.clone()),
            Err(error) => results[item.index] = Some(Err(error)),
        }
    }

    if completed.is_empty() {
        return;
    }

    emit_progress();
    for item in &completed {
        wait_for_web_query_completion(ctx, api_url, tenant, &item.entity_id, &item.query_type);
    }

    emit_progress();
    let get_requests: Vec<HttpRequest> = completed
        .iter()
        .map(|item| {
            let key = escape_odata_key(&item.entity_id);
            HttpRequest {
                method: "GET".to_string(),
                url: format!("{api_url}/tdata/WebQueries('{key}')"),
                headers: internal_headers_for_tool(
                    &item.call.plan.tool_name,
                    Some(item.call.tool_call_id.as_str()),
                ),
                body: String::new(),
            }
        })
        .collect();

    let get_responses = match ctx.http_call_batch(&get_requests) {
        Ok(responses) => responses,
        Err(error) => {
            for item in &completed {
                results[item.index] = Some(Err(error.clone()));
            }
            return;
        }
    };

    let recent_user_messages = if completed.iter().any(|item| item.query_type == "search") {
        Some(recent_user_messages(ctx, api_url, tenant, 8))
    } else {
        None
    };

    for (item, response) in completed.iter().zip(get_responses.into_iter()) {
        let key = escape_odata_key(&item.entity_id);
        let entity_path = format!("/tdata/WebQueries('{key}')");
        let result = match interpret_batch_json_response(&entity_path, response, false, tenant) {
            Ok(value) => value,
            Err(error) => {
                results[item.index] = Some(Err(error));
                continue;
            }
        };

        let result_fields = result.get("fields").cloned().unwrap_or(result.clone());
        let (status, results_raw) =
            match interpret_web_query_entity_result(&item.query_type, &result_fields) {
                Ok(parts) => parts,
                Err(error) => {
                    results[item.index] = Some(Err(error));
                    continue;
                }
            };

        let parsed_result =
            serde_json::from_str::<Value>(results_raw).unwrap_or_else(|_| json!(results_raw));
        if item.query_type == "fetch" && web_search_results_empty(&parsed_result) {
            results[item.index] = Some(Err(format!(
                "web_fetch: fetched no readable content from {}; try a more specific page or search first",
                item.raw_value
            )));
            continue;
        }

        if item.query_type == "search" && web_search_results_empty(&parsed_result) {
            let retry_query = recent_user_messages
                .as_ref()
                .and_then(|messages| fallback_web_search_query(&item.raw_value, messages));
            if let Some(retry_query) = retry_query {
                ctx.log(
                    "info",
                    &format!(
                        "web_search: retrying vague zero-result query '{}' as '{}'",
                        item.raw_value, retry_query
                    ),
                );
                results[item.index] = Some(web_query_dispatch(
                    ctx,
                    api_url,
                    tenant,
                    "search",
                    &retry_query,
                    "",
                ));
                continue;
            }
        }

        ctx.log(
            "info",
            &format!(
                "web_query: {} completed with status {status}",
                item.query_type
            ),
        );
        results[item.index] = Some(Ok(parsed_result));
    }
}

fn wait_for_web_query_completion(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    entity_id: &str,
    query_type: &str,
) {
    let path = format!(
        "/observe/entities/WebQuery/{entity_id}/wait?statuses=Complete,Failed&timeout_ms=30000&poll_ms=250"
    );
    if let Err(error) = http_get(ctx, api_url, tenant, &path) {
        ctx.log(
            "warn",
            &format!(
                "web_query: wait for {query_type} WebQuery '{entity_id}' did not complete cleanly; reading current entity state: {error}"
            ),
        );
    }
}

fn str_arg(args: &[Value], idx: usize, name: &str, method: &str) -> Result<String, String> {
    args.get(idx)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("temper.{method}(): missing argument '{name}' at position {idx}"))
}

fn opt_str_arg(args: &[Value], idx: usize) -> Option<String> {
    args.get(idx)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn obj_arg(args: &[Value], idx: usize, name: &str, method: &str) -> Result<Value, String> {
    args.get(idx)
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| {
            format!("temper.{method}(): missing object argument '{name}' at position {idx}")
        })
}

fn obj_arg_or_empty(args: &[Value], idx: usize) -> Value {
    args.get(idx)
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}))
}

fn escape_odata_key(key: &str) -> String {
    key.replace('\'', "''")
}

fn escape_odata_string_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn encode_odata_filter_literal(value: &str) -> String {
    escape_odata_string_literal(value)
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('+', "%2B")
}

/// Minimal headers for internal Temper API calls.
/// Auth headers are injected by the WASM host — see ADR-0043.
/// Tool observability is now provided by the structured guest span API;
/// span-hint headers remain available through `internal_headers_for_tool`
/// for compatibility paths that cannot yet own a guest span per request.
fn internal_headers() -> Vec<(String, String)> {
    vec![("Content-Type".to_string(), "application/json".to_string())]
}

pub fn check_cedar_denial(status: u16, body: &str) -> Option<String> {
    if status == 403 {
        if let Ok(parsed) = serde_json::from_str::<Value>(body) {
            // Direct decision_id field (e.g. from /api/authorize)
            if let Some(did) = parsed.get("decision_id").and_then(|v| v.as_str()) {
                return Some(format!("CEDAR_DENIED:{}:{}", did, body));
            }
            // Error message formats from different endpoints:
            //   OData:       "... (decision: PD-xxx)"
            //   API (specs): "... Decision PD-xxx"
            if let Some(msg) = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                // Format 1: OData — "(decision: PD-xxx)"
                if let Some(start) = msg.find("(decision: ") {
                    let after = &msg[start + "(decision: ".len()..];
                    if let Some(end) = after.find(')') {
                        let did = &after[..end];
                        return Some(format!("CEDAR_DENIED:{}:{}", did, body));
                    }
                }
                // Format 2: API — "Decision PD-xxx" (at end of message)
                if let Some(start) = msg.find("Decision PD-") {
                    let did = msg[start + "Decision ".len()..].trim();
                    if !did.is_empty() {
                        return Some(format!("CEDAR_DENIED:{}:{}", did, body));
                    }
                }
            }
        }
    }
    None
}

fn http_get(ctx: &Context, api_url: &str, _tenant: &str, path: &str) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
        return Err(denial);
    }
    if resp.status >= 400 {
        return Err(format!("HTTP GET {path}: {} {}", resp.status, resp.body));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {path}: {e}"))
}

fn http_post(
    ctx: &Context,
    api_url: &str,
    _tenant: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
        return Err(denial);
    }
    if resp.status >= 400 {
        return Err(format!("HTTP POST {path}: {} {}", resp.status, resp.body));
    }
    if resp.body.is_empty() {
        return Ok(json!({"ok": true}));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {path}: {e}"))
}

fn http_patch(
    ctx: &Context,
    api_url: &str,
    _tenant: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("PATCH", &url, &headers, &body.to_string())?;
    if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
        return Err(denial);
    }
    if resp.status >= 400 {
        return Err(format!("HTTP PATCH {path}: {} {}", resp.status, resp.body));
    }
    if resp.body.is_empty() {
        return Ok(json!({"ok": true}));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {path}: {e}"))
}

fn http_delete(ctx: &Context, api_url: &str, _tenant: &str, path: &str) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("DELETE", &url, &headers, "")?;
    if let Some(denial) = check_cedar_denial(resp.status, &resp.body) {
        return Err(denial);
    }
    if resp.status >= 400 {
        return Err(format!("HTTP DELETE {path}: {} {}", resp.status, resp.body));
    }
    if resp.body.is_empty() {
        return Ok(json!({"ok": true}));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {path}: {e}"))
}

// ---------------------------------------------------------------------------
// Image file helpers
// ---------------------------------------------------------------------------

fn is_image_extension(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}

fn media_type_from_extension(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::{
        BatchableToolPlan, BatchableToolPlanKind, LAZY_SANDBOX,
        MAX_INLINE_SANDBOX_IMAGE_BASE64_CHARS, ODataQueryArg, batchable_tool_plan_from_code,
        coalesce_sandbox_args, encode_odata_filter_literal, escape_odata_string_literal,
        fallback_web_search_query, genesis_registry_tenant, has_model_csdl,
        interpret_cached_web_query_result, interpret_web_query_entity_result, is_image_extension,
        is_vague_web_search_query, json_dumps, json_loads, media_type_from_extension,
        normalize_image_model_for_provider, normalize_image_provider, normalize_odata_query_arg,
        record_dispatch_image_result, repository_id_for, sandbox_identity_from_fields,
        sandbox_image_read_result, take_dispatch_image_results, tool_span_hint_headers_for,
        web_query_cache_lookup_path, web_search_results_empty,
    };
    use serde_json::json;

    #[test]
    fn detects_model_csdl_at_root() {
        assert!(has_model_csdl(&["model.csdl.xml".to_string()]));
    }

    #[test]
    fn genesis_publish_helpers_use_registry_id_conventions() {
        assert_eq!(
            repository_id_for("Arni Labs", "Katagami Commons"),
            "rp-arni-labs-katagami-commons"
        );
    }

    #[test]
    fn genesis_registry_tenant_defaults_to_default() {
        assert_eq!(genesis_registry_tenant(&serde_json::Map::new()), "default");
        let mut input = serde_json::Map::new();
        input.insert("registry_tenant".to_string(), json!("team-a"));
        assert_eq!(genesis_registry_tenant(&input), "team-a");
    }

    #[test]
    fn json_dumps_serializes_agent_payloads_without_imports() {
        let serialized = json_dumps(
            &[json!({
                "source_ids": ["src-1", "src-2"],
                "summary": "ink + editorial",
            })],
            &[(json!("ensure_ascii"), json!(false))],
        )
        .expect("json.dumps should serialize supported values");

        assert_eq!(
            serialized,
            json!("{\"source_ids\":[\"src-1\",\"src-2\"],\"summary\":\"ink + editorial\"}")
        );
    }

    #[test]
    fn json_loads_parses_agent_payloads_without_imports() {
        let parsed = json_loads(
            &[json!(
                "{\"source_ids\":[\"src-1\"],\"archive_status\":\"deferred\"}"
            )],
            &[],
        )
        .expect("json.loads should parse supported JSON");

        assert_eq!(
            parsed,
            json!({"source_ids": ["src-1"], "archive_status": "deferred"})
        );
    }

    #[test]
    fn detects_model_csdl_in_nested_bundle() {
        assert!(has_model_csdl(&[
            "InlineProbe/model.csdl.xml".to_string(),
            "InlineProbe/order.ioa.toml".to_string(),
        ]));
    }

    #[test]
    fn rejects_bundle_without_model_csdl() {
        assert!(!has_model_csdl(&["bookmark.ioa.toml".to_string()]));
    }

    #[test]
    fn vague_web_search_query_detects_meta_follow_ups() {
        assert!(is_vague_web_search_query("super famous"));
        assert!(is_vague_web_search_query("that repo"));
        assert!(!is_vague_web_search_query("openclaw github repo"));
    }

    #[test]
    fn fallback_web_search_query_uses_recent_repo_subject() {
        let retry = fallback_web_search_query(
            "super famous",
            &[
                "Can you clone openclaw repo".to_string(),
                "I asked you to clone openclaw you don't remember?".to_string(),
            ],
        );

        assert_eq!(retry.as_deref(), Some("openclaw github repo"));
    }

    #[test]
    fn fallback_web_search_query_ignores_specific_queries() {
        let retry = fallback_web_search_query(
            "openclaw github repo",
            &["Can you clone openclaw repo".to_string()],
        );

        assert_eq!(retry, None);
    }

    #[test]
    fn interpret_web_query_entity_result_requires_complete_status() {
        let err = interpret_web_query_entity_result(
            "search",
            &json!({
                "status": "Created",
                "results": "[]",
            }),
        )
        .unwrap_err();

        assert_eq!(err, "web_search: query never completed (status=Created)");
    }

    #[test]
    fn interpret_web_query_entity_result_surfaces_recorded_errors() {
        let err = interpret_web_query_entity_result(
            "fetch",
            &json!({
                "status": "Failed",
                "error": "missing exa_api_key",
            }),
        )
        .unwrap_err();

        assert_eq!(err, "web_fetch: missing exa_api_key");
    }

    #[test]
    fn web_query_cache_lookup_path_escapes_single_quotes() {
        let path = web_query_cache_lookup_path("fetch", "", "https://example.com/that's-all");

        assert!(path.contains("Status%20eq%20'Complete'"));
        assert!(path.contains("QueryType%20eq%20'fetch'"));
        assert!(path.contains("Url%20eq%20'https://example.com/that''s-all'"));
    }

    #[test]
    fn interpret_cached_web_query_result_reads_first_completed_entity() {
        let cached = interpret_cached_web_query_result(
            "fetch",
            &json!({
                "value": [{
                    "fields": {
                        "status": "Complete",
                        "results": "{\"title\":\"cached\"}"
                    }
                }]
            }),
        )
        .expect("cached lookup should parse");

        assert_eq!(cached, Some(json!({"title": "cached"})));
    }

    #[test]
    fn web_search_results_empty_treats_blank_strings_as_empty() {
        assert!(web_search_results_empty(&json!("")));
        assert!(web_search_results_empty(&json!("[]")));
        assert!(!web_search_results_empty(&json!("headline text")));
    }

    #[test]
    fn openai_codex_image_generation_drops_public_openai_image_model_names() {
        let provider = normalize_image_provider("codex");

        assert_eq!(provider, "openai_codex");
        assert_eq!(
            normalize_image_model_for_provider(&provider, "gpt-image-2"),
            ""
        );
        assert_eq!(
            normalize_image_model_for_provider(&provider, " dall-e-3 "),
            ""
        );
    }

    #[test]
    fn dispatch_image_results_are_collected_and_cleared() {
        let _ = take_dispatch_image_results();
        let image = json!({
            "__temperpaw_image": true,
            "file_id": "fl-cat",
            "path": "/generated/images/cat.png"
        });

        record_dispatch_image_result(&image);

        assert_eq!(take_dispatch_image_results(), vec![image]);
        assert!(take_dispatch_image_results().is_empty());
    }

    #[test]
    fn test_is_image_extension() {
        assert!(is_image_extension("/tmp/screenshot.png"));
        assert!(is_image_extension("/tmp/photo.JPG"));
        assert!(is_image_extension("file.jpeg"));
        assert!(is_image_extension("file.gif"));
        assert!(is_image_extension("file.webp"));
        assert!(!is_image_extension("/tmp/data.txt"));
        assert!(!is_image_extension("/tmp/code.rs"));
        assert!(!is_image_extension("/tmp/doc.pdf"));
        assert!(!is_image_extension("no_extension"));
    }

    #[test]
    fn test_media_type_from_extension() {
        assert_eq!(media_type_from_extension("file.png"), "image/png");
        assert_eq!(media_type_from_extension("file.jpg"), "image/jpeg");
        assert_eq!(media_type_from_extension("file.jpeg"), "image/jpeg");
        assert_eq!(media_type_from_extension("file.gif"), "image/gif");
        assert_eq!(media_type_from_extension("file.webp"), "image/webp");
        assert_eq!(
            media_type_from_extension("file.bmp"),
            "application/octet-stream"
        );
    }

    #[test]
    fn sandbox_identity_uses_lazy_cache_when_entity_state_is_empty() {
        LAZY_SANDBOX.with(|cell| {
            *cell.borrow_mut() = Some((
                "https://sandbox.example".to_string(),
                "sb-lazy".to_string(),
                "modal".to_string(),
            ));
        });

        let (sandbox_id, provider) = sandbox_identity_from_fields(&json!({}));

        assert_eq!(sandbox_id.as_deref(), Some("sb-lazy"));
        assert_eq!(provider.as_deref(), Some("modal"));

        LAZY_SANDBOX.with(|cell| *cell.borrow_mut() = None);
    }

    #[test]
    fn sandbox_identity_prefers_persisted_entity_state_over_lazy_cache() {
        LAZY_SANDBOX.with(|cell| {
            *cell.borrow_mut() = Some((
                "https://sandbox.example".to_string(),
                "sb-lazy".to_string(),
                "modal".to_string(),
            ));
        });

        let (sandbox_id, provider) = sandbox_identity_from_fields(&json!({
            "sandbox_id": "sb-persisted",
            "sandbox_provider": "tensorlake"
        }));

        assert_eq!(sandbox_id.as_deref(), Some("sb-persisted"));
        assert_eq!(provider.as_deref(), Some("tensorlake"));

        LAZY_SANDBOX.with(|cell| *cell.borrow_mut() = None);
    }

    #[test]
    fn escape_odata_string_literal_doubles_apostrophes() {
        assert_eq!(escape_odata_string_literal("that's it"), "that''s it");
    }

    #[test]
    fn encode_odata_filter_literal_percent_encodes_spaces() {
        assert_eq!(
            encode_odata_filter_literal("neo brutalism ui"),
            "neo%20brutalism%20ui"
        );
    }

    #[test]
    fn normalize_odata_query_arg_treats_blank_filters_as_no_query() {
        assert_eq!(normalize_odata_query_arg(""), None);
        assert_eq!(normalize_odata_query_arg("   "), None);
        assert_eq!(normalize_odata_query_arg("$filter="), None);
        assert_eq!(normalize_odata_query_arg("?$filter=   "), None);
    }

    #[test]
    fn normalize_odata_query_arg_preserves_filters_and_raw_queries() {
        assert_eq!(
            normalize_odata_query_arg("Status eq 'Ready'"),
            Some(ODataQueryArg::Filter("Status eq 'Ready'".to_string()))
        );
        assert_eq!(
            normalize_odata_query_arg("?$top=5&$orderby=CreatedAt desc"),
            Some(ODataQueryArg::Raw(
                "$top=5&$orderby=CreatedAt desc".to_string()
            ))
        );
    }

    #[test]
    fn batchable_tool_plan_parses_web_fetch_literal() {
        let plan = batchable_tool_plan_from_code("temper.web_fetch('https://example.com/docs')");

        assert_eq!(
            plan,
            Some(BatchableToolPlan {
                tool_name: "temper.web_fetch".to_string(),
                kind: BatchableToolPlanKind::WebQueryFetch {
                    url: "https://example.com/docs".to_string(),
                },
            })
        );
    }

    #[test]
    fn batchable_tool_plan_parses_show_spec_literal() {
        let plan = batchable_tool_plan_from_code("temper.show_spec(\"Session\")");

        assert_eq!(
            plan,
            Some(BatchableToolPlan {
                tool_name: "temper.show_spec".to_string(),
                kind: BatchableToolPlanKind::DirectGet {
                    path: "/observe/specs/Session".to_string(),
                    unwrap_value_array: false,
                },
            })
        );
    }

    #[test]
    fn batchable_tool_plan_uses_tenant_scoped_decisions_path() {
        let plan = batchable_tool_plan_from_code("temper.get_decisions()");

        assert_eq!(
            plan,
            Some(BatchableToolPlan {
                tool_name: "temper.get_decisions".to_string(),
                kind: BatchableToolPlanKind::DirectGet {
                    path: "/api/tenants/{tenant}/decisions?status=pending".to_string(),
                    unwrap_value_array: false,
                },
            })
        );
    }

    #[test]
    fn decision_poll_path_is_tenant_scoped() {
        assert_eq!(
            super::tenant_decision_path("default", "PD-123"),
            "/api/tenants/default/decisions/PD-123"
        );
    }

    #[test]
    fn cedar_denial_parser_accepts_top_level_decision_id() {
        let denial = super::check_cedar_denial(
            403,
            r#"{"decision_id":"PD-123","error":{"code":"AuthorizationDenied","message":"no matching permit policy Decision PD-123"}}"#,
        )
        .expect("decision-bearing denial should parse");

        assert!(denial.starts_with("CEDAR_DENIED:PD-123:"));
    }

    #[test]
    fn tool_span_hints_use_datadog_tool_operation_semconv() {
        let headers = tool_span_hint_headers_for(Some("temper.get"), Some("call-123"));
        let lookup = |key: &str| {
            headers
                .iter()
                .find(|(header, _)| header == key)
                .map(|(_, value)| value.as_str())
        };

        assert_eq!(lookup("X-Temper-Span-Name"), Some("tool.temper.get"));
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.operation.name"),
            Some("execute_tool")
        );
        assert_eq!(lookup("X-Temper-Span-Attr-tool.name"), Some("temper.get"));
        assert_eq!(lookup("X-Temper-Span-Attr-tool.call_id"), Some("call-123"));
    }

    #[test]
    fn batchable_tool_plan_rejects_assignments_and_non_literal_args() {
        assert_eq!(
            batchable_tool_plan_from_code("result = temper.web_search(query)"),
            None
        );
        assert_eq!(
            batchable_tool_plan_from_code("temper.get(\"Sessions\", session_id)"),
            None
        );
    }

    #[test]
    fn sandbox_read_kwargs_become_options_arg() {
        let args = coalesce_sandbox_args(
            "read",
            &[json!("/tmp/thumbnail.jpg")],
            &[(json!("inline"), json!(true))],
        )
        .unwrap();

        assert_eq!(
            args,
            vec![json!("/tmp/thumbnail.jpg"), json!({"inline": true})]
        );
    }

    #[test]
    fn sandbox_image_read_result_omits_large_base64_by_default() {
        let result = sandbox_image_read_result(
            "/tmp/thumbnail.jpg",
            "image/jpeg",
            "a".repeat(MAX_INLINE_SANDBOX_IMAGE_BASE64_CHARS + 1),
            &json!({}),
        );

        assert_eq!(result["__temperpaw_image"], true);
        assert_eq!(result["source_path"], "/tmp/thumbnail.jpg");
        assert_eq!(result["content_ref"], "sandbox_file");
        assert!(result.get("base64_data").is_none());
    }

    #[test]
    fn sandbox_image_read_result_includes_base64_when_requested() {
        let result = sandbox_image_read_result(
            "/tmp/thumbnail.jpg",
            "image/jpeg",
            "abcd".to_string(),
            &json!({"inline": true}),
        );

        assert_eq!(result["base64_data"], "abcd");
        assert!(result.get("content_ref").is_none());
    }
    #[test]
    fn continuation_preserves_dispatch_state_and_defers_approval_or_done() {
        super::DISPATCH_OUTPUT.with(|v| *v.borrow_mut() = Some("required output".into()));
        super::DISPATCH_IMAGE_RESULTS.with(|v| *v.borrow_mut() = vec![json!({"path":"image.png"})]);
        super::LAZY_SANDBOX
            .with(|v| *v.borrow_mut() = Some(("url".into(), "id".into(), "provider".into())));
        let saved = super::continuation_state();
        super::restore_continuation_state(&json!({"output":null,"images":[],"sandbox":null}))
            .unwrap();
        super::restore_continuation_state(&saved).unwrap();
        assert_eq!(super::continuation_state(), saved);
        assert!(super::continuation_allowed());
        super::CEDAR_DENIAL.with(|v| *v.borrow_mut() = Some("approval".into()));
        assert!(!super::continuation_allowed());
        super::CEDAR_DENIAL.with(|v| *v.borrow_mut() = None);
        super::DONE_RESULT.with(|v| *v.borrow_mut() = Some("done".into()));
        assert!(!super::continuation_allowed());
        super::DONE_RESULT.with(|v| *v.borrow_mut() = None);
        super::restore_continuation_state(&json!({"output":null,"images":[],"sandbox":null}))
            .unwrap();
    }
}
