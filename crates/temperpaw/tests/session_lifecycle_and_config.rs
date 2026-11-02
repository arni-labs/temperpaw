use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

#[test]
fn runtime_model_provider_selection_has_no_hardcoded_llm_fallbacks() {
    let root = repo_root();
    let checked_files = [
        "os-apps/paw-agent/specs/session.ioa.toml",
        "os-apps/paw-agent/specs/agent.ioa.toml",
        "os-apps/paw-agent/specs/cron_job.ioa.toml",
        "os-apps/paw-wiki/specs/wiki_job.ioa.toml",
        "os-apps/paw-managed-agents/specs/managed_agent.ioa.toml",
        "os-apps/paw-agent/wasm/context_preparer/src/lib.rs",
        "os-apps/paw-agent/wasm/provider_caller/src/lib.rs",
        "os-apps/paw-agent/wasm/provider_response_applier/src/lib.rs",
        "os-apps/paw-agent/wasm/context_compactor/src/lib.rs",
        "os-apps/paw-agent/wasm/monty_repl/src/entity_ops.rs",
        "os-apps/paw-wiki/wasm/build_session_message/src/lib.rs",
        "os-apps/paw-channels/wasm/route_message/src/lib.rs",
        "os-apps/paw-managed-agents/wasm/common.rs",
        "os-apps/paw-managed-agents/wasm/session_orchestrator/src/lib.rs",
    ];
    let prohibited = [
        "initial = \"claude-sonnet",
        "initial = \"anthropic\"",
        "unwrap_or(\"claude-sonnet",
        "unwrap_or(\"anthropic\")",
        "unwrap_or_else(|| \"claude-sonnet",
        "unwrap_or_else(|| \"anthropic\"",
        "\"provider\": \"anthropic\"",
        "falling back to {alt}",
        "let alternatives = [\"anthropic\"",
        "infer_provider(",
    ];

    for relative in checked_files {
        let contents = read(root.join(relative));
        for needle in prohibited {
            assert!(
                !contents.contains(needle),
                "{relative} should not contain hardcoded runtime model/provider fallback `{needle}`"
            );
        }
    }
}
