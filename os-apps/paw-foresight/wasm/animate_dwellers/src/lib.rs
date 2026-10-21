//! animate_dwellers — living worlds: casting, traversals, stories
//! (ADR-004 C4).
//!
//! Dweller living is event-driven, never ambient: sessions animate dwellers
//! when the world is constructed or updated. One activity, three products —
//! contradiction reports (stress tests that feed pricing), accumulated
//! lived experience (the dweller's track record), and first-person stories
//! through the consistency gate.
//!
//! Phases, keyed on the triggering action:
//! - World.AnimateDwellers: spawn ONE casting session that proposes 2-3
//!   personas grounded in the world's settled claims. It self-reports
//!   World.DwellersCast.
//! - World.DwellersCast: create Agent + persona File + Dweller entities,
//!   Activate each, persist the ids, start the fan-out.
//! - World.SpawnNextDweller: spawn one traversal/story session per tick
//!   (bounded self-loop under session admission caps, ADR-0005).
//! - Dweller.RecordContradiction: append a contradiction flag to the
//!   route's challenge flags and re-price the claim — dweller
//!   stress-testing participates in costing (ADR-004).
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

/// Personas per world (the casting session may propose fewer).
const MAX_DWELLERS: usize = 3;

/// Dweller sessions spawned per fan-out tick (they are heavyweight: 50-turn
/// traversal + story sessions).
const CHUNK: usize = 1;

/// Hard stop for the self-loop.
const MAX_CHECKS: usize = 4;

/// A dweller contradiction prices as contradiction/low (25 × 0.5): lived
/// incoherence is a real cost signal, but one dweller's testimony is not an
/// adversary's audit. Tunable prior (ADR-004 commitment 1).
const CONTRADICTION_REPRICE: f64 = 12.5;

/// Repriced claims above this are "strained" (mirrors
/// aggregate_costs::REACHABLE_MAX).
const REACHABLE_MAX: f64 = 120.0;

const CASTER_TOOLS: &str = "temper_get,temper_list,temper_action";
const DWELLER_TOOLS: &str = "temper_get,temper_list,temper_create,temper_action,temper_write";

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

