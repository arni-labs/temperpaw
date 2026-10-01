//! decompose_endpoint — split an endpoint's imagined future into separable
//! claims (ADR-004).
//!
//! Three phases, keyed on the triggering action (the consistency_gate
//! pattern):
//!
//! - Endpoint.SubmitForRepair: fetch the bundle, inline it, spawn ONE
//!   decomposer session. The decomposer self-reports DecompositionComplete
//!   with 3 claims.
//! - Endpoint.DecompositionComplete: create one Claim entity per extracted
//!   claim (original_text frozen here), then dispatch ClaimsAttached with
//!   the created ids.
//! - Endpoint.ClaimsAttached / Endpoint.SpawnNextBridge: dispatch
//!   Claim.SubmitForBridge for the next chunk of claims, then self-loop
//!   via SpawnNextBridge until every claim is bridging. Chunked so the
//!   repairer-session fan-out stays under session admission caps
//!   (ADR-0005: no Rust loops over sessions).
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

/// Claims dispatched per fan-out tick. Each SubmitForBridge spawns one
/// repairer session, so the instantaneous session-create burst per tick is
/// CHUNK, well under the admission cap of ~10.
const CHUNK: usize = 2;

/// Hard stop for the self-loop: ceil(MAX_CLAIMS / CHUNK) + 1.
const MAX_CHECKS: usize = 3;

const MIN_CLAIMS: usize = 3;
const MAX_CLAIMS: usize = 3;

const DECOMPOSER_TOOLS: &str = "temper_get,temper_list,temper_action";

/// Read a string field from an OData row. List/GET rows nest snake_case
/// values under "fields" with lowercase status/entity_id at the top level;
/// some surfaces serve PascalCase top-level properties. Check both.
fn row_str<'a>(row: &'a Value, pascal: &str) -> &'a str {
    fn snake(p: &str) -> String {
        let mut s = String::new();
        for (i, ch) in p.chars().enumerate() {
            if ch.is_uppercase() {
                if i > 0 {
                    s.push('_');
                }
                s.extend(ch.to_lowercase());
            } else {
                s.push(ch);
            }
        }
        s
    }
    let s = snake(pascal);
    if let Some(v) = row
        .get("fields")
        .and_then(|f| f.get(s.as_str()))
        .and_then(|v| v.as_str())
    {
        return v;
    }
    if let Some(v) = row.get(pascal).and_then(|v| v.as_str()) {
        return v;
    }
    // List rows also carry lowercase top-level keys (status, entity_id).
    row.get(s.as_str()).and_then(|v| v.as_str()).unwrap_or("")
}

/// Truncate inlined bundle content at a char boundary; claims must never be
/// extracted from silently-missing text, so the truncation is loud.
fn inline_file(raw: &str) -> String {
    const CAP: usize = 30_000;
    if raw.len() <= CAP {
        return raw.to_string();
    }
    let mut end = CAP;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[TRUNCATED AT 30KB — extract claims only from text that survived the cut]",
        &raw[..end]
    )
}

/// Parse the decomposer's claims_json: a JSON array of {"text": "..."}
/// (a bare array of strings is accepted too — models drift). Returns the
/// claim texts, deduplicated, order preserved.
fn parse_claims(raw: &str) -> Vec<String> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match parsed.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut seen: Vec<String> = Vec::new();
    for entry in arr {
        let text = entry
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                entry
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() && !seen.contains(&trimmed) {
            seen.push(trimmed);
        }
    }
    seen
}

/// The chunk of claim indices to dispatch on this tick, given the cursor.
/// Returns (start, end_exclusive); empty when the cursor is past the end.
fn chunk_bounds(cursor: usize, total: usize) -> (usize, usize) {
    let start = cursor * CHUNK;
    let end = (start + CHUNK).min(total);
    (start.min(total), end)
}

/// The decomposer's working contract. Single source of truth for the action
/// and parameter names it must use — tested below, asserted nowhere else.
fn decomposer_prompt(
    endpoint_id: &str,
    world_name: &str,
    domain: &str,
    target_date: &str,
    bundle_inline: &str,
) -> String {
    format!(
        "You are the Decomposer for endpoint {endpoint_id} (world \"{world_name}\", \
         domain: {domain}, target date: {target_date}).\n\n\
         The endpoint's document bundle — an imagined future native to {target_date} — is \
         inlined below between BEGIN BUNDLE and END BUNDLE.\n\n\
         --- BEGIN BUNDLE ---\n{bundle_inline}\n--- END BUNDLE ---\n\n\
         Extract the future's separable, load-bearing claims: the distinct assertions this \
         future stands on, each one checkable in principle, none restating another. A good \
         claim names who/what changed and by roughly when. Do NOT list details that merely \
         decorate a load-bearing claim — fold them into it. Extract exactly 3 load-bearing claims, ranked by importance; \
         if the bundle genuinely contains fewer than 3 separable claims, report the ones \
         that exist.\n\n\
         Then self-report exactly once:\n\
         temper.action(\"Endpoints\", \"{endpoint_id}\", \"DecompositionComplete\", \
         {{\"claims_json\": \"[{{\\\"text\\\": \\\"<claim 1>\\\"}}, {{\\\"text\\\": \
         \\\"<claim 2>\\\"}}, ...]\"}})\n\
         Then call temper.done(\"complete\")."
    )
}

