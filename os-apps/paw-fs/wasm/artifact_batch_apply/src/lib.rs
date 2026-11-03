use serde::Deserialize;
use serde_json::{Value, json};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::bounded_reads;

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    content: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let api_url = ctx
            .config
            .get("temper_api_url")
            .ok_or("artifact_batch_apply: missing temper_api_url config")?
            .clone();
        apply_batch(&ctx, &api_url)?;
        set_success_result("", &json!({"status": "artifact_batch_apply_complete"}));
        Ok(())
    })();

    match result {
        Ok(()) => 0,
        Err(error) => {
            set_error_result(&error);
            1
        }
    }
}

fn apply_batch(ctx: &Context, api_url: &str) -> Result<(), String> {
    let tenant = ctx.tenant.as_str();
    let batch_id = ctx.entity_id.as_str();
    let fields = ctx.entity_state.get("fields").unwrap_or(&ctx.entity_state);
    let workspace_id = field_str(fields, &["workspace_id", "WorkspaceId"])
        .ok_or("artifact_batch_apply: missing workspace_id")?;
    let manifest_raw = field_str(fields, &["files_manifest", "FilesManifest"])
        .ok_or("artifact_batch_apply: missing files_manifest")?;
    let files: Vec<ManifestFile> = serde_json::from_str(manifest_raw)
        .map_err(|error| format!("artifact_batch_apply: invalid files_manifest: {error}"))?;

    let mut total_bytes = 0usize;
    let mut applied_count = 0usize;
    for file in files {
        let content = file
            .content
            .as_deref()
            .or(file.body.as_deref())
            .unwrap_or_default()
            .to_string();
        if content.is_empty() {
            continue;
        }
        match apply_file(ctx, api_url, tenant, workspace_id, &file, &content) {
            Ok((file_id, size_bytes)) => {
                total_bytes = total_bytes.saturating_add(size_bytes);
                applied_count = applied_count.saturating_add(1);
                post(
                    ctx,
                    api_url,
                    tenant,
                    &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.RecordFileApplied"),
                    &json!({
                        "path": file.path,
                        "file_id": file_id,
                        "size_bytes": size_bytes,
                    }),
                )?;
            }
            Err(error) => {
                let _ = post(
                    ctx,
                    api_url,
                    tenant,
                    &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.RecordFileFailed"),
                    &json!({"path": file.path, "error": error}),
                );
                return Err(error);
            }
        }
    }

    if applied_count == 0 && total_bytes == 0 {
        return Ok(());
    }

    let bucket_key = format!("artifact_batch:{batch_id}");
    let bucket = post(
        ctx,
        api_url,
        tenant,
        "/tdata/WorkspaceUsageBuckets",
        &json!({
            "WorkspaceId": workspace_id,
            "BucketKey": bucket_key,
            "ArtifactBatchId": batch_id,
        }),
    )?;
    let bucket_id = entity_id(&bucket)
        .ok_or("artifact_batch_apply: WorkspaceUsageBucket created but no Id returned")?;
    post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/WorkspaceUsageBuckets('{bucket_id}')/Temper.ApplyDelta"),
        &json!({
            "workspace_id": workspace_id,
            "bucket_key": bucket_key,
            "artifact_batch_id": batch_id,
            "bytes_delta": total_bytes,
            "file_delta": applied_count,
        }),
    )?;
    post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/ArtifactBatches('{batch_id}')/Temper.Complete"),
        &json!({"usage_bucket_id": bucket_id}),
    )?;
    Ok(())
}

fn apply_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file: &ManifestFile,
    content: &str,
) -> Result<(String, usize), String> {
    let path = normalize_path(&file.path)?;
    let mime_type = file
        .mime_type
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| mime_from_ext(&path));
    let file_id = ensure_file(ctx, api_url, tenant, workspace_id, &path, mime_type)?;
    let url = format!("{api_url}/tdata/Files('{file_id}')/$value");
    let headers = vec![
        ("X-Tenant-Id".to_string(), tenant.to_string()),
        ("Content-Type".to_string(), mime_type.to_string()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        (
            "x-temper-principal-id".to_string(),
            "artifact-batch".to_string(),
        ),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];
    let resp = ctx.http_call("PUT", &url, &headers, content)?;
    if resp.status >= 400 {
        return Err(format!(
            "artifact_batch_apply: upload failed for {path} (HTTP {})",
            resp.status
        ));
    }
    Ok((file_id, content.len()))
}

