//! Entity operations ported from tool_runner/entity_tools.rs.
//!
//! These methods are dispatched from `temper.<method>()` calls in Monty code.
//! They use the same HTTP patterns as dispatch.rs (ctx.http_call, JSON serialization).

use base64::Engine;
use serde_json::{Value, json};
use temper_wasm_sdk::context::Context;
use tool_catalog::DEFAULT_TOOLS_ENABLED;
use wasm_helpers::{
    bounded_reads, read_content_file_version, read_session_from_temperfs, runtime_headers,
};

use crate::dispatch;

#[cfg(target_arch = "wasm32")]
const FILE_UPLOAD_STREAM_CHUNK_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// spawn_session
// ---------------------------------------------------------------------------

pub fn spawn_session(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    workdir: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = spawn_session_input(args)?;

    let task = require_str(&input, "task", "spawn_session")?;
    let requested_id = input.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");

    // Extract parent session fields early — used for provider/model inheritance,
    // workspace sharing, depth guard, and project context.
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let parent_id = ctx
        .entity_state
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Provider/model selection is explicit input first, then inherited parent
    // Session config. A missing value is a configuration error.
    let parent_provider = fields
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parent_model = fields.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let parent_provider_options_json = fields
        .get("provider_options_json")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parent_temperature = fields
        .get("temperature")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let requested_model = input
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if parent_model.is_empty() {
                None
            } else {
                Some(parent_model)
            }
        })
        .ok_or_else(|| {
            "spawn_session requires model: pass opts.model or invoke from a configured parent Session"
                .to_string()
        })?;
    let provider = input
        .get("provider")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if parent_provider.is_empty() {
                None
            } else {
                Some(parent_provider)
            }
        })
        .ok_or_else(|| {
            "spawn_session requires provider: pass opts.provider or invoke from a configured parent Session"
                .to_string()
        })?;
    let model = resolve_spawn_model_for_provider(
        provider,
        requested_model,
        parent_model,
        ctx.config.get("default_llm_model").map(String::as_str),
    )?;
    if model != requested_model.trim() {
        ctx.log(
            "warn",
            &format!(
                "spawn_session: replaced incompatible model {requested_model} with {model} for provider={provider}"
            ),
        );
    }
    let temperature = input
        .get("temperature")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if parent_temperature.is_empty() {
                None
            } else {
                Some(parent_temperature)
            }
        })
        .unwrap_or("1.0");
    let provider_options_json = input
        .get("provider_options_json")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if parent_provider_options_json.is_empty() {
                None
            } else {
                Some(parent_provider_options_json)
            }
        })
        .unwrap_or("");
    let tools = input
        .get("tools")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_TOOLS_ENABLED);
    let soul_id = input.get("soul_id").and_then(|v| v.as_str()).unwrap_or("");
    // Workspace sharing: allow child to reuse parent's workspace.
    let share_workspace = input
        .get("share_workspace")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let parent_workspace_id = fields
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let child_workspace_id = input
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if share_workspace && !parent_workspace_id.is_empty() {
                Some(parent_workspace_id)
            } else {
                None
            }
        })
        .unwrap_or("");

    let child_sandbox_url = input
        .get("sandbox_url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(sandbox_url);
    let child_workdir = input
        .get("workdir")
        .and_then(|v| v.as_str())
        .unwrap_or(workdir);
    let background = input
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_turns = input
        .get("max_turns")
        .and_then(|v| v.as_str())
        .unwrap_or("20");
    let mode = input
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("execute");
    // In plan mode, override tools to PLAN_MODE_TOOLS unless explicitly provided
    let tools = if mode == "plan" && input.get("tools").is_none() {
        super::dispatch::PLAN_MODE_TOOLS
    } else {
        tools
    };

    let run_tools_timeout_secs = ctx
        .config
        .get("timeout_secs")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(120);
    let default_wait_timeout_ms = ((run_tools_timeout_secs.saturating_sub(30)).max(30)) * 1000;
    let wait_timeout_ms = input
        .get("timeout_ms")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            ctx.config
                .get("spawn_session_wait_timeout_ms")
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(default_wait_timeout_ms)
        .max(1_000);

    // Depth guard (fields already extracted above)
    let current_depth = fields
        .get("session_depth")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if current_depth >= 5 {
        return Err("spawn_session: agent_depth guard hit (max depth 5)".into());
    }

    // Create Session entity
    let mut create_body = json!({ "ParentSessionId": parent_id });
    if !requested_id.is_empty() {
        create_body["Id"] = Value::String(requested_id.to_string());
    }
    let resp = http_post(
        ctx,
        api_url,
        tenant,
        parent_id,
        "/tdata/Sessions",
        &create_body,
    )?;
    let child_id = resp
        .get("entity_id")
        .or_else(|| resp.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if child_id.is_empty() {
        return Err("spawn_session: created entity has no Id".into());
    }

    // Configure
    let config_body = json!({
        "system_prompt": input.get("system_prompt").and_then(Value::as_str).unwrap_or(""),
        "model": model, "provider": provider, "provider_options_json": provider_options_json, "temperature": temperature, "tools_enabled": tools,
        "soul_id": soul_id, "user_message": task, "parent_session_id": parent_id,
        "sandbox_url": child_sandbox_url, "workdir": child_workdir,
        "workspace_id": child_workspace_id,
        "session_depth": current_depth + 1, "max_turns": max_turns,
        "session_mode": mode,
        "project_harness_id": input
            .get("project_harness_id")
            .and_then(|v| v.as_str())
            .or_else(|| fields.get("project_harness_id").and_then(|v| v.as_str()))
            .unwrap_or(""),
        "project_id": input
            .get("project_id")
            .and_then(|v| v.as_str())
            .or_else(|| fields.get("project_id").and_then(|v| v.as_str()))
            .unwrap_or(""),
    });
    http_post(
        ctx,
        api_url,
        tenant,
        parent_id,
        &format!("/tdata/Sessions('{child_id}')/TemperPaw.Configure"),
        &config_body,
    )?;

    // Configure schedules ProvisionWorkspace automatically (ADR-0022).
    // Sandbox is provisioned lazily on first sandbox tool call.

    if background {
        return Ok(json!({
            "session_id": child_id,
            "status": "provisioning",
            "background": true,
        }));
    }

    // Wait for completion
    let wait_url = format!(
        "/observe/entities/Session/{child_id}/wait?statuses=Completed,Failed,Cancelled&timeout_ms={wait_timeout_ms}&poll_ms=250"
    );
    let entity = http_get(ctx, api_url, tenant, parent_id, &wait_url)?;
    let status = entity_field_str_val(&entity, "Status");
    let result = entity_field_str_val(&entity, "Result");

    Ok(json!({
        "session_id": child_id,
        "status": status,
        "result": result,
    }))
}

// ---------------------------------------------------------------------------
// list_sessions
// ---------------------------------------------------------------------------

pub fn list_sessions(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = list_sessions_input(args);
    let eid = ctx_entity_id(ctx);

    let filter = input.get("filter").and_then(|v| v.as_str()).unwrap_or("");
    let top = input.get("top").and_then(|v| v.as_i64()).unwrap_or(50);

    let mut path = String::from("/tdata/Sessions");
    let mut query_parts: Vec<String> = Vec::new();
    if !filter.is_empty() {
        query_parts.push(format!("$filter={}", urlenc(filter)));
    }
    if top > 0 {
        query_parts.push(format!("$top={top}"));
    }
    if !query_parts.is_empty() {
        path.push('?');
        path.push_str(&query_parts.join("&"));
    }

    let resp = http_get(ctx, api_url, tenant, eid, &path)?;
    Ok(resp.get("value").cloned().unwrap_or(resp))
}

// ---------------------------------------------------------------------------
// abort_session
// ---------------------------------------------------------------------------

pub fn abort_session(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = abort_session_input(args)?;
    let session_id = require_str(&input, "session_id", "abort_session")?;
    let eid = ctx_entity_id(ctx);
    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Sessions('{session_id}')/TemperPaw.Cancel"),
        &json!({}),
    )?;
    Ok(json!({ "session_id": session_id, "status": "cancelled" }))
}

// ---------------------------------------------------------------------------
// steer_session
// ---------------------------------------------------------------------------

pub fn steer_session(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = steer_session_input(args)?;
    let session_id = require_str(&input, "session_id", "steer_session")?;
    let message = require_str(&input, "message", "steer_session")?;
    let eid = ctx_entity_id(ctx);

    // Get current steering messages
    let entity = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Sessions('{session_id}')"),
    )?;
    let existing = entity_field_str_val(&entity, "SteeringMessages");
    let existing = if existing.is_empty() { "[]" } else { &existing };
    let mut queue: Vec<Value> = serde_json::from_str(existing).unwrap_or_default();
    queue.push(json!({ "content": message }));

    let body = json!({
        "steering_messages": serde_json::to_string(&queue).unwrap_or_else(|_| "[]".to_string())
    });
    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Sessions('{session_id}')/TemperPaw.Steer"),
        &body,
    )?;
    Ok(json!({ "session_id": session_id, "steered": true }))
}

// ---------------------------------------------------------------------------
// save_memory
// ---------------------------------------------------------------------------

