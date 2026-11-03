use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("temperpaw crate should live under crates/temperpaw")
        .to_path_buf()
}

fn repo_file(path: &str) -> String {
    let root = repo_root();
    fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn paw_fs_file_policy_permits_the_session_read_alias_family() {
    // temper.read from sessions authorizes content access through a family
    // of capitalized action names. Dropping any of them silently re-breaks
    // every session file read (the session dies instead of pausing).
    let engine = paw_fs_engine();
    let file = paw_fs_file();
    let reader = temper_authz::SecurityContext::from_resolved_identity("member-1", "human", None);
    for action in [
        "Read",
        "Download",
        "GetContent",
        "GetValue",
        "Stream",
        "Open",
        "GetText",
        "FetchContent",
        "Content",
    ] {
        assert!(
            engine
                .authorize(&reader, action, "File", &file)
                .is_allowed(),
            "file.cedar must permit the {action} read alias"
        );
    }
}

#[test]
fn paw_fs_session_writes_ride_moves_the_kernel_makes_for_the_caller() {
    let engine = paw_fs_engine();
    let file = paw_fs_file();
    let member =
        || temper_authz::SecurityContext::from_resolved_identity("member-1", "human", None);
    let made_for = |cause: serde_json::Value| {
        let mut ctx = member();
        ctx.cause = Some(serde_json::from_value(cause).expect("a move cause"));
        ctx
    };
    let hop = serde_json::json!({
        "rule": "Session:Turn:agent_turn",
        "action": "Turn",
        "entity_type": "Session",
        "entity_id": "s-1"
    });
    let mut tool_call = member();
    tool_call
        .context_attrs
        .insert("module".into(), serde_json::json!("monty_repl"));
    let relay = temper_authz::SecurityContext::from_resolved_identity(
        "service:wasm-runtime",
        "service",
        None,
    );
    for action in ["create", "Create", "StreamUpdated", "update"] {
        assert!(
            !engine
                .authorize(&member(), action, "File", &file)
                .is_allowed(),
            "a caller's own request cannot {action} a File"
        );
        for made in [
            made_for(serde_json::json!({ "answer": hop, "depth": 1 })),
            made_for(serde_json::json!({ "reaction": hop, "depth": 1 })),
            made_for(serde_json::json!({ "depth": 0, "timer": { "armed_by": "member-1" } })),
            tool_call.clone(),
            relay.clone(),
        ] {
            assert!(
                engine.authorize(&made, action, "File", &file).is_allowed(),
                "a session's {action} for its caller"
            );
        }
    }
}

fn paw_fs_engine() -> temper_authz::AuthzEngine {
    temper_authz::AuthzEngine::new(&repo_file("os-apps/paw-fs/policies/file.cedar"))
        .expect("file.cedar should parse")
}

fn paw_fs_file() -> std::collections::HashMap<String, serde_json::Value> {
    std::collections::HashMap::from([
        ("id".to_string(), serde_json::json!("f-1")),
        ("status".to_string(), serde_json::json!("Active")),
    ])
}
