//! Filesystem operations — each function maps 1:1 to a FUSE/POSIX syscall.
//!
//! All entity I/O goes through `ctx.http_call` to the Temper OData API.

use temper_wasm_sdk::prelude::*;

use crate::path;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn runtime_headers(tenant: &str) -> Vec<(String, String)> {
    vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Tenant-Id".to_string(), tenant.to_string()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), "system".to_string()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn http_get(ctx: &Context, url: &str, tenant: &str) -> Result<Value, String> {
    let headers = runtime_headers(tenant);
    let resp = ctx.http_call("GET", url, &headers, "")?;
    if resp.status >= 400 {
        return Err(format!("GET {url}: HTTP {}", resp.status));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {url}: {e}"))
}

fn http_post(ctx: &Context, url: &str, tenant: &str, body: &Value) -> Result<Value, String> {
    let headers = runtime_headers(tenant);
    let resp = ctx.http_call("POST", url, &headers, &body.to_string())?;
    if resp.status >= 400 {
        return Err(format!("POST {url}: HTTP {} — {}", resp.status, resp.body));
    }
    if resp.body.is_empty() {
        return Ok(json!({"ok": true}));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse response from {url}: {e}"))
}

fn extract_id(resp: &Value) -> Option<String> {
    resp.get("entity_id")
        .or_else(|| resp.get("Id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn odata_encode(s: &str) -> String {
    s.replace('\'', "''")
        .replace('%', "%25")
        .replace(' ', "%20")
}

/// Find a directory by Name + WorkspaceId + ParentId (null or specific).
fn find_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    name: &str,
    parent_id: Option<&str>,
) -> Result<Option<String>, String> {
    let name_enc = odata_encode(name);
    // `ParentId eq '<id>'` is a lossless string-equality that pushes down. But the root's
    // old `ParentId eq null` is NON-LOSSLESS -> disables index pushdown -> full Directories
    // scan -> 413 QueryTooLarge at workspace scale (ARN-68). Name + WorkspaceId already
    // uniquely identify the root, so omit the null predicate (lossless pushdown) and confirm
    // ParentId is unset in-memory.
    let parent_clause = match parent_id {
        Some(pid) => format!("%20and%20ParentId%20eq%20'{}'", odata_encode(pid)),
        None => String::new(),
    };
    let url = format!(
        "{api_url}/tdata/Directories?$filter=Name%20eq%20'{name_enc}'%20and%20WorkspaceId%20eq%20'{ws_id}'{parent_clause}"
    );
    let resp = http_get(ctx, &url, tenant)?;
    let items = resp.get("value").and_then(|v| v.as_array());
    let chosen = items.and_then(|arr| {
        arr.iter().find(|v| match parent_id {
            // root: accept only an entry whose ParentId is null / absent / empty
            None => v
                .get("ParentId")
                .map_or(true, |p| p.is_null() || p.as_str().map_or(true, str::is_empty)),
            // a specific parent is already constrained by the query
            Some(_) => true,
        })
    });
    Ok(chosen.and_then(extract_id))
}

/// Find a file by Path + WorkspaceId.
fn find_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    file_path: &str,
) -> Result<Option<String>, String> {
    let path_enc = odata_encode(file_path);
    let url = format!(
        "{api_url}/tdata/Files?$filter=Path%20eq%20'{path_enc}'%20and%20WorkspaceId%20eq%20'{ws_id}'"
    );
    let resp = http_get(ctx, &url, tenant)?;
    let items = resp.get("value").and_then(|v| v.as_array());
    Ok(items
        .and_then(|arr| arr.first())
        .and_then(|v| extract_id(v)))
}

/// Resolve or create the directory hierarchy for a given dir_path.
/// Returns the leaf directory Id.
fn ensure_dirs(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    dir_path: &str,
) -> Result<String, String> {
    let segments = path::dir_segments(dir_path);

    // Root directory: Name="/", Path="/", ParentId=null
    let mut parent_id = match find_directory(ctx, api_url, tenant, ws_id, "/", None)? {
        Some(id) => id,
        None => {
            let resp = http_post(
                ctx,
                &format!("{api_url}/tdata/Directories"),
                tenant,
                &json!({"Name": "/", "Path": "/", "WorkspaceId": ws_id}),
            )?;
            extract_id(&resp).ok_or("mkdir: root dir created but no Id returned")?
        }
    };

    if segments.is_empty() {
        return Ok(parent_id);
    }

    // Walk each segment, creating missing directories.
    let mut current_path = String::new();
    for seg in &segments {
        current_path = format!("{current_path}/{seg}");

        match find_directory(ctx, api_url, tenant, ws_id, seg, Some(&parent_id))? {
            Some(id) => {
                parent_id = id;
            }
            None => {
                // Create directory.
                let resp = http_post(
                    ctx,
                    &format!("{api_url}/tdata/Directories"),
                    tenant,
                    &json!({
                        "Name": seg,
                        "Path": current_path,
                        "ParentId": parent_id,
                        "WorkspaceId": ws_id,
                    }),
                )?;
                let new_id = extract_id(&resp).ok_or("mkdir: dir created but no Id returned")?;

                // AddChild on parent.
                if let Err(e) = http_post(
                    ctx,
                    &format!("{api_url}/tdata/Directories('{parent_id}')/Temper.AddChild"),
                    tenant,
                    &json!({}),
                ) {
                    ctx.log(
                        "warn",
                        &format!("mkdir: AddChild failed on {parent_id}: {e}"),
                    );
                }

                parent_id = new_id;
            }
        }
    }

    Ok(parent_id)
}

// ---------------------------------------------------------------------------
// FUSE-mapped operations
// ---------------------------------------------------------------------------

/// `mkdir -p` — create directory hierarchy at path.
pub fn mkdir(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<(), String> {
    let normalized = path::normalize(raw_path)?;
    let dir_id = ensure_dirs(ctx, api_url, tenant, ws_id, &normalized)?;

    set_success_result(
        "DirCreated",
        &json!({"last_dir_id": dir_id, "last_dir_path": normalized}),
    );
    Ok(())
}

/// `creat` — create file entity at path. Returns existing file if path already taken.
pub fn create_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
    mime_type: Option<&str>,
) -> Result<(), String> {
    let normalized = path::normalize(raw_path)?;
    let (dir_path, filename) = path::parse(&normalized)?;
    let mime = mime_type.unwrap_or_else(|| path::mime_from_ext(&normalized));

    // Ensure parent directory exists.
    let dir_id = ensure_dirs(ctx, api_url, tenant, ws_id, dir_path)?;

    // Check if file already exists at this path.
    if let Some(existing_id) = find_file(ctx, api_url, tenant, ws_id, &normalized)? {
        set_success_result(
            "FileCreated",
            &json!({"last_file_id": existing_id, "last_file_path": normalized}),
        );
        return Ok(());
    }

    // Create File entity.
    let resp = http_post(
        ctx,
        &format!("{api_url}/tdata/Files"),
        tenant,
        &json!({
            "Name": filename,
            "Path": normalized,
            "DirectoryId": dir_id,
            "WorkspaceId": ws_id,
            "MimeType": mime,
        }),
    )?;
    let file_id = extract_id(&resp).ok_or("create_file: File created but no Id returned")?;

    // AddChild on parent directory.
    if let Err(e) = http_post(
        ctx,
        &format!("{api_url}/tdata/Directories('{dir_id}')/Temper.AddChild"),
        tenant,
        &json!({}),
    ) {
        ctx.log(
            "warn",
            &format!("create_file: AddChild failed on dir {dir_id}: {e}"),
        );
    }

    set_success_result(
        "FileCreated",
        &json!({"last_file_id": file_id, "last_file_path": normalized}),
    );
    Ok(())
}

/// `stat` — resolve path to file entity.
pub fn resolve_path(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<(), String> {
    let normalized = path::normalize(raw_path)?;
    let file_id = find_file(ctx, api_url, tenant, ws_id, &normalized)?
        .ok_or_else(|| format!("file not found: {normalized}"))?;

    set_success_result(
        "PathResolved",
        &json!({"last_file_id": file_id, "last_file_path": normalized}),
    );
    Ok(())
}

/// `readdir` — list directory contents.
pub fn list_dir(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<(), String> {
    let normalized = path::normalize(raw_path)?;

    // Resolve directory by walking path segments.
    let dir_id = ensure_dirs(ctx, api_url, tenant, ws_id, &normalized)?;

    // List child directories.
    let dirs_url = format!(
        "{api_url}/tdata/Directories?$filter=ParentId%20eq%20'{dir_id}'%20and%20WorkspaceId%20eq%20'{ws_id}'"
    );
    let dirs_resp = http_get(ctx, &dirs_url, tenant)?;
    let dirs = dirs_resp
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // List child files.
    let files_url = format!(
        "{api_url}/tdata/Files?$filter=DirectoryId%20eq%20'{dir_id}'%20and%20WorkspaceId%20eq%20'{ws_id}'"
    );
    let files_resp = http_get(ctx, &files_url, tenant)?;
    let files = files_resp
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let listing = json!({
        "path": normalized,
        "directories": dirs.iter().map(|d| {
            json!({
                "name": d.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                "id": extract_id(d).unwrap_or_default(),
            })
        }).collect::<Vec<_>>(),
        "files": files.iter().map(|f| {
            json!({
                "name": f.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                "id": extract_id(f).unwrap_or_default(),
                "mime_type": f.get("MimeType").and_then(|v| v.as_str()).unwrap_or(""),
                "size_bytes": f.get("SizeBytes").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        }).collect::<Vec<_>>(),
    });

    set_success_result(
        "DirListed",
        &json!({"last_listing": serde_json::to_string(&listing).unwrap_or_else(|_| "{}".into())}),
    );
    Ok(())
}

/// `unlink` — archive file at path.
pub fn delete_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_path: &str,
) -> Result<(), String> {
    let normalized = path::normalize(raw_path)?;
    let file_id = find_file(ctx, api_url, tenant, ws_id, &normalized)?
        .ok_or_else(|| format!("file not found: {normalized}"))?;

    // Archive the file.
    http_post(
        ctx,
        &format!("{api_url}/tdata/Files('{file_id}')/Temper.FileArchive"),
        tenant,
        &json!({}),
    )?;

    // Get file metadata to find parent directory.
    let file_resp = http_get(ctx, &format!("{api_url}/tdata/Files('{file_id}')"), tenant)?;
    let dir_id = file_resp
        .get("DirectoryId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // RemoveChild on parent directory.
    if !dir_id.is_empty() {
        if let Err(e) = http_post(
            ctx,
            &format!("{api_url}/tdata/Directories('{dir_id}')/Temper.RemoveChild"),
            tenant,
            &json!({}),
        ) {
            ctx.log(
                "warn",
                &format!("delete_file: RemoveChild failed on dir {dir_id}: {e}"),
            );
        }
    }

    set_success_result(
        "FileDeleted",
        &json!({"last_file_id": file_id, "last_file_path": normalized}),
    );
    Ok(())
}

/// `rename` — move/rename file from old path to new path.
pub fn rename(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    ws_id: &str,
    raw_old_path: &str,
    raw_new_path: &str,
) -> Result<(), String> {
    let old_normalized = path::normalize(raw_old_path)?;
    let new_normalized = path::normalize(raw_new_path)?;

    // Find the existing file.
    let file_id = find_file(ctx, api_url, tenant, ws_id, &old_normalized)?
        .ok_or_else(|| format!("file not found: {old_normalized}"))?;

    // Get current file metadata for the old directory.
    let file_resp = http_get(ctx, &format!("{api_url}/tdata/Files('{file_id}')"), tenant)?;
    let old_dir_id = file_resp
        .get("DirectoryId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Parse new path into directory + filename.
    let (new_dir_path, new_filename) = path::parse(&new_normalized)?;

    // Ensure the new parent directory exists.
    let new_dir_id = ensure_dirs(ctx, api_url, tenant, ws_id, new_dir_path)?;

    // PATCH the File entity: update Name, Path, DirectoryId.
    let headers = runtime_headers(tenant);
    let patch_body = json!({
        "Name": new_filename,
        "Path": new_normalized,
        "DirectoryId": new_dir_id,
    });
    let resp = ctx.http_call(
        "PATCH",
        &format!("{api_url}/tdata/Files('{file_id}')"),
        &headers,
        &patch_body.to_string(),
    )?;
    if resp.status >= 400 {
        return Err(format!(
            "rename: PATCH failed (HTTP {}): {}",
            resp.status, resp.body
        ));
    }

    // Update directory child counts if the directory changed.
    if old_dir_id != new_dir_id {
        // RemoveChild on old parent.
        if !old_dir_id.is_empty() {
            if let Err(e) = http_post(
                ctx,
                &format!("{api_url}/tdata/Directories('{old_dir_id}')/Temper.RemoveChild"),
                tenant,
                &json!({}),
            ) {
                ctx.log(
                    "warn",
                    &format!("rename: RemoveChild failed on dir {old_dir_id}: {e}"),
                );
            }
        }

        // AddChild on new parent.
        if let Err(e) = http_post(
            ctx,
            &format!("{api_url}/tdata/Directories('{new_dir_id}')/Temper.AddChild"),
            tenant,
            &json!({}),
        ) {
            ctx.log(
                "warn",
                &format!("rename: AddChild failed on dir {new_dir_id}: {e}"),
            );
        }
    }

    set_success_result(
        "FileRenamed",
        &json!({"last_file_id": file_id, "last_file_path": new_normalized}),
    );
    Ok(())
}