pub fn save_memory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = save_memory_input(args)?;
    let key = require_str(&input, "key", "save_memory")?;
    let content = require_str(&input, "content", "save_memory")?;
    let memory_type = input
        .get("memory_type")
        .and_then(|v| v.as_str())
        .unwrap_or("reference");
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let agent_id = fields
        .get("agent_id")
        .or_else(|| fields.get("AgentId"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let soul_id = fields.get("soul_id").and_then(|v| v.as_str()).unwrap_or("");

    let body = json!({
        "Key": key, "Content": content, "MemoryType": memory_type,
        "AgentId": agent_id, "SoulId": soul_id,
    });
    let eid = ctx_entity_id(ctx);
    let resp = http_post(ctx, api_url, tenant, eid, "/tdata/Memories", &body)?;

    // Dispatch Save action
    let memory_id = resp
        .get("entity_id")
        .or_else(|| resp.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !memory_id.is_empty() {
        let _ = http_post(
            ctx,
            api_url,
            tenant,
            eid,
            &format!("/tdata/Memories('{memory_id}')/TemperPaw.Save"),
            &json!({}),
        );
    }
    Ok(json!({ "saved": true, "key": key, "memory_type": memory_type }))
}

// ---------------------------------------------------------------------------
// recall_memory
// ---------------------------------------------------------------------------

pub fn recall_memory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = recall_memory_input(args)?;
    let query = require_str(&input, "query", "recall_memory")?;
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let agent_id = fields
        .get("agent_id")
        .or_else(|| fields.get("AgentId"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let eid = ctx_entity_id(ctx);
    let resp = http_get(ctx, api_url, tenant, eid, "/tdata/Memories")?;
    let memories = resp
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|mem| {
            entity_field_str_val(mem, "Status") == "Active"
                && entity_field_str_val(mem, "AgentId") == agent_id
                && (entity_field_str_val(mem, "Key").contains(query)
                    || entity_field_str_val(mem, "Content").contains(query))
        })
        .collect::<Vec<_>>();

    Ok(json!({ "memories": memories }))
}

// ---------------------------------------------------------------------------
// write — temper.write(path, content, opts?)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum WriteUploadContent {
    Text(String),
    BrowserImageBytes(Vec<u8>),
    SandboxImageSource(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WriteUpload {
    content: WriteUploadContent,
    mime_type: String,
}

pub fn write(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    write_with_sandbox(ctx, api_url, tenant, "", args)
}

pub fn write_with_sandbox(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = write_input(args)?;
    let upload = write_upload_from_value(&input.path, &input.content, &input.opts)?;
    let upload = resolve_sandbox_upload(ctx, sandbox_url, upload)?;
    let mime_type = upload.mime_type.clone();

    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &input.opts, true)?;
    let eid = ctx_entity_id(ctx);
    let file = resolve_pawfs_file_for_write(
        ctx,
        api_url,
        tenant,
        eid,
        &ws_id,
        &input.path,
        &input.opts,
        &mime_type,
    )?;

    let url = format!("{api_url}/tdata/Files('{}')/$value", file.id);
    let headers = vec![
        ("X-Tenant-Id".to_string(), tenant.to_string()),
        ("Content-Type".to_string(), mime_type.to_string()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), eid.to_string()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];
    match upload.content {
        WriteUploadContent::Text(content) => {
            let resp = ctx.http_call("PUT", &url, &headers, &content)?;
            if resp.status >= 400 {
                return Err(format!(
                    "temper.write(): content upload failed (HTTP {})",
                    resp.status
                ));
            }
        }
        WriteUploadContent::BrowserImageBytes(bytes) => {
            put_file_value_stream(&url, &headers, &bytes)
                .map_err(|error| format!("temper.write(): image content upload failed: {error}"))?;
        }
        WriteUploadContent::SandboxImageSource(_) => {
            return Err("temper.write(): sandbox image source was not resolved".to_string());
        }
    }

    // Confirm the file actually reached Ready (bytes durably stored) before
    // returning its id. The $value PUT dispatches StreamUpdated server-side; a
    // file still in Created here means the bytes did not persist and would
    // surface later as a 404 on $value. Fail loudly so the caller retries
    // instead of attaching a phantom file. (ARN-55 / RFC-0001 Stage B.)
    verify_file_ready(ctx, api_url, tenant, eid, &file.id)?;

    Ok(json!({
        "file_id": file.id,
        "path": file.path,
        "workspace_id": ws_id,
    }))
}

/// True when a Files entity is in a state that guarantees its blob is stored.
/// A File reaches Ready/Locked only after StreamUpdated, which the kernel
/// dispatches once the bytes are durably persisted (temper-fs invariant
/// `ReadyFilesHaveContent`). `Created`/`Archived` do not guarantee content.
fn file_status_is_ready(file: &Value) -> bool {
    let status = file
        .get("Status")
        .or_else(|| file.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("");
    matches!(status, "Ready" | "Locked")
}

/// Read a freshly-written Files entity back and confirm it reached Ready, so
/// `temper.write` never returns a file_id whose blob is missing.
fn verify_file_ready(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    file_id: &str,
) -> Result<(), String> {
    let file = http_get(
        ctx,
        api_url,
        tenant,
        principal_id,
        &format!("/tdata/Files('{file_id}')"),
    )?;
    if !file_status_is_ready(&file) {
        let status = file
            .get("Status")
            .or_else(|| file.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(format!(
            "temper.write(): file '{file_id}' did not reach Ready after upload (Status='{status}'); the bytes did not persist. Retry the write."
        ));
    }
    Ok(())
}

pub fn write_many(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    write_many_with_sandbox(ctx, api_url, tenant, "", args)
}

pub fn write_many_with_sandbox(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    args: &[Value],
) -> Result<Value, String> {
    let input = write_many_input(args)?;
    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &input.opts, true)?;
    let eid = ctx_entity_id(ctx);

    let manifest_files = input
        .files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "mime_type": file.opts.get("mime_type").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect::<Vec<_>>();
    let files_manifest = serde_json::to_string(&manifest_files)
        .map_err(|error| format!("temper.write_many(): manifest serialization failed: {error}"))?;

    let batch_resp = http_post(
        ctx,
        api_url,
        tenant,
        eid,
        "/tdata/ArtifactBatches",
        &json!({
            "WorkspaceId": ws_id,
            "FilesManifest": files_manifest,
            "FileCount": input.files.len(),
        }),
    )?;
    let batch_id = pawfs_entity_id(&batch_resp)
        .ok_or_else(|| "temper.write_many(): ArtifactBatch created but no Id returned".to_string())?
        .to_string();

    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.Submit"),
        &json!({
            "workspace_id": ws_id,
            "files_manifest": files_manifest,
            "submitted_by": eid,
            "file_count": input.files.len(),
        }),
    )?;
    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.Apply?await_integration=true"),
        &json!({}),
    )?;

    let mut results = Vec::with_capacity(input.files.len());
    let mut total_bytes: usize = 0;
    for file_input in input.files {
        match upload_write_many_file(ctx, api_url, tenant, eid, sandbox_url, &ws_id, file_input) {
            Ok(applied) => {
                total_bytes = total_bytes.saturating_add(applied.size_bytes);
                http_post(
                    ctx,
                    api_url,
                    tenant,
                    eid,
                    &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.RecordFileApplied"),
                    &json!({
                        "path": applied.path,
                        "file_id": applied.file_id,
                        "size_bytes": applied.size_bytes,
                    }),
                )?;
                results.push(json!({
                    "path": applied.path,
                    "file_id": applied.file_id,
                    "size_bytes": applied.size_bytes,
                }));
            }
            Err(error) => {
                let _ = http_post(
                    ctx,
                    api_url,
                    tenant,
                    eid,
                    &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.Fail"),
                    &json!({"error": error.clone()}),
                );
                return Err(error);
            }
        }
    }

    let bucket_key = format!("artifact_batch:{batch_id}");
    let bucket_resp = http_post(
        ctx,
        api_url,
        tenant,
        eid,
        "/tdata/WorkspaceUsageBuckets",
        &json!({
            "WorkspaceId": ws_id,
            "BucketKey": bucket_key,
            "ArtifactBatchId": batch_id,
        }),
    )?;
    let usage_bucket_id = pawfs_entity_id(&bucket_resp)
        .ok_or_else(|| {
            "temper.write_many(): WorkspaceUsageBucket created but no Id returned".to_string()
        })?
        .to_string();
    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/WorkspaceUsageBuckets('{usage_bucket_id}')/Temper.ApplyDelta"),
        &json!({
            "workspace_id": ws_id,
            "bucket_key": bucket_key,
            "artifact_batch_id": batch_id,
            "bytes_delta": total_bytes,
            "file_delta": results.len(),
        }),
    )?;
    http_post(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.Complete"),
        &json!({"usage_bucket_id": usage_bucket_id}),
    )?;

    Ok(json!({
        "batch_id": batch_id,
        "workspace_id": ws_id,
        "usage_bucket_id": usage_bucket_id,
        "files": results,
    }))
}

#[derive(Debug, Clone)]
struct WriteManyInput {
    files: Vec<WriteInput>,
    opts: Value,
}

#[derive(Debug, Clone)]
struct AppliedWriteManyFile {
    path: String,
    file_id: String,
    size_bytes: usize,
}

fn write_many_input(args: &[Value]) -> Result<WriteManyInput, String> {
    if let Some(input) = args.first().filter(|value| value.is_object()) {
        let files = input
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| "temper.write_many(): missing 'files' array".to_string())?;
        let opts = input
            .get("opts")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut parsed = Vec::with_capacity(files.len());
        for file in files {
            let mut write_input = write_input(&[file.clone()])?;
            write_input.opts = merge_write_many_opts(&opts, &write_input.opts);
            parsed.push(write_input);
        }
        return Ok(WriteManyInput {
            files: parsed,
            opts,
        });
    }

    let files = args
        .first()
        .and_then(Value::as_array)
        .ok_or_else(|| "temper.write_many(): first argument must be a files array".to_string())?;
    let opts = obj_arg_or_empty(args, 1);
    let mut parsed = Vec::with_capacity(files.len());
    for file in files {
        let mut write_input = write_input(&[file.clone()])?;
        write_input.opts = merge_write_many_opts(&opts, &write_input.opts);
        parsed.push(write_input);
    }
    Ok(WriteManyInput {
        files: parsed,
        opts,
    })
}

