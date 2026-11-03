//! Skill Installer — WASM integration for SkillInstall entities.
//!
//! The integration is intentionally small and entity-first: a SkillInstall
//! transitions to Installing, this module fetches/validates the skill package,
//! writes the runtime SKILL.md into TemperFS, creates provenance/binding
//! entities, and self-reports with InstallComplete or InstallFailed.

use sha2::{Digest, Sha256};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::bounded_reads;

const APP_DOCS_WORKSPACE_ID: &str = "os-app-docs";
const APP_DOCS_WORKSPACE_NAME: &str = "apps";
const APP_DOCS_QUOTA_BYTES: i64 = 1_099_511_627_776;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLocation {
    source_type: String,
    source_url: String,
    source_ref: String,
    source_subpath: String,
    fetch_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallTarget {
    scope_type: String,
    scope_id: String,
    skill_slug: String,
    skill_path: String,
}

#[derive(Debug, Clone)]
struct InstalledFile {
    file_id: String,
    skill_path: String,
    workspace_id: String,
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    match run_impl() {
        Ok(()) => 0,
        Err(error) => {
            set_error_result(&error);
            1
        }
    }
}

fn run_impl() -> Result<(), String> {
    let ctx = Context::from_host()?;
    ctx.log("info", "skill_installer: starting");

    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let temper_api_url = resolve_temper_api_url(&ctx, &fields);
    let tenant = ctx.tenant.clone();
    let install_id = ctx
        .entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(&ctx.entity_id)
        .to_string();

    let source = resolve_source_location(&fields)?;
    let content = load_skill_content(&ctx, &source, &fields)?;
    let frontmatter = extract_skill_frontmatter(&content);
    let requested_name = field_str(&fields, &["requested_skill_name", "RequestedSkillName"]);
    let skill_name = if !frontmatter.name.is_empty() {
        frontmatter.name.clone()
    } else if !requested_name.is_empty() {
        requested_name
    } else {
        skill_name_from_source(&source)
    };
    let skill_slug = skill_slug(&skill_name);
    let target = InstallTarget {
        scope_type: normalize_scope_type(&field_str(
            &fields,
            &["target_scope_type", "TargetScopeType"],
        ))?,
        scope_id: field_str(&fields, &["target_scope_id", "TargetScopeId"]),
        skill_slug: skill_slug.clone(),
        skill_path: String::new(),
    };
    let target = InstallTarget {
        skill_path: skill_temperfs_path(&target.scope_type, &target.scope_id, &skill_slug)?,
        ..target
    };

    let content_digest = content_digest(&content);
    let companion_manifest = field_str(&fields, &["companion_manifest", "CompanionManifest"]);
    let companions = parse_companion_manifest(&companion_manifest)?;
    let installed_file = install_skill_file(
        &ctx,
        &temper_api_url,
        &tenant,
        &target,
        &content,
        &companions,
    )?;

    let package_id = register_package(
        &ctx,
        &temper_api_url,
        &tenant,
        &install_id,
        &skill_name,
        &frontmatter.description,
        &source,
        &content_digest,
        &companion_manifest,
    )?;

    let installed_at = Context::get_time_millis().to_string();
    let binding_id = activate_binding(
        &ctx,
        &temper_api_url,
        &tenant,
        &package_id,
        &install_id,
        &skill_name,
        &target,
        &installed_file,
        &content_digest,
        &source.source_url,
        &installed_at,
    )?;

    set_success_result(
        "InstallComplete",
        &json!({
            "package_id": package_id,
            "binding_id": binding_id,
            "skill_path": installed_file.skill_path,
            "workspace_id": installed_file.workspace_id,
            "file_id": installed_file.file_id,
            "content_digest": content_digest,
            "installed_skill_name": skill_name,
            "installed_at": installed_at,
        }),
    );

    ctx.log(
        "info",
        &format!(
            "skill_installer: installed {} at {}",
            skill_slug, installed_file.skill_path
        ),
    );
    Ok(())
}

fn resolve_temper_api_url(ctx: &Context, fields: &Value) -> String {
    let from_fields = field_str(fields, &["temper_api_url", "TemperApiUrl"]);
    if !from_fields.is_empty() {
        return from_fields;
    }
    ctx.config
        .get("temper_api_url")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn resolve_source_location(fields: &Value) -> Result<SourceLocation, String> {
    let inline_content = field_str(fields, &["inline_content", "InlineContent"]);
    let source_url = field_str(fields, &["source_url", "SourceUrl"]);
    let requested_type = field_str(fields, &["source_type", "SourceType"]);
    let source_ref = field_str(fields, &["source_ref", "SourceRef"]);
    let source_subpath = field_str(fields, &["source_subpath", "SourceSubpath"]);

    let source_type = if !requested_type.is_empty() {
        requested_type
    } else if !inline_content.is_empty() {
        "inline".to_string()
    } else if source_url.contains("github.com/") && source_url.contains("/tree/") {
        "github_tree".to_string()
    } else {
        "raw_url".to_string()
    };

    let fetch_url = match source_type.as_str() {
        "inline" => String::new(),
        "github_tree" => github_tree_skill_raw_url(&source_url, &source_ref, &source_subpath)?,
        "raw_url" => {
            if source_url.is_empty() {
                return Err("source_url is required for raw_url skill installs".to_string());
            }
            source_url.clone()
        }
        other => {
            return Err(format!(
                "unsupported source_type '{other}'. Expected inline, raw_url, or github_tree"
            ));
        }
    };

    let (resolved_ref, resolved_subpath) = if source_type == "github_tree" {
        github_tree_ref_and_subpath(&source_url, &source_ref, &source_subpath)?
    } else {
        (source_ref, source_subpath)
    };

    Ok(SourceLocation {
        source_type,
        source_url,
        source_ref: resolved_ref,
        source_subpath: resolved_subpath,
        fetch_url,
    })
}

fn load_skill_content(
    ctx: &Context,
    source: &SourceLocation,
    fields: &Value,
) -> Result<String, String> {
    if source.source_type == "inline" {
        let inline = field_str(fields, &["inline_content", "InlineContent"]);
        if inline.trim().is_empty() {
            return Err("inline skill installs require inline_content".to_string());
        }
        return Ok(inline);
    }

    let headers: Vec<(String, String)> = Vec::new();
    let resp = ctx.http_call("GET", &source.fetch_url, &headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "failed to fetch skill source {}: HTTP {} {}",
            source.fetch_url,
            resp.status,
            truncate(&resp.body, 240)
        ));
    }
    if resp.body.trim().is_empty() {
        return Err(format!(
            "skill source {} returned empty content",
            source.fetch_url
        ));
    }
    Ok(resp.body)
}

