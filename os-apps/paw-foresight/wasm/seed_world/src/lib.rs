//! seed_world — spawns the skeleton builders for a world (ADR-002).
//!
//! On World.Seed: spawn a surveyor session (determined facts only — the
//! skeleton) and a bookmaker session (market-priced marginals — enrichment
//! that must never block the world). The surveyor self-reports
//! World.SeedComplete; the bookmaker just adds EventNodes while it can.
//!
//! Hindcast worlds (hindcast_mode = "true") get web tools stripped at
//! Configure time: evidence comes only from the frozen corpus.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const SURVEYOR_TOOLS: &str =
    "temper_get,temper_list,temper_create,temper_action,temper_read,temper_write";
const WEB_TOOLS: &str = ",temper_web_search,temper_web_fetch";

fn tools_enabled(hindcast: bool) -> String {
    if hindcast {
        SURVEYOR_TOOLS.to_string()
    } else {
        format!("{SURVEYOR_TOOLS}{WEB_TOOLS}")
    }
}

/// Truncate inlined corpus content at a char boundary; the skeleton must
/// never be grounded in silently-missing text, so the truncation is loud.
fn inline_corpus(raw: &str) -> String {
    const CAP: usize = 30_000;
    if raw.len() <= CAP {
        return raw.to_string();
    }
    let mut end = CAP;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[TRUNCATED AT 30KB — record only facts whose sources survived the cut]",
        &raw[..end]
    )
}

/// Fetch the corpus content for prompt inlining. A configured corpus that
/// cannot be read is a hard error — a world grounded in a corpus nobody can
/// see is not a world.
fn fetch_corpus(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    corpus_file_id: &str,
) -> Result<String, String> {
    if corpus_file_id.is_empty() {
        return Ok(String::new());
    }
    let url = format!("{api}/tdata/Files('{corpus_file_id}')/$value");
    let resp = ctx.http_call("GET", &url, headers, "")?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "seed_world: could not fetch corpus {corpus_file_id} (HTTP {})",
            resp.status
        ));
    }
    Ok(inline_corpus(&resp.body))
}

/// The surveyor's working contract. Single source of truth for the action
/// and parameter names it must use — tested below, asserted nowhere else.
fn surveyor_prompt(
    world_id: &str,
    agent_id: &str,
    name: &str,
    domain: &str,
    description: &str,
    target_date: &str,
    corpus_inline: &str,
    hindcast: bool,
) -> String {
    let corpus_line = if corpus_inline.is_empty() {
        "No corpus file was provided.".to_string()
    } else {
        // Inlined, not temper.read: session file reads resolve by path inside
        // the session's workspace, and a harness-uploaded corpus has neither —
        // three live surveyor sessions died thrashing on this before the
        // gate's inline pattern (wall 9) was applied here too.
        format!(
            "The world corpus — the domain documents this world is grounded in — is \
             inlined below between BEGIN CORPUS and END CORPUS.\n\n\
             --- BEGIN CORPUS ---\n{corpus_inline}\n--- END CORPUS ---"
        )
    };
    let research_line = if hindcast {
        "This is a HINDCAST world: you have NO web access by design. Use only the corpus. \
         Never state anything dated after the world's vantage."
            .to_string()
    } else {
        "Use temper.web_search / temper.web_fetch to verify what is already determined."
            .to_string()
    };
    format!(
        "You are the Surveyor for world {world_id} (\"{name}\", domain: {domain}).\n\
         {description}\n\n\
         Your job is the skeleton: record what is ALREADY DETERMINED about this domain \
         between today and {target_date}. You are forbidden from predicting. Determined \
         means: demographics already alive, infrastructure already funded or under \
         construction, dated commitments (elections, expirations, scheduled releases, \
         contract cliffs), regulations already enacted with future effect. If a claim \
         needs a probability, it is not yours to record.\n\n\
         {corpus_line}\n{research_line}\n\n\
         For each determined fact, create an EventNode:\n\
         temper.create(\"EventNodes\", {{\"world_id\": \"{world_id}\", \"statement\": \"...\", \
         \"layer\": \"slow|mid\", \"probability\": \"1.0\", \"provenance\": \"determined\", \
         \"source_refs\": \"[\\\"<url-or-corpus-ref>\\\"]\", \"resolve_by\": \"YYYY-MM-DD\", \
         \"author_agent_id\": \"{agent_id}\"}})\n\
         Aim for 8-20 load-bearing facts, slow layer first. Every statement needs a source.\n\n\
         THEN name the 3-5 load-bearing UNCERTAINTY AXES of this domain (ADR-006): the \
         dimensions on which {target_date} genuinely FORKS — not determined, the questions \
         whose answers most change the world. For each, give a short axis name and the \
         CONSENSUS POLE (what most informed observers currently expect). These are the \
         dimensions the engine will sample diverse futures across, so make them orthogonal \
         and genuinely contested — not restatements of one another.\n\n\
         Then write a one-page skeleton summary (markdown) with the temper.write tool — this \
         is the ONLY way to create a FILE, and your workspace already exists. Do NOT create \
         Files, Directories, or Workspaces yourself, and do NOT invent a file-creation API: \
         temper.create is for EventNodes only, never for files. Call temper.write exactly like \
         this:\n\
         result = temper.write(\"/skeleton.md\", \"<your markdown skeleton summary>\")\n\
         temper.write returns {{\"file_id\": \"...\", \"path\": \"...\", \"workspace_id\": \
         \"...\"}}. Use result[\"file_id\"] as graph_snapshot_file_id below, then finish by \
         self-reporting (uncertainty_axes is a JSON array of {{\"axis\": \"...\", \"consensus_pole\": \"...\"}}):\n\
         temper.action(\"Worlds\", \"{world_id}\", \"SeedComplete\", \
         {{\"skeleton_node_count\": \"<n>\", \"graph_snapshot_file_id\": \"<result file_id>\", \
         \"uncertainty_axes\": \"[{{\\\"axis\\\": \\\"...\\\", \\\"consensus_pole\\\": \\\"...\\\"}}, ...]\"}})\n\
         Then call temper.done(\"complete\")."
    )
}