/// Workspace name for a world — the cross-module rendezvous key. Every
/// session-spawning module must resolve the same per-world workspace by
/// this exact name.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

/// Resolve (or create) the per-world PawFS workspace and return its id.
/// The decomposer carries no temper_write tool, but every corridor session
/// is Configured with the world workspace uniformly.
fn ensure_world_workspace(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<String, String> {
    let name = workspace_name(world_id);
    // Workspace rows are not readable by agent principals (paw-fs Cedar has
    // no read/list permit on Workspace), so idempotent lookup is impossible:
    // create one per spawn batch. Correctness needs only that each session's
    // Configure workspace matches the files it writes.
    let create_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Workspaces"),
        headers,
        &json!({ "name": name }).to_string(),
    )?;
    if create_resp.status < 200 || create_resp.status >= 300 {
        return Err(format!(
            "create Workspace {name} failed (HTTP {})",
            create_resp.status
        ));
    }
    serde_json::from_str::<Value>(&create_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "Workspace create returned no entity_id".to_string())
}

fn spawn_session(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    name: &str,
    role: &str,
    model: &str,
    provider: &str,
    tools: &str,
    max_turns: &str,
    user_message: &str,
    workspace_id: &str,
    latency_profile: &str,
) -> Result<String, String> {
    let agent_body = json!({ "Name": name, "Role": role });
    let agent_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Agents"),
        headers,
        &agent_body.to_string(),
    )?;
    if agent_resp.status < 200 || agent_resp.status >= 300 {
        return Err(format!(
            "create Agent {name} failed (HTTP {})",
            agent_resp.status
        ));
    }
    let agent_id = serde_json::from_str::<Value>(&agent_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Agent create returned no entity_id")?;

    let session_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions"),
        headers,
        &json!({ "agent_id": agent_id }).to_string(),
    )?;
    if session_resp.status < 200 || session_resp.status >= 300 {
        return Err(format!(
            "create Session for {name} failed (HTTP {})",
            session_resp.status
        ));
    }
    let session_id = serde_json::from_str::<Value>(&session_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Session create returned no entity_id")?;

    let message = user_message.replace("{AGENT_ID}", &agent_id);
    let configure_body = json!({
        "model": model,
        "provider": provider,
        "agent_name": role,
        "tools_enabled": tools,
        "tool_choice": "required",
        "provider_latency_profile": latency_profile,
        "max_turns": max_turns,
        "user_message": message,
        "sandbox_url": "none",
        "workspace_id": workspace_id,
        "temper_api_url": api
    });
    let configure_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions('{session_id}')/TemperPaw.Configure"),
        headers,
        &configure_body.to_string(),
    )?;
    if configure_resp.status < 200 || configure_resp.status >= 300 {
        return Err(format!(
            "Configure for {name} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("decompose_endpoint: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(session_id)
}

fn system_headers(ctx: &Context, entity_id: &str) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), entity_id.to_string()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

/// Fetch a single entity authoritatively (GET by id dodges the list
/// projection lag — v1 lesson, pinned in the proofs).
fn fetch_entity(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
) -> Result<Value, String> {
    let resp = ctx.http_call("GET", &format!("{api}/tdata/{set}('{id}')"), headers, "")?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("GET {set}('{id}') failed (HTTP {})", resp.status));
    }
    serde_json::from_str(&resp.body).map_err(|e| format!("{set}('{id}') parse: {e}"))
}

fn dispatch(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
    action: &str,
    body: &Value,
) -> Result<(), String> {
    let resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/{set}('{id}')/TemperPaw.{action}"),
        headers,
        &body.to_string(),
    )?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "{set}.{action} on {id} failed (HTTP {}): {}",
            resp.status,
            &resp.body[..resp.body.len().min(200)]
        ));
    }
    Ok(())
}