pub fn github_tree_skill_raw_url(
    source_url: &str,
    source_ref: &str,
    source_subpath: &str,
) -> Result<String, String> {
    let (owner, repo, resolved_ref, resolved_subpath) =
        parse_github_tree(source_url, source_ref, source_subpath)?;
    let main_path = if resolved_subpath.ends_with("/SKILL.md") || resolved_subpath == "SKILL.md" {
        resolved_subpath
    } else if resolved_subpath.is_empty() {
        "SKILL.md".to_string()
    } else {
        format!("{resolved_subpath}/SKILL.md")
    };
    Ok(format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{resolved_ref}/{main_path}"
    ))
}

fn github_tree_ref_and_subpath(
    source_url: &str,
    source_ref: &str,
    source_subpath: &str,
) -> Result<(String, String), String> {
    let (_, _, resolved_ref, resolved_subpath) =
        parse_github_tree(source_url, source_ref, source_subpath)?;
    Ok((resolved_ref, resolved_subpath))
}

fn parse_github_tree(
    source_url: &str,
    source_ref: &str,
    source_subpath: &str,
) -> Result<(String, String, String, String), String> {
    let without_host = source_url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| source_url.trim().strip_prefix("http://github.com/"))
        .ok_or_else(|| format!("github_tree source_url must be a GitHub tree URL: {source_url}"))?;
    let mut parts = without_host.splitn(3, '/');
    let owner = parts.next().unwrap_or("").trim();
    let repo = parts.next().unwrap_or("").trim().trim_end_matches(".git");
    let rest = parts.next().unwrap_or("");
    if owner.is_empty() || repo.is_empty() {
        return Err(format!(
            "github_tree source_url is missing owner or repo: {source_url}"
        ));
    }

    if !source_ref.trim().is_empty() {
        return Ok((
            owner.to_string(),
            repo.to_string(),
            source_ref.trim().to_string(),
            source_subpath.trim_matches('/').to_string(),
        ));
    }

    let tree_rest = rest.strip_prefix("tree/").ok_or_else(|| {
        format!("github_tree source_url must include /tree/<ref>/<path>: {source_url}")
    })?;
    let mut tree_parts = tree_rest.splitn(2, '/');
    let resolved_ref = tree_parts.next().unwrap_or("").trim();
    let resolved_subpath = tree_parts.next().unwrap_or("").trim_matches('/');
    if resolved_ref.is_empty() {
        return Err(format!(
            "github_tree source_url is missing ref: {source_url}"
        ));
    }
    Ok((
        owner.to_string(),
        repo.to_string(),
        resolved_ref.to_string(),
        if source_subpath.trim().is_empty() {
            resolved_subpath.to_string()
        } else {
            source_subpath.trim_matches('/').to_string()
        },
    ))
}

