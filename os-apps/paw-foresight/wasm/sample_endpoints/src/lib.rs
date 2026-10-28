//! sample_endpoints — starts a corridor pass for a world (ADR-002).
//!
//! On World.SampleEndpoints: for each budget slot, create an Endpoint entity
//! carrying a deterministically assigned driver stance (modal first, then an
//! anti-modal spread), then spawn an endpoint-writer session against it. Each
//! writer produces a document bundle native to the world's target date and
//! self-reports Endpoint.SubmitForRepair.
//!
//! Hindcast worlds (hindcast_mode = "true") get web tools stripped at
//! Configure time: evidence comes only from the frozen corpus.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const WRITER_TOOLS: &str =
    "temper_get,temper_list,temper_create,temper_action,temper_read,temper_write";
const WEB_TOOLS: &str = ",temper_web_search,temper_web_fetch";

fn tools_enabled(hindcast: bool) -> String {
    if hindcast {
        WRITER_TOOLS.to_string()
    } else {
        format!("{WRITER_TOOLS}{WEB_TOOLS}")
    }
}

/// How many endpoints to sample this pass. Defaults to 3, hard-capped at 5 —
/// token spend grows linearly with writers, and each endpoint fans out into
/// repairer and adversary sessions besides.
fn endpoint_budget(raw: &str) -> usize {
    raw.trim().parse::<usize>().unwrap_or(3).min(5)
}

/// Parse the surveyor's named uncertainty axes (ADR-006): a JSON array of
/// {"axis": "...", "consensus_pole": "..."}.
fn parse_axes(raw: &str) -> Vec<(String, String)> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    let axis = o.get("axis").and_then(|x| x.as_str())?.trim();
                    if axis.is_empty() {
                        return None;
                    }
                    let pole = o
                        .get("consensus_pole")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    Some((axis.to_string(), pole.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Deterministic driver stance for the i-th endpoint writer (ADR-006). Slot 0
/// is the modal future. Each later slot inverts a DIFFERENT named uncertainty
/// axis from the surveyor, so the anti-modal worlds are distinct by
/// construction (not generic percentile stances that converge — the run-1 G2
/// failure). Falls back to a generic tail stance when there are fewer named
/// axes than budget slots. Pure: same (i, axes), same stance, every rerun.
fn driver_stance(i: usize, axes: &[(String, String)]) -> String {
    if i == 0 {
        return "modal: take the consensus view on every major uncertainty".to_string();
    }
    if let Some((axis, pole)) = axes.get(i - 1) {
        let pole_clause = if pole.is_empty() {
            String::new()
        } else {
            format!(" (consensus expects: {pole})")
        };
        return format!(
            "anti-modal on the \"{axis}\" axis: take a position genuinely OPPOSITE the \
             consensus{pole_clause}; hold the consensus on every OTHER axis. This world exists \
             to explore that one fork — make it concrete and specific to that axis."
        );
    }
    // Fewer named axes than budget slots: generic tail stance (no axis to name).
    format!(
        "anti-modal: pick the {i}-th most load-bearing uncertainty and take a tail view on it, \
         consensus elsewhere"
    )
}

/// Truncate inlined file content at a char boundary; endpoints must never be
/// grounded in silently-missing text, so the truncation is loud.
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
        "{}\n\n[TRUNCATED AT 30KB — ground claims only in text that survived the cut]",
        &raw[..end]
    )
}

/// Fetch a world file's content for prompt inlining. A configured file that
/// cannot be read is a hard error.
fn fetch_world_file(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    file_id: &str,
    label: &str,
) -> Result<String, String> {
    if file_id.is_empty() {
        return Ok(String::new());
    }
    let url = format!("{api}/tdata/Files('{file_id}')/$value");
    let resp = ctx.http_call("GET", &url, headers, "")?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "sample_endpoints: could not fetch {label} {file_id} (HTTP {})",
            resp.status
        ));
    }
    Ok(inline_file(&resp.body))
}