/// Phase 1 (SubmitForRepair): inline the bundle, spawn the decomposer.
fn phase_spawn_decomposer(ctx: &Context, fields: &Value) -> Result<(), String> {
    let endpoint_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx, &endpoint_id);

    let get = |k: &str| -> String {
        fields
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let world_id = get("world_id");
    if world_id.is_empty() {
        return Err("Endpoint.world_id is required".to_string());
    }

    let world = fetch_entity(ctx, &api, &headers, "Worlds", &world_id)?;
    let model = row_str(&world, "AgentModel").to_string();
    let provider = row_str(&world, "AgentProvider").to_string();
    if model.trim().is_empty() || provider.trim().is_empty() {
        return Err("World.agent_model and World.agent_provider are required".to_string());
    }

    let bundle_file_id = get("bundle_file_id");
    if bundle_file_id.is_empty() {
        return Err(
            "Endpoint has no bundle_file_id: nothing to decompose (writer must SubmitForRepair with a bundle)"
                .to_string(),
        );
    }
    let bundle_resp = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Files('{bundle_file_id}')/$value"),
        &headers,
        "",
    )?;
    if bundle_resp.status < 200 || bundle_resp.status >= 300 {
        return Err(format!(
            "could not fetch bundle {bundle_file_id} (HTTP {})",
            bundle_resp.status
        ));
    }
    let bundle_inline = inline_file(&bundle_resp.body);

    let workspace_id = ensure_world_workspace(ctx, &api, &headers, &world_id)?;
    let prompt = decomposer_prompt(
        &endpoint_id,
        row_str(&world, "Name"),
        row_str(&world, "Domain"),
        row_str(&world, "TargetDate"),
        &bundle_inline,
    );
    spawn_session(
        ctx,
        &api,
        &headers,
        &format!("Decomposer-{endpoint_id}"),
        "decomposer",
        &model,
        &provider,
        DECOMPOSER_TOOLS,
        "12",
        &prompt,
        &workspace_id,
        if row_str(&world, "ExplorationPhase") == "first_pass" {
            "foresight_first_pass"
        } else {
            "standard"
        },
    )?;
    // No follow-up dispatch: the decomposer self-reports
    // DecompositionComplete, which re-enters this module.
    set_success_result("", &json!({}));
    Ok(())
}

/// Phase 2 (DecompositionComplete): create Claim entities, report their ids.
fn phase_create_claims(ctx: &Context, fields: &Value) -> Result<(), String> {
    let endpoint_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx, &endpoint_id);

    let get = |k: &str| -> String {
        fields
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let world_id = get("world_id");
    let claims = parse_claims(&get("claims_json"));
    if claims.is_empty() {
        return Err(
            "DecompositionComplete carried no parseable claims (claims_json must be a JSON array)"
                .to_string(),
        );
    }
    if claims.len() > MAX_CLAIMS {
        ctx.log(
            "warn",
            &format!(
                "decompose_endpoint: {} claims extracted; keeping the first {MAX_CLAIMS}",
                claims.len()
            ),
        );
    }
    if claims.len() < MIN_CLAIMS {
        ctx.log(
            "warn",
            &format!(
                "decompose_endpoint: only {} separable claims extracted (target {MIN_CLAIMS}-{MAX_CLAIMS}); proceeding — a thin future is an honest finding",
                claims.len()
            ),
        );
    }

    let mut claim_ids: Vec<String> = Vec::new();
    for text in claims.iter().take(MAX_CLAIMS) {
        let body = json!({
            "world_id": world_id,
            "endpoint_id": endpoint_id,
            "original_text": text,
            "current_text": text,
        });
        let resp = ctx.http_call(
            "POST",
            &format!("{api}/tdata/Claims"),
            &headers,
            &body.to_string(),
        )?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!("create Claim failed (HTTP {})", resp.status));
        }
        let id = serde_json::from_str::<Value>(&resp.body)
            .ok()
            .and_then(|v| {
                v.get("entity_id")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .ok_or("Claim create returned no entity_id")?;
        claim_ids.push(id);
    }

    dispatch(
        ctx,
        &api,
        &headers,
        "Endpoints",
        &endpoint_id,
        "ClaimsAttached",
        &json!({ "claim_ids": serde_json::to_string(&claim_ids).unwrap_or_default() }),
    )?;
    set_success_result("", &json!({}));
    Ok(())
}

