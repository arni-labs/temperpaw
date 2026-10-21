//! spawn_repairers — attaches a repair route to a claim (ADR-002, ADR-004).
//!
//! On Claim.SubmitForBridge: create one Path entity (a route for this
//! claim), create a fresh repairer agent, bind it to the path, and spawn
//! its session. The repairer works BACKWARD from the claim — with the
//! endpoint's bundle inlined as context — proposing the intermediate
//! EventNodes the claim requires, declaring depends_on edges between them,
//! and flagging every place it had to bend the world. If the claim as
//! written cannot be bridged honestly, the repairer amends it through the
//! diff-carrying Claim.AmendText and flags the amendment as deformation
//! (the ADR-004 drift constraint). It self-reports Path.RepairComplete.
//!
//! Hindcast worlds (hindcast_mode = "true") get web tools stripped at
//! Configure time: evidence comes only from the frozen corpus.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const REPAIRER_TOOLS: &str =
    "temper_get,temper_list,temper_create,temper_action,temper_read,temper_write";
const WEB_TOOLS: &str = ",temper_web_search,temper_web_fetch";

fn tools_enabled(hindcast: bool) -> String {
    if hindcast {
        REPAIRER_TOOLS.to_string()
    } else {
        format!("{REPAIRER_TOOLS}{WEB_TOOLS}")
    }
}

/// Read one property from a single-entity OData response. Properties arrive
/// either snake_case inside a "fields" object or PascalCase at the top
/// level, depending on the endpoint shape — accept both.
fn entity_field(entity: &Value, snake: &str, pascal: &str) -> String {
    entity
        .get("fields")
        .and_then(|f| f.get(snake))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| entity.get(pascal).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

/// Truncate inlined bundle content at a char boundary; routes must never be
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
        "{}\n\n[TRUNCATED AT 30KB — ground the bridge only in text that survived the cut]",
        &raw[..end]
    )
}

/// Fetch and inline the world's lag table file, if it has one. Empty string
/// when the world declares no lag table or the file is unreadable (the prompt
/// degrades to a loud "no lag table" instruction rather than failing the
/// route — a missing table must never silently produce confident dates).
fn fetch_lag_table(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world: &Value,
) -> String {
    let file_id = entity_field(world, "lag_table_file_id", "LagTableFileId");
    if file_id.is_empty() {
        return String::new();
    }
    match ctx.http_call(
        "GET",
        &format!("{api}/tdata/Files('{file_id}')/$value"),
        headers,
        "",
    ) {
        Ok(r) if (200..300).contains(&r.status) => inline_file(&r.body),
        Ok(r) => {
            ctx.log(
                "warn",
                &format!(
                    "spawn_repairers: lag table {file_id} unreadable (HTTP {}); repairer will \
                     justify dates from cited durations instead",
                    r.status
                ),
            );
            String::new()
        }
        Err(e) => {
            ctx.log(
                "warn",
                &format!("spawn_repairers: lag table {file_id} fetch failed ({e})"),
            );
            String::new()
        }
    }
}

