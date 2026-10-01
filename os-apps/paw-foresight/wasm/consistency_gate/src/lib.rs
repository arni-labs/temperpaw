//! consistency_gate — the blocking gate between Drafted and published canon
//! (ADR-002).
//!
//! Two triggers land here, both with the artifact in Checking, split on the
//! triggering action:
//! - Artifact.SubmitForCheck: spawn a checker session
//!   that reads the artifact and its cited EventNodes, then self-reports a
//!   structured verdict via Artifact.CheckerVerdict.
//! - Artifact.CheckerVerdict: validate the verdict
//!   deterministically and own the transition — PassCheck only for a clean
//!   schema-valid pass, FailCheck for everything else.
//!
//! The LLM judges; the WASM owns the transition. A malformed or evasive
//! verdict never passes silently.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const CHECKER_TOOLS: &str = "temper_get,temper_list,temper_action,temper_read";

// --- Deterministic verdict validation --------------------------------------

/// The gate's decision after validating a checker verdict.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail { note: String },
}

/// Truncate on a char boundary so a giant violations list cannot blow the
/// check_failure_note field.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Strict schema validation of the checker's verdict_json: an object with a
/// boolean "pass" and an array "violations". Passing requires BOTH pass=true
/// and zero violations — a checker that lists violations but claims pass is
/// contradicting itself and fails the gate.
fn validate_verdict(raw: &str) -> Verdict {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Verdict::Fail {
                note: format!("malformed checker verdict: {e}"),
            };
        }
    };
    let Some(obj) = parsed.as_object() else {
        return Verdict::Fail {
            note: "malformed checker verdict: not a JSON object".to_string(),
        };
    };
    let Some(pass) = obj.get("pass").and_then(Value::as_bool) else {
        return Verdict::Fail {
            note: "malformed checker verdict: missing boolean \"pass\"".to_string(),
        };
    };
    let Some(violations) = obj.get("violations").and_then(Value::as_array) else {
        return Verdict::Fail {
            note: "malformed checker verdict: missing array \"violations\"".to_string(),
        };
    };
    if pass && violations.is_empty() {
        return Verdict::Pass;
    }
    let note = if violations.is_empty() {
        "checker reported pass=false (no violations listed)".to_string()
    } else {
        let serialized =
            serde_json::to_string(violations).unwrap_or_else(|_| "[unserializable]".to_string());
        truncate(&serialized, 2000)
    };
    Verdict::Fail { note }
}

// --- Checker prompt (spawn phase) -------------------------------------------

/// The checker's working contract. Single source of truth for the entity
/// sets, action, and parameter names it must use — tested below.
fn checker_prompt(artifact_id: &str, content_text: &str, cited_node_ids: &str) -> String {
    format!(
        "You are the consistency checker for artifact {artifact_id}. The artifact wants to \
         enter its world's canon; your verdict is the only thing standing between it and \
         publication. Judge it, do not fix it.\n\n\
         The artifact text is included below in full (sessions cannot reliably read \
         cross-workspace files, so the gate inlines it).\n\n\
         1. Read every cited EventNode: for each id in {cited_node_ids} call \
         temper.get(\"EventNodes\", \"<id>\")\n\n\
         Verify:\n\
         - Every citation exists: each cited node id must resolve to a real EventNode.\n\
         - No statement in the text contradicts a Resolved node's resolution, or any node \
         with provenance \"determined\".\n\
         - Dates inside the text are consistent with the cited nodes' resolve_by dates.\n\n\
         Report your verdict via STRICTLY this JSON shape (an object with boolean \"pass\" \
         and array \"violations\" — nothing else passes the gate):\n\
         temper.action(\"Artifacts\", \"{artifact_id}\", \"CheckerVerdict\", \
         {{\"verdict_json\": \"{{\\\"pass\\\": true|false, \\\"violations\\\": \
         [{{\\\"kind\\\": \\\"...\\\", \\\"where\\\": \\\"...\\\", \\\"note\\\": \
         \\\"...\\\"}}]}}\", \"checker_session_id\": \"{{SESSION_ID}}\"}})\n\
         pass=true requires an empty violations array. If you find violations, list every \
         one (kind, where, note) and set pass=false.\n\n\
         Then call temper.done(\"complete\").\n\n\
         ===== ARTIFACT TEXT =====\n{content_text}\n===== END ARTIFACT TEXT ====="
    )
}