fn row_status(row: &Value) -> &str {
    row.get("status")
        .or_else(|| row.get("Status"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn row_id(row: &Value) -> &str {
    row.get("entity_id")
        .or_else(|| row.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Parse the casting session's personas_json: a JSON array of
/// {"name", "role", "epistemic_role", "sketch"}. Entries without a name are
/// dropped; capped at MAX_DWELLERS.
fn parse_personas(raw: &str) -> Vec<Value> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match parsed.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter(|p| {
            p.get("name")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        })
        .take(MAX_DWELLERS)
        .cloned()
        .collect()
}

/// The chunk of dweller indices for this tick.
fn chunk_bounds(cursor: usize, total: usize) -> (usize, usize) {
    let start = cursor * CHUNK;
    let end = (start + CHUNK).min(total);
    (start.min(total), end)
}

/// Reprice arithmetic for a dweller contradiction.
fn contradiction_reprice(old_cost: f64) -> (f64, &'static str) {
    let new_cost = old_cost + CONTRADICTION_REPRICE;
    let classification = if new_cost <= REACHABLE_MAX {
        "reachable"
    } else {
        "strained"
    };
    (new_cost, classification)
}

/// The casting contract. Single source of truth for the action and
/// parameter names it must use — tested below, asserted nowhere else.
fn caster_prompt(
    world_id: &str,
    name: &str,
    domain: &str,
    target_date: &str,
    claims_inline: &str,
) -> String {
    format!(
        "You are the Casting Director for world {world_id} (\"{name}\", domain: {domain}, \
         target date: {target_date}).\n\n\
         The world's settled claims — its load-bearing structure — with their verdicts:\n\
         {claims_inline}\n\n\
         Propose 2-3 DWELLERS: people who would actually live inside this future. Not \
         protagonists of a plot — inhabitants whose ordinary lives the claims would reshape. \
         Each needs: a name, a role (their job/place in the world), an epistemic_role (what \
         their vantage lets them SEE that others cannot — an insider, a skeptic, someone the \
         change costs something), and a 3-5 sentence sketch grounding them in specific \
         claims.\n\n\
         Then self-report exactly once:\n\
         temper.action(\"Worlds\", \"{world_id}\", \"DwellersCast\", {{\"personas_json\": \
         \"[{{\\\"name\\\": \\\"...\\\", \\\"role\\\": \\\"...\\\", \\\"epistemic_role\\\": \
         \\\"...\\\", \\\"sketch\\\": \\\"...\\\"}}, ...]\"}})\n\
         Then call temper.done(\"complete\")."
    )
}

/// The traversal/story contract.
#[allow(clippy::too_many_arguments)]
fn dweller_prompt(
    dweller_id: &str,
    world_id: &str,
    world_name: &str,
    target_date: &str,
    persona: &str,
    claims_inline: &str,
    route_inline: &str,
    canonical_path_id: &str,
    agent_id: &str,
) -> String {
    format!(
        "You ARE the dweller {dweller_id} of world {world_id} (\"{world_name}\", target date \
         {target_date}). This is your persona — inhabit it, do not describe it:\n\
         {persona}\n\n\
         The world's settled claims (its physics — your life happens under these):\n\
         {claims_inline}\n\n\
         The canonical route — the chain of events that produced your present, earliest \
         first:\n\
         {route_inline}\n\n\
         LIVE the timeline: walk those events as your own biography. Where were you when \
         each happened? What did each cost or give you? Be concrete — your employer, your \
         money, your tools, your people.\n\n\
         1. Record your traversal: write your lived timeline notes with temper.write \
         (markdown), then:\n\
         temper.action(\"Dwellers\", \"{dweller_id}\", \"RecordTraversal\", {{\"path_id\": \
         \"{canonical_path_id}\", \"traversal_note_file_id\": \"<file-id-from-temper.write>\"}})\n\
         2. If — and only if — some event CANNOT be lived coherently from your vantage (it \
         contradicts another event, your incentives, or simple arithmetic of your life), \
         file it honestly:\n\
         temper.action(\"Dwellers\", \"{dweller_id}\", \"RecordContradiction\", {{\"path_id\": \
         \"{canonical_path_id}\", \"claim_id\": \"<the claim id it breaks, from the list \
         above>\", \"note\": \"<one sentence: what cannot hold and why>\"}})\n\
         3. Write YOUR STORY: first person, 600-1200 words, a specific day or episode of \
         your life inside this world — not a summary of the timeline. Ground every \
         world-fact in the claims and events above. Save it with temper.write (markdown), \
         then create the artifact and submit it to the gate:\n\
         temper.create(\"Artifacts\", {{\"world_id\": \"{world_id}\", \"path_id\": \
         \"{canonical_path_id}\", \"kind\": \"story\", \"title\": \"<your story's title>\", \
         \"author_dweller_id\": \"{dweller_id}\", \"author_agent_id\": \"{agent_id}\", \
         \"content_file_id\": \"<file-id-from-temper.write>\", \"cited_node_ids\": \
         \"[\\\"<event-node-id>\\\", ...]\"}})\n\
         temper.action(\"Artifacts\", \"<artifact-id-from-create>\", \"SubmitForCheck\", {{}})\n\
         Then call temper.done(\"complete\")."
    )
}

/// Workspace name for a world — the cross-module rendezvous key.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

fn ensure_world_workspace(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<String, String> {
    let name = workspace_name(world_id);
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

fn create_entity(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    body: &Value,
) -> Result<String, String> {
    let resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/{set}"),
        headers,
        &body.to_string(),
    )?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("create {set} failed (HTTP {})", resp.status));
    }
    serde_json::from_str::<Value>(&resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| format!("{set} create returned no entity_id"))
}

#[allow(clippy::too_many_arguments)]
fn spawn_session(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    agent_id: &str,
    role: &str,
    model: &str,
    provider: &str,
    tools: &str,
    max_turns: &str,
    user_message: &str,
    workspace_id: &str,
) -> Result<String, String> {
    let session_id = create_entity(
        ctx,
        api,
        headers,
        "Sessions",
        &json!({ "agent_id": agent_id }),
    )?;
    let message = user_message.replace("{AGENT_ID}", agent_id);
    let configure_body = json!({
        "model": model,
        "provider": provider,
        "agent_name": role,
        "tools_enabled": tools,
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
            "Configure for {role} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("animate_dwellers: spawned {role} session {session_id}"),
    );
    Ok(session_id)
}

fn system_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
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

fn list(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    filter: &str,
) -> Result<Vec<Value>, String> {
    let url = format!("{api}/tdata/{set}?$filter={filter}");
    let r = ctx.http_call("GET", &url, headers, "")?;
    if !(200..300).contains(&r.status) {
        return Err(format!("list {set} failed (HTTP {})", r.status));
    }
    let body: Value = serde_json::from_str(&r.body).unwrap_or(json!({}));
    Ok(body
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
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
    let r = ctx.http_call(
        "POST",
        &format!("{api}/tdata/{set}('{id}')/TemperPaw.{action}"),
        headers,
        &body.to_string(),
    )?;
    if !(200..300).contains(&r.status) {
        return Err(format!(
            "{set}.{action} on {id} failed (HTTP {}): {}",
            r.status,
            &r.body[..r.body.len().min(200)]
        ));
    }
    Ok(())
}

/// The world's settled claims, inlined for prompts: id, verdict, cost, text.
fn claims_inline(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<String, String> {
    let rows = list(
        ctx,
        api,
        headers,
        "Claims",
        &format!("world_id eq '{world_id}'"),
    )?;
    let mut out = String::new();
    for c in &rows {
        let status = row_status(c);
        if !matches!(status, "Settled" | "Unreachable") {
            continue;
        }
        let text = {
            let cur = row_str(c, "CurrentText");
            if cur.is_empty() {
                row_str(c, "OriginalText").to_string()
            } else {
                cur.to_string()
            }
        };
        let verdict = if status == "Unreachable" {
            "unreachable".to_string()
        } else {
            format!(
                "{} (cost {})",
                row_str(c, "Classification"),
                row_str(c, "BestRouteCost")
            )
        };
        out.push_str(&format!("- [{}] {verdict}: {text}\n", row_id(c)));
    }
    if out.is_empty() {
        out = "(no settled claims — this world predates claim decomposition)".to_string();
    }
    Ok(out)
}

/// The canonical route's required nodes, inlined earliest-first:
/// id, resolve_by, statement.
fn route_inline(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    canonical_path_id: &str,
) -> Result<String, String> {
    if canonical_path_id.is_empty() {
        return Ok("(no canonical route bound)".to_string());
    }
    let r = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Paths('{canonical_path_id}')"),
        headers,
        "",
    )?;
    if !(200..300).contains(&r.status) {
        return Ok("(canonical route unreadable)".to_string());
    }
    let path: Value = serde_json::from_str(&r.body).unwrap_or(json!({}));
    let required: Vec<String> = Some(row_str(&path, "RequiredNodeIds"))
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut nodes: Vec<(String, String, String)> = Vec::new();
    for node_id in &required {
        if let Ok(nr) = ctx.http_call(
            "GET",
            &format!("{api}/tdata/EventNodes('{node_id}')"),
            headers,
            "",
        ) {
            if (200..300).contains(&nr.status) {
                let node: Value = serde_json::from_str(&nr.body).unwrap_or(json!({}));
                nodes.push((
                    row_str(&node, "ResolveBy").to_string(),
                    node_id.clone(),
                    row_str(&node, "Statement").to_string(),
                ));
            }
        }
    }
    nodes.sort();
    let mut out = String::new();
    for (date, id, statement) in &nodes {
        out.push_str(&format!("- [{id}] by {date}: {statement}\n"));
    }
    if out.is_empty() {
        out = "(the canonical route lists no required nodes)".to_string();
    }
    Ok(out)
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        match ctx.trigger_action.as_str() {
            "AnimateDwellers" => phase_cast(&ctx, &fields),
            "DwellersCast" => phase_create_dwellers(&ctx, &fields),
            "SpawnNextDweller" => {
                let cursor = fields
                    .get("check_count")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                phase_fan_out(&ctx, &fields, cursor)
            }
            "RecordContradiction" => phase_contradiction(&ctx, &fields),
            other => Err(format!(
                "animate_dwellers: unexpected trigger action {other:?}"
            )),
        }
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn get_str(fields: &Value, key: &str) -> String {
    fields
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// World.AnimateDwellers: spawn the casting session.
fn phase_cast(ctx: &Context, fields: &Value) -> Result<(), String> {
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let model = get_str(fields, "agent_model");
    let provider = get_str(fields, "agent_provider");
    if model.trim().is_empty() || provider.trim().is_empty() {
        return Err("World.agent_model and World.agent_provider are required".to_string());
    }

    let claims = claims_inline(ctx, &api, &headers, &world_id)?;
    let workspace_id = ensure_world_workspace(ctx, &api, &headers, &world_id)?;
    let caster_agent = create_entity(
        ctx,
        &api,
        &headers,
        "Agents",
        &json!({ "Name": format!("Caster-{world_id}"), "Role": "caster" }),
    )?;
    let prompt = caster_prompt(
        &world_id,
        &get_str(fields, "name"),
        &get_str(fields, "domain"),
        &get_str(fields, "target_date"),
        &claims,
    );
    spawn_session(
        ctx,
        &api,
        &headers,
        &caster_agent,
        "caster",
        &model,
        &provider,
        CASTER_TOOLS,
        "12",
        &prompt,
        &workspace_id,
    )?;
    // No dispatch: the caster self-reports DwellersCast.
    set_success_result("", &json!({}));
    Ok(())
}

/// World.DwellersCast: create Agent + persona File + Dweller per persona.
fn phase_create_dwellers(ctx: &Context, fields: &Value) -> Result<(), String> {
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let personas = parse_personas(&get_str(fields, "personas_json"));
    if personas.is_empty() {
        return Err(
            "DwellersCast carried no parseable personas (personas_json must be a JSON array with names)"
                .to_string(),
        );
    }

    let mut dweller_ids: Vec<String> = Vec::new();
    for p in &personas {
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let epistemic = p
            .get("epistemic_role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let sketch = p.get("sketch").and_then(|v| v.as_str()).unwrap_or("");

        let agent_id = create_entity(
            ctx,
            &api,
            &headers,
            "Agents",
            &json!({ "Name": format!("Dweller-{name}"), "Role": "dweller" }),
        )?;

        // Persona sketch as a paw-fs File (the dweller's persistent identity
        // document; prompts inline it — sessions cannot read it by id).
        let file_id = create_entity(
            ctx,
            &api,
            &headers,
            "Files",
            &json!({ "name": format!("persona-{name}") }),
        )?;
        let put = ctx.http_call(
            "PUT",
            &format!("{api}/tdata/Files('{file_id}')/$value"),
            &headers,
            &format!("# {name}\n\nRole: {role}\nEpistemic role: {epistemic}\n\n{sketch}\n"),
        );
        if let Ok(r) = put {
            if r.status >= 400 {
                ctx.log(
                    "warn",
                    &format!(
                        "animate_dwellers: persona file write failed (HTTP {}); continuing",
                        r.status
                    ),
                );
            }
        }

        let dweller_id = create_entity(
            ctx,
            &api,
            &headers,
            "Dwellers",
            &json!({
                "world_id": world_id,
                "name": name,
                "role": role,
                "epistemic_role": epistemic,
            }),
        )?;
        dispatch(
            ctx,
            &api,
            &headers,
            "Dwellers",
            &dweller_id,
            "Activate",
            &json!({ "agent_id": agent_id, "persona_file_id": file_id }),
        )?;
        dweller_ids.push(dweller_id);
    }

    // Persist the roster and start the fan-out in one transition. dweller_ids
    // rides as a SpawnNextDweller param (like check_count) rather than a raw
    // PATCH: Cedar permits system flows to dispatch the action but not to PATCH
    // a World's fields, and state vars are set via action params anyway
    // (entity-first). Once set here it persists across the omitting re-ticks,
    // exactly as name/target_date/canonical_path_id do.
    set_success_result(
        "SpawnNextDweller",
        &json!({
            "dweller_ids": serde_json::to_string(&dweller_ids).unwrap_or_default(),
            "check_count": "0",
        }),
    );
    Ok(())
}

/// World.SpawnNextDweller: one traversal/story session per tick.
fn phase_fan_out(ctx: &Context, fields: &Value, cursor: usize) -> Result<(), String> {
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    if cursor > MAX_CHECKS {
        return Err(format!(
            "SpawnNextDweller exceeded MAX_CHECKS ({MAX_CHECKS}) — runaway self-loop guard"
        ));
    }

    let dweller_ids: Vec<String> = fields
        .get("dweller_ids")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();
    if dweller_ids.is_empty() {
        return Err("SpawnNextDweller with no dweller roster".to_string());
    }

    let model = get_str(fields, "agent_model");
    let provider = get_str(fields, "agent_provider");
    let canonical_path_id = get_str(fields, "canonical_path_id");
    let claims = claims_inline(ctx, &api, &headers, &world_id)?;
    let route = route_inline(ctx, &api, &headers, &canonical_path_id)?;
    let workspace_id = ensure_world_workspace(ctx, &api, &headers, &world_id)?;

    let (start, end) = chunk_bounds(cursor, dweller_ids.len());
    for dweller_id in &dweller_ids[start..end] {
        // The dweller's identity: fetch the entity, inline its persona file.
        let dr = ctx.http_call(
            "GET",
            &format!("{api}/tdata/Dwellers('{dweller_id}')"),
            &headers,
            "",
        )?;
        let dweller: Value = serde_json::from_str(&dr.body).unwrap_or(json!({}));
        let agent_id = row_str(&dweller, "AgentId").to_string();
        let persona_file_id = row_str(&dweller, "PersonaFileId").to_string();
        let mut persona = format!(
            "Name: {}\nRole: {}\nEpistemic role: {}",
            row_str(&dweller, "Name"),
            row_str(&dweller, "Role"),
            row_str(&dweller, "EpistemicRole"),
        );
        if !persona_file_id.is_empty() {
            if let Ok(pf) = ctx.http_call(
                "GET",
                &format!("{api}/tdata/Files('{persona_file_id}')/$value"),
                &headers,
                "",
            ) {
                if (200..300).contains(&pf.status) && !pf.body.trim().is_empty() {
                    persona = pf.body.clone();
                }
            }
        }
        let prompt = dweller_prompt(
            dweller_id,
            &world_id,
            &get_str(fields, "name"),
            &get_str(fields, "target_date"),
            &persona,
            &claims,
            &route,
            &canonical_path_id,
            "{AGENT_ID}",
        );
        // The dweller's BACKING agent runs the session — its track record is
        // the dweller's.
        spawn_session(
            ctx,
            &api,
            &headers,
            &agent_id,
            "dweller",
            &model,
            &provider,
            DWELLER_TOOLS,
            "50",
            &prompt,
            &workspace_id,
        )?;
    }
    ctx.log(
        "info",
        &format!(
            "animate_dwellers: animated dwellers {start}..{end} of {} for world {world_id}",
            dweller_ids.len()
        ),
    );

    if end < dweller_ids.len() {
        set_success_result(
            "SpawnNextDweller",
            &json!({ "check_count": (cursor + 1).to_string() }),
        );
    } else {
        set_success_result("", &json!({}));
    }
    Ok(())
}

/// Dweller.RecordContradiction: lived incoherence feeds pricing.
fn phase_contradiction(ctx: &Context, fields: &Value) -> Result<(), String> {
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let path_id = get_str(fields, "path_id");
    let claim_id = get_str(fields, "claim_id");
    let note = get_str(fields, "note");

    if path_id.is_empty() {
        ctx.log(
            "warn",
            "animate_dwellers: contradiction without a path_id — recorded on the dweller only",
        );
        set_success_result("", &json!({}));
        return Ok(());
    }

    // Append the contradiction to the route's challenge flags.
    let pr = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Paths('{path_id}')"),
        &headers,
        "",
    )?;
    if (200..300).contains(&pr.status) {
        let path: Value = serde_json::from_str(&pr.body).unwrap_or(json!({}));
        let mut flags: Vec<Value> = Some(row_str(&path, "ChallengeFlags"))
            .filter(|s| !s.is_empty())
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        flags.push(json!({
            "kind": "contradiction",
            "severity": "low",
            "note": format!("dweller contradiction ({}): {note}", ctx.entity_id),
        }));
        let append = json!({
            "challenge_flags": serde_json::to_string(&flags).unwrap_or_default()
        });
        let r = dispatch(
            ctx,
            &api,
            &headers,
            "Paths",
            &path_id,
            "AppendChallengeFlag",
            &append,
        );
        match r {
            Ok(()) => {}
            Err(e) => ctx.log(
                "warn",
                &format!("animate_dwellers: challenge-flag append on {path_id} failed: {e}"),
            ),
        }
    }

    // Re-price the settled claim the contradiction touches.
    if !claim_id.is_empty() {
        if let Ok(cr) = ctx.http_call(
            "GET",
            &format!("{api}/tdata/Claims('{claim_id}')"),
            &headers,
            "",
        ) {
            if (200..300).contains(&cr.status) {
                let claim: Value = serde_json::from_str(&cr.body).unwrap_or(json!({}));
                if row_status(&claim) == "Settled" {
                    let old = row_str(&claim, "BestRouteCost")
                        .parse::<f64>()
                        .unwrap_or(f64::MAX);
                    let (new_cost, class) = contradiction_reprice(old);
                    if let Err(e) = dispatch(
                        ctx,
                        &api,
                        &headers,
                        "Claims",
                        &claim_id,
                        "Reprice",
                        &json!({
                            "best_route_cost": format!("{:.2}", new_cost),
                            "classification": class,
                        }),
                    ) {
                        ctx.log("warn", &format!("animate_dwellers: {e}"));
                    } else {
                        ctx.log(
                            "info",
                            &format!(
                                "animate_dwellers: claim {claim_id} repriced {} -> {} after dweller contradiction",
                                format!("{:.2}", old),
                                format!("{:.2}", new_cost)
                            ),
                        );
                    }
                }
            }
        }
    }

    set_success_result("", &json!({}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Prompt-contract tests: the generated prompts must reference the exact
    // entity sets, action names, and parameter names the specs declare.

    #[test]
    fn caster_prompt_carries_the_dwellers_cast_contract() {
        let p = caster_prompt(
            "w-1",
            "After the Toolmakers",
            "software",
            "2045-06-11",
            "- [c-1] reachable (cost 20): the ledger wins",
        );
        for needle in [
            "temper.action(\"Worlds\", \"w-1\", \"DwellersCast\"",
            "\"personas_json\"",
            "\\\"epistemic_role\\\"",
            "the ledger wins",
            "2-3 DWELLERS",
        ] {
            assert!(p.contains(needle), "caster prompt missing: {needle}");
        }
        assert!(!p.contains("temper.read("));
    }

    #[test]
    fn dweller_prompt_carries_traversal_contradiction_and_story_contracts() {
        let p = dweller_prompt(
            "d-1",
            "w-1",
            "After the Toolmakers",
            "2045-06-11",
            "Name: Mara",
            "- [c-1] reachable: claim text",
            "- [n-1] by 2027-01-01: telemetry ships",
            "p-9",
            "a-1",
        );
        for needle in [
            "temper.action(\"Dwellers\", \"d-1\", \"RecordTraversal\"",
            "\"traversal_note_file_id\"",
            "temper.action(\"Dwellers\", \"d-1\", \"RecordContradiction\"",
            "\"claim_id\"",
            "\"note\"",
            "temper.create(\"Artifacts\"",
            "\"kind\": \"story\"",
            "\"author_dweller_id\": \"d-1\"",
            "\"content_file_id\"",
            "\"cited_node_ids\"",
            "SubmitForCheck",
            "first person",
            "telemetry ships",
        ] {
            assert!(p.contains(needle), "dweller prompt missing: {needle}");
        }
        assert!(!p.contains("temper.read("), "context is inlined, never read");
    }

    #[test]
    fn personas_parse_capped_and_named() {
        let raw = r#"[{"name": "Mara", "role": "engineer"}, {"name": ""}, {"role": "nameless"},
                      {"name": "Juno"}, {"name": "Onx"}, {"name": "Fourth"}]"#;
        let p = parse_personas(raw);
        assert_eq!(p.len(), MAX_DWELLERS);
        assert_eq!(p[0]["name"], "Mara");
        assert!(parse_personas("not json").is_empty());
    }

    #[test]
    fn fan_out_chunks_cover_the_roster_within_the_loop_guard() {
        assert_eq!(chunk_bounds(0, 3), (0, 1));
        assert_eq!(chunk_bounds(1, 3), (1, 2));
        assert_eq!(chunk_bounds(2, 3), (2, 3));
        assert_eq!(chunk_bounds(3, 3), (3, 3));
        assert!(MAX_DWELLERS.div_ceil(CHUNK) <= MAX_CHECKS);
    }

    #[test]
    fn contradiction_reprice_adds_a_low_contradiction() {
        let (cost, class) = contradiction_reprice(107.5);
        assert_eq!(cost, 120.0);
        assert_eq!(class, "reachable");
        let (cost2, class2) = contradiction_reprice(115.0);
        assert_eq!(cost2, 127.5);
        assert_eq!(class2, "strained");
    }

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        assert_eq!(workspace_name("w-1"), "world-w-1");
    }
}