fn merge_write_many_opts(batch_opts: &Value, file_opts: &Value) -> Value {
    let mut merged = batch_opts.as_object().cloned().unwrap_or_default();
    if let Some(file_map) = file_opts.as_object() {
        for (key, value) in file_map {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

fn upload_write_many_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    sandbox_url: &str,
    ws_id: &str,
    input: WriteInput,
) -> Result<AppliedWriteManyFile, String> {
    let upload = write_upload_from_value(&input.path, &input.content, &input.opts)?;
    let upload = resolve_sandbox_upload(ctx, sandbox_url, upload)?;
    let mime_type = upload.mime_type.clone();
    let file = resolve_pawfs_file_for_write(
        ctx,
        api_url,
        tenant,
        principal_id,
        ws_id,
        &input.path,
        &input.opts,
        &mime_type,
    )?;

    let url = format!("{api_url}/tdata/Files('{}')/$value", file.id);
    let headers = vec![
        ("X-Tenant-Id".to_string(), tenant.to_string()),
        ("Content-Type".to_string(), mime_type),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        (
            "x-temper-principal-id".to_string(),
            principal_id.to_string(),
        ),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];
    let size_bytes = match upload.content {
        WriteUploadContent::Text(content) => {
            let size_bytes = content.len();
            let resp = ctx.http_call("PUT", &url, &headers, &content)?;
            if resp.status >= 400 {
                return Err(format!(
                    "temper.write_many(): content upload failed for '{}' (HTTP {})",
                    input.path, resp.status
                ));
            }
            size_bytes
        }
        WriteUploadContent::BrowserImageBytes(bytes) => {
            let size_bytes = bytes.len();
            put_file_value_stream(&url, &headers, &bytes).map_err(|error| {
                format!(
                    "temper.write_many(): image content upload failed for '{}': {error}",
                    input.path
                )
            })?;
            size_bytes
        }
        WriteUploadContent::SandboxImageSource(_) => {
            return Err("temper.write_many(): sandbox image source was not resolved".to_string());
        }
    };

    Ok(AppliedWriteManyFile {
        path: file.path,
        file_id: file.id,
        size_bytes,
    })
}

#[derive(Debug, Clone)]
struct WriteInput {
    path: String,
    content: Value,
    opts: Value,
}

fn write_input(args: &[Value]) -> Result<WriteInput, String> {
    if let Some(input) = args.first().filter(|value| value.is_object()) {
        let path = require_str(input, "path", "write")?.to_string();
        let content = input
            .get("content")
            .or_else(|| input.get("body"))
            .cloned()
            .or_else(|| {
                if is_sandbox_image_marker(input) {
                    Some(input.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| "temper.write(): missing 'content'".to_string())?;
        let opts = write_opts_from_object_input(input);
        return Ok(WriteInput {
            path,
            content,
            opts,
        });
    }

    let path = pos_str(args, 0, "path", "write")?;
    let content = args
        .get(1)
        .ok_or_else(|| "temper.write(): missing 'content' at position 1".to_string())?
        .clone();
    let opts = obj_arg_or_empty(args, 2);
    Ok(WriteInput {
        path,
        content,
        opts,
    })
}

fn write_opts_from_object_input(input: &Value) -> Value {
    let mut opts = input
        .get("opts")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));

    let Some(map) = opts.as_object_mut() else {
        return opts;
    };

    for key in [
        "mime_type",
        "workspace_id",
        "workspace",
        "file_id",
        "entity_id",
        "id",
    ] {
        if !map.contains_key(key)
            && let Some(value) = input.get(key)
        {
            map.insert(key.to_string(), value.clone());
        }
    }

    opts
}

fn resolve_sandbox_upload(
    ctx: &Context,
    sandbox_url: &str,
    upload: WriteUpload,
) -> Result<WriteUpload, String> {
    let WriteUpload { content, mime_type } = upload;
    let WriteUploadContent::SandboxImageSource(source_path) = content else {
        return Ok(WriteUpload { content, mime_type });
    };

    let handle = sandbox_handle_for_write(ctx, sandbox_url)?;
    let bytes = read_sandbox_image_bytes(ctx, &handle, &source_path, &mime_type)?;
    Ok(WriteUpload {
        content: WriteUploadContent::BrowserImageBytes(bytes),
        mime_type,
    })
}

fn sandbox_handle_for_write(
    ctx: &Context,
    sandbox_url: &str,
) -> Result<wasm_helpers::sandbox::SandboxHandle, String> {
    use wasm_helpers::sandbox;

    let resolved_sandbox_url = if sandbox_url.is_empty() {
        dispatch::peek_lazy_sandbox_url().unwrap_or_default()
    } else {
        sandbox_url.to_string()
    };

    if resolved_sandbox_url.is_empty() {
        return Err(
            "temper.write(): sandbox image source requires an attached sandbox".to_string(),
        );
    }

    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let provider = fields
        .get("sandbox_provider")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(dispatch::peek_lazy_sandbox_provider)
        .unwrap_or_else(|| {
            sandbox::resolve_sandbox_provider(ctx, &fields)
                .unwrap_or_else(|_| "tensorlake".to_string())
        });
    let sandbox_id = fields
        .get("sandbox_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(dispatch::peek_lazy_sandbox_id)
        .unwrap_or_default();

    Ok(sandbox::SandboxHandle {
        sandbox_url: resolved_sandbox_url,
        sandbox_id,
        provider,
    })
}

fn read_sandbox_image_bytes(
    ctx: &Context,
    handle: &wasm_helpers::sandbox::SandboxHandle,
    source_path: &str,
    expected_mime: &str,
) -> Result<Vec<u8>, String> {
    let command = format!(
        "base64 -w0 {} 2>/dev/null || base64 {}",
        shell_quote(source_path),
        shell_quote(source_path)
    );
    let result = wasm_helpers::sandbox::sandbox_exec(ctx, handle, &command, "/")?;
    if result.exit_code != 0 {
        return Err(format!(
            "temper.write(): failed to read sandbox image source '{source_path}' (exit {}): {}",
            result.exit_code, result.stderr
        ));
    }

    let compact: String = result
        .stdout
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Err(format!(
            "temper.write(): sandbox image source '{source_path}' is empty"
        ));
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|error| {
            format!(
                "temper.write(): sandbox image source '{source_path}' is not valid base64: {error}"
            )
        })?;
    let detected_mime = detect_browser_image_mime(&decoded).ok_or_else(|| {
        format!(
            "temper.write(): sandbox image source '{source_path}' is not a supported browser image"
        )
    })?;
    let normalized_expected = normalize_image_mime(expected_mime)
        .ok_or_else(|| format!("temper.write(): unsupported image MIME type '{expected_mime}'"))?;
    if detected_mime != normalized_expected {
        return Err(format!(
            "temper.write(): declared MIME type '{normalized_expected}' does not match sandbox image bytes '{detected_mime}'"
        ));
    }

    Ok(decoded)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_arch = "wasm32")]
fn put_file_value_stream(
    url: &str,
    headers: &[(String, String)],
    bytes: &[u8],
) -> Result<(), String> {
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (mut request_body, response_body, response_head) =
        temper_wasm_sdk::http_stream::streaming_call("PUT", url, &header_refs)
            .map_err(|error| format!("streaming PUT $value failed to start: {error}"))?;

    for chunk in bytes.chunks(FILE_UPLOAD_STREAM_CHUNK_BYTES) {
        request_body
            .write_all_chunk(chunk)
            .map_err(|error| format!("streaming PUT $value failed while writing body: {error}"))?;
    }
    request_body
        .finish()
        .map_err(|error| format!("streaming PUT $value failed while closing body: {error}"))?;

    let head = response_head()
        .map_err(|error| format!("streaming PUT $value failed before response: {error}"))?;
    let _ = response_body.close();
    if head.status >= 400 || head.status == 0 {
        let stream_error = head
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("x-temper-stream-error"))
            .map(|(_, value)| format!(": {value}"))
            .unwrap_or_default();
        return Err(format!("HTTP {}{stream_error}", head.status));
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn put_file_value_stream(
    _url: &str,
    _headers: &[(String, String)],
    _bytes: &[u8],
) -> Result<(), String> {
    Err("streaming file uploads require the Temper WASM host".to_string())
}

// ---------------------------------------------------------------------------
// read — temper.read(path, opts?)
// ---------------------------------------------------------------------------

pub fn read(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let path = pos_str(args, 0, "path", "read")?;
    let opts = obj_arg_or_empty(args, 1);

    if let Some(file_id) = pawfs_file_id_from_path_or_opts(&path, &opts) {
        let content = read_pawfs_file_value(ctx, api_url, &file_id, "temper.read()")?;
        return Ok(render_read_output(content, &opts));
    }

    if let Some(content) = try_read_global_scoped_path(ctx, api_url, tenant, &path, &opts)? {
        return Ok(render_read_output(content, &opts));
    }

    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &opts, false)?;
    let file = find_pawfs_file(ctx, api_url, tenant, &ws_id, &path)?
        .ok_or_else(|| format!("temper.read(): file not found at '{path}'"))?;
    let content = read_pawfs_file_value(ctx, api_url, &file.id, "temper.read()")?;

    Ok(render_read_output(content, &opts))
}

// ---------------------------------------------------------------------------
// ls — temper.ls(path, opts?)
// ---------------------------------------------------------------------------

pub fn ls(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let path = pos_str(args, 0, "path", "ls")?;
    let opts = obj_arg_or_empty(args, 1);
    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &opts, false)?;
    let listing = list_pawfs_directory(ctx, api_url, tenant, &ws_id, &path)?;
    Ok(listing.into_json())
}

// ---------------------------------------------------------------------------
// edit — temper.edit(path, old_string, new_string, opts?)
// ---------------------------------------------------------------------------

pub fn edit(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let path = pos_str(args, 0, "path", "edit")?;
    let old_string = pos_str(args, 1, "old_string", "edit")?;
    let new_string = pos_str(args, 2, "new_string", "edit")?;
    let opts = obj_arg_or_empty(args, 3);

    // Read current content
    let read_args = vec![json!(path.clone()), opts.clone()];
    let content_val = read(ctx, api_url, tenant, &read_args)?;
    let content = content_val.as_str().unwrap_or("").to_string();

    if !content.contains(&old_string) {
        return Err(format!(
            "temper.edit(): old_string not found in file at '{path}'"
        ));
    }

    let new_content = content.replacen(&old_string, &new_string, 1);

    // Write back
    let write_args = vec![json!(path), json!(new_content), opts];
    write(ctx, api_url, tenant, &write_args)?;

    Ok(json!({"ok": true}))
}

// ---------------------------------------------------------------------------
// rename — temper.rename(old_path, new_path, opts?)
// ---------------------------------------------------------------------------

pub fn rename(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let old_path = pos_str(args, 0, "old_path", "rename")?;
    let new_path = pos_str(args, 1, "new_path", "rename")?;
    let opts = obj_arg_or_empty(args, 2);
    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &opts, false)?;
    let eid = ctx_entity_id(ctx);
    let file = rename_pawfs_file(ctx, api_url, tenant, eid, &ws_id, &old_path, &new_path)?;

    Ok(json!({
        "file_id": file.id,
        "old_path": old_path,
        "new_path": file.path,
    }))
}

// ---------------------------------------------------------------------------
// search_history — temper.search_history(pattern)
// ---------------------------------------------------------------------------

pub fn search_history(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let pattern = pos_str(args, 0, "pattern", "search_history")?;

    // Get session_file_id from entity state
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let session_file_id = fields
        .get("session_file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if session_file_id.is_empty() {
        return Err("search_history: no session_file_id in entity state".into());
    }

    let headers = runtime_headers(ctx, tenant, &fields, None, Some("application/json"));
    let session_jsonl = read_session_from_temperfs(ctx, api_url, tenant, &fields, session_file_id)?;

    let tree = session_tree_lib::SessionTree::from_jsonl(&session_jsonl);
    let pattern_lower = pattern.to_lowercase();
    let mut matches: Vec<Value> = Vec::new();
    let mut content_fetches = 0;
    const MAX_CONTENT_FETCHES: usize = 20;
    const MAX_RESULTS: usize = 50;

    for entry_id in tree.entry_ids() {
        if matches.len() >= MAX_RESULTS {
            break;
        }

        let entry = match tree.get(entry_id) {
            Some(e) => e,
            None => continue,
        };

        // Extract role from entry data
        let role = entry
            .data
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let entry_type_str = entry.entry_type.as_str();

        // Try inline content first
        let inline_content = extract_entry_text(&entry.data);
        let inline_lower = inline_content.to_lowercase();

        if inline_lower.contains(&pattern_lower) {
            // Find matching excerpt
            let excerpt = extract_excerpt(&inline_content, &pattern, 500);
            matches.push(json!({
                "entry_id": entry_id,
                "role": role,
                "entry_type": entry_type_str,
                "excerpt": excerpt,
                "source": "inline",
            }));
            continue;
        }

        // If entry has immutable content version and no inline match, fetch that.
        if let Some(ref content_file_version_id) = entry.content_file_version_id {
            if content_fetches >= MAX_CONTENT_FETCHES {
                continue;
            }
            content_fetches += 1;

            match read_content_file_version(
                ctx,
                api_url,
                tenant,
                &json!({}),
                content_file_version_id,
            ) {
                Ok(raw) => {
                    let raw_lower = raw.to_lowercase();
                    if raw_lower.contains(&pattern_lower) {
                        let excerpt = extract_excerpt(&raw, &pattern, 500);
                        matches.push(json!({
                            "entry_id": entry_id,
                            "role": role,
                            "entry_type": entry_type_str,
                            "excerpt": excerpt,
                            "source": "content_file_version",
                            "content_file_version_id": content_file_version_id,
                        }));
                        continue;
                    }
                }
                Err(_) => {}
            }
        }

        // Fall back to current file head for older session entries.
        if let Some(ref content_file_id) = entry.content_file_id {
            if content_fetches >= MAX_CONTENT_FETCHES {
                continue;
            }
            content_fetches += 1;

            let file_url = format!("{api_url}/tdata/Files('{content_file_id}')/$value");
            let file_resp = ctx.http_call("GET", &file_url, &headers, "");
            if let Ok(file_resp) = file_resp {
                if file_resp.status < 400 {
                    let file_lower = file_resp.body.to_lowercase();
                    if file_lower.contains(&pattern_lower) {
                        let excerpt = extract_excerpt(&file_resp.body, &pattern, 500);
                        matches.push(json!({
                            "entry_id": entry_id,
                            "role": role,
                            "entry_type": entry_type_str,
                            "excerpt": excerpt,
                            "source": "content_file",
                            "content_file_id": content_file_id,
                        }));
                    }
                }
            }
        }
    }

    Ok(json!({
        "matches": matches,
        "total": matches.len(),
        "entries_searched": tree.entry_ids().len(),
        "content_files_fetched": content_fetches,
    }))
}

/// Extract text content from a session entry's data field.
fn extract_entry_text(data: &Value) -> String {
    // Try "content" as string
    if let Some(s) = data.get("content").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    // Try "content" as array of content blocks (Anthropic format)
    if let Some(arr) = data.get("content").and_then(|v| v.as_array()) {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|block| {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    Some(text.to_string())
                } else {
                    None
                }
            })
            .collect();
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    // Try "summary" field (compaction entries)
    if let Some(s) = data.get("summary").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    String::new()
}