/// Phase 3 (ClaimsAttached / SpawnNextBridge): dispatch the next chunk of
/// SubmitForBridge, self-loop until done.
fn phase_fan_out(ctx: &Context, fields: &Value, cursor: usize) -> Result<(), String> {
    let endpoint_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx, &endpoint_id);

    if cursor > MAX_CHECKS {
        return Err(format!(
            "SpawnNextBridge exceeded MAX_CHECKS ({MAX_CHECKS}) — runaway self-loop guard"
        ));
    }

    let claim_ids: Vec<String> = fields
        .get("claim_ids")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();
    if claim_ids.is_empty() {
        return Err("ClaimsAttached carried no claim ids".to_string());
    }

    let (start, end) = chunk_bounds(cursor, claim_ids.len());
    for claim_id in &claim_ids[start..end] {
        dispatch(
            ctx,
            &api,
            &headers,
            "Claims",
            claim_id,
            "SubmitForBridge",
            &json!({"revision_brief":""}),
        )?;
    }
    ctx.log(
        "info",
        &format!(
            "decompose_endpoint: bridged claims {start}..{end} of {} for endpoint {endpoint_id}",
            claim_ids.len()
        ),
    );

    if end < claim_ids.len() {
        dispatch(
            ctx,
            &api,
            &headers,
            "Endpoints",
            &endpoint_id,
            "SpawnNextBridge",
            &json!({ "check_count": (cursor + 1).to_string() }),
        )?;
    }
    set_success_result("", &json!({}));
    Ok(())
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        match ctx.trigger_action.as_str() {
            "SubmitForRepair" => phase_spawn_decomposer(&ctx, &fields),
            "DecompositionComplete" | "PrepareClaims" => phase_create_claims(&ctx, &fields),
            "ClaimsAttached" => phase_fan_out(&ctx, &fields, 0),
            "SpawnNextBridge" => {
                let cursor = fields
                    .get("check_count")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                phase_fan_out(&ctx, &fields, cursor)
            }
            other => Err(format!(
                "decompose_endpoint: unexpected trigger action {other:?}"
            )),
        }
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Prompt-contract tests: the generated prompt must reference the exact
    // entity sets, action names, and parameter names the specs declare.
    // The API silently drops unknown fields — drift here is a silent failure.

    #[test]
    fn decomposer_prompt_carries_the_decomposition_complete_contract() {
        let p = decomposer_prompt(
            "e-1",
            "Test",
            "ai coding tools",
            "2045-06-11",
            "bundle body text",
        );
        for needle in [
            "temper.action(\"Endpoints\", \"e-1\", \"DecompositionComplete\"",
            "\"claims_json\"",
            "BEGIN BUNDLE",
            "bundle body text",
            "exactly 3 load-bearing claims, ranked by importance",
            "load-bearing",
        ] {
            assert!(p.contains(needle), "decomposer prompt missing: {needle}");
        }
        // Bundles are inlined, never temper.read (sessions cannot resolve
        // harness/WASM-created files by id — wall 13/15).
        assert!(!p.contains("temper.read("));
    }

    #[test]
    fn claims_parse_from_objects_strings_and_dedupe() {
        let objs = r#"[{"text": "A happens"}, {"text": "B happens"}, {"text": "A happens"}]"#;
        assert_eq!(parse_claims(objs), vec!["A happens", "B happens"]);
        let strs = r#"["A happens", " ", "C happens"]"#;
        assert_eq!(parse_claims(strs), vec!["A happens", "C happens"]);
        assert!(parse_claims("not json").is_empty());
        assert!(parse_claims("{\"text\": \"not an array\"}").is_empty());
    }

    #[test]
    fn chunk_bounds_cover_all_claims_exactly_once() {
        // 5 claims, CHUNK=2: ticks at cursors 0,1,2 cover 0..2, 2..4, 4..5.
        assert_eq!(chunk_bounds(0, 5), (0, 2));
        assert_eq!(chunk_bounds(1, 5), (2, 4));
        assert_eq!(chunk_bounds(2, 5), (4, 5));
        // Past the end: empty, never out of range.
        assert_eq!(chunk_bounds(3, 5), (5, 5));
        // MAX_CLAIMS at CHUNK=2 finishes within MAX_CHECKS ticks.
        let ticks_needed = MAX_CLAIMS.div_ceil(CHUNK);
        assert!(ticks_needed <= MAX_CHECKS);
    }

    #[test]
    fn inlining_truncates_loudly_at_cap() {
        let big = "x".repeat(40_000);
        let inlined = inline_file(&big);
        assert!(inlined.len() < 31_000);
        assert!(inlined.contains("TRUNCATED AT 30KB"));
        assert_eq!(inline_file("tiny"), "tiny");
    }

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        assert_eq!(workspace_name("w-1"), "world-w-1");
    }
}