pub fn extract_skill_frontmatter(content: &str) -> SkillFrontmatter {
    let frontmatter = frontmatter_block(content).unwrap_or_default();
    SkillFrontmatter {
        name: frontmatter_value(frontmatter, "name"),
        description: frontmatter_value(frontmatter, "description"),
    }
}

fn frontmatter_block(content: &str) -> Option<&str> {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return Some(&rest[..end]);
        }
        if let Some(end) = rest.find("---") {
            return Some(&rest[..end]);
        }
    }
    if let Some(rest) = content.strip_prefix("+++") {
        if let Some(end) = rest.find("\n+++") {
            return Some(&rest[..end]);
        }
        if let Some(end) = rest.find("+++") {
            return Some(&rest[..end]);
        }
    }
    None
}

fn frontmatter_value(frontmatter: &str, key: &str) -> String {
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&format!("{key}:")) {
            return clean_frontmatter_value(trim_frontmatter_value_at_next_key(value, key));
        }
        if let Some((candidate, value)) = trimmed.split_once('=')
            && candidate.trim() == key
        {
            return clean_frontmatter_value(value);
        }
    }

    let marker = format!("{key}:");
    let Some(start) = frontmatter.find(&marker) else {
        return String::new();
    };
    let value_start = start + marker.len();
    let mut value_end = frontmatter.len();
    for next_key in [" name:", " description:", " license:", " publisher:"] {
        if next_key.trim_start() == marker {
            continue;
        }
        if let Some(relative) = frontmatter[value_start..].find(next_key) {
            value_end = value_end.min(value_start + relative);
        }
    }
    clean_frontmatter_value(&frontmatter[value_start..value_end])
}

fn trim_frontmatter_value_at_next_key<'a>(value: &'a str, current_key: &str) -> &'a str {
    let mut value_end = value.len();
    for next_key in [" name:", " description:", " license:", " publisher:"] {
        let next_name = next_key.trim().trim_end_matches(':');
        if next_name == current_key {
            continue;
        }
        if let Some(relative) = value.find(next_key) {
            value_end = value_end.min(relative);
        }
    }
    &value[..value_end]
}