/// Extract a truncated excerpt around the first match of pattern in text.
fn extract_excerpt(text: &str, pattern: &str, max_len: usize) -> String {
    let text_lower = text.to_lowercase();
    let pattern_lower = pattern.to_lowercase();
    if let Some(pos) = text_lower.find(&pattern_lower) {
        let start = pos.saturating_sub(max_len / 4);
        let end = (pos + pattern.len() + max_len * 3 / 4).min(text.len());
        // Align to char boundaries
        let start = text[..start]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end = text[..end]
            .char_indices()
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(text.len())
            .min(text.len());
        let excerpt = &text[start..end];
        if start > 0 || end < text.len() {
            let mut result = String::new();
            if start > 0 {
                result.push_str("...");
            }
            result.push_str(excerpt);
            if end < text.len() {
                result.push_str("...");
            }
            result
        } else {
            excerpt.to_string()
        }
    } else {
        text[..text.len().min(max_len)].to_string()
    }
}

// ---------------------------------------------------------------------------
// grep — temper.grep(pattern, path, opts?)
// ---------------------------------------------------------------------------

pub fn grep(ctx: &Context, api_url: &str, tenant: &str, args: &[Value]) -> Result<Value, String> {
    let pattern = pos_str(args, 0, "pattern", "grep")?;
    let path = pos_str(args, 1, "path", "grep")?;
    let opts = obj_arg_or_empty(args, 2);
    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &opts, false)?;
    let case_insensitive = opts
        .get("case_insensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_results = opts
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;

    let search_pattern = if case_insensitive {
        pattern.to_lowercase()
    } else {
        pattern.clone()
    };

    // Try resolving as a single file first
    let file_paths = resolve_grep_targets(ctx, api_url, tenant, &ws_id, &path)?;

    let mut matches: Vec<Value> = Vec::new();

    for file_path in file_paths {
        if matches.len() >= max_results {
            break;
        }

        // Read file content
        let read_args = vec![json!(file_path), json!({"workspace_id": ws_id})];
        let content_val = match read(ctx, api_url, tenant, &read_args) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let content = content_val.as_str().unwrap_or("");

        for (line_num, line) in content.lines().enumerate() {
            if matches.len() >= max_results {
                break;
            }
            let haystack = if case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if haystack.contains(&search_pattern) {
                matches.push(json!({
                    "file": file_path,
                    "line_number": line_num + 1,
                    "line": line,
                }));
            }
        }
    }

    Ok(json!({ "matches": matches, "total": matches.len() }))
}

/// Resolve grep targets: if path is a file return vec![path], if dir return all files recursively.
fn resolve_grep_targets(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    path: &str,
) -> Result<Vec<String>, String> {
    // Try to resolve as a file first
    if let Some(file) = find_pawfs_file(ctx, api_url, tenant, ws_id, path)? {
        return Ok(vec![file.path]);
    }

    // It's a directory — list recursively
    let mut files = Vec::new();
    list_dir_recursive(ctx, api_url, tenant, ws_id, path, 0, 5, &mut files, 500)?;
    Ok(files)
}

// ---------------------------------------------------------------------------
// glob — temper.glob(pattern, path?)
// ---------------------------------------------------------------------------

pub fn glob_files(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    args: &[Value],
) -> Result<Value, String> {
    let pattern = pos_str(args, 0, "pattern", "glob")?;
    let path = args
        .get(1)
        .and_then(|v| v.as_str())
        .unwrap_or("/")
        .to_string();
    let opts = obj_arg_or_empty(args, 2);
    let ws_id = resolve_workspace_id(ctx, api_url, tenant, &opts, false)?;

    let mut all_files = Vec::new();
    list_dir_recursive(
        ctx,
        api_url,
        tenant,
        &ws_id,
        &path,
        0,
        5,
        &mut all_files,
        500,
    )?;

    let matches: Vec<&String> = all_files
        .iter()
        .filter(|f| glob_match(&pattern, f))
        .collect();

    Ok(json!(matches))
}

// ---------------------------------------------------------------------------
// list_dir_recursive — shared helper for grep/glob
// ---------------------------------------------------------------------------

fn list_dir_recursive(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    dir_path: &str,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<String>,
    max_files: usize,
) -> Result<(), String> {
    if depth >= max_depth || out.len() >= max_files {
        return Ok(());
    }

    let listing = list_pawfs_directory(ctx, api_url, tenant, ws_id, dir_path)?;

    for file in &listing.files {
        if out.len() >= max_files {
            break;
        }
        out.push(file.path.clone());
    }

    for directory in &listing.directories {
        if out.len() >= max_files {
            break;
        }
        list_dir_recursive(
            ctx,
            api_url,
            tenant,
            ws_id,
            &directory.path,
            depth + 1,
            max_depth,
            out,
            max_files,
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// glob_match — simple glob pattern matcher (no regex crate)
// ---------------------------------------------------------------------------

fn glob_match(pattern: &str, path: &str) -> bool {
    // Handle ** (match any path segments)
    if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let prefix = parts[0].trim_end_matches('/');
            let suffix = parts[1].trim_start_matches('/');
            // Path must start with prefix (or prefix is empty)
            let path_match = if prefix.is_empty() {
                true
            } else {
                path.starts_with(prefix) || path.starts_with(&format!("{prefix}/"))
            };
            if !path_match {
                return false;
            }
            // Suffix must match the tail of the path
            if suffix.is_empty() {
                return true;
            }
            // Match suffix as a simple glob against each possible tail
            let check_path = if prefix.is_empty() {
                path
            } else {
                &path[prefix.len()..]
            };
            for (i, _) in check_path.char_indices() {
                let tail = &check_path[i..];
                if tail.starts_with('/') || i == 0 {
                    let tail = tail.trim_start_matches('/');
                    if simple_glob_match(suffix, tail) {
                        return true;
                    }
                }
            }
            return false;
        }
    }
    simple_glob_match(pattern, path)
}

/// Match a simple glob pattern (supports * and ?) against a string.
/// * matches any characters except /. ? matches exactly one character except /.
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    simple_glob_match_recursive(&pat, 0, &txt, 0)
}

