//! workspace_fs — WASM module for FUSE-mapped filesystem operations on Workspace entities.
//!
//! Each integration trigger dispatches to a single filesystem operation:
//! mkdir, create_file, resolve_path, list_dir, delete_file.
//!
//! The `operation` config field selects which function to run.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

mod ops;
mod path;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;

        let api_url = ctx
            .config
            .get("temper_api_url")
            .ok_or("workspace_fs: missing temper_api_url config")?
            .clone();
        let operation = ctx
            .config
            .get("operation")
            .ok_or("workspace_fs: missing operation config")?
            .clone();
        let tenant = ctx.tenant.clone();
        let ws_id = ctx.entity_id.clone();

        let raw_path = ctx
            .trigger_params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("workspace_fs: missing path parameter")?;

        ctx.log(
            "info",
            &format!("workspace_fs: op={operation} path={raw_path} ws={ws_id}"),
        );

        let new_path = ctx
            .trigger_params
            .get("new_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let op_result = match operation.as_str() {
            "mkdir" => ops::mkdir(&ctx, &api_url, &tenant, &ws_id, raw_path),
            "create_file" => {
                let mime_type = ctx.trigger_params.get("mime_type").and_then(|v| v.as_str());
                ops::create_file(&ctx, &api_url, &tenant, &ws_id, raw_path, mime_type)
            }
            "resolve_path" => ops::resolve_path(&ctx, &api_url, &tenant, &ws_id, raw_path),
            "list_dir" => ops::list_dir(&ctx, &api_url, &tenant, &ws_id, raw_path),
            "delete_file" => ops::delete_file(&ctx, &api_url, &tenant, &ws_id, raw_path),
            "rename" => match ctx.trigger_params.get("new_path").and_then(|v| v.as_str()) {
                Some(new_path) => ops::rename(&ctx, &api_url, &tenant, &ws_id, raw_path, new_path),
                None => Err("workspace_fs: missing new_path parameter".to_string()),
            },
            other => Err(format!("workspace_fs: unknown operation: {other}")),
        };
        emit_fs_observability(&ctx, &operation, &ws_id, raw_path, new_path, &op_result);
        op_result
    })();

    match result {
        Ok(()) => 0,
        Err(e) => {
            set_error_result(&e);
            1
        }
    }
}

fn emit_fs_observability(
    ctx: &Context,
    operation: &str,
    workspace_id: &str,
    path: &str,
    new_path: &str,
    result: &Result<(), String>,
) {
    let fields = build_fs_observability_fields(operation, workspace_id, path, new_path, result);
    let _ = ctx.log_structured("info", "temperpaw.fs operation", &fields);
}

fn build_fs_observability_fields(
    operation: &str,
    workspace_id: &str,
    path: &str,
    new_path: &str,
    result: &Result<(), String>,
) -> Value {
    let outcome = if result.is_ok() { "success" } else { "error" };
    let mut fields = json!({
        "observability_event": "temperpaw.fs",
        "workspace_id": workspace_id,
        "fs": {
            "operation": operation,
            "outcome": outcome,
            "backend": "temperfs-workspace",
            "path": path,
            "new_path": new_path,
        }
    });
    if let Err(error) = result {
        fields["error"] = json!({
            "kind": "WorkspaceFsError",
            "message": error,
        });
    }
    fields
}