fn clean_frontmatter_value(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

pub fn skill_temperfs_path(scope_type: &str, scope_id: &str, slug: &str) -> Result<String, String> {
    match scope_type {
        "system" => Ok(format!("/system/skills/{slug}/SKILL.md")),
        "project" => {
            if scope_id.is_empty() {
                return Err("target_scope_id is required for project-scoped skill installs".into());
            }
            Ok(format!("/projects/{scope_id}/skills/{slug}/SKILL.md"))
        }
        "agent" => {
            if scope_id.is_empty() {
                return Err("target_scope_id is required for agent-scoped skill installs".into());
            }
            Ok(format!("/agents/{scope_id}/skills/{slug}/SKILL.md"))
        }
        other => Err(format!(
            "unsupported target_scope_type '{other}'. Expected system, project, or agent"
        )),
    }
}

fn install_skill_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    target: &InstallTarget,
    content: &str,
    companions: &[(String, String)],
) -> Result<InstalledFile, String> {
    ensure_app_docs_workspace(ctx, api_url, tenant)?;
    let dir_path = parent_path(&target.skill_path)?;
    let dir_id = ensure_dirs(ctx, api_url, tenant, APP_DOCS_WORKSPACE_ID, &dir_path)?;
    let file_id = ensure_file(
        ctx,
        api_url,
        tenant,
        APP_DOCS_WORKSPACE_ID,
        &target.skill_path,
        &dir_id,
        "SKILL.md",
        "text/markdown",
    )?;
    write_file_value(ctx, api_url, tenant, &file_id, content, "text/markdown")?;

    for (relative_path, companion_content) in companions {
        let clean_relative = relative_path.trim_matches('/');
        if clean_relative.is_empty() || clean_relative == "SKILL.md" {
            continue;
        }
        let companion_path = format!("{dir_path}/{clean_relative}");
        let companion_dir = parent_path(&companion_path)?;
        let companion_dir_id =
            ensure_dirs(ctx, api_url, tenant, APP_DOCS_WORKSPACE_ID, &companion_dir)?;
        let name = clean_relative.rsplit('/').next().unwrap_or(clean_relative);
        let companion_file_id = ensure_file(
            ctx,
            api_url,
            tenant,
            APP_DOCS_WORKSPACE_ID,
            &companion_path,
            &companion_dir_id,
            name,
            mime_type_for_path(clean_relative),
        )?;
        write_file_value(
            ctx,
            api_url,
            tenant,
            &companion_file_id,
            companion_content,
            mime_type_for_path(clean_relative),
        )?;
    }

    Ok(InstalledFile {
        file_id,
        skill_path: target.skill_path.clone(),
        workspace_id: APP_DOCS_WORKSPACE_ID.to_string(),
    })
}

fn ensure_app_docs_workspace(ctx: &Context, api_url: &str, tenant: &str) -> Result<(), String> {
    let headers = api_headers(ctx, tenant, Some("application/json"));
    let url = format!("{api_url}/tdata/Workspaces('{APP_DOCS_WORKSPACE_ID}')");
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status == 200 {
        return Ok(());
    }
    if resp.status != 404 {
        ctx.log(
            "warn",
            &format!(
                "skill_installer: workspace lookup returned HTTP {}: {}",
                resp.status,
                truncate(&resp.body, 160)
            ),
        );
    }

    let body = json!({
        "Id": APP_DOCS_WORKSPACE_ID,
        "Name": APP_DOCS_WORKSPACE_NAME,
        "QuotaLimit": APP_DOCS_QUOTA_BYTES,
    });
    let create_url = format!("{api_url}/tdata/Workspaces");
    let create = ctx.http_call("POST", &create_url, &headers, &body.to_string())?;
    if (200..300).contains(&create.status) || create.status == 409 {
        return Ok(());
    }
    Err(format!(
        "failed to ensure app docs workspace: HTTP {} {}",
        create.status,
        truncate(&create.body, 240)
    ))
}