/// The endpoint writer's working contract. Single source of truth for the
/// action and parameter names it must use — tested below, asserted nowhere
/// else.
#[allow(clippy::too_many_arguments)]
fn endpoint_writer_prompt(
    world_id: &str,
    endpoint_id: &str,
    agent_id: &str,
    name: &str,
    domain: &str,
    target_date: &str,
    stance: &str,
    corpus_inline: &str,
    driver_basis_inline: &str,
    revision_brief: &str,
    hindcast: bool,
) -> String {
    let corpus_line = if corpus_inline.is_empty() {
        "No corpus file was provided.".to_string()
    } else {
        // Inlined, not temper.read: session file reads resolve by path inside
        // the session's workspace, and harness-uploaded world files have
        // neither — same wall the gate (and seed_world) hit before inlining.
        format!(
            "The world corpus — the domain documents this world is grounded in — is \
             inlined below between BEGIN CORPUS and END CORPUS.\n\
             --- BEGIN CORPUS ---\n{corpus_inline}\n--- END CORPUS ---"
        )
    };
    let driver_line = if driver_basis_inline.is_empty() {
        String::new()
    } else {
        format!(
            "The driver basis — the named drivers your stance bends — is inlined below.\n\
             --- BEGIN DRIVER BASIS ---\n{driver_basis_inline}\n--- END DRIVER BASIS ---\n"
        )
    };
    let research_line = if hindcast {
        "This is a HINDCAST world: you have NO web access by design. Use only the corpus and \
         the skeleton, and never reference anything dated after the world's vantage."
            .to_string()
    } else {
        "Use temper.web_search / temper.web_fetch to ground your future in real present-day \
         actors, products, and prices."
            .to_string()
    };
    // ADR-006: when the diversity gate sends a world back, it carries the
    // sibling it collapsed onto. The rewrite must diverge, not reword.
    let resteer_block = if revision_brief.trim().is_empty() {
        String::new()
    } else {
        format!(
            "THIS IS A RE-STEER, not a first draft. Your previous bundle was too close to \
             another world being explored in parallel. Write a GENUINELY DIFFERENT future for \
             this same stance — a different mechanism, different load-bearing events, a \
             different shape — not a reworded version. What it must diverge from:\n{revision_brief}\n\n"
        )
    };
    format!(
        "You are the endpoint writer for world {world_id} (\"{name}\", domain: {domain}), \
         endpoint {endpoint_id}.\n\n\
         {resteer_block}\
         You are writing DOCUMENTS NATIVE TO {target_date}: artifacts that exist inside that \
         future, not predictions about it. Write under this driver stance: {stance}\n\n\
         First read the skeleton: temper.list(\"EventNodes\", \"world_id eq '{world_id}'\"). \
         Nodes with provenance \"determined\" are settled facts — your future may not \
         contradict any \"determined\" node.\n\
         {corpus_line}\n{driver_line}{research_line}\n\n\
         Then write a document bundle: 2-4 documents, every one dated AT {target_date}:\n\
         - a retrospective or postmortem looking back from {target_date},\n\
         - a news item,\n\
         - at least one in-world primary document (a filing, a review, a changelog).\n\
         Documents must contain specific dates, named actors, and numbers — vague futures \
         cannot be repaired.\n\n\
         Save the whole bundle as ONE markdown file with the temper.write tool — this is the \
         ONLY way to create a FILE, and your workspace already exists. Do NOT create Files, \
         Directories, or Workspaces yourself, and do NOT invent a file-creation API via \
         temper.action — temper.write is the whole job. Call it exactly like this:\n\
         result = temper.write(\"/bundle.md\", \"<your full markdown bundle>\")\n\
         temper.write returns {{\"file_id\": \"...\", \"path\": \"...\", \"workspace_id\": \
         \"...\"}}. Use result[\"file_id\"] as bundle_file_id below, then self-report:\n\
         temper.action(\"Endpoints\", \"{endpoint_id}\", \"BundleWritten\", \
         {{\"bundle_file_id\": \"<result file_id>\", \"summary\": \"<one line>\", \
         \"author_agent_id\": \"{agent_id}\"}})\n\
         Then call temper.done(\"complete\"). The diversity gate, not you, starts repair."
    )
}

/// Workspace name for a world — the cross-module rendezvous key. Every
/// session-spawning module must resolve the same per-world workspace by
/// this exact name.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