/// The repairer's working contract. Single source of truth for the action
/// and parameter names it must use — tested below, asserted nowhere else.
#[allow(clippy::too_many_arguments)]
fn repairer_prompt(
    path_id: &str,
    claim_id: &str,
    claim_text: &str,
    world_id: &str,
    agent_id: &str,
    bundle_inline: &str,
    lag_table_inline: &str,
    target_date: &str,
    revision_brief: &str,
    hindcast: bool,
) -> String {
    let bundle_block = if bundle_inline.is_empty() {
        "The endpoint reported no bundle; work from the claim and the skeleton.".to_string()
    } else {
        // Inlined, not temper.read: session file reads resolve by path inside
        // the session's workspace and cannot find WASM/harness-created files
        // by id (walls 13/15 — the v1 prompt's temper.read silently failed
        // and repairers degraded to the summary).
        format!(
            "The endpoint's full document bundle — the imagined future this claim belongs \
             to — is inlined below as CONTEXT between BEGIN BUNDLE and END BUNDLE.\n\
             --- BEGIN BUNDLE ---\n{bundle_inline}\n--- END BUNDLE ---"
        )
    };
    let revision_block = if revision_brief.is_empty() {
        String::new()
    } else {
        format!(
            "THIS IS A SEARCH STEP, not a first attempt. A prior route for this claim drew \
             these objections — find a CHEAPER mechanism that avoids them rather than \
             re-arguing them:\n{revision_brief}\n\n"
        )
    };
    let research_line = if hindcast {
        "This is a HINDCAST world: you have NO web access by design. Judge lags and incentives \
         from the corpus and the skeleton, and never reference anything dated after the \
         world's vantage."
            .to_string()
    } else {
        "Use temper.web_search / temper.web_fetch to check historical durations and actor \
         incentives against reality."
            .to_string()
    };
    // The lag table is the world's record of how long transitions in this
    // domain have HISTORICALLY taken. Inlined as the date anchor so the
    // repairer derives resolve_by dates from real durations rather than
    // eyeballing tidy month/quarter ends (D2 grounding).
    let lag_block = if lag_table_inline.trim().is_empty() {
        "No lag table was provided for this world. Justify every date from historical \
         durations you can cite, and flag any step that must move faster than its real-world \
         precedent."
            .to_string()
    } else {
        format!(
            "The world's LAG TABLE — historical durations for the kinds of transitions this \
             domain has seen — is inlined below. It is your date anchor.\n\
             --- BEGIN LAG TABLE ---\n{lag_table_inline}\n--- END LAG TABLE ---"
        )
    };
    let horizon = if target_date.trim().is_empty() {
        "the world's horizon".to_string()
    } else {
        target_date.to_string()
    };
    // "Today" is rhetorical, exactly as the surveyor frames it: there is no
    // stored present-date field, so the repairer resolves the present from the
    // web in live mode and from the corpus vantage in hindcast mode. The only
    // stored anchor is the horizon (target_date); the claim's own date is in
    // the claim text.
    let today_clause = if hindcast {
        "TODAY (this world's vantage — never date anything after it, and read it from the \
         corpus, not from your training cutoff)"
    } else {
        "TODAY (this world's present — verify the current date with web search rather than \
         assuming your training cutoff)"
    };
    let date_contract = format!(
        "DATE DISCIPLINE — every resolve_by must be earned, not rounded:\n\
         - Place every intermediate event between {today_clause} and the claim's own date \
         (the world's horizon is {horizon}).\n\
         - Date each node by adding the HISTORICAL LAG for that kind of transition (from the \
         LAG TABLE above) to the date of the prerequisite it depends on — never to a tidy \
         month- or quarter-end chosen for neatness.\n\
         - If a step must happen FASTER than its historical lag, that is a compressed lag: \
         raise a \"lag\" cost flag (severity by how far below history you pushed it) and name \
         the compressed transition in the note. A plausible-but-rushed date is still a cost.\n\
         - If the table has no row for a transition, derive the date from the closest analogue \
         it does list and name that analogue in your repair log.\n\n"
    );
    format!(
        "You are the Repairer for route {path_id} of claim {claim_id}, world {world_id}.\n\n\
         THE CLAIM you must bridge — this one assertion, not the whole future:\n\
         \"{claim_text}\"\n\n\
         {bundle_block}\n\n\
         {lag_block}\n\n\
         {revision_block}Read the skeleton: temper.list(\"EventNodes\", \"world_id eq \
         '{world_id}'\"). Nodes with provenance \"determined\" are settled facts your bridge \
         may not contradict.\n\
         {research_line}\n\n\
         Work BACKWARD from the claim: for THIS claim to hold by its date, what must have \
         happened, by when, done by whom? Derive the chain of intermediate events from the \
         claim back to the skeleton.\n\n\
         {date_contract}\
         For each required intermediate event, propose an EventNode:\n\
         temper.create(\"EventNodes\", {{\"world_id\": \"{world_id}\", \"statement\": \"...\", \
         \"layer\": \"mid|fast\", \"probability\": \"<honest 0-1>\", \"provenance\": \
         \"authored\", \"source_refs\": \"[]\", \"resolve_by\": \"YYYY-MM-DD\", \
         \"author_agent_id\": \"{agent_id}\"}})\n\
         Then declare what depends on what — for every node that requires an earlier node:\n\
         temper.action(\"EventNodes\", \"<later-node-id>\", \"UpdateEdges\", \
         {{\"edges\": \"[\\\"<earlier-node-id>\\\", ...]\"}})\n\n\
         If the claim AS WRITTEN cannot be bridged honestly but a weaker or narrower version \
         can, amend it — never silently:\n\
         temper.action(\"Claims\", \"{claim_id}\", \"AmendText\", {{\"old_text\": \"<exact \
         current text>\", \"new_text\": \"<amended text>\", \"justification\": \"<why>\", \
         \"agent_id\": \"{agent_id}\"}})\n\
         and add a \"deformation\" flag below — amendment is never free (severity: low = \
         clarified wording, medium = narrowed scope, high = changed meaning).\n\n\
         Flag every place you bend the world, honestly. Kinds:\n\
         - \"contradiction\": the repair conflicts with a determined node\n\
         - \"incentive\": an actor must act against its interests\n\
         - \"lag\": a process compressed below its historical duration\n\
         - \"miracle\": an unexplained discontinuity\n\
         - \"deformation\": you amended the claim to make it bridgeable\n\
         Severity: \"low\" | \"medium\" | \"high\". You flag costs; you NEVER compute scores — \
         costing is deterministic and runs elsewhere.\n\n\
         Write your repair log (markdown: the backward chain with your reasoning) with the \
         temper.write tool — this is the ONLY way to create a FILE, and your workspace already \
         exists. Do NOT create Files, Directories, or Workspaces yourself, and do NOT invent a \
         file-creation API: temper.create is for EventNodes only, never for files. Call \
         temper.write exactly like this:\n\
         result = temper.write(\"/repair-log.md\", \"<your markdown repair log>\")\n\
         temper.write returns {{\"file_id\": \"...\", \"path\": \"...\", \"workspace_id\": \
         \"...\"}}. Use result[\"file_id\"] as repair_log_file_id below, then self-report:\n\
         temper.action(\"Paths\", \"{path_id}\", \"RepairComplete\", {{\"repair_log_file_id\": \
         \"<result file_id>\", \"required_node_ids\": \"[\\\"<event-node-id>\\\", \
         ...]\", \"cost_flags\": \"[{{\\\"kind\\\": \\\"...\\\", \\\"severity\\\": \\\"...\\\", \
         \\\"note\\\": \\\"...\\\"}}]\"}})\n\
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

fn create_agent(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    name: &str,
    role: &str,
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
    serde_json::from_str::<Value>(&agent_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "Agent create returned no entity_id".to_string())
}

#[allow(clippy::too_many_arguments)]
fn start_session(
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
    let session_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions"),
        headers,
        &json!({ "agent_id": agent_id }).to_string(),
    )?;
    if session_resp.status < 200 || session_resp.status >= 300 {
        return Err(format!(
            "create Session for agent {agent_id} failed (HTTP {})",
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
            "Configure for agent {agent_id} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("spawn_repairers: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(session_id)
}

/// Entry point.
/// Revision mode: the Path itself re-enters Solving; spawn a repairer for
/// the same route with the adversary's objections inlined. No new Path, no
/// claim bookkeeping — the route keeps its identity and its round counter.
fn revise_route(
    ctx: &Context,
    _fields: &Value,
    get: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    let path_id = ctx.entity_id.clone();
    let claim_id = get("claim_id");
    let world_id = get("world_id");
    let revision_brief = get("revision_brief");
    if claim_id.is_empty() {
        return Err("RevisionRequested on a path without claim_id (legacy routes do not revise)".to_string());
    }

    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), path_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];

    let world_resp = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Worlds('{world_id}')"),
        &headers,
        "",
    )?;
    if world_resp.status < 200 || world_resp.status >= 300 {
        return Err(format!(
            "fetch World {world_id} failed (HTTP {})",
            world_resp.status
        ));
    }
    let world: Value = serde_json::from_str(&world_resp.body).unwrap_or(json!({}));
    let model = entity_field(&world, "agent_model", "AgentModel");
    let provider = entity_field(&world, "agent_provider", "AgentProvider");
    let hindcast = entity_field(&world, "hindcast_mode", "HindcastMode") == "true";
    let target_date = entity_field(&world, "target_date", "TargetDate");
    let lag_table_inline = fetch_lag_table(ctx, &api, &headers, &world);

    let claim_resp = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Claims('{claim_id}')"),
        &headers,
        "",
    )?;
    let claim: Value = serde_json::from_str(&claim_resp.body).unwrap_or(json!({}));
    let claim_text = {
        let current = entity_field(&claim, "current_text", "CurrentText");
        if current.trim().is_empty() {
            entity_field(&claim, "original_text", "OriginalText")
        } else {
            current
        }
    };

    let workspace_id = ensure_world_workspace(ctx, &api, &headers, &world_id)?;
    let repairer_agent_id = create_agent(
        ctx,
        &api,
        &headers,
        &format!("Repairer-{path_id}-rev"),
        "repairer",
    )?;
    let repairer_msg = repairer_prompt(
        &path_id,
        &claim_id,
        &claim_text,
        &world_id,
        "{AGENT_ID}",
        "",
        &lag_table_inline,
        &target_date,
        &revision_brief,
        hindcast,
    );
    start_session(
        ctx,
        &api,
        &headers,
        &repairer_agent_id,
        "repairer",
        &model,
        &provider,
        &tools_enabled(hindcast),
        "50",
        &repairer_msg,
        &workspace_id,
    )?;
    ctx.log(
        "info",
        &format!("spawn_repairers: revision repairer spawned for route {path_id}"),
    );
    // No dispatch: the repairer self-reports RepairComplete on this path.
    set_success_result("", &json!({}));
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let get = |k: &str| -> String {
            fields
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        // Two spawn modes (ADR-004):
        // - Claim.SubmitForBridge: the triggering entity is a Claim; create a
        //   NEW Path (route) and spawn its repairer.
        // - Path.RevisionRequested: the triggering entity is the Path itself;
        //   spawn a repairer against the SAME route, briefed on the
        //   objections it must answer.
        if ctx.trigger_action == "RevisionRequested" || ctx.trigger_action == "ResumeRepair" {
            // ResumeRepair (self-heal): re-spawn a repairer for this same route
            // after its session died; same path as a revision, just no new brief.
            return revise_route(&ctx, &fields, &get);
        }

        // The triggering entity is a Claim (ADR-004): one SubmitForBridge =
        // one new route for this claim.
        let claim_id = ctx.entity_id.clone();
        let world_id = get("world_id");
        if world_id.trim().is_empty() {
            return Err("Claim.world_id is required".to_string());
        }
        let endpoint_id = get("endpoint_id");
        let claim_text = {
            let current = get("current_text");
            if current.trim().is_empty() {
                get("original_text")
            } else {
                current
            }
        };
        if claim_text.trim().is_empty() {
            return Err("Claim has no text to bridge".to_string());
        }
        let revision_brief = get("revision_brief");
        let route_index = get("route_count").trim().parse::<usize>().unwrap_or(0);

        let api = ctx
            .config
            .get("temper_api_url")
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-tenant-id".to_string(), ctx.tenant.clone()),
            ("x-temper-principal-kind".to_string(), "agent".to_string()),
            ("x-temper-principal-id".to_string(), claim_id.clone()),
            ("x-temper-agent-type".to_string(), "system".to_string()),
        ];

        // The world owns the model/provider/hindcast configuration.
        let world_resp = ctx.http_call(
            "GET",
            &format!("{api}/tdata/Worlds('{world_id}')"),
            &headers,
            "",
        )?;
        if world_resp.status < 200 || world_resp.status >= 300 {
            return Err(format!(
                "fetch World {world_id} failed (HTTP {})",
                world_resp.status
            ));
        }
        let world: Value = serde_json::from_str(&world_resp.body).unwrap_or(json!({}));
        let model = entity_field(&world, "agent_model", "AgentModel");
        let provider = entity_field(&world, "agent_provider", "AgentProvider");
        if model.trim().is_empty() || provider.trim().is_empty() {
            return Err("World.agent_model and World.agent_provider are required".to_string());
        }
        let hindcast = entity_field(&world, "hindcast_mode", "HindcastMode") == "true";
        let target_date = entity_field(&world, "target_date", "TargetDate");
        let lag_table_inline = fetch_lag_table(&ctx, &api, &headers, &world);

        // The endpoint's bundle is the claim's context — inlined, because
        // session file reads cannot resolve WASM/harness-created file ids
        // (walls 13/15).
        let mut bundle_inline = String::new();
        let mut author_agent_id = String::new();
        if !endpoint_id.is_empty() {
            let ep_resp = ctx.http_call(
                "GET",
                &format!("{api}/tdata/Endpoints('{endpoint_id}')"),
                &headers,
                "",
            )?;
            if ep_resp.status >= 200 && ep_resp.status < 300 {
                let ep: Value = serde_json::from_str(&ep_resp.body).unwrap_or(json!({}));
                author_agent_id = entity_field(&ep, "author_agent_id", "AuthorAgentId");
                let bundle_file_id = entity_field(&ep, "bundle_file_id", "BundleFileId");
                if !bundle_file_id.is_empty() {
                    let bundle_resp = ctx.http_call(
                        "GET",
                        &format!("{api}/tdata/Files('{bundle_file_id}')/$value"),
                        &headers,
                        "",
                    )?;
                    if bundle_resp.status >= 200 && bundle_resp.status < 300 {
                        bundle_inline = inline_file(&bundle_resp.body);
                    } else {
                        ctx.log(
                            "warn",
                            &format!(
                                "spawn_repairers: bundle {bundle_file_id} unreadable (HTTP {}); \
                                 bridging from the claim alone",
                                bundle_resp.status
                            ),
                        );
                    }
                }
            }
        }

        // One workspace per world: the repairer writes its repair log there.
        let workspace_id = ensure_world_workspace(&ctx, &api, &headers, &world_id)?;

        // 1. The Path (route) exists before its repairer does: the repairer
        // self-reports RepairComplete against a real entity id.
        let path_body = json!({
            "world_id": world_id,
            "endpoint_id": endpoint_id,
            "claim_id": claim_id,
            "route_index": route_index.to_string(),
            "repairer_agent_id": "",
        });
        let path_resp = ctx.http_call(
            "POST",
            &format!("{api}/tdata/Paths"),
            &headers,
            &path_body.to_string(),
        )?;
        if path_resp.status < 200 || path_resp.status >= 300 {
            return Err(format!("create Path failed (HTTP {})", path_resp.status));
        }
        let path_id = serde_json::from_str::<Value>(&path_resp.body)
            .ok()
            .and_then(|v| {
                v.get("entity_id")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .ok_or("Path create returned no entity_id")?;

        // 2. Create the repairer agent. Freshly created here, so by
        // construction it can never equal the endpoint's author_agent_id —
        // the ADR repairer != author mechanism.
        let repairer_agent_id = create_agent(
            &ctx,
            &api,
            &headers,
            &format!("Repairer-{path_id}"),
            "repairer",
        )?;
        ctx.log(
            "info",
            &format!(
                "spawn_repairers: repairer {repairer_agent_id} != endpoint author \
                 {author_agent_id} by construction"
            ),
        );
        let patch_body = json!({ "repairer_agent_id": repairer_agent_id });
        match ctx.http_call(
            "PATCH",
            &format!("{api}/tdata/Paths('{path_id}')"),
            &headers,
            &patch_body.to_string(),
        ) {
            Ok(r) if r.status < 400 => {}
            Ok(r) => ctx.log(
                "warn",
                &format!(
                    "spawn_repairers: PATCH Paths('{path_id}') repairer_agent_id failed \
                     (HTTP {}); Cedar's assigned-repairer check won't bind",
                    r.status
                ),
            ),
            Err(e) => ctx.log(
                "warn",
                &format!(
                    "spawn_repairers: PATCH Paths('{path_id}') repairer_agent_id failed ({e}); \
                     Cedar's assigned-repairer check won't bind"
                ),
            ),
        }

        // 3. Spawn the repairer session.
        let repairer_msg = repairer_prompt(
            &path_id,
            &claim_id,
            &claim_text,
            &world_id,
            "{AGENT_ID}",
            &bundle_inline,
            &lag_table_inline,
            &target_date,
            &revision_brief,
            hindcast,
        );
        start_session(
            &ctx,
            &api,
            &headers,
            &repairer_agent_id,
            "repairer",
            &model,
            &provider,
            &tools_enabled(hindcast),
            "50",
            &repairer_msg,
            &workspace_id,
        )?;

        // 4. Record the attached route on the claim.
        set_success_result(
            "RoutesAttached",
            &json!({
                "path_ids": format!("[\"{path_id}\"]"),
                "route_count": (route_index + 1).to_string(),
            }),
        );
        ctx.log(
            "info",
            &format!(
                "spawn_repairers: route {path_id} (index {route_index}) attached to claim \
                 {claim_id} (repairer will report RepairComplete)"
            ),
        );
        Ok(())
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

    fn prompt(hindcast: bool) -> String {
        repairer_prompt(
            "p-1",
            "c-1",
            "the ledger becomes the unit of work",
            "w-1",
            "a-1",
            "bundle body text",
            "design review: median 4-6 weeks; standards adoption: 12-18 months",
            "2026-12-13",
            "",
            hindcast,
        )
    }

    #[test]
    fn repairer_prompt_carries_the_repair_complete_contract() {
        let p = prompt(false);
        for needle in [
            "temper.action(\"Paths\", \"p-1\", \"RepairComplete\"",
            "\"repair_log_file_id\"",
            "\"required_node_ids\"",
            "\"cost_flags\"",
        ] {
            assert!(p.contains(needle), "repairer prompt missing: {needle}");
        }
    }

    #[test]
    fn repairer_prompt_gives_explicit_file_write_recipe() {
        let p = prompt(false);
        assert!(
            p.contains("temper.write(\"/repair-log.md\""),
            "repairer prompt must show the literal temper.write call"
        );
        assert!(
            p.contains("\"file_id\""),
            "repairer prompt must name the file_id return field"
        );
        assert!(
            p.contains("Do NOT") && p.contains("Directories"),
            "repairer prompt must forbid improvising Directories file creation"
        );
        assert!(
            !p.contains("<file-id-from-temper.write>"),
            "the bare placeholder must be gone: the recipe captures result[\"file_id\"]"
        );
    }

    #[test]
    fn repairer_works_backward_and_flags_all_five_kinds_without_scoring() {
        let p = prompt(false);
        for needle in [
            "Work BACKWARD",
            "\"contradiction\"",
            "\"incentive\"",
            "\"lag\"",
            "\"miracle\"",
            "\"deformation\"",
            "NEVER compute scores",
        ] {
            assert!(p.contains(needle), "repairer prompt missing: {needle}");
        }
    }

    #[test]
    fn repairer_bridges_one_claim_with_the_bundle_inlined() {
        // ADR-004: the unit of repair is a claim, the bundle is inlined
        // context (temper.read cannot resolve WASM-created file ids), and
        // amendment goes through the diff-carrying AmendText + deformation.
        let p = prompt(false);
        for needle in [
            "THE CLAIM you must bridge",
            "the ledger becomes the unit of work",
            "BEGIN BUNDLE",
            "bundle body text",
            "temper.action(\"Claims\", \"c-1\", \"AmendText\"",
            "\"old_text\"",
            "\"new_text\"",
            "\"justification\"",
            "amendment is never free",
            "temper.action(\"EventNodes\", \"<later-node-id>\", \"UpdateEdges\"",
        ] {
            assert!(p.contains(needle), "repairer prompt missing: {needle}");
        }
        assert!(!p.contains("temper.read("), "bundles are inlined, never temper.read");
    }

    #[test]
    fn revision_brief_turns_the_prompt_into_a_search_step() {
        let p = repairer_prompt(
            "p-2",
            "c-1",
            "claim text",
            "w-1",
            "a-1",
            "",
            "",
            "2026-12-13",
            "lag/high: standards converge too fast via voluntary adoption",
            false,
        );
        assert!(p.contains("THIS IS A SEARCH STEP"));
        assert!(p.contains("standards converge too fast"));
        assert!(p.contains("CHEAPER mechanism"));
        // First routes carry no search banner.
        assert!(!prompt(false).contains("THIS IS A SEARCH STEP"));
    }

    #[test]
    fn repairer_proposes_authored_event_nodes() {
        let p = prompt(false);
        for needle in [
            "temper.create(\"EventNodes\"",
            "\"world_id\": \"w-1\"",
            "\"provenance\": \"authored\"",
            "\"author_agent_id\": \"a-1\"",
            "temper.list(\"EventNodes\", \"world_id eq 'w-1'\")",
        ] {
            assert!(p.contains(needle), "repairer prompt missing: {needle}");
        }
    }

    #[test]
    fn lag_table_and_date_discipline_anchor_dates_to_history() {
        // D2 grounding: the repairer must derive resolve_by dates from the
        // world's lag table, anchored to TODAY (rhetorical — there is no
        // present-date field) and the horizon, and price compression as a lag
        // cost — never eyeball tidy month/quarter ends.
        let p = prompt(false);
        for needle in [
            "BEGIN LAG TABLE",
            "standards adoption: 12-18 months",
            "DATE DISCIPLINE",
            "TODAY",
            "verify the current date with web search", // live mode: not the cutoff
            "2026-12-13",                              // target date is the horizon
            "HISTORICAL LAG",
            "compressed lag",
            "\"lag\" cost flag",
            "never to a tidy",
        ] {
            assert!(p.contains(needle), "repairer prompt missing: {needle}");
        }
        // frontier_date (the scoreable horizon) is NOT the present and must
        // never be presented to the repairer as "today".
        assert!(
            !p.contains("present (\"today\") is"),
            "frontier_date must not be mislabeled as today"
        );
    }

    #[test]
    fn hindcast_date_discipline_pins_the_vantage_not_the_clock() {
        // A hindcast repairer reads "today" from the corpus vantage, never the
        // live web or its training cutoff.
        let p = prompt(true);
        assert!(p.contains("this world's vantage"));
        assert!(p.contains("read it from the corpus"));
        assert!(!p.contains("verify the current date with web search"));
    }

    #[test]
    fn missing_lag_table_degrades_loudly_not_silently() {
        // No lag table for the world: the repairer is told so explicitly and
        // still must justify dates from cited durations — never a silent pass.
        let p = repairer_prompt(
            "p-1", "c-1", "claim", "w-1", "a-1", "bundle", "", "2026-12-13", "", false,
        );
        assert!(p.contains("No lag table was provided"));
        assert!(p.contains("DATE DISCIPLINE"));
        assert!(!p.contains("BEGIN LAG TABLE"));
    }

    #[test]
    fn empty_horizon_falls_back_to_a_descriptive_phrase() {
        // A world that never set a target date still gets a coherent date
        // contract rather than an empty placeholder.
        let p = repairer_prompt("p-1", "c-1", "claim", "w-1", "a-1", "", "", "", "", false);
        assert!(p.contains("the world's horizon"));
        assert!(p.contains("DATE DISCIPLINE"));
        assert!(p.contains("TODAY"));
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
        let p = prompt(true);
        assert!(p.contains("NO web access"));
        assert!(p.contains("never reference anything dated after the world's vantage"));
        assert!(!p.contains("temper.web_search /"));
        let open = prompt(false);
        assert!(open.contains("temper.web_search"));
    }

    #[test]
    fn entity_field_reads_snake_case_fields_and_pascal_case_top_level() {
        let snake = json!({ "fields": { "agent_model": "m1" } });
        assert_eq!(entity_field(&snake, "agent_model", "AgentModel"), "m1");

        let pascal = json!({ "AgentModel": "m2" });
        assert_eq!(entity_field(&pascal, "agent_model", "AgentModel"), "m2");

        // fields wins when both shapes are present and non-empty.
        let both = json!({ "AgentModel": "m2", "fields": { "agent_model": "m1" } });
        assert_eq!(entity_field(&both, "agent_model", "AgentModel"), "m1");

        // An empty snake_case value falls through to PascalCase.
        let empty_snake = json!({ "AgentModel": "m2", "fields": { "agent_model": "" } });
        assert_eq!(entity_field(&empty_snake, "agent_model", "AgentModel"), "m2");

        let neither = json!({});
        assert_eq!(entity_field(&neither, "agent_model", "AgentModel"), "");
    }
}