fn ensure_dirs(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    dir_path: &str,
) -> Result<String, String> {
    let normalized = normalize_path(dir_path)?;
    if let Some(existing) = find_directory(ctx, api_url, tenant, workspace_id, &normalized)? {
        return Ok(existing);
    }

    let mut parent_id = match find_directory(ctx, api_url, tenant, workspace_id, "/")? {
        Some(id) => id,
        None => create_directory(ctx, api_url, tenant, workspace_id, "/", "/", None)?,
    };
    if normalized == "/" {
        return Ok(parent_id);
    }

    let mut current = String::new();
    for segment in normalized.trim_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        current.push('/');
        current.push_str(segment);
        if let Some(existing) = find_directory(ctx, api_url, tenant, workspace_id, &current)? {
            parent_id = existing;
            continue;
        }
        let new_id = create_directory(
            ctx,
            api_url,
            tenant,
            workspace_id,
            segment,
            &current,
            Some(&parent_id),
        )?;
        post_action_best_effort(
            ctx,
            api_url,
            tenant,
            "Directories",
            &parent_id,
            "AddChild",
            &json!({}),
        );
        parent_id = new_id;
    }

    Ok(parent_id)
}

#[allow(clippy::too_many_arguments)]
fn ensure_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_path: &str,
    directory_id: &str,
    name: &str,
    mime_type: &str,
) -> Result<String, String> {
    if let Some(existing) = find_file(ctx, api_url, tenant, workspace_id, file_path)? {
        return Ok(existing);
    }
    let headers = api_headers(ctx, tenant, Some("application/json"));
    let body = json!({
        "Name": name,
        "Path": file_path,
        "DirectoryId": directory_id,
        "WorkspaceId": workspace_id,
        "MimeType": mime_type,
    });
    let url = format!("{api_url}/tdata/Files");
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "failed to create File at {file_path}: HTTP {} {}",
            resp.status,
            truncate(&resp.body, 240)
        ));
    }
    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|err| format!("failed to parse File create response: {err}"))?;
    let file_id = extract_id(&parsed).ok_or("File create response did not include entity_id")?;
    post_action_best_effort(
        ctx,
        api_url,
        tenant,
        "Directories",
        directory_id,
        "AddChild",
        &json!({}),
    );
    post_action_best_effort(
        ctx,
        api_url,
        tenant,
        "Workspaces",
        workspace_id,
        "IncrementFileCount",
        &json!({}),
    );
    Ok(file_id)
}

fn write_file_value(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    file_id: &str,
    content: &str,
    mime_type: &str,
) -> Result<(), String> {
    let headers = api_headers(ctx, tenant, Some(mime_type));
    let url = format!("{api_url}/tdata/Files('{file_id}')/$value");
    let resp = ctx.http_call("PUT", &url, &headers, content)?;
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!(
            "failed to write File('{file_id}') content: HTTP {} {}",
            resp.status,
            truncate(&resp.body, 240)
        ))
    }
}

fn find_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let filter = format!(
        "Path eq '{}' and WorkspaceId eq '{}'",
        bounded_reads::odata_escape(path),
        bounded_reads::odata_escape(workspace_id)
    );
    first_entity_id(ctx, api_url, tenant, "Directories", &filter)
}

fn find_file(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let filter = format!(
        "Path eq '{}' and WorkspaceId eq '{}'",
        bounded_reads::odata_escape(path),
        bounded_reads::odata_escape(workspace_id)
    );
    first_entity_id(ctx, api_url, tenant, "Files", &filter)
}

fn create_directory(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    workspace_id: &str,
    name: &str,
    path: &str,
    parent_id: Option<&str>,
) -> Result<String, String> {
    let headers = api_headers(ctx, tenant, Some("application/json"));
    let body = json!({
        "Name": name,
        "Path": path,
        "ParentId": parent_id,
        "WorkspaceId": workspace_id,
    });
    let url = format!("{api_url}/tdata/Directories");
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "failed to create Directory at {path}: HTTP {} {}",
            resp.status,
            truncate(&resp.body, 240)
        ));
    }
    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|err| format!("failed to parse Directory create response: {err}"))?;
    extract_id(&parsed).ok_or_else(|| "Directory create response did not include entity_id".into())
}