/// Resolve (or create) the per-world PawFS workspace and return its id.
/// Sessions are Configured with this id so temper.write lands inside a
/// workspace PawFS Cedar accepts — without it, File create is denied
/// (resource.workspaceId must match the principal's workspace).
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
    // Configure workspace matches the files it writes; file READS are
    // unrestricted, so cross-session reads work across workspaces.
    let create_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Workspaces"),
        headers,
        &json!({ "name": name, "quota_limit": "104857600" }).to_string(),
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
        .ok_or_else(|| format!("Workspace {name} create returned no entity_id"))
}

#[allow(clippy::too_many_arguments)]
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

    // The prompt needs the real agent id for author fields; substitute now.
    let message = user_message.replace("{AGENT_ID}", &agent_id);
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
            "Configure for {name} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("sample_endpoints: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(agent_id)
}

// --- Diversity gate (ADR-006, D3) --------------------------------------------

/// Minimum cosine distance between two worlds' bundle-heads to count as
/// distinct. Tunable prior; calibrate from the per-round distances logged by
/// the gate (domain claims measured ~0.29–0.35 apart, near-restatements ~0.06,
/// so 0.15 flags collapse without dropping genuinely different futures).
const DIVERSITY_MIN_DISTANCE: f32 = 0.15;

/// Re-steer rounds allowed before the gate gives up and discards a persistent
/// near-duplicate (bounds the loop; ADR-006).
const GATE_MAX_ROUNDS: usize = 2;

/// Chars of each bundle to embed. mxbai-embed-large caps at ~512 tokens, so the
/// whole 30KB bundle cannot be embedded; the head (the dated retrospective that
/// opens every bundle) is the diversity signal available pre-decomposition.
const BUNDLE_HEAD_CHARS: usize = 1800;

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

fn api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn system_headers(ctx: &Context, principal_id: &str) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), principal_id.to_string()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
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

fn fetch_entity(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
) -> Option<Value> {
    let r = ctx
        .http_call("GET", &format!("{api}/tdata/{set}('{id}')"), headers, "")
        .ok()?;
    if !(200..300).contains(&r.status) {
        return None;
    }
    serde_json::from_str(&r.body).ok()
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

fn embed_config(ctx: &Context) -> (String, String) {
    let nonempty = |k: &str| {
        ctx.config
            .get(k)
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
    };
    (
        nonempty("embedding_endpoint")
            .unwrap_or_else(|| "http://127.0.0.1:11434/api/embed".to_string()),
        nonempty("embedding_model").unwrap_or_else(|| "mxbai-embed-large".to_string()),
    )
}

/// Embed `texts`, or None if the endpoint is unreachable / returns the wrong
/// count (the gate then degrades to "let everything through" rather than block
/// the pass — a missing embedder must never wedge a world; logged loudly).
fn fetch_embeddings(ctx: &Context, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Some(Vec::new());
    }
    let (endpoint, model) = embed_config(ctx);
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let body = corridor_embed::build_embed_request(&model, texts);
    let r = ctx.http_call("POST", &endpoint, &headers, &body).ok()?;
    if !(200..300).contains(&r.status) {
        ctx.log(
            "warn",
            &format!("sample_endpoints: embedding endpoint {endpoint} HTTP {}", r.status),
        );
        return None;
    }
    let vecs = corridor_embed::parse_embeddings(&r.body);
    if vecs.len() != texts.len() {
        ctx.log(
            "warn",
            &format!(
                "sample_endpoints: embedding count {} != {} requested",
                vecs.len(),
                texts.len()
            ),
        );
        return None;
    }
    Some(vecs)
}

/// The first BUNDLE_HEAD_CHARS chars of an endpoint's bundle, at a char
/// boundary — the diversity signal. Empty string if unreadable.
fn fetch_bundle_head(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    bundle_file_id: &str,
) -> String {
    if bundle_file_id.is_empty() {
        return String::new();
    }
    match ctx.http_call(
        "GET",
        &format!("{api}/tdata/Files('{bundle_file_id}')/$value"),
        headers,
        "",
    ) {
        Ok(r) if (200..300).contains(&r.status) => {
            let body = r.body;
            if body.len() <= BUNDLE_HEAD_CHARS {
                body
            } else {
                let mut end = BUNDLE_HEAD_CHARS;
                while !body.is_char_boundary(end) {
                    end -= 1;
                }
                body[..end].to_string()
            }
        }
        _ => String::new(),
    }
}