fn ensure_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    path: &str,
    mime_type: &str,
) -> Result<String, String> {
    if let Some(file_id) = find_entity_id(
        ctx,
        api_url,
        tenant,
        "Files",
        &format!(
            "Path eq '{}' and WorkspaceId eq '{}'",
            bounded_reads::odata_escape(path),
            bounded_reads::odata_escape(workspace_id)
        ),
    )? {
        return Ok(file_id);
    }

    let (dir_path, name) = parse_file_path(path)?;
    let directory_id = ensure_directory(ctx, api_url, tenant, workspace_id, dir_path)?;
    let file = post(
        ctx,
        api_url,
        tenant,
        "/tdata/Files",
        &json!({
            "Name": name,
            "Path": path,
            "DirectoryId": directory_id,
            "WorkspaceId": workspace_id,
            "MimeType": mime_type,
        }),
    )?;
    let file_id =
        entity_id(&file).ok_or("artifact_batch_apply: File created but no Id returned")?;
    let _ = post(
        ctx,
        api_url,
        tenant,
        &format!("/tdata/Directories('{directory_id}')/Temper.AddChild"),
        &json!({}),
    );
    Ok(file_id)
}

fn ensure_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    path: &str,
) -> Result<String, String> {
    if let Some(id) = find_directory(ctx, api_url, tenant, workspace_id, path)? {
        return Ok(id);
    }
    let mut parent_id = match find_directory(ctx, api_url, tenant, workspace_id, "/")? {
        Some(id) => id,
        None => {
            let root = post(
                ctx,
                api_url,
                tenant,
                "/tdata/Directories",
                &json!({"Name": "/", "Path": "/", "WorkspaceId": workspace_id}),
            )?;
            entity_id(&root).ok_or("artifact_batch_apply: root Directory created but no Id")?
        }
    };
    if path == "/" {
        return Ok(parent_id);
    }

    let mut current_path = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        current_path.push('/');
        current_path.push_str(segment);
        if let Some(id) = find_directory(ctx, api_url, tenant, workspace_id, &current_path)? {
            parent_id = id;
            continue;
        }
        let directory = post(
            ctx,
            api_url,
            tenant,
            "/tdata/Directories",
            &json!({
                "Name": segment,
                "Path": current_path,
                "ParentId": parent_id,
                "WorkspaceId": workspace_id,
            }),
        )?;
        let new_id =
            entity_id(&directory).ok_or("artifact_batch_apply: Directory created but no Id")?;
        let _ = post(
            ctx,
            api_url,
            tenant,
            &format!("/tdata/Directories('{parent_id}')/Temper.AddChild"),
            &json!({}),
        );
        parent_id = new_id;
    }
    Ok(parent_id)
}

fn find_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    path: &str,
) -> Result<Option<String>, String> {
    find_entity_id(
        ctx,
        api_url,
        tenant,
        "Directories",
        &format!(
            "Path eq '{}' and WorkspaceId eq '{}'",
            bounded_reads::odata_escape(path),
            bounded_reads::odata_escape(workspace_id)
        ),
    )
}

fn find_entity_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    set_name: &str,
    filter: &str,
) -> Result<Option<String>, String> {
    bounded_reads::find_first_non_archived_entity_id(
        ctx,
        api_url,
        &headers(tenant),
        set_name,
        filter,
        bounded_reads::POINT_LOOKUP_TOP,
        "artifact_batch_apply: bounded entity lookup",
    )
}

fn post(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api_url}{path}");
    let resp = ctx.http_call("POST", &url, &headers(tenant), &body.to_string())?;
    if resp.status >= 400 {
        return Err(format!("POST {path}: HTTP {} {}", resp.status, resp.body));
    }
    if resp.body.is_empty() {
        return Ok(json!({"ok": true}));
    }
    serde_json::from_str(&resp.body).map_err(|error| format!("parse POST {path}: {error}"))
}

fn headers(tenant: &str) -> Vec<(String, String)> {
    vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Tenant-Id".to_string(), tenant.to_string()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        (
            "x-temper-principal-id".to_string(),
            "artifact-batch".to_string(),
        ),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn field_str<'a>(fields: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = fields.get(*key).and_then(Value::as_str) {
            return Some(value);
        }
    }
    None
}

fn entity_id(value: &Value) -> Option<String> {
    value
        .get("entity_id")
        .or_else(|| value.get("Id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("artifact_batch_apply: empty path".to_string());
    }
    let mut path = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    Ok(path)
}

fn parse_file_path(path: &str) -> Result<(&str, &str), String> {
    match path.rsplit_once('/') {
        Some(("", filename)) if !filename.is_empty() => Ok(("/", filename)),
        Some((dir, filename)) if !filename.is_empty() => Ok((dir, filename)),
        _ => Err(format!("artifact_batch_apply: invalid file path {path}")),
    }
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
        _ => "application/octet-stream",
    }
}
