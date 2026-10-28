use std::fs;
use std::path::{Path, PathBuf};

const OLD_TEMPER_REV: &str = "c584a52b59924e66502576646f50131b0d763a2a";

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

/// The Temper rev the server builds against: temper-platform's pin in crates/temperpaw/Cargo.toml.
fn server_temper_rev() -> String {
    repo_file("crates/temperpaw/Cargo.toml")
        .lines()
        .find(|line| {
            line.starts_with("temper-platform = ") && line.contains("arni-labs/temper.git")
        })
        .and_then(|line| line.split("rev = \"").nth(1))
        .and_then(|rest| rest.get(..40))
        .map(str::to_string)
        .expect("crates/temperpaw/Cargo.toml must pin temper-platform to an arni-labs/temper rev")
}

fn collect_cargo_tomls(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let entry = entry.unwrap_or_else(|e| panic!("read_dir entry {}: {e}", dir.display()));
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("file_type {}: {e}", entry.path().display()));
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_cargo_tomls(&path, out);
        } else if file_type.is_file() && path.file_name().is_some_and(|name| name == "Cargo.toml") {
            out.push(path);
        }
    }
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

#[test]
fn packaged_wasm_sdk_pins_match_temper_dependency_revision() {
    let root = repo_root();
    let expected_rev = server_temper_rev();
    let mut manifests = Vec::new();
    collect_cargo_tomls(&root.join("os-apps"), &mut manifests);

    let mut stale = Vec::new();
    for manifest in manifests {
        let source = fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        if source.contains("temper-wasm-sdk")
            && (source.contains(OLD_TEMPER_REV) || !source.contains(&expected_rev))
        {
            stale.push(
                manifest
                    .strip_prefix(&root)
                    .unwrap_or(&manifest)
                    .display()
                    .to_string(),
            );
        }
    }

    assert!(
        stale.is_empty(),
        "packaged WASM manifests must pin temper-wasm-sdk to the server's Temper rev {expected_rev}; stale: {stale:?}"
    );
}
