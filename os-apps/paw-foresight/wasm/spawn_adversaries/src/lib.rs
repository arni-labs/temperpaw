//! spawn_adversaries — attacks a completed repair (ADR-002).
//!
//! On Path.RepairComplete: create a fresh adversary agent, bind it to the
//! path, and spawn its session. The adversary tries to BREAK the repair on
//! four fronts (contradictions, incentives, lags, miracles) and flags every
//! cost the repairer hid. It refutes only — no temper_create in its toolset,
//! because adversaries never add EventNodes — and it self-reports
//! Path.ChallengeComplete, which triggers deterministic costing.
//!
//! Hindcast worlds (hindcast_mode = "true") get web tools stripped at
//! Configure time: evidence comes only from the frozen corpus.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

/// No temper_create: adversaries refute, they never add nodes.
const ADVERSARY_TOOLS: &str = "temper_get,temper_list,temper_action,temper_read,temper_write";
const WEB_TOOLS: &str = ",temper_web_search,temper_web_fetch";

fn tools_enabled(hindcast: bool) -> String {
    if hindcast {
        ADVERSARY_TOOLS.to_string()
    } else {
        format!("{ADVERSARY_TOOLS}{WEB_TOOLS}")
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

/// The adversary's working contract. Single source of truth for the action
/// and parameter names it must use — tested below, asserted nowhere else.
fn adversary_prompt(
    path_id: &str,
    world_id: &str,
    repair_log_file_id: &str,
    bundle_file_id: &str,
    repair_flags: &str,
    hindcast: bool,
) -> String {
    let repair_log_line = if repair_log_file_id.is_empty() {
        "The repairer reported no log file; attack from the bundle and the skeleton alone."
            .to_string()
    } else {
        format!("Read the repair log first: temper.read(\"{repair_log_file_id}\").")
    };
    let bundle_line = if bundle_file_id.is_empty() {
        "No endpoint bundle was available.".to_string()
    } else {
        format!(
            "Read the endpoint bundle: temper.read(\"{bundle_file_id}\") — the future this \
             repair claims to reach."
        )
    };
    let flags = if repair_flags.trim().is_empty() {
        "[]"
    } else {
        repair_flags
    };
    let research_line = if hindcast {
        "This is a HINDCAST world: you have NO web access by design. Attack from the corpus, \
         the skeleton, and the logs alone, and never reference anything dated after the \
         world's vantage."
            .to_string()
    } else {
        "Use temper.web_search / temper.web_fetch to check claimed durations and actor \
         incentives against the historical record."
            .to_string()
    };
    format!(
        "You are the Adversary for path {path_id} in world {world_id}. Your job is to BREAK \
         this repair.\n\n\
         {repair_log_line}\n\
         {bundle_line}\n\
         Read the skeleton: temper.list(\"EventNodes\", \"world_id eq '{world_id}'\"). Nodes \
         with provenance \"determined\" are settled facts.\n\
         The repairer flagged these costs itself: {flags}\n\
         {research_line}\n\n\
         Attack on four fronts:\n\
         - \"contradiction\": determined nodes the repair conflicts with that the repairer \
         missed\n\
         - \"incentive\": actors made to act against their interests — reason about what each \
         named actor would actually do\n\
         - \"lag\": processes compressed below their historical durations\n\
         - \"miracle\": unexplained discontinuities dressed as ordinary steps\n\
         Also flag dishonesty: every cost the repairer should have flagged but didn't.\n\n\
         Produce your flags in the same shape: [{{\"kind\": \
         \"contradiction|incentive|lag|miracle\", \"severity\": \"low|medium|high\", \"note\": \
         \"...\"}}]. You refute; you never repair, and you never compute scores — do not add \
         or fix EventNodes, and costing is deterministic and runs elsewhere.\n\n\
         Write a challenge log with temper.write (markdown: each attack with its reasoning), \
         then self-report:\n\
         temper.action(\"Paths\", \"{path_id}\", \"ChallengeComplete\", \
         {{\"challenge_log_file_id\": \"<file-id-from-temper.write>\", \"challenge_flags\": \
         \"[{{\\\"kind\\\": \\\"...\\\", \\\"severity\\\": \\\"...\\\", \\\"note\\\": \
         \\\"...\\\"}}]\"}})\n\
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

fn dispatch_action(
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
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!(
            "{set}('{id}').{action} failed (HTTP {}): {}",
            resp.status,
            &resp.body[..resp.body.len().min(200)]
        ))
    }
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

    // The prompt needs the real agent id wherever it self-identifies.
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
        &format!("spawn_adversaries: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(session_id)
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

        let path_id = ctx.entity_id.clone();
        let world_id = get("world_id");
        if world_id.trim().is_empty() {
            return Err("Path.world_id is required".to_string());
        }
        let endpoint_id = get("endpoint_id");
        let repair_log_file_id = get("repair_log_file_id");
        let repair_flags = get("cost_flags");

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

        // One workspace per world: the adversary writes its challenge log
        // there. Without it, temper.write fails Cedar — hard error.
        let workspace_id = ensure_world_workspace(&ctx, &api, &headers, &world_id)?;

        // The endpoint holds the document bundle the repair claims to reach.
        // Losing it weakens the attack but doesn't block it: warn and proceed.
        let bundle_file_id = if endpoint_id.is_empty() {
            ctx.log("warn", "spawn_adversaries: path has no endpoint_id");
            String::new()
        } else {
            match ctx.http_call(
                "GET",
                &format!("{api}/tdata/Endpoints('{endpoint_id}')"),
                &headers,
                "",
            ) {
                Ok(r) if r.status >= 200 && r.status < 300 => {
                    let endpoint: Value = serde_json::from_str(&r.body).unwrap_or(json!({}));
                    entity_field(&endpoint, "bundle_file_id", "BundleFileId")
                }
                Ok(r) => {
                    ctx.log(
                        "warn",
                        &format!(
                            "spawn_adversaries: fetch Endpoint {endpoint_id} failed (HTTP {})",
                            r.status
                        ),
                    );
                    String::new()
                }
                Err(e) => {
                    ctx.log(
                        "warn",
                        &format!("spawn_adversaries: fetch Endpoint {endpoint_id} failed ({e})"),
                    );
                    String::new()
                }
            }
        };

        // Create the adversary agent and bind it to the path before session
        // spawn so Path.ChallengeComplete remains assigned-agent guarded.
        let adversary_agent_id = create_agent(
            &ctx,
            &api,
            &headers,
            &format!("Adversary-{path_id}"),
            "adversary",
        )?;
        dispatch_action(
            &ctx,
            &api,
            &headers,
            "Paths",
            &path_id,
            "AssignAdversary",
            &json!({ "adversary_agent_id": adversary_agent_id }),
        )?;

        // Spawn the adversary session.
        let adversary_msg = adversary_prompt(
            &path_id,
            &world_id,
            &repair_log_file_id,
            &bundle_file_id,
            &repair_flags,
            hindcast,
        );
        start_session(
            &ctx,
            &api,
            &headers,
            &adversary_agent_id,
            "adversary",
            &model,
            &provider,
            &tools_enabled(hindcast),
            "40",
            &adversary_msg,
            &workspace_id,
        )?;

        ctx.log(
            "info",
            &format!(
                "spawn_adversaries: adversary attached to path {path_id} \
                 (will report ChallengeComplete)"
            ),
        );
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

    fn prompt(hindcast: bool) -> String {
        adversary_prompt(
            "p-1",
            "w-1",
            "file-log",
            "file-bundle",
            r#"[{"kind":"lag","severity":"low","note":"t"}]"#,
            hindcast,
        )
    }

    #[test]
    fn adversary_prompt_carries_the_challenge_complete_contract() {
        let p = prompt(false);
        for needle in [
            "temper.action(\"Paths\", \"p-1\", \"ChallengeComplete\"",
            "\"challenge_log_file_id\"",
            "\"challenge_flags\"",
        ] {
            assert!(p.contains(needle), "adversary prompt missing: {needle}");
        }
        // The adversary's flags land in challenge_flags — never in the
        // repairer's field.
        assert!(!p.contains("cost_flags"));
    }

    #[test]
    fn adversary_breaks_on_four_fronts_and_audits_honesty() {
        let p = prompt(false);
        for needle in [
            "BREAK",
            "\"contradiction\"",
            "\"incentive\"",
            "\"lag\"",
            "\"miracle\"",
            "should have flagged but didn't",
            "never repair",
            "never compute scores",
        ] {
            assert!(p.contains(needle), "adversary prompt missing: {needle}");
        }
    }

    #[test]
    fn adversary_never_creates_nodes() {
        let p = prompt(false);
        assert!(!p.contains("temper.create"));
        assert!(!ADVERSARY_TOOLS.contains("temper_create"));
        assert!(!tools_enabled(false).contains("temper_create"));
        assert!(!tools_enabled(true).contains("temper_create"));
    }

    #[test]
    fn adversary_reads_both_logs_and_the_skeleton() {
        let p = prompt(false);
        for needle in [
            "temper.read(\"file-log\")",
            "temper.read(\"file-bundle\")",
            "temper.list(\"EventNodes\", \"world_id eq 'w-1'\")",
            r#"[{"kind":"lag","severity":"low","note":"t"}]"#,
        ] {
            assert!(p.contains(needle), "adversary prompt missing: {needle}");
        }
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
        let snake = json!({ "fields": { "bundle_file_id": "f1" } });
        assert_eq!(entity_field(&snake, "bundle_file_id", "BundleFileId"), "f1");

        let pascal = json!({ "BundleFileId": "f2" });
        assert_eq!(entity_field(&pascal, "bundle_file_id", "BundleFileId"), "f2");

        // An empty snake_case value falls through to PascalCase.
        let empty_snake = json!({ "BundleFileId": "f2", "fields": { "bundle_file_id": "" } });
        assert_eq!(
            entity_field(&empty_snake, "bundle_file_id", "BundleFileId"),
            "f2"
        );

        let neither = json!({});
        assert_eq!(entity_field(&neither, "bundle_file_id", "BundleFileId"), "");
    }
}