fn simple_glob_match_recursive(pattern: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    if pi == pattern.len() && ti == text.len() {
        return true;
    }
    if pi == pattern.len() {
        return false;
    }

    match pattern[pi] {
        '*' => {
            // * matches zero or more characters (not /)
            // Try matching zero characters, then one, then two, etc.
            let mut t = ti;
            loop {
                if simple_glob_match_recursive(pattern, pi + 1, text, t) {
                    return true;
                }
                if t >= text.len() || text[t] == '/' {
                    return false;
                }
                t += 1;
            }
        }
        '?' => {
            if ti < text.len() && text[ti] != '/' {
                simple_glob_match_recursive(pattern, pi + 1, text, ti + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && text[ti] == c {
                simple_glob_match_recursive(pattern, pi + 1, text, ti + 1)
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// run_coding_agent
// ---------------------------------------------------------------------------

pub fn run_coding_agent(
    ctx: &Context,
    _api_url: &str,
    _tenant: &str,
    sandbox_url: &str,
    workdir: &str,
    args: &[Value],
) -> Result<Value, String> {
    use wasm_helpers::sandbox::{self, SandboxHandle};

    let input = obj_arg(args, 0, "opts", "run_coding_agent")?;
    let agent_type = require_str(&input, "agent_type", "run_coding_agent")?;
    let task = require_str(&input, "task", "run_coding_agent")?;
    let agent_workdir = input
        .get("workdir")
        .and_then(|v| v.as_str())
        .unwrap_or(workdir);
    let background = input
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if sandbox_url.is_empty() {
        return Err("run_coding_agent: sandbox_url is empty".into());
    }

    // Build sandbox handle from entity state
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let provider = fields
        .get("sandbox_provider")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| dispatch::peek_lazy_sandbox_provider())
        .unwrap_or_else(|| {
            sandbox::resolve_sandbox_provider(ctx, &fields)
                .unwrap_or_else(|_| "tensorlake".to_string())
        });
    let sandbox_id = fields
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| dispatch::peek_lazy_sandbox_id())
        .unwrap_or_default();
    let handle = SandboxHandle {
        sandbox_url: sandbox_url.to_string(),
        sandbox_id,
        provider,
    };

    let escaped_task = task.replace('\'', "'\\''");
    let command = match agent_type {
        "claude-code" => format!(
            "cd {agent_workdir} && claude --permission-mode bypassPermissions --print '{escaped_task}'"
        ),
        "codex" => format!("cd {agent_workdir} && codex exec '{escaped_task}'"),
        "pi" => format!("cd {agent_workdir} && pi -p '{escaped_task}'"),
        "opencode" => format!("cd {agent_workdir} && opencode run '{escaped_task}'"),
        _ => return Err(format!("unsupported coding agent type: {agent_type}")),
    };

    let final_cmd = if background {
        format!("nohup bash -c '{command}' > /tmp/coding-agent-{agent_type}.log 2>&1 & echo $!")
    } else {
        command.clone()
    };

    // Execute via sandbox provider abstraction
    let result = sandbox::sandbox_exec(ctx, &handle, &final_cmd, agent_workdir)?;

    if background {
        return Ok(json!({
            "agent_type": agent_type,
            "status": "started_background",
            "command": final_cmd,
        }));
    }

    let mut output = result.stdout;
    if !result.stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&result.stderr);
    }

    Ok(json!({
        "agent_type": agent_type,
        "output": output,
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_str<'a>(input: &'a Value, key: &str, method: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("temper.{method}(): missing '{key}'"))
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

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn spawn_session_input(args: &[Value]) -> Result<Value, String> {
    if let Ok(input) = obj_arg(args, 0, "opts", "spawn_session") {
        return Ok(input);
    }

    let task = pos_str(args, 0, "task", "spawn_session")?;
    let mut input = json!({ "task": task });

    if let Some(soul_id) = args.get(1).and_then(value_as_string) {
        input["soul_id"] = json!(soul_id);
    }
    if let Some(model) = args.get(2).and_then(value_as_string) {
        input["model"] = json!(model);
    }
    if let Some(tools) = args.get(3).and_then(value_as_string) {
        input["tools"] = json!(tools);
    }
    if let Some(legacy_workdir) = args.get(4).and_then(value_as_string) {
        input["workdir"] = json!(legacy_workdir);
    }
    if let Some(legacy_sandbox_url) = args.get(5).and_then(value_as_string) {
        input["sandbox_url"] = json!(legacy_sandbox_url);
    }
    if let Some(max_turns) = args.get(6).and_then(value_as_string) {
        input["max_turns"] = json!(max_turns);
    }
    if let Some(background) = args.get(7).and_then(|value| value.as_bool()) {
        input["background"] = json!(background);
    }

    Ok(input)
}

fn resolve_spawn_model_for_provider(
    provider: &str,
    requested_model: &str,
    parent_model: &str,
    default_model: Option<&str>,
) -> Result<String, String> {
    let requested = requested_model.trim();
    if normalize_provider_name(provider) != "openai_codex"
        || !is_unsupported_chatgpt_codex_model(requested)
    {
        return Ok(requested.to_string());
    }

    for fallback in [parent_model, default_model.unwrap_or("")] {
        let candidate = fallback.trim();
        if !candidate.is_empty() && !is_unsupported_chatgpt_codex_model(candidate) {
            return Ok(candidate.to_string());
        }
    }

    Err(format!(
        "spawn_session cannot use model `{requested}` with provider=openai_codex; pass a Codex-compatible model such as gpt-5.5"
    ))
}

fn normalize_provider_name(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "open_router" => "openrouter".to_string(),
        "hf" | "hugging_face" | "hugging-face" => "huggingface".to_string(),
        "fireworks_ai" | "fireworks-ai" => "fireworks".to_string(),
        "sakana" | "sakana-fugu" | "fugu" => "sakana_fugu".to_string(),
        "ollama" | "local" | "local-openai" => "local_openai".to_string(),
        "openai-compatible" | "openai_compat" | "openai-compat" | "custom_openai" => {
            "openai_compatible".to_string()
        }
        other => other.to_string(),
    }
}

fn is_unsupported_chatgpt_codex_model(model: &str) -> bool {
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "gpt-4.1" | "gpt-4.1-mini" | "gpt-4.1-nano"
    )
}

fn list_sessions_input(args: &[Value]) -> Value {
    let input = obj_arg_or_empty(args, 0);
    if input.is_object() && !input.as_object().is_none_or(|obj| obj.is_empty()) {
        return input;
    }

    let mut legacy = json!({});
    if let Some(filter) = args.get(0).and_then(value_as_string) {
        legacy["filter"] = json!(filter);
    }
    if let Some(top) = args.get(1).and_then(value_as_i64) {
        legacy["top"] = json!(top);
    }
    legacy
}

fn abort_session_input(args: &[Value]) -> Result<Value, String> {
    if let Ok(input) = obj_arg(args, 0, "opts", "abort_session") {
        return Ok(input);
    }

    Ok(json!({
        "session_id": pos_str(args, 0, "session_id", "abort_session")?
    }))
}

fn steer_session_input(args: &[Value]) -> Result<Value, String> {
    if let Ok(input) = obj_arg(args, 0, "opts", "steer_session") {
        return Ok(input);
    }

    Ok(json!({
        "session_id": pos_str(args, 0, "session_id", "steer_session")?,
        "message": pos_str(args, 1, "message", "steer_session")?
    }))
}

fn save_memory_input(args: &[Value]) -> Result<Value, String> {
    if let Ok(input) = obj_arg(args, 0, "opts", "save_memory") {
        return Ok(input);
    }

    let mut input = json!({
        "key": pos_str(args, 0, "key", "save_memory")?,
        "content": pos_str(args, 1, "content", "save_memory")?,
    });
    if let Some(memory_type) = args.get(2).and_then(value_as_string) {
        input["memory_type"] = json!(memory_type);
    }
    Ok(input)
}

fn recall_memory_input(args: &[Value]) -> Result<Value, String> {
    if let Ok(input) = obj_arg(args, 0, "opts", "recall_memory") {
        return Ok(input);
    }

    Ok(json!({
        "query": pos_str(args, 0, "query", "recall_memory")?
    }))
}

fn ctx_entity_id(ctx: &Context) -> &str {
    ctx.entity_state
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn entity_field_str_val(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn entity_field_str_any<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(found) = value.get(*key).and_then(|v| v.as_str()) {
            return Some(found);
        }
        if let Some(found) = value
            .get("fields")
            .and_then(|fields| fields.get(*key))
            .and_then(|v| v.as_str())
        {
            return Some(found);
        }
    }
    None
}

fn is_global_scoped_path(path: &str) -> bool {
    path == "/system"
        || path == "/agents"
        || path == "/projects"
        || path.starts_with("/system/")
        || path.starts_with("/agents/")
        || path.starts_with("/projects/")
}

fn escape_odata_string(value: &str) -> String {
    bounded_reads::odata_escape(value)
}

fn global_scoped_default_workspace_id() -> &'static str {
    "os-app-docs"
}

fn global_scoped_workspace_id(opts: &Value) -> String {
    opts.get("workspace_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(global_scoped_default_workspace_id())
        .to_string()
}

fn global_scoped_file_filter(path: &str, ws_id: &str) -> String {
    format!(
        "Path eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(path),
        escape_odata_string(ws_id)
    )
}

fn is_pawfs_file_id(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("fl-") && trimmed.len() > 3
}

fn pawfs_file_id_from_opts(opts: &Value) -> Option<String> {
    for key in ["file_id", "entity_id", "id"] {
        if let Some(id) = opts
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| is_pawfs_file_id(value))
        {
            return Some(id.to_string());
        }
    }
    None
}

fn pawfs_file_id_from_path_or_opts(path: &str, opts: &Value) -> Option<String> {
    pawfs_file_id_from_opts(opts).or_else(|| {
        if is_pawfs_file_id(path) {
            Some(path.trim().to_string())
        } else {
            None
        }
    })
}

fn render_read_output(content: String, opts: &Value) -> Value {
    let offset = opts
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let limit = opts
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    if offset.is_some() || limit.is_some() {
        let lines: Vec<&str> = content.lines().collect();
        let start = offset.unwrap_or(0);
        let end = limit
            .map(|l| (start + l).min(lines.len()))
            .unwrap_or(lines.len());
        if start >= lines.len() {
            return json!({
                "content": "",
                "total_lines": lines.len(),
                "offset": start,
                "limit": limit.unwrap_or(0),
            });
        }
        let numbered: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}\t{}", start + i + 1, line))
            .collect();
        return json!({
            "content": numbered.join("\n"),
            "total_lines": lines.len(),
            "offset": start,
            "limit": end - start,
        });
    }

    json!(content)
}

/// Minimal headers for internal Temper API calls.
/// Auth headers (tenant, principal, agent-type, bearer token) are injected
/// by the WASM host for internal calls — see ADR-0043.
fn internal_headers() -> Vec<(String, String)> {
    vec![("Content-Type".to_string(), "application/json".to_string())]
}

fn write_upload_from_value(
    path: &str,
    content: &Value,
    opts: &Value,
) -> Result<WriteUpload, String> {
    let declared_mime = opts
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| mime_from_ext(path))
        .to_string();

    if let Some((source_path, media_type)) = sandbox_image_source_candidate(content) {
        let mime_type = media_type.unwrap_or_else(|| declared_mime.clone());
        let normalized_mime = normalize_image_mime(&mime_type)
            .ok_or_else(|| format!("temper.write(): unsupported image MIME type '{mime_type}'"))?;
        return Ok(WriteUpload {
            content: WriteUploadContent::SandboxImageSource(source_path),
            mime_type: normalized_mime.to_string(),
        });
    }

    if let Some((base64_data, media_type)) = sandbox_image_candidate(content) {
        let mime_type = media_type.unwrap_or_else(|| declared_mime.clone());
        return browser_image_upload(base64_data.trim(), &mime_type);
    }

    let Some(text) = content.as_str() else {
        return Err("temper.write(): content must be a string or sandbox image result".to_string());
    };

    if let Some((base64_data, media_type)) = sandbox_image_json_candidate(text) {
        let mime_type = media_type.unwrap_or_else(|| declared_mime.clone());
        return browser_image_upload(base64_data.trim(), &mime_type);
    }

    if let Some((source_path, media_type)) = sandbox_image_source_json_candidate(text) {
        let mime_type = media_type.unwrap_or_else(|| declared_mime.clone());
        let normalized_mime = normalize_image_mime(&mime_type)
            .ok_or_else(|| format!("temper.write(): unsupported image MIME type '{mime_type}'"))?;
        return Ok(WriteUpload {
            content: WriteUploadContent::SandboxImageSource(source_path),
            mime_type: normalized_mime.to_string(),
        });
    }

    if is_raster_image_mime(&declared_mime) || text.trim_start().starts_with("data:image/") {
        return browser_image_upload(text.trim(), &declared_mime);
    }

    Ok(WriteUpload {
        content: WriteUploadContent::Text(text.to_string()),
        mime_type: declared_mime,
    })
}

fn is_sandbox_image_marker(value: &Value) -> bool {
    value.get("__temperpaw_image").and_then(Value::as_bool) == Some(true)
}

fn sandbox_image_candidate(value: &Value) -> Option<(String, Option<String>)> {
    if !is_sandbox_image_marker(value) {
        return None;
    }
    let base64_data = value.get("base64_data")?.as_str()?.trim();
    if base64_data.is_empty() {
        return None;
    }
    let media_type = value
        .get("media_type")
        .or_else(|| value.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some((base64_data.to_string(), media_type))
}

fn sandbox_image_source_candidate(value: &Value) -> Option<(String, Option<String>)> {
    if !is_sandbox_image_marker(value) {
        return None;
    }
    let source_path = value
        .get("source_path")
        .or_else(|| value.get("sandbox_path"))
        .and_then(Value::as_str)?
        .trim();
    if source_path.is_empty() {
        return None;
    }
    let media_type = value
        .get("media_type")
        .or_else(|| value.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some((source_path.to_string(), media_type))
}

fn sandbox_image_json_candidate(text: &str) -> Option<(String, Option<String>)> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.contains("__temperpaw_image") {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    sandbox_image_candidate(&value)
}

fn sandbox_image_source_json_candidate(text: &str) -> Option<(String, Option<String>)> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.contains("__temperpaw_image") {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    sandbox_image_source_candidate(&value)
}

fn browser_image_upload(raw: &str, declared_mime: &str) -> Result<WriteUpload, String> {
    let normalized_declared = normalize_image_mime(declared_mime)
        .ok_or_else(|| format!("temper.write(): unsupported image MIME type '{declared_mime}'"))?;
    let (base64_text, data_url_mime) = split_data_url_base64(raw)?;
    if let Some(data_url_mime) = data_url_mime {
        let normalized_data_url_mime = normalize_image_mime(data_url_mime).ok_or_else(|| {
            format!("temper.write(): unsupported data URL image MIME type '{data_url_mime}'")
        })?;
        if normalized_data_url_mime != normalized_declared {
            return Err(format!(
                "temper.write(): declared MIME type '{normalized_declared}' does not match data URL MIME type '{normalized_data_url_mime}'"
            ));
        }
    }

    let compact: String = base64_text
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Err("temper.write(): image payload is empty".to_string());
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|error| format!("temper.write(): image payload is not valid base64: {error}"))?;
    let detected_mime = detect_browser_image_mime(&decoded).ok_or_else(|| {
        "temper.write(): decoded payload is not a supported browser image".to_string()
    })?;
    if detected_mime != normalized_declared {
        return Err(format!(
            "temper.write(): declared MIME type '{normalized_declared}' does not match decoded image bytes '{detected_mime}'"
        ));
    }

    Ok(WriteUpload {
        content: WriteUploadContent::BrowserImageBytes(decoded),
        mime_type: detected_mime.to_string(),
    })
}

fn split_data_url_base64(raw: &str) -> Result<(&str, Option<&str>), String> {
    let Some(rest) = raw.strip_prefix("data:") else {
        return Ok((raw, None));
    };
    let Some((metadata, data)) = rest.split_once(',') else {
        return Err("temper.write(): data URL is missing ',' separator".to_string());
    };
    let mut parts = metadata.split(';');
    let mime_type = parts.next().unwrap_or("");
    if !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return Err("temper.write(): data URL image payload must be base64 encoded".to_string());
    }
    Ok((data, Some(mime_type)))
}

fn normalize_image_mime(mime_type: &str) -> Option<&'static str> {
    match mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/svg+xml" => Some("image/svg+xml"),
        _ => None,
    }
}

fn is_raster_image_mime(mime_type: &str) -> bool {
    matches!(
        normalize_image_mime(mime_type),
        Some("image/jpeg" | "image/png" | "image/gif" | "image/webp")
    )
}

fn detect_browser_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }

    let text = std::str::from_utf8(bytes).ok()?.trim_start();
    if text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg")) {
        return Some("image/svg+xml");
    }

    None
}