#[allow(clippy::too_many_arguments)]
fn register_package(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    install_id: &str,
    skill_name: &str,
    description: &str,
    source: &SourceLocation,
    digest: &str,
    companion_manifest: &str,
) -> Result<String, String> {
    let filter = format!(
        "ContentDigest eq '{}' and SkillName eq '{}'",
        bounded_reads::odata_escape(digest),
        bounded_reads::odata_escape(skill_name)
    );
    let package_id = match first_entity_id(ctx, api_url, tenant, "SkillPackages", &filter)? {
        Some(id) => id,
        None => {
            let body = json!({
                "PackageName": skill_name,
                "SkillName": skill_name,
                "Description": description,
                "SourceType": source.source_type,
                "SourceUrl": source.source_url,
                "SourceRef": source.source_ref,
                "SourceSubpath": source.source_subpath,
                "ContentDigest": digest,
                "MainFilePath": source.fetch_url,
                "CompanionManifest": companion_manifest,
                "Publisher": publisher_from_source_url(&source.source_url),
                "License": "",
                "LastInstallId": install_id,
            });
            create_entity(ctx, api_url, tenant, "SkillPackages", &body)?
        }
    };

    post_action(
        ctx,
        api_url,
        tenant,
        "SkillPackages",
        &package_id,
        "Register",
        &json!({
            "package_name": skill_name,
            "skill_name": skill_name,
            "description": description,
            "source_type": source.source_type,
            "source_url": source.source_url,
            "source_ref": source.source_ref,
            "source_subpath": source.source_subpath,
            "content_digest": digest,
            "main_file_path": source.fetch_url,
            "companion_manifest": companion_manifest,
            "publisher": publisher_from_source_url(&source.source_url),
            "license": "",
            "last_install_id": install_id,
        }),
    )?;

    Ok(package_id)
}

#[allow(clippy::too_many_arguments)]
fn activate_binding(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    package_id: &str,
    install_id: &str,
    skill_name: &str,
    target: &InstallTarget,
    installed_file: &InstalledFile,
    digest: &str,
    source_url: &str,
    activated_at: &str,
) -> Result<String, String> {
    let body = json!({
        "PackageId": package_id,
        "InstallId": install_id,
        "SkillName": skill_name,
        "ScopeType": target.scope_type,
        "ScopeId": target.scope_id,
        "SkillPath": installed_file.skill_path,
        "WorkspaceId": installed_file.workspace_id,
        "FileId": installed_file.file_id,
        "ContentDigest": digest,
        "SourceUrl": source_url,
        "ActivatedAt": activated_at,
    });
    let binding_id = create_entity(ctx, api_url, tenant, "SkillBindings", &body)?;
    post_action(
        ctx,
        api_url,
        tenant,
        "SkillBindings",
        &binding_id,
        "Activate",
        &json!({
            "package_id": package_id,
            "install_id": install_id,
            "skill_name": skill_name,
            "scope_type": target.scope_type,
            "scope_id": target.scope_id,
            "skill_path": installed_file.skill_path,
            "workspace_id": installed_file.workspace_id,
            "file_id": installed_file.file_id,
            "content_digest": digest,
            "source_url": source_url,
            "activated_at": activated_at,
        }),
    )?;
    Ok(binding_id)
}

fn create_entity(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    entity_set: &str,
    body: &Value,
) -> Result<String, String> {
    let headers = api_headers(ctx, tenant, Some("application/json"));
    let url = format!("{api_url}/tdata/{entity_set}");
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "failed to create {entity_set}: HTTP {} {}",
            resp.status,
            truncate(&resp.body, 240)
        ));
    }
    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|err| format!("failed to parse {entity_set} create response: {err}"))?;
    extract_id(&parsed).ok_or_else(|| format!("{entity_set} create response did not include id"))
}

fn first_entity_id(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    entity_set: &str,
    filter: &str,
) -> Result<Option<String>, String> {
    let headers = api_headers(ctx, tenant, Some("application/json"));
    bounded_reads::find_first_non_archived_entity_id(
        ctx,
        api_url,
        &headers,
        entity_set,
        filter,
        bounded_reads::POINT_LOOKUP_TOP,
        "skill_installer: bounded entity lookup",
    )
}