/// The per-world context every writer session needs, gathered once.
struct WriterCtx {
    world_id: String,
    name: String,
    domain: String,
    target_date: String,
    corpus_inline: String,
    driver_basis_inline: String,
    model: String,
    provider: String,
    hindcast: bool,
    workspace_id: String,
}

/// Build the writer context from a World's fields (already fetched), resolving
/// the workspace and inlining the corpus + driver basis.
fn load_writer_ctx(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world: &Value,
) -> Result<WriterCtx, String> {
    let f = |k: &str| row_str(world, k).to_string();
    let world_id = row_id(world).to_string();
    let model = f("AgentModel");
    let provider = f("AgentProvider");
    if model.trim().is_empty() || provider.trim().is_empty() {
        return Err("World.agent_model and World.agent_provider are required".to_string());
    }
    let workspace_id = ensure_world_workspace(ctx, api, headers, &world_id)?;
    let corpus_inline = fetch_world_file(ctx, api, headers, &f("CorpusFileId"), "corpus")?;
    let driver_basis_inline =
        fetch_world_file(ctx, api, headers, &f("DriverConfigFileId"), "driver basis")?;
    Ok(WriterCtx {
        world_id,
        name: f("Name"),
        domain: f("Domain"),
        target_date: f("TargetDate"),
        corpus_inline,
        driver_basis_inline,
        model,
        provider,
        hindcast: f("HindcastMode") == "true",
        workspace_id,
    })
}

/// Spawn (or re-spawn) one writer session against an existing Endpoint.
fn spawn_writer(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    wc: &WriterCtx,
    endpoint_id: &str,
    stance: &str,
    revision_brief: &str,
    label: &str,
) -> Result<(), String> {
    let writer_msg = endpoint_writer_prompt(
        &wc.world_id,
        endpoint_id,
        "{AGENT_ID}",
        &wc.name,
        &wc.domain,
        &wc.target_date,
        stance,
        &wc.corpus_inline,
        &wc.driver_basis_inline,
        revision_brief,
        wc.hindcast,
    );
    spawn_session(
        ctx,
        api,
        headers,
        label,
        "endpoint-writer",
        &wc.model,
        &wc.provider,
        &tools_enabled(wc.hindcast),
        "50",
        &writer_msg,
        &wc.workspace_id,
    )?;
    Ok(())
}

/// World.SampleEndpoints: create one Endpoint per budget slot and spawn its
/// writer. Writers now self-report BundleWritten (ADR-006); the diversity gate,
/// not the writer, starts repair.
fn phase_sample(ctx: &Context) -> Result<(), String> {
    let world = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx, &world_id);
    // load_writer_ctx reads PascalCase via row_str; ctx.entity_state nests under
    // "fields" (snake) — wrap it so row_str finds them.
    let world_wrapped = json!({ "entity_id": world_id, "fields": world });
    let wc = load_writer_ctx(ctx, &api, &headers, &world_wrapped)?;
    let budget = endpoint_budget(row_str(&world_wrapped, "EndpointBudget"));
    // ADR-006: the surveyor's named axes steer the anti-modal worlds so they
    // are distinct by construction; empty -> generic tail stances.
    let axes = parse_axes(row_str(&world_wrapped, "UncertaintyAxes"));
    ctx.log(
        "info",
        &format!("sample_endpoints: sampling {budget} worlds across {} named axes", axes.len()),
    );

    for i in 0..budget {
        let stance = driver_stance(i, &axes);
        let endpoint_resp = ctx.http_call(
            "POST",
            &format!("{api}/tdata/Endpoints"),
            &headers,
            &json!({
                "world_id": world_id,
                "driver_config": json!({ "stance": stance }).to_string(),
            })
            .to_string(),
        )?;
        if !(200..300).contains(&endpoint_resp.status) {
            return Err(format!("create Endpoint {i} failed (HTTP {})", endpoint_resp.status));
        }
        let endpoint_id = serde_json::from_str::<Value>(&endpoint_resp.body)
            .ok()
            .and_then(|v| v.get("entity_id").and_then(|x| x.as_str()).map(str::to_string))
            .ok_or("Endpoint create returned no entity_id")?;
        spawn_writer(
            ctx,
            &api,
            &headers,
            &wc,
            &endpoint_id,
            &stance,
            "",
            &format!("EndpointWriter-{world_id}-{i}"),
        )?;
    }

    set_success_result("EndpointsSampled", &json!({}));
    ctx.log(
        "info",
        &format!("sample_endpoints: spawned {budget} endpoint writers for world {world_id}"),
    );
    Ok(())
}