/// Truncate inline artifact content at a char boundary; the verdict must
/// never be based on silently-missing text, so the truncation is loud.
fn inline_content(raw: &str) -> String {
    const CAP: usize = 30_000;
    if raw.len() <= CAP {
        return raw.to_string();
    }
    let mut end = CAP;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[TRUNCATED BY GATE AT 30KB — flag kind \"truncated_content\" if the remainder matters]", &raw[..end])
}

// --- Session spawning --------------------------------------------------------

/// Workspace name for a world — the cross-module rendezvous key. Every
/// session-spawning module must resolve the same per-world workspace by
/// this exact name.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

/// Resolve (or create) the per-world PawFS workspace and return its id.
/// The checker carries no temper_write tool, but every corridor session is
/// Configured with the world workspace uniformly — without one, any PawFS
/// File create is denied (resource.workspaceId must match the principal's
/// workspace).
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

    // The prompt needs real ids the module only knows now; substitute them.
    let message = user_message
        .replace("{AGENT_ID}", &agent_id)
        .replace("{SESSION_ID}", &session_id);
    let configure_body = json!({
        "model": model,
        "provider": provider,
        "agent_name": role,
        "tools_enabled": tools,
        "tool_choice": "required",
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
        &format!("consistency_gate: spawned {role} agent {agent_id} session {session_id}"),
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

        // Phase split on the TRIGGERING ACTION, not field emptiness: a
        // resubmitted artifact still carries its previous verdict_json, and
        // keying on the field made the gate validate the stale verdict
        // instead of spawning a fresh checker (live runs 8b/8c).
        match ctx.trigger_action.as_str() {
            "SubmitForCheck" => spawn_phase(&ctx, &fields),
            "CheckerVerdict" => validate_phase(&ctx, &get("verdict_json")),
            "PassCheck" => {
                // Gate-cleared artifacts publish automatically (deterministic
                // policy, ADR-004 C4): the gate owns both transitions.
                ctx.log(
                    "info",
                    &format!(
                        "consistency_gate: artifact {} gate-cleared; publishing",
                        ctx.entity_id
                    ),
                );
                set_success_result("Publish", &json!({}));
                Ok(())
            }
            other => {
                // Backward-compatible fallback for unexpected wiring.
                ctx.log(
                    "warn",
                    &format!("consistency_gate: unexpected trigger action {other}; falling back to field split"),
                );
                let verdict_json = get("verdict_json");
                if verdict_json.trim().is_empty() {
                    spawn_phase(&ctx, &fields)
                } else {
                    validate_phase(&ctx, &verdict_json)
                }
            }
        }
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

/// verdict_json empty (SubmitForCheck): spawn the checker session.
fn spawn_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let get = |k: &str| -> String {
        fields
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let artifact_id = ctx.entity_id.clone();
    let world_id = get("world_id");
    if world_id.is_empty() {
        return Err("Artifact.world_id is required to spawn a checker".to_string());
    }
    let content_file_id = get("content_file_id");
    if content_file_id.is_empty() {
        return Err("Artifact.content_file_id is required: nothing to check".to_string());
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
        ("x-temper-principal-id".to_string(), artifact_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];

    // Model and provider come from the owning world.
    let world_resp = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Worlds('{world_id}')"),
        &headers,
        "",
    )?;
    if world_resp.status < 200 || world_resp.status >= 300 {
        return Err(format!(
            "failed to read World {world_id} (HTTP {})",
            world_resp.status
        ));
    }
    let world: Value = serde_json::from_str(&world_resp.body).unwrap_or(json!({}));
    let world_field = |snake: &str, pascal: &str| -> String {
        world
            .get("fields")
            .and_then(|f| f.get(snake))
            .or_else(|| world.get(pascal))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let model = world_field("agent_model", "AgentModel");
    let provider = world_field("agent_provider", "AgentProvider");
    if model.trim().is_empty() || provider.trim().is_empty() {
        return Err("World.agent_model and World.agent_provider are required".to_string());
    }

    // The checker must read the artifact's content file, and temper.read
    // resolves inside the session's own workspace — so the checker joins the
    // ARTIFACT'S workspace (read off the File row). Falls back to a fresh
    // world workspace only if the File row hides its workspace.
    let workspace_id = {
        let file_url = format!("{api}/tdata/Files('{content_file_id}')");
        let file_ws = ctx
            .http_call("GET", &file_url, &headers, "")
            .ok()
            .filter(|r| (200..300).contains(&r.status))
            .and_then(|r| serde_json::from_str::<Value>(&r.body).ok())
            .map(|row| {
                row.get("fields")
                    .and_then(|f| f.get("workspace_id"))
                    .or_else(|| row.get("WorkspaceId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();
        if file_ws.is_empty() {
            ensure_world_workspace(ctx, &api, &headers, &world_id)?
        } else {
            file_ws
        }
    };

    // The gate fetches the artifact content itself and inlines it: session
    // file reads are workspace-scoped and the author's file may live in a
    // directory tree with no workspace at all (live runs 8a/8b both failed
    // honestly on artifact_unreadable before this).
    let content_url = format!("{api}/tdata/Files('{content_file_id}')/$value");
    let content_resp = ctx.http_call("GET", &content_url, &headers, "")?;
    if content_resp.status < 200 || content_resp.status >= 300 {
        return Err(format!(
            "gate could not fetch artifact content {content_file_id} (HTTP {})",
            content_resp.status
        ));
    }
    let content_text = inline_content(&content_resp.body);
    let prompt = checker_prompt(&artifact_id, &content_text, &get("cited_node_ids"));
    spawn_session(
        ctx,
        &api,
        &headers,
        &format!("Checker-{artifact_id}"),
        "checker",
        &model,
        &provider,
        CHECKER_TOOLS,
        "30",
        &prompt,
        &workspace_id,
    )?;
    // No result: the checker self-reports CheckerVerdict, which re-enters
    // this module in the validate phase.
    // A successful run with nothing to dispatch must still set a
    // result: the host treats an empty result as failure.
    set_success_result("", &json!({}));
    Ok(())
}

/// verdict_json non-empty (CheckerVerdict): deterministic gate decision.
fn validate_phase(ctx: &Context, verdict_json: &str) -> Result<(), String> {
    match validate_verdict(verdict_json) {
        Verdict::Pass => {
            ctx.log(
                "info",
                &format!("consistency_gate: artifact {} passed the gate", ctx.entity_id),
            );
            set_success_result("PassCheck", &json!({}));
        }
        Verdict::Fail { note } => {
            ctx.log(
                "info",
                &format!(
                    "consistency_gate: artifact {} failed the gate: {note}",
                    ctx.entity_id
                ),
            );
            set_success_result("FailCheck", &json!({ "check_failure_note": note }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Verdict validation matrix -----------------------------------------

    #[test]
    fn valid_pass_with_no_violations_passes() {
        assert_eq!(
            validate_verdict(r#"{"pass": true, "violations": []}"#),
            Verdict::Pass
        );
    }

    #[test]
    fn valid_fail_carries_the_serialized_violations() {
        let v = validate_verdict(
            r#"{"pass": false, "violations": [{"kind": "contradiction", "where": "para 2", "note": "node n-1 resolved no"}]}"#,
        );
        match v {
            Verdict::Fail { note } => {
                assert!(note.contains("contradiction"));
                assert!(note.contains("para 2"));
            }
            Verdict::Pass => panic!("a failing verdict must not pass the gate"),
        }
    }

    #[test]
    fn violations_with_pass_true_is_a_self_contradiction_and_fails() {
        let v = validate_verdict(
            r#"{"pass": true, "violations": [{"kind": "date", "where": "title", "note": "off by a year"}]}"#,
        );
        assert!(matches!(v, Verdict::Fail { .. }));
    }

    #[test]
    fn malformed_json_fails_with_a_malformed_note() {
        for raw in ["not json", "[1,2]", "\"a string\"", "42"] {
            match validate_verdict(raw) {
                Verdict::Fail { note } => {
                    assert!(
                        note.starts_with("malformed checker verdict:"),
                        "expected malformed note for {raw:?}, got {note:?}"
                    );
                }
                Verdict::Pass => panic!("malformed verdict {raw:?} must never pass"),
            }
        }
    }

    #[test]
    fn missing_or_mistyped_keys_fail() {
        for raw in [
            r#"{"violations": []}"#,                       // missing pass
            r#"{"pass": true}"#,                           // missing violations
            r#"{"pass": "true", "violations": []}"#,       // pass not boolean
            r#"{"pass": true, "violations": "none"}"#,     // violations not array
            r#"{"pass": null, "violations": []}"#,         // pass null
        ] {
            assert!(
                matches!(validate_verdict(raw), Verdict::Fail { .. }),
                "schema-invalid verdict {raw:?} must fail the gate"
            );
        }
    }

    #[test]
    fn pass_false_with_empty_violations_still_fails_honestly() {
        match validate_verdict(r#"{"pass": false, "violations": []}"#) {
            Verdict::Fail { note } => assert!(note.contains("pass=false")),
            Verdict::Pass => panic!("pass=false must fail"),
        }
    }

    #[test]
    fn failure_notes_are_truncated_to_2000_chars() {
        let huge_note = "x".repeat(5000);
        let raw = format!(
            r#"{{"pass": false, "violations": [{{"kind": "k", "where": "w", "note": "{huge_note}"}}]}}"#
        );
        match validate_verdict(&raw) {
            Verdict::Fail { note } => assert!(note.len() <= 2000),
            Verdict::Pass => panic!("must fail"),
        }
        // truncate is char-boundary safe.
        let multibyte = "é".repeat(1500);
        assert!(truncate(&multibyte, 2000).len() <= 2000);
    }

    // --- Workspace rendezvous ------------------------------------------------

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        // All six session-spawning modules must derive the exact same
        // workspace name from a world id, or their sessions land in
        // different workspaces.
        assert_eq!(workspace_name("w-1"), "world-w-1");
        assert_eq!(workspace_name("0197abc"), "world-0197abc");
    }

    // --- Prompt contract -----------------------------------------------------

    #[test]
    fn checker_prompt_carries_the_verdict_contract() {
        let p = checker_prompt("art-1", "The artifact body text.", r#"["n-1", "n-2"]"#);
        for needle in [
            "===== ARTIFACT TEXT =====",
            "The artifact body text.",
            "temper.get(\"EventNodes\"",
            "[\"n-1\", \"n-2\"]",
            "temper.action(\"Artifacts\", \"art-1\", \"CheckerVerdict\"",
            "verdict_json",
            "checker_session_id",
            "{SESSION_ID}",
            "\\\"pass\\\"",
            "\\\"violations\\\"",
            "temper.done",
        ] {
            assert!(p.contains(needle), "checker prompt missing: {needle}");
        }
        // The checker verifies, it never publishes.
        assert!(!p.contains("PassCheck"));
        assert!(!p.contains("Publish"));
    }
}