fn http_get(
    ctx: &Context,
    api_url: &str,
    _tenant: &str,
    _principal_id: &str,
    path: &str,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if let Some(denial) = dispatch::check_cedar_denial(resp.status, &resp.body) {
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
    _principal_id: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if let Some(denial) = dispatch::check_cedar_denial(resp.status, &resp.body) {
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
    _principal_id: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let headers = internal_headers();
    let resp = ctx.http_call("PATCH", &url, &headers, &body.to_string())?;
    if let Some(denial) = dispatch::check_cedar_denial(resp.status, &resp.body) {
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

fn pos_str(args: &[Value], idx: usize, name: &str, method: &str) -> Result<String, String> {
    args.get(idx)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("temper.{method}(): missing '{name}' at position {idx}"))
}

fn find_workspace(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let name_enc = urlenc(name);
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Workspaces?$filter=Name%20eq%20'{name_enc}'"),
    )?;
    let items = resp.get("value").and_then(|v| v.as_array());
    Ok(items
        .and_then(|arr| arr.first())
        .and_then(|v| {
            v.get("entity_id")
                .or_else(|| v.get("Id"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string()))
}

fn session_workspace_id(ctx: &Context) -> Option<String> {
    ctx.entity_state
        .get("fields")
        .and_then(|fields| fields.get("workspace_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn try_read_global_scoped_path(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    path: &str,
    opts: &Value,
) -> Result<Option<String>, String> {
    if !is_global_scoped_path(path) {
        return Ok(None);
    }

    let ws_id = global_scoped_workspace_id(opts);
    let Some(file) = find_pawfs_file(ctx, api_url, tenant, &ws_id, path)? else {
        return Ok(None);
    };
    let content = read_pawfs_file_value(ctx, api_url, &file.id, "temper.read()")?;
    Ok(Some(content))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PawFsDirectory {
    id: String,
    name: String,
    path: String,
    parent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PawFsFile {
    id: String,
    name: String,
    path: String,
    directory_id: String,
    mime_type: String,
    size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PawFsListing {
    path: String,
    directories: Vec<PawFsDirectory>,
    files: Vec<PawFsFile>,
}

impl PawFsListing {
    fn into_json(self) -> Value {
        json!({
            "path": self.path,
            "directories": self.directories.into_iter().map(|directory| {
                json!({
                    "id": directory.id,
                    "name": directory.name,
                    "path": directory.path,
                    "type": "directory",
                })
            }).collect::<Vec<_>>(),
            "files": self.files.into_iter().map(|file| {
                json!({
                    "id": file.id,
                    "name": file.name,
                    "path": file.path,
                    "type": "file",
                    "mime_type": file.mime_type,
                    "size_bytes": file.size_bytes,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

fn pawfs_normalize_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path must not be empty".to_string());
    }

    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };

    let mut result = String::with_capacity(with_slash.len());
    let mut prev_slash = false;
    for ch in with_slash.chars() {
        if ch == '/' {
            if !prev_slash {
                result.push('/');
            }
            prev_slash = true;
        } else {
            result.push(ch);
            prev_slash = false;
        }
    }

    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }

    Ok(result)
}

fn pawfs_parse_file_path(path: &str) -> Result<(&str, &str), String> {
    match path.rsplit_once('/') {
        Some(("", filename)) if !filename.is_empty() => Ok(("/", filename)),
        Some((dir, filename)) if !filename.is_empty() => Ok((dir, filename)),
        _ => Err(format!("invalid file path (no filename): {path}")),
    }
}

fn pawfs_path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn pawfs_filter_path_and_workspace(path: &str, ws_id: &str) -> String {
    format!(
        "Path eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(path),
        escape_odata_string(ws_id)
    )
}

fn pawfs_filter_parent_and_workspace(parent_id: &str, ws_id: &str) -> String {
    format!(
        "ParentId eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(parent_id),
        escape_odata_string(ws_id)
    )
}

fn pawfs_filter_root_directory(ws_id: &str) -> String {
    // `ParentId eq null` is non-lossless -> disables index pushdown -> full Directories
    // scan -> 413 QueryTooLarge at workspace scale (ARN-68). Name '/' + WorkspaceId already
    // uniquely identify the root, so omit the null predicate to keep the read pushed-down.
    format!(
        "Name eq '/' and WorkspaceId eq '{}'",
        escape_odata_string(ws_id)
    )
}

fn pawfs_filter_name_parent_and_workspace(name: &str, parent_id: &str, ws_id: &str) -> String {
    format!(
        "Name eq '{}' and ParentId eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(name),
        escape_odata_string(parent_id),
        escape_odata_string(ws_id)
    )
}

fn pawfs_filter_name_in_directory(name: &str, directory_id: &str, ws_id: &str) -> String {
    format!(
        "Name eq '{}' and DirectoryId eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(name),
        escape_odata_string(directory_id),
        escape_odata_string(ws_id)
    )
}

fn pawfs_entity_status(value: &Value) -> &str {
    entity_field_str_any(value, &["Status", "status"]).unwrap_or("")
}

fn pawfs_is_archived(value: &Value) -> bool {
    pawfs_entity_status(value) == "Archived"
}

fn pawfs_first_non_archived_entity(resp: &Value) -> Option<&Value> {
    resp.get("value")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| !pawfs_is_archived(item))
}

fn pawfs_first_non_archived_entity_id(resp: &Value) -> Option<String> {
    pawfs_first_non_archived_entity(resp)
        .and_then(pawfs_entity_id)
        .map(str::to_string)
}

fn pawfs_entity_id(value: &Value) -> Option<&str> {
    entity_field_str_any(value, &["entity_id", "Id", "id"])
}

fn entity_field_i64_any(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(found) = value.get(*key).and_then(Value::as_i64) {
            return Some(found);
        }
        if let Some(found) = value
            .get("fields")
            .and_then(|fields| fields.get(*key))
            .and_then(Value::as_i64)
        {
            return Some(found);
        }
        if let Some(found) = value
            .get(*key)
            .and_then(Value::as_str)
            .and_then(|raw| raw.parse::<i64>().ok())
        {
            return Some(found);
        }
        if let Some(found) = value
            .get("fields")
            .and_then(|fields| fields.get(*key))
            .and_then(Value::as_str)
            .and_then(|raw| raw.parse::<i64>().ok())
        {
            return Some(found);
        }
    }
    None
}

fn pawfs_directory_from_value(value: &Value) -> Option<PawFsDirectory> {
    let id = pawfs_entity_id(value)?.to_string();
    let path = entity_field_str_any(value, &["Path", "path"])
        .unwrap_or("/")
        .to_string();
    let name = entity_field_str_any(value, &["Name", "name"])
        .unwrap_or_else(|| {
            if path == "/" {
                "/"
            } else {
                path.rsplit('/').next().unwrap_or("")
            }
        })
        .to_string();
    let parent_id = entity_field_str_any(value, &["ParentId", "parent_id"]).map(str::to_string);
    Some(PawFsDirectory {
        id,
        name,
        path,
        parent_id,
    })
}

fn pawfs_file_from_value(value: &Value) -> Option<PawFsFile> {
    let id = pawfs_entity_id(value)?.to_string();
    let path = entity_field_str_any(value, &["Path", "path"])?.to_string();
    let name = entity_field_str_any(value, &["Name", "name"])
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(""))
        .to_string();
    let directory_id = entity_field_str_any(value, &["DirectoryId", "directory_id"])
        .unwrap_or("")
        .to_string();
    let mime_type = entity_field_str_any(value, &["MimeType", "mime_type"])
        .unwrap_or("")
        .to_string();
    let size_bytes = entity_field_i64_any(value, &["SizeBytes", "size_bytes"]).unwrap_or(0);
    Some(PawFsFile {
        id,
        name,
        path,
        directory_id,
        mime_type,
        size_bytes,
    })
}

fn find_pawfs_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<Option<PawFsDirectory>, String> {
    let path = pawfs_normalize_path(raw_path)?;
    if let Some(directory_id) =
        resolve_pawfs_directory_id_by_walk(ctx, api_url, tenant, ws_id, &path)?
    {
        return fetch_pawfs_directory_by_id(ctx, api_url, tenant, &directory_id)
            .map(Some)
            .or_else(|error| {
                ctx.log(
                    "warn",
                    &format!("temper.pawfs(): walked directory lookup failed for '{path}': {error}"),
                );
                Ok(None)
            });
    }

    let filter = pawfs_filter_path_and_workspace(&path, ws_id);
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &bounded_reads::bounded_collection_query_path(
            "Directories",
            &filter,
            bounded_reads::POINT_LOOKUP_TOP,
        ),
    )?;
    Ok(pawfs_first_non_archived_entity(&resp).and_then(pawfs_directory_from_value))
}

fn fetch_pawfs_directory_by_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    directory_id: &str,
) -> Result<PawFsDirectory, String> {
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Directories('{directory_id}')"),
    )?;
    pawfs_directory_from_value(&resp)
        .ok_or_else(|| format!("temper.pawfs(): directory '{directory_id}' not found or invalid"))
}

fn resolve_pawfs_directory_id_by_walk(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    dir_path: &str,
) -> Result<Option<String>, String> {
    let normalized = pawfs_normalize_path(dir_path)?;
    let eid = ctx_entity_id(ctx);

    let root_filter = pawfs_filter_root_directory(ws_id);
    let root_resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &bounded_reads::bounded_collection_query_path(
            "Directories",
            &root_filter,
            bounded_reads::POINT_LOOKUP_TOP,
        ),
    )?;
    let Some(mut parent_id) = pawfs_first_non_archived_entity_id(&root_resp) else {
        return Ok(None);
    };

    if normalized == "/" {
        return Ok(Some(parent_id));
    }

    for segment in pawfs_path_segments(&normalized) {
        let filter = pawfs_filter_name_parent_and_workspace(segment, &parent_id, ws_id);
        let resp = http_get(
            ctx,
            api_url,
            tenant,
            eid,
            &bounded_reads::bounded_collection_query_path(
                "Directories",
                &filter,
                bounded_reads::POINT_LOOKUP_TOP,
            ),
        )?;
        let Some(directory_id) = pawfs_first_non_archived_entity_id(&resp) else {
            return Ok(None);
        };
        parent_id = directory_id;
    }

    Ok(Some(parent_id))
}

fn find_pawfs_file_by_walk(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<Option<PawFsFile>, String> {
    let path = pawfs_normalize_path(raw_path)?;
    let (dir_path, filename) = pawfs_parse_file_path(&path)?;
    let Some(directory_id) =
        resolve_pawfs_directory_id_by_walk(ctx, api_url, tenant, ws_id, dir_path)?
    else {
        return Ok(None);
    };

    let filter = pawfs_filter_name_in_directory(filename, &directory_id, ws_id);
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &bounded_reads::bounded_collection_query_path(
            "Files",
            &filter,
            bounded_reads::POINT_LOOKUP_TOP,
        ),
    )?;
    Ok(pawfs_first_non_archived_entity(&resp).and_then(pawfs_file_from_value))
}

fn find_pawfs_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<Option<PawFsFile>, String> {
    if let Some(file) = find_pawfs_file_by_walk(ctx, api_url, tenant, ws_id, raw_path)? {
        return Ok(Some(file));
    }

    let path = pawfs_normalize_path(raw_path)?;
    let filter = pawfs_filter_path_and_workspace(&path, ws_id);
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &bounded_reads::bounded_collection_query_path(
            "Files",
            &filter,
            bounded_reads::POINT_LOOKUP_TOP,
        ),
    )?;
    Ok(pawfs_first_non_archived_entity(&resp).and_then(pawfs_file_from_value))
}

fn fetch_pawfs_file_by_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    file_id: &str,
) -> Result<PawFsFile, String> {
    let eid = ctx_entity_id(ctx);
    let resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Files('{file_id}')"),
    )?;
    pawfs_file_from_value(&resp)
        .ok_or_else(|| format!("temper.pawfs(): file '{file_id}' not found or invalid"))
}

fn resolve_pawfs_file_for_write(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    ws_id: &str,
    raw_path: &str,
    opts: &Value,
    mime_type: &str,
) -> Result<PawFsFile, String> {
    if let Some(file_id) = pawfs_file_id_from_path_or_opts(raw_path, opts) {
        if let Ok(file) = fetch_pawfs_file_by_id(ctx, api_url, tenant, &file_id) {
            return Ok(file);
        }

        let path = if is_pawfs_file_id(raw_path) {
            raw_path.trim().to_string()
        } else {
            raw_path.to_string()
        };
        return Ok(PawFsFile {
            id: file_id,
            name: String::new(),
            path,
            directory_id: String::new(),
            mime_type: mime_type.to_string(),
            size_bytes: 0,
        });
    }

    ensure_pawfs_file(
        ctx,
        api_url,
        tenant,
        principal_id,
        ws_id,
        raw_path,
        mime_type,
    )
}

fn create_pawfs_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    ws_id: &str,
    path: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<PawFsDirectory, String> {
    let mut body = json!({
        "Name": name,
        "Path": path,
        "WorkspaceId": ws_id,
    });
    if let Some(parent_id) = parent_id {
        body["ParentId"] = json!(parent_id);
    }

    let resp = http_post(
        ctx,
        api_url,
        tenant,
        principal_id,
        "/tdata/Directories",
        &body,
    )?;
    let id = pawfs_entity_id(&resp)
        .ok_or_else(|| "temper.pawfs(): Directory created but no Id returned".to_string())?
        .to_string();
    Ok(PawFsDirectory {
        id,
        name: name.to_string(),
        path: path.to_string(),
        parent_id: parent_id.map(str::to_string),
    })
}

fn ensure_pawfs_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<PawFsDirectory, String> {
    let normalized = pawfs_normalize_path(raw_path)?;
    if let Some(directory) = find_pawfs_directory(ctx, api_url, tenant, ws_id, &normalized)? {
        return Ok(directory);
    }

    let mut parent = match find_pawfs_directory(ctx, api_url, tenant, ws_id, "/")? {
        Some(directory) => directory,
        None => create_pawfs_directory(ctx, api_url, tenant, principal_id, ws_id, "/", "/", None)?,
    };

    if normalized == "/" {
        return Ok(parent);
    }

    let mut current_path = String::new();
    for segment in pawfs_path_segments(&normalized) {
        current_path.push('/');
        current_path.push_str(segment);

        if let Some(directory) = find_pawfs_directory(ctx, api_url, tenant, ws_id, &current_path)? {
            parent = directory;
            continue;
        }

        let directory = create_pawfs_directory(
            ctx,
            api_url,
            tenant,
            principal_id,
            ws_id,
            &current_path,
            segment,
            Some(&parent.id),
        )?;
        if let Err(error) = http_post(
            ctx,
            api_url,
            tenant,
            principal_id,
            &format!("/tdata/Directories('{}')/Temper.AddChild", parent.id),
            &json!({}),
        ) {
            ctx.log(
                "warn",
                &format!(
                    "temper.pawfs(): AddChild failed on directory {}: {error}",
                    parent.id
                ),
            );
        }
        parent = directory;
    }

    Ok(parent)
}

fn ensure_pawfs_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    ws_id: &str,
    raw_path: &str,
    mime_type: &str,
) -> Result<PawFsFile, String> {
    let normalized = pawfs_normalize_path(raw_path)?;
    if let Some(file) = find_pawfs_file(ctx, api_url, tenant, ws_id, &normalized)? {
        return Ok(file);
    }

    let (dir_path, filename) = pawfs_parse_file_path(&normalized)?;
    let directory = ensure_pawfs_directory(ctx, api_url, tenant, principal_id, ws_id, dir_path)?;
    let body = json!({
        "Name": filename,
        "Path": normalized,
        "DirectoryId": directory.id.clone(),
        "WorkspaceId": ws_id,
        "MimeType": mime_type,
    });
    let resp = http_post(ctx, api_url, tenant, principal_id, "/tdata/Files", &body)?;
    let id = pawfs_entity_id(&resp)
        .ok_or_else(|| "temper.write(): File created but no Id returned".to_string())?
        .to_string();

    if let Err(error) = http_post(
        ctx,
        api_url,
        tenant,
        principal_id,
        &format!("/tdata/Directories('{}')/Temper.AddChild", directory.id),
        &json!({}),
    ) {
        ctx.log(
            "warn",
            &format!(
                "temper.write(): AddChild failed on directory {}: {error}",
                directory.id
            ),
        );
    }

    Ok(PawFsFile {
        id,
        name: filename.to_string(),
        path: normalized,
        directory_id: directory.id,
        mime_type: mime_type.to_string(),
        size_bytes: 0,
    })
}

fn read_pawfs_file_value(
    ctx: &Context,
    api_url: &str,
    file_id: &str,
    method: &str,
) -> Result<String, String> {
    let url = format!("{api_url}/tdata/Files('{file_id}')/$value");
    let headers = vec![("Accept".to_string(), "application/octet-stream".to_string())];
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "{method}: content read failed (HTTP {})",
            resp.status
        ));
    }
    Ok(resp.body)
}