/// Endpoint.BundleWritten: the all-written barrier. When no sibling endpoint is
/// still being written, fire the diversity gate (ADR-006).
fn phase_barrier(ctx: &Context) -> Result<(), String> {
    let ep = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let world_id = ep.get("world_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if world_id.is_empty() {
        return Err("BundleWritten on an endpoint without world_id".to_string());
    }
    let api = api_url(ctx);
    let headers = system_headers(ctx, &ctx.entity_id);

    let endpoints = list(ctx, &api, &headers, "Endpoints", &format!("world_id eq '{world_id}'"))?;
    let mut pending = false; // a writer still running (Sampled)
    let mut written = 0usize;
    for e in &endpoints {
        // The triggering endpoint's transition to Written may not be visible in
        // the list projection yet — trust our own.
        let status = if row_id(e) == ctx.entity_id {
            "Written".to_string()
        } else {
            row_status(e).to_string()
        };
        match status.as_str() {
            "Sampled" => pending = true,
            "Written" => written += 1,
            _ => {}
        }
    }
    if pending || written == 0 {
        ctx.log(
            "info",
            &format!("sample_endpoints: barrier waiting (pending writers={pending}, written={written})"),
        );
        set_success_result("", &json!({}));
        return Ok(());
    }

    // All ungated endpoints have a bundle: run the gate. Increment the round.
    let rounds = fetch_entity(ctx, &api, &headers, "Worlds", &world_id)
        .map(|w| row_str(&w, "GateRounds").parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    dispatch(
        ctx,
        &api,
        &headers,
        "Worlds",
        &world_id,
        "GateDiversity",
        &json!({ "gate_rounds": (rounds + 1).to_string() }),
    )?;
    set_success_result("", &json!({}));
    ctx.log(
        "info",
        &format!("sample_endpoints: all {written} worlds written; dispatched GateDiversity (round {})", rounds + 1),
    );
    Ok(())
}

/// World.GateDiversity: embed the written worlds, release the diverse ones into
/// repair and re-steer (or, past the round cap, discard) the collapsed ones.
/// Idempotent: acts only on endpoints still in Written.
fn phase_gate(ctx: &Context) -> Result<(), String> {
    let world = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx, &world_id);
    let rounds = world.get("gate_rounds").and_then(|v| v.as_str()).unwrap_or("0").parse::<usize>().unwrap_or(0);

    let endpoints = list(ctx, &api, &headers, "Endpoints", &format!("world_id eq '{world_id}'"))?;
    // Released worlds are fixed references; written worlds are this round's candidates.
    let mut ref_heads: Vec<String> = Vec::new();
    let mut ref_summaries: Vec<String> = Vec::new();
    let mut cand_ids: Vec<String> = Vec::new();
    let mut cand_heads: Vec<String> = Vec::new();
    let mut cand_summaries: Vec<String> = Vec::new();
    for e in &endpoints {
        let id = row_id(e).to_string();
        let summary = row_str(e, "Summary").to_string();
        // ADR-006 (calibrated on run-1d): gate on the SUMMARY — it carries a
        // world's thesis/divergence (modal vs anti-modal-on-axis measured 0.298
        // apart), while the bundle-HEAD's shared dated-market retrospective
        // drowns a single-axis fork (the same pair measured only 0.111, a false
        // collapse). Fall back to the bundle-head only when a summary is missing.
        let text = if !summary.trim().is_empty() {
            summary.clone()
        } else {
            fetch_bundle_head(ctx, &api, &headers, &row_str(e, "BundleFileId").to_string())
        };
        match row_status(e) {
            "Written" => {
                cand_ids.push(id);
                cand_heads.push(text);
                cand_summaries.push(summary);
            }
            "UnderRepair" | "Scored" | "Weighted" => {
                ref_heads.push(text);
                ref_summaries.push(summary);
            }
            _ => {}
        }
    }
    if cand_ids.is_empty() {
        ctx.log("info", "sample_endpoints: gate found no written worlds (already released); no-op");
        set_success_result("", &json!({}));
        return Ok(());
    }

    // Embed references + candidates in one batch.
    let mut all = ref_heads.clone();
    all.extend(cand_heads.clone());
    let keep: Vec<bool> = match fetch_embeddings(ctx, &all) {
        Some(vecs) => {
            let ref_vecs = vecs[..ref_heads.len()].to_vec();
            let cand_vecs = vecs[ref_heads.len()..].to_vec();
            // For the re-steer brief: which kept/ref world each collapsed one is nearest.
            let decisions = corridor_embed::select_diverse(&ref_vecs, &cand_vecs, DIVERSITY_MIN_DISTANCE);
            // Log the round's pairwise floor for calibration.
            ctx.log(
                "info",
                &format!(
                    "sample_endpoints: gate round {rounds} — {} candidates, {} references, kept {}",
                    cand_ids.len(),
                    ref_heads.len(),
                    decisions.iter().filter(|k| **k).count()
                ),
            );
            // Stash vectors for nearest-sibling briefs.
            // (kept candidates also become references as select_diverse walks.)
            let _ = &cand_vecs;
            decisions
        }
        None => {
            // No embedder: never wedge the pass — release everything, loudly.
            ctx.log(
                "warn",
                "sample_endpoints: embedder unreachable; gate releasing all worlds undiverse-checked",
            );
            vec![true; cand_ids.len()]
        }
    };

    for (i, keep_it) in keep.iter().enumerate() {
        if *keep_it {
            dispatch(ctx, &api, &headers, "Endpoints", &cand_ids[i], "SubmitForRepair", &json!({}))?;
            ctx.log("info", &format!("sample_endpoints: gate released world {} into repair", cand_ids[i]));
        } else if rounds < GATE_MAX_ROUNDS {
            // Brief the rewrite on the kept world it most resembles.
            let nearest_summary = ref_summaries
                .iter()
                .chain(cand_summaries.iter())
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "another world being explored in parallel".to_string());
            let brief = format!(
                "Your future read too close to: \"{}\". Diverge on the load-bearing mechanism, not the wording.",
                nearest_summary
            );
            dispatch(ctx, &api, &headers, "Endpoints", &cand_ids[i], "ReSteer", &json!({ "revision_brief": brief }))?;
            ctx.log("info", &format!("sample_endpoints: gate re-steering collapsed world {} (round {rounds})", cand_ids[i]));
        } else {
            dispatch(
                ctx,
                &api,
                &headers,
                "Endpoints",
                &cand_ids[i],
                "Discard",
                &json!({ "discard_reason": format!("diversity gate: still a near-duplicate after {GATE_MAX_ROUNDS} re-steer rounds") }),
            )?;
            ctx.log("info", &format!("sample_endpoints: gate discarded persistent near-duplicate {}", cand_ids[i]));
        }
    }
    set_success_result("", &json!({}));
    Ok(())
}