/// The bookmaker's working contract: import live market-priced questions as
/// EventNodes. Enrichment only — it never reports world completion.
fn bookmaker_prompt(
    world_id: &str,
    agent_id: &str,
    domain: &str,
    target_date: &str,
    corpus_inline: &str,
    hindcast: bool,
) -> String {
    let source_line = if hindcast {
        let corpus_block = if corpus_inline.is_empty() {
            "No corpus was provided".to_string()
        } else {
            format!(
                "The frozen corpus is inlined below between BEGIN CORPUS and END \
                 CORPUS.\n\n--- BEGIN CORPUS ---\n{corpus_inline}\n--- END CORPUS ---"
            )
        };
        format!(
            "This is a HINDCAST world: no web access. If the frozen corpus contains recorded \
             market prices, import those; otherwise create nothing and finish.\n{corpus_block}"
        )
    } else {
        "Search public prediction markets (Polymarket, Kalshi, Metaculus) with \
         temper.web_search / temper.web_fetch for questions resolving before the target \
         date that bear on this domain."
            .to_string()
    };
    format!(
        "You are the Bookmaker for world {world_id} (domain: {domain}, target: {target_date}).\n\n\
         {source_line}\n\n\
         For each relevant market question, create an EventNode with the market's current \
         price as the probability:\n\
         temper.create(\"EventNodes\", {{\"world_id\": \"{world_id}\", \"statement\": \"<the \
         market question, verbatim>\", \"layer\": \"mid\", \"probability\": \"<0.00-1.00>\", \
         \"provenance\": \"market\", \"source_refs\": \"[\\\"<market-url>\\\"]\", \
         \"resolve_by\": \"YYYY-MM-DD\", \"author_agent_id\": \"{agent_id}\"}})\n\
         Import at most 10. Markets are enrichment: if none exist or fetches fail, that is \
         fine — never invent prices. When done (or stuck), call temper.done(\"complete\") \
         with a one-line summary."
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
        &format!("seed_world: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(agent_id)
}

/// Entry point.
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

        let world_id = ctx.entity_id.clone();
        let model = get("agent_model");
        let provider = get("agent_provider");
        if model.trim().is_empty() || provider.trim().is_empty() {
            return Err("World.agent_model and World.agent_provider are required".to_string());
        }
        let hindcast = get("hindcast_mode") == "true";
        let tools = tools_enabled(hindcast);

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
            ("x-temper-principal-id".to_string(), world_id.clone()),
            ("x-temper-agent-type".to_string(), "system".to_string()),
        ];

        // One workspace per world: every session this module spawns writes
        // its files there. Without it, temper.write fails Cedar — hard error.
        let workspace_id = ensure_world_workspace(&ctx, &api, &headers, &world_id)?;

        let corpus_inline = fetch_corpus(&ctx, &api, &headers, &get("corpus_file_id"))?;

        let surveyor_msg = surveyor_prompt(
            &world_id,
            "{AGENT_ID}",
            &get("name"),
            &get("domain"),
            &get("description"),
            &get("target_date"),
            &corpus_inline,
            hindcast,
        );
        spawn_session(
            &ctx,
            &api,
            &headers,
            &format!("Surveyor-{world_id}"),
            "surveyor",
            &model,
            &provider,
            &tools,
            "40",
            &surveyor_msg,
            &workspace_id,
        )?;

        // Bookmaker is enrichment: a spawn failure is logged, never fatal.
        let bookmaker_msg = bookmaker_prompt(
            &world_id,
            "{AGENT_ID}",
            &get("domain"),
            &get("target_date"),
            &corpus_inline,
            hindcast,
        );
        if let Err(e) = spawn_session(
            &ctx,
            &api,
            &headers,
            &format!("Bookmaker-{world_id}"),
            "bookmaker",
            &model,
            &provider,
            &tools,
            "30",
            &bookmaker_msg,
            &workspace_id,
        ) {
            ctx.log("warn", &format!("seed_world: bookmaker spawn failed: {e}"));
        }

        ctx.log("info", "seed_world: done (surveyor will report SeedComplete)");
        // A successful run with nothing to dispatch must still set a
        // result: the host treats an empty result as failure.
        set_success_result("", &json!({}));
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

    #[test]
    fn surveyor_prompt_carries_the_event_node_and_seed_complete_contract() {
        let p = surveyor_prompt(
            "w-1",
            "a-1",
            "Test",
            "ai coding tools",
            "desc",
            "2026-12-11",
            "frozen corpus body text",
            false,
        );
        for needle in [
            "temper.create(\"EventNodes\"",
            "\"world_id\": \"w-1\"",
            "\"provenance\": \"determined\"",
            "\"author_agent_id\": \"a-1\"",
            "temper.action(\"Worlds\", \"w-1\", \"SeedComplete\"",
            "skeleton_node_count",
            "graph_snapshot_file_id",
            "BEGIN CORPUS",
            "frozen corpus body text",
            "forbidden from predicting",
            // ADR-006: the surveyor also names the uncertainty axes that steer
            // the diverse worlds, reported in SeedComplete.
            "UNCERTAINTY AXES",
            "consensus_pole",
            "\"uncertainty_axes\":",
        ] {
            assert!(p.contains(needle), "surveyor prompt missing: {needle}");
        }
        // Sessions cannot resolve harness-uploaded files by id (path+workspace
        // resolution) — the corpus must be inlined, never temper.read.
        assert!(
            !p.contains("temper.read("),
            "surveyor prompt must not ask the session to read the corpus"
        );
    }

    #[test]
    fn surveyor_prompt_gives_explicit_file_write_recipe() {
        let p = surveyor_prompt(
            "w-1",
            "a-1",
            "Test",
            "ai coding tools",
            "desc",
            "2026-12-11",
            "corpus",
            false,
        );
        assert!(
            p.contains("temper.write(\"/skeleton.md\""),
            "surveyor prompt must show the literal temper.write call"
        );
        assert!(
            p.contains("\"file_id\""),
            "surveyor prompt must name the file_id return field"
        );
        assert!(
            p.contains("Do NOT") && p.contains("Directories"),
            "surveyor prompt must forbid improvising Directories file creation"
        );
        assert!(
            !p.contains("<file-id-from-temper.write>"),
            "the bare placeholder must be gone: the recipe captures result[\"file_id\"]"
        );
    }

    #[test]
    fn bookmaker_prompt_imports_market_provenance_only() {
        let p = bookmaker_prompt("w-1", "a-1", "ai coding tools", "2026-12-11", "", false);
        for needle in [
            "temper.create(\"EventNodes\"",
            "\"provenance\": \"market\"",
            "never invent prices",
        ] {
            assert!(p.contains(needle), "bookmaker prompt missing: {needle}");
        }
        assert!(
            !p.contains("SeedComplete"),
            "bookmaker must not report world completion"
        );
    }

    #[test]
    fn hindcast_bookmaker_sees_the_corpus_inline() {
        let p = bookmaker_prompt(
            "w-1",
            "a-1",
            "ai coding tools",
            "2025-12-11",
            "recorded market prices live here",
            true,
        );
        assert!(p.contains("BEGIN CORPUS"));
        assert!(p.contains("recorded market prices live here"));
        assert!(!p.contains("temper.read("));
    }

    #[test]
    fn corpus_inlining_truncates_loudly_at_cap() {
        let big = "x".repeat(40_000);
        let inlined = inline_corpus(&big);
        assert!(inlined.len() < 31_000);
        assert!(inlined.contains("TRUNCATED AT 30KB"));
        let small = inline_corpus("tiny");
        assert_eq!(small, "tiny");
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
    fn hindcast_mode_strips_web_tools_everywhere() {
        assert!(!tools_enabled(true).contains("web"));
        assert!(tools_enabled(false).contains("temper_web_search"));
        let p = surveyor_prompt("w", "a", "n", "d", "", "2026-01-01", "", true);
        assert!(p.contains("NO web access"));
        assert!(!p.contains("temper.web_search /"));
        let b = bookmaker_prompt("w", "a", "d", "2026-01-01", "", true);
        assert!(b.contains("no web access"));
    }
}