fn list_pawfs_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<PawFsListing, String> {
    let normalized = pawfs_normalize_path(raw_path)?;
    let directory = find_pawfs_directory(ctx, api_url, tenant, ws_id, &normalized)?
        .ok_or_else(|| format!("temper.ls(): directory not found at '{normalized}'"))?;
    let eid = ctx_entity_id(ctx);
    let child_filter = urlenc(&pawfs_filter_parent_and_workspace(&directory.id, ws_id));

    let dirs_resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Directories?$filter={child_filter}"),
    )?;
    let mut directories = dirs_resp
        .get("value")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| !pawfs_is_archived(item))
                .filter_map(pawfs_directory_from_value)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    directories.sort_by(|left, right| left.path.cmp(&right.path));

    let file_filter = urlenc(&format!(
        "DirectoryId eq '{}' and WorkspaceId eq '{}'",
        escape_odata_string(&directory.id),
        escape_odata_string(ws_id)
    ));
    let files_resp = http_get(
        ctx,
        api_url,
        tenant,
        eid,
        &format!("/tdata/Files?$filter={file_filter}"),
    )?;
    let mut files = files_resp
        .get("value")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| !pawfs_is_archived(item))
                .filter_map(pawfs_file_from_value)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(PawFsListing {
        path: normalized,
        directories,
        files,
    })
}