/// Endpoint.ReSteer / Endpoint.ResumeWriter: re-spawn the writer with the
/// endpoint's stance and any differentiate brief. ReSteer (gate-dispatched)
/// rewrites a near-duplicate to diverge; ResumeWriter (the Sampled
/// state_timeout) re-drives a writer that died or was orphaned by a restart.
fn phase_resteer(ctx: &Context) -> Result<(), String> {
    let ep = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let g = |k: &str| ep.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let world_id = g("world_id");
    if world_id.is_empty() {
        return Err("ReSteer on an endpoint without world_id".to_string());
    }
    let api = api_url(ctx);
    let headers = system_headers(ctx, &ctx.entity_id);
    let world = fetch_entity(ctx, &api, &headers, "Worlds", &world_id)
        .ok_or_else(|| format!("ReSteer: world {world_id} not found"))?;
    let wc = load_writer_ctx(ctx, &api, &headers, &world)?;

    let stance = serde_json::from_str::<Value>(&g("driver_config"))
        .ok()
        .and_then(|v| v.get("stance").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default();
    let reason = if ctx.trigger_action == "ResumeWriter" {
        "resume"
    } else {
        "resteer"
    };
    spawn_writer(
        ctx,
        &api,
        &headers,
        &wc,
        &ctx.entity_id,
        &stance,
        &g("revision_brief"),
        &format!("EndpointWriter-{}-{reason}", ctx.entity_id),
    )?;
    set_success_result("", &json!({}));
    ctx.log(
        "info",
        &format!("sample_endpoints: {reason} writer for endpoint {}", ctx.entity_id),
    );
    Ok(())
}

/// Entry point: route by the triggering action (ADR-006 added the gate phases).
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        match ctx.trigger_action.as_str() {
            "SampleEndpoints" => phase_sample(&ctx),
            "BundleWritten" => phase_barrier(&ctx),
            "GateDiversity" => phase_gate(&ctx),
            // ReSteer (gate-dispatched) and ResumeWriter (the Sampled
            // state_timeout self-heal) share one path: re-spawn the writer with
            // the endpoint's stance + any revision_brief (ADR-0050/ADR-007).
            "ReSteer" | "ResumeWriter" => phase_resteer(&ctx),
            other => {
                ctx.log("warn", &format!("sample_endpoints: unexpected trigger {other}; nothing to do"));
                set_success_result("", &json!({}));
                Ok(())
            }
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

    // Prompt-contract tests: the generated prompts must reference the exact
    // entity sets, action names, and parameter names the specs declare.
    // The API silently drops unknown fields — drift here is a silent failure.

    fn writer_prompt(stance: &str, hindcast: bool) -> String {
        endpoint_writer_prompt(
            "w-1",
            "e-1",
            "a-1",
            "Test",
            "ai coding tools",
            "2026-12-11",
            stance,
            "corpus body text",
            "driver basis text",
            "",
            hindcast,
        )
    }

    #[test]
    fn writer_prompt_inlines_world_files_instead_of_temper_read() {
        // Sessions cannot resolve harness-uploaded files by id (path+workspace
        // resolution) — world files must be inlined, never temper.read.
        let p = writer_prompt(&driver_stance(0, &[]), true);
        assert!(p.contains("BEGIN CORPUS"));
        assert!(p.contains("corpus body text"));
        assert!(p.contains("BEGIN DRIVER BASIS"));
        assert!(p.contains("driver basis text"));
        assert!(!p.contains("temper.read("));
    }

    #[test]
    fn file_inlining_truncates_loudly_at_cap() {
        let big = "x".repeat(40_000);
        let inlined = inline_file(&big);
        assert!(inlined.len() < 31_000);
        assert!(inlined.contains("TRUNCATED AT 30KB"));
        assert_eq!(inline_file("tiny"), "tiny");
    }

    #[test]
    fn stance_assignment_inverts_named_axes_and_falls_back() {
        // ADR-006: each anti-modal slot inverts a DIFFERENT named axis, so the
        // worlds are distinct by construction (not generic converging stances).
        let axes = vec![
            ("capability slope".to_string(), "incremental gains".to_string()),
            ("regulation".to_string(), "light-touch".to_string()),
        ];
        assert!(driver_stance(0, &axes).starts_with("modal:"));
        let s1 = driver_stance(1, &axes);
        let s2 = driver_stance(2, &axes);
        assert!(s1.contains("capability slope") && s1.contains("incremental gains"));
        assert!(s2.contains("regulation") && s2.contains("light-touch"));
        assert_ne!(s1, s2, "different axes -> genuinely different stances");
        // Deterministic.
        assert_eq!(driver_stance(1, &axes), s1);
        // Fewer axes than slots -> generic tail fallback, never a panic.
        let s3 = driver_stance(3, &axes);
        assert!(s3.starts_with("anti-modal:"));
        // No axes at all -> modal first, generic anti-modal after.
        assert!(driver_stance(0, &[]).starts_with("modal:"));
        assert!(driver_stance(1, &[]).starts_with("anti-modal:"));
    }

    #[test]
    fn budget_defaults_to_three_and_caps_at_five() {
        assert_eq!(endpoint_budget(""), 3);
        assert_eq!(endpoint_budget("junk"), 3);
        assert_eq!(endpoint_budget("-1"), 3);
        assert_eq!(endpoint_budget("1"), 1);
        assert_eq!(endpoint_budget("5"), 5);
        assert_eq!(endpoint_budget("10"), 5);
    }

    #[test]
    fn writer_prompt_carries_the_bundle_written_contract() {
        // ADR-006: the writer parks the bundle in Written via BundleWritten; the
        // diversity gate, not the writer, starts repair.
        let p = writer_prompt(&driver_stance(0, &[]), false);
        for needle in [
            "temper.action(\"Endpoints\", \"e-1\", \"BundleWritten\"",
            "\"bundle_file_id\"",
            "\"summary\"",
            "\"author_agent_id\": \"a-1\"",
            "gate, not you, starts repair",
        ] {
            assert!(p.contains(needle), "writer prompt missing: {needle}");
        }
        // The writer must NOT self-report SubmitForRepair anymore.
        assert!(!p.contains("\"SubmitForRepair\""));
    }

    #[test]
    fn writer_prompt_gives_explicit_file_write_recipe() {
        let p = writer_prompt(&driver_stance(0, &[]), false);
        assert!(
            p.contains("temper.write(\"/bundle.md\""),
            "writer prompt must show the literal temper.write call"
        );
        assert!(
            p.contains("\"file_id\""),
            "writer prompt must name the file_id return field"
        );
        assert!(
            p.contains("Do NOT") && p.contains("Directories"),
            "writer prompt must forbid improvising Directories file creation"
        );
        assert!(
            !p.contains("<file-id-from-temper.write>"),
            "the bare placeholder must be gone: the recipe captures result[\"file_id\"]"
        );
    }

    #[test]
    fn re_steer_brief_demands_divergence_not_rewording() {
        let p = endpoint_writer_prompt(
            "w-1", "e-1", "a-1", "Test", "ai coding tools", "2026-12-11",
            &driver_stance(1, &[]), "", "",
            "Your future read too close to: \"agents replace junior devs\".",
            false,
        );
        assert!(p.contains("THIS IS A RE-STEER"));
        assert!(p.contains("agents replace junior devs"));
        assert!(p.contains("GENUINELY DIFFERENT"));
        // First drafts carry no re-steer banner.
        assert!(!writer_prompt(&driver_stance(1, &[]), false).contains("THIS IS A RE-STEER"));
    }

    #[test]
    fn writer_prompt_is_native_to_the_target_date_under_its_stance() {
        let stance = driver_stance(1, &[]);
        let p = writer_prompt(&stance, false);
        assert!(p.contains("NATIVE TO 2026-12-11"));
        assert!(p.contains("dated AT 2026-12-11"));
        assert!(p.contains(&stance), "the assigned stance must appear");
    }

    #[test]
    fn writer_prompt_enforces_the_skeleton_constraint() {
        let p = writer_prompt(&driver_stance(0, &[]), false);
        assert!(p.contains("temper.list(\"EventNodes\", \"world_id eq 'w-1'\")"));
        assert!(p.contains("may not contradict any \"determined\" node"));
    }

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        // All six session-spawning modules must derive the exact same
        // workspace name from a world id, or their sessions write into
        // different workspaces.
        assert_eq!(workspace_name("w-1"), "world-w-1");
        assert_eq!(workspace_name("0197abc"), "world-0197abc");
    }

    #[test]
    fn hindcast_mode_strips_web_tools_and_pins_the_vantage() {
        assert!(!tools_enabled(true).contains("web"));
        assert!(tools_enabled(false).contains("temper_web_search"));
        let p = writer_prompt(&driver_stance(0, &[]), true);
        assert!(p.contains("NO web access"));
        assert!(p.contains("never reference anything dated after the world's vantage"));
        assert!(!p.contains("temper.web_search /"));
        let open = writer_prompt(&driver_stance(0, &[]), false);
        assert!(open.contains("temper.web_search"));
    }
}