fn post_action(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    entity_set: &str,
    entity_id: &str,
    action: &str,
    body: &Value,
) -> Result<(), String> {
    let headers = api_headers(ctx, tenant, Some("application/json"));
    let url = format!("{api_url}/tdata/{entity_set}('{entity_id}')/Temper.{action}");
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!(
            "{entity_set}('{entity_id}').{action} failed: HTTP {} {}",
            resp.status,
            truncate(&resp.body, 240)
        ))
    }
}

fn post_action_best_effort(
    ctx: &Context,
    api_url: &str,
    tenant: &str,
    entity_set: &str,
    entity_id: &str,
    action: &str,
    body: &Value,
) {
    if let Err(error) = post_action(ctx, api_url, tenant, entity_set, entity_id, action, body) {
        ctx.log(
            "warn",
            &format!("skill_installer: best-effort action failed: {error}"),
        );
    }
}

fn api_headers(ctx: &Context, tenant: &str, content_type: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("x-tenant-id".to_string(), tenant.to_string()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];
    if let Some(content_type) = content_type {
        headers.push(("content-type".to_string(), content_type.to_string()));
        headers.push(("accept".to_string(), "application/json".to_string()));
    }
    if let Some(key) = ctx
        .config
        .get("temper_api_key")
        .filter(|key| !key.is_empty())
    {
        headers.push(("authorization".to_string(), format!("Bearer {key}")));
    }
    headers
}

fn field_str(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn extract_id(value: &Value) -> Option<String> {
    value
        .get("entity_id")
        .or_else(|| value.get("Id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_scope_type(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err("target_scope_type is required".into());
    }
    match value.as_str() {
        "system" | "project" | "agent" => Ok(value),
        _ => Err(format!(
            "unsupported target_scope_type '{raw}'. Expected system, project, or agent"
        )),
    }
}

fn skill_name_from_source(source: &SourceLocation) -> String {
    source
        .source_subpath
        .trim_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("skill")
        .to_string()
}

pub fn skill_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "skill".to_string()
    } else {
        slug
    }
}

fn content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn parse_companion_manifest(raw: &str) -> Result<Vec<(String, String)>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Value = serde_json::from_str(trimmed)
        .map_err(|err| format!("companion_manifest must be JSON: {err}"))?;
    let Some(object) = parsed.as_object() else {
        return Err("companion_manifest must be a JSON object of path to content".into());
    };
    let mut companions = Vec::new();
    for (path, value) in object {
        let content = if let Some(text) = value.as_str() {
            text.to_string()
        } else if let Some(text) = value.get("content").and_then(Value::as_str) {
            text.to_string()
        } else {
            return Err(format!(
                "companion_manifest entry '{path}' must be a string or object with content"
            ));
        };
        companions.push((path.clone(), content));
    }
    companions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(companions)
}

fn parent_path(path: &str) -> Result<String, String> {
    let normalized = normalize_path(path)?;
    let Some((parent, _name)) = normalized.rsplit_once('/') else {
        return Err(format!("invalid path: {path}"));
    };
    if parent.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(parent.to_string())
    }
}

fn normalize_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path cannot be empty".into());
    }
    let mut normalized = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

fn mime_type_for_path(path: &str) -> &'static str {
    if path.ends_with(".md") {
        "text/markdown"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".toml") {
        "application/toml"
    } else {
        "text/plain"
    }
}

fn publisher_from_source_url(source_url: &str) -> String {
    source_url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| source_url.trim().strip_prefix("http://github.com/"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string()
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        let mut end = max;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &value[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parser_handles_inline_skill_header() {
        let fm = extract_skill_frontmatter(
            "--- name: design-taste-frontend description: Senior UI/UX Engineer. --- # Body",
        );
        assert_eq!(fm.name, "design-taste-frontend");
        assert_eq!(fm.description, "Senior UI/UX Engineer.");
    }
}