fn rename_pawfs_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    principal_id: &str,
    ws_id: &str,
    old_path: &str,
    new_path: &str,
) -> Result<PawFsFile, String> {
    let file = find_pawfs_file(ctx, api_url, tenant, ws_id, old_path)?
        .ok_or_else(|| format!("temper.rename(): file not found at '{old_path}'"))?;
    let normalized_new_path = pawfs_normalize_path(new_path)?;
    if let Some(existing) = find_pawfs_file(ctx, api_url, tenant, ws_id, &normalized_new_path)?
        && existing.id != file.id
    {
        return Err(format!(
            "temper.rename(): target already exists at '{normalized_new_path}'"
        ));
    }

    let (new_dir_path, new_filename) = pawfs_parse_file_path(&normalized_new_path)?;
    let new_directory =
        ensure_pawfs_directory(ctx, api_url, tenant, principal_id, ws_id, new_dir_path)?;
    let body = json!({
        "Name": new_filename,
        "Path": normalized_new_path,
        "DirectoryId": new_directory.id.clone(),
    });
    http_patch(
        ctx,
        api_url,
        tenant,
        principal_id,
        &format!("/tdata/Files('{}')", file.id),
        &body,
    )?;

    if !file.directory_id.is_empty() && file.directory_id != new_directory.id {
        if let Err(error) = http_post(
            ctx,
            api_url,
            tenant,
            principal_id,
            &format!(
                "/tdata/Directories('{}')/Temper.RemoveChild",
                file.directory_id
            ),
            &json!({}),
        ) {
            ctx.log(
                "warn",
                &format!(
                    "temper.rename(): RemoveChild failed on directory {}: {error}",
                    file.directory_id
                ),
            );
        }
        if let Err(error) = http_post(
            ctx,
            api_url,
            tenant,
            principal_id,
            &format!("/tdata/Directories('{}')/Temper.AddChild", new_directory.id),
            &json!({}),
        ) {
            ctx.log(
                "warn",
                &format!(
                    "temper.rename(): AddChild failed on directory {}: {error}",
                    new_directory.id
                ),
            );
        }
    }

    Ok(PawFsFile {
        id: file.id,
        name: new_filename.to_string(),
        path: normalized_new_path,
        directory_id: new_directory.id,
        mime_type: file.mime_type,
        size_bytes: file.size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=";

    #[test]
    fn file_status_is_ready_accepts_ready_and_locked_only() {
        assert!(file_status_is_ready(&json!({ "Status": "Ready" })));
        assert!(file_status_is_ready(&json!({ "Status": "Locked" })));
        assert!(!file_status_is_ready(&json!({ "Status": "Created" })));
        assert!(!file_status_is_ready(&json!({ "Status": "Archived" })));
        assert!(!file_status_is_ready(&json!({})));
        assert!(file_status_is_ready(&json!({ "status": "Ready" })));
    }

    #[test]
    fn spawn_session_input_accepts_legacy_positional_arguments() {
        let input = spawn_session_input(&[
            json!("clone the repo"),
            json!("SWE"),
            json!("claude-sonnet-4-6"),
            json!("bash,read"),
            json!("/workspace/repo"),
            json!("https://sandbox.example"),
            json!(12),
            json!(true),
        ])
        .unwrap();

        assert_eq!(input["task"], "clone the repo");
        assert_eq!(input["soul_id"], "SWE");
        assert_eq!(input["model"], "claude-sonnet-4-6");
        assert_eq!(input["tools"], "bash,read");
        assert_eq!(input["workdir"], "/workspace/repo");
        assert_eq!(input["sandbox_url"], "https://sandbox.example");
        assert_eq!(input["max_turns"], "12");
        assert_eq!(input["background"], true);
    }

    #[test]
    fn codex_spawn_model_replaces_gpt_4_1_with_parent_model() {
        let model =
            resolve_spawn_model_for_provider("openai_codex", "gpt-4.1", "gpt-5.5", Some("gpt-5.5"))
                .unwrap();

        assert_eq!(model, "gpt-5.5");
    }

    #[test]
    fn codex_spawn_model_replaces_gpt_4_1_with_default_when_parent_is_bad() {
        let model = resolve_spawn_model_for_provider(
            "openai_codex",
            "gpt-4.1-mini",
            "gpt-4.1",
            Some("gpt-5.5"),
        )
        .unwrap();

        assert_eq!(model, "gpt-5.5");
    }

    #[test]
    fn codex_spawn_model_rejects_gpt_4_1_without_compatible_fallback() {
        let err =
            resolve_spawn_model_for_provider("openai_codex", "gpt-4.1", "", None).unwrap_err();

        assert!(err.contains("gpt-4.1"));
        assert!(err.contains("openai_codex"));
    }

    #[test]
    fn public_openai_spawn_model_keeps_gpt_4_1() {
        let model = resolve_spawn_model_for_provider("openai", "gpt-4.1", "gpt-5.5", None).unwrap();

        assert_eq!(model, "gpt-4.1");
    }

    #[test]
    fn list_sessions_input_accepts_legacy_filter_and_top_arguments() {
        let input = list_sessions_input(&[json!("Status eq 'Active'"), json!(25)]);

        assert_eq!(input["filter"], "Status eq 'Active'");
        assert_eq!(input["top"], 25);
    }

    #[test]
    fn abort_session_input_accepts_legacy_session_id() {
        let input = abort_session_input(&[json!("sess-123")]).unwrap();
        assert_eq!(input["session_id"], "sess-123");
    }

    #[test]
    fn steer_session_input_accepts_legacy_session_id_and_message() {
        let input = steer_session_input(&[json!("sess-123"), json!("Please continue")]).unwrap();
        assert_eq!(input["session_id"], "sess-123");
        assert_eq!(input["message"], "Please continue");
    }

    #[test]
    fn save_memory_input_accepts_legacy_positional_arguments() {
        let input =
            save_memory_input(&[json!("repo"), json!("openclaw/openclaw"), json!("project")])
                .unwrap();

        assert_eq!(input["key"], "repo");
        assert_eq!(input["content"], "openclaw/openclaw");
        assert_eq!(input["memory_type"], "project");
    }

    #[test]
    fn recall_memory_input_accepts_legacy_query_argument() {
        let input = recall_memory_input(&[json!("openclaw")]).unwrap();
        assert_eq!(input["query"], "openclaw");
    }

    #[test]
    fn scoped_virtual_path_detection_covers_global_scope_roots() {
        assert!(is_global_scoped_path(
            "/system/knowledge/design-principles.md"
        ));
        assert!(is_global_scoped_path(
            "/agents/sl-bootstrap-agent-soul-curator/skills/research-direction/SKILL.md"
        ));
        assert!(is_global_scoped_path(
            "/projects/proj-123/skills/review-quality/SKILL.md"
        ));
        assert!(!is_global_scoped_path("/katagami/index.md"));
    }

    #[test]
    fn scoped_virtual_path_file_filter_scopes_workspace_and_escapes_quotes() {
        assert_eq!(
            global_scoped_file_filter("/system/knowledge/we're-here.md", "os-app-docs"),
            "Path eq '/system/knowledge/we''re-here.md' and WorkspaceId eq 'os-app-docs'"
        );
    }

    #[test]
    fn pawfs_file_id_detection_accepts_fl_prefix() {
        assert!(is_pawfs_file_id(
            "fl-019edd73-9c8f-7141-88c8-bdb796d40ce9"
        ));
        assert!(!is_pawfs_file_id("/katagami/DESIGN.md"));
        assert!(!is_pawfs_file_id("fl-"));
    }

    #[test]
    fn pawfs_file_id_from_opts_prefers_explicit_file_id() {
        let opts = json!({
            "file_id": "fl-019edd73-9c8f-7141-88c8-bdb796d40ce9",
            "entity_id": "fl-00000000-0000-0000-0000-000000000001",
            "workspace_id": "ws-test"
        });
        assert_eq!(
            pawfs_file_id_from_path_or_opts("/katagami/DESIGN.md", &opts),
            Some("fl-019edd73-9c8f-7141-88c8-bdb796d40ce9".to_string())
        );
    }

    #[test]
    fn pawfs_file_id_from_path_accepts_direct_id_argument() {
        let opts = json!({});
        assert_eq!(
            pawfs_file_id_from_path_or_opts(
                "fl-019edd73-9c8f-7141-88c8-bdb796d40ce9",
                &opts
            ),
            Some("fl-019edd73-9c8f-7141-88c8-bdb796d40ce9".to_string())
        );
    }

    #[test]
    fn global_scoped_workspace_id_defaults_to_os_app_docs() {
        assert_eq!(global_scoped_workspace_id(&json!({})), "os-app-docs");
        assert_eq!(
            global_scoped_workspace_id(&json!({"workspace_id": "ws-custom"})),
            "ws-custom"
        );
    }

    #[test]
    fn pawfs_path_filter_omits_status_ne_for_lossless_pushdown() {
        assert_eq!(
            pawfs_filter_path_and_workspace("/katagami/DESIGN.md", "ws-test"),
            "Path eq '/katagami/DESIGN.md' and WorkspaceId eq 'ws-test'"
        );
        assert!(!pawfs_filter_path_and_workspace("/katagami/DESIGN.md", "ws-test")
            .contains("Status ne"));
    }

    #[test]
    fn pawfs_walk_filters_use_parent_and_directory_pushdown() {
        // ARN-68: the root filter must NOT use the non-lossless `ParentId eq null`
        // (it disables index pushdown -> full Directories scan -> 413). Name '/' +
        // WorkspaceId uniquely identify the root.
        assert_eq!(
            pawfs_filter_root_directory("os-app-docs"),
            "Name eq '/' and WorkspaceId eq 'os-app-docs'"
        );
        assert!(!pawfs_filter_root_directory("os-app-docs").contains("eq null"));
        assert_eq!(
            pawfs_filter_name_parent_and_workspace("skills", "dir-parent", "os-app-docs"),
            "Name eq 'skills' and ParentId eq 'dir-parent' and WorkspaceId eq 'os-app-docs'"
        );
        assert_eq!(
            pawfs_filter_name_in_directory("SKILL.md", "dir-leaf", "os-app-docs"),
            "Name eq 'SKILL.md' and DirectoryId eq 'dir-leaf' and WorkspaceId eq 'os-app-docs'"
        );
        for filter in [
            pawfs_filter_root_directory("os-app-docs"),
            pawfs_filter_name_parent_and_workspace("skills", "dir-parent", "os-app-docs"),
            pawfs_filter_name_in_directory("SKILL.md", "dir-leaf", "os-app-docs"),
        ] {
            assert!(!filter.contains("Status ne"));
            assert!(!filter.contains("Path eq"));
        }
    }

    #[test]
    fn pawfs_first_non_archived_entity_skips_archived_rows() {
        let resp = json!({
            "value": [
                {"Id": "fl-archived", "Status": "Archived", "Path": "/a.md"},
                {"Id": "fl-active", "Status": "Ready", "Path": "/a.md"},
            ]
        });
        assert_eq!(
            pawfs_first_non_archived_entity_id(&resp),
            Some("fl-active".to_string())
        );
    }

    #[test]
    fn write_upload_accepts_sandbox_image_object() {
        let upload = write_upload_from_value(
            "/tmp/thumbnail.png",
            &json!({
                "__temperpaw_image": true,
                "media_type": "image/png",
                "base64_data": PNG_1X1
            }),
            &json!({}),
        )
        .unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert!(matches!(
            upload.content,
            WriteUploadContent::BrowserImageBytes(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        ));
    }

    #[test]
    fn write_upload_accepts_sandbox_image_source_handle() {
        let upload = write_upload_from_value(
            "/tmp/thumbnail.png",
            &json!({
                "__temperpaw_image": true,
                "media_type": "image/png",
                "source_path": "/tmp/thumbnail.png"
            }),
            &json!({}),
        )
        .unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert_eq!(
            upload.content,
            WriteUploadContent::SandboxImageSource("/tmp/thumbnail.png".to_string())
        );
    }

    #[test]
    fn write_input_accepts_object_shape_and_top_level_options() {
        let input = write_input(&[json!({
            "path": "/tmp/thumbnail.jpg",
            "content": {
                "__temperpaw_image": true,
                "media_type": "image/jpeg",
                "source_path": "/tmp/thumbnail.jpg"
            },
            "mime_type": "image/jpeg",
            "workspace_id": "ws-test"
        })])
        .unwrap();

        assert_eq!(input.path, "/tmp/thumbnail.jpg");
        assert_eq!(input.opts["mime_type"], "image/jpeg");
        assert_eq!(input.opts["workspace_id"], "ws-test");
        assert!(is_sandbox_image_marker(&input.content));
    }

    #[test]
    fn write_upload_accepts_json_stringified_sandbox_image_object() {
        let upload = write_upload_from_value(
            "/tmp/thumbnail.png",
            &json!(
                json!({
                    "__temperpaw_image": true,
                    "media_type": "image/png",
                    "base64_data": PNG_1X1
                })
                .to_string()
            ),
            &json!({}),
        )
        .unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert!(matches!(
            upload.content,
            WriteUploadContent::BrowserImageBytes(_)
        ));
    }

    #[test]
    fn write_upload_accepts_json_stringified_sandbox_image_source_handle() {
        let upload = write_upload_from_value(
            "/tmp/thumbnail.png",
            &json!(
                json!({
                    "__temperpaw_image": true,
                    "media_type": "image/png",
                    "source_path": "/tmp/thumbnail.png"
                })
                .to_string()
            ),
            &json!({}),
        )
        .unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert_eq!(
            upload.content,
            WriteUploadContent::SandboxImageSource("/tmp/thumbnail.png".to_string())
        );
    }

    #[test]
    fn write_upload_accepts_legacy_temperpaw_image_marker() {
        let upload = write_upload_from_value(
            "/tmp/thumbnail.png",
            &json!({
                "__temperpaw_image": true,
                "media_type": "image/png",
                "base64_data": PNG_1X1
            }),
            &json!({}),
        )
        .unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert!(matches!(
            upload.content,
            WriteUploadContent::BrowserImageBytes(_)
        ));
    }

    #[test]
    fn write_upload_decodes_raster_base64_for_image_paths() {
        let upload =
            write_upload_from_value("/tmp/thumbnail.png", &json!(PNG_1X1), &json!({})).unwrap();

        assert_eq!(upload.mime_type, "image/png");
        assert!(matches!(
            upload.content,
            WriteUploadContent::BrowserImageBytes(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        ));
    }

    #[test]
    fn write_upload_rejects_base64_text_that_is_not_an_image() {
        let error = write_upload_from_value("/tmp/thumbnail.png", &json!("aGVsbG8="), &json!({}))
            .unwrap_err();

        assert!(error.contains("not a supported browser image"));
    }
}

fn resolve_workspace_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    opts: &Value,
    create_if_missing: bool,
) -> Result<String, String> {
    if let Some(workspace_id) = opts
        .get("workspace_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        return Ok(workspace_id.to_string());
    }

    if let Some(workspace_name) = opts
        .get("workspace")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        return if create_if_missing {
            ensure_workspace(ctx, api_url, tenant, workspace_name)
        } else {
            find_workspace(ctx, api_url, tenant, workspace_name)?
                .ok_or_else(|| format!("workspace '{}' not found", workspace_name))
        };
    }

    if let Some(workspace_id) = session_workspace_id(ctx) {
        return Ok(workspace_id);
    }

    if create_if_missing {
        ensure_workspace(ctx, api_url, tenant, "default")
    } else {
        find_workspace(ctx, api_url, tenant, "default")?
            .ok_or_else(|| "workspace 'default' not found".to_string())
    }
}

fn ensure_workspace(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    name: &str,
) -> Result<String, String> {
    if let Some(id) = find_workspace(ctx, api_url, tenant, name)? {
        return Ok(id);
    }
    let eid = ctx_entity_id(ctx);
    let resp = http_post(
        ctx,
        api_url,
        tenant,
        eid,
        "/tdata/Workspaces",
        &json!({"Name": name}),
    )?;
    resp.get("entity_id")
        .or_else(|| resp.get("Id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "temper.write(): Workspace created but no Id returned".into())
}

fn mime_from_ext(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "ts" => "application/typescript",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn urlenc(s: &str) -> String {
    bounded_reads::odata_filter_urlencode(s)
}
