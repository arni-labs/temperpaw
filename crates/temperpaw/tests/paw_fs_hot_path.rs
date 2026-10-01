use std::fs;
use std::path::{Path, PathBuf};

const EXPECTED_TEMPER_REV: &str = "dbb7269f760495db6fcfa889f857407b47e36548";
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
fn monty_hot_path_does_not_route_file_io_through_workspace_actions() {
    let source = repo_file("os-apps/paw-agent/wasm/monty_repl/src/entity_ops.rs");
    for forbidden in [
        "Workspaces('{ws_id}')/Temper.MkDir",
        "Workspaces('{ws_id}')/Temper.CreateFile",
        "Workspaces('{ws_id}')/Temper.ResolvePath",
        "Workspaces('{ws_id}')/Temper.ListDir",
    ] {
        assert!(
            !source.contains(forbidden),
            "Monty hot-path file IO still calls legacy workspace action {forbidden}"
        );
    }
}

#[test]
fn file_stream_updated_no_longer_increments_workspace_usage() {
    let source = repo_file("os-apps/paw-fs/specs/file.ioa.toml");
    assert!(
        !source.contains("file_stream_updated_increments_workspace_usage"),
        "File.StreamUpdated must not charge every write to Workspace"
    );
    assert!(
        !source.contains("target_action = \"IncrementUsage\""),
        "File.StreamUpdated should emit usage deltas to WorkspaceUsageBucket instead"
    );
}

#[test]
fn workspace_fs_legacy_module_does_not_count_files_on_workspace_hot_path() {
    let source = repo_file("os-apps/paw-fs/wasm/workspace_fs/src/ops.rs");
    for forbidden in ["IncrementFileCount", "DecrementFileCount"] {
        assert!(
            !source.contains(forbidden),
            "legacy workspace_fs module still mutates Workspace.{forbidden}"
        );
    }
}

#[test]
fn artifact_batch_apply_uses_bounded_lossless_file_filters() {
    let source = repo_file("os-apps/paw-fs/wasm/artifact_batch_apply/src/lib.rs");
    assert!(
        !source.contains("Status ne 'Archived'"),
        "ArtifactBatch file and directory lookups must not use Status ne 'Archived'; it prevents query pushdown and causes QueryTooLarge"
    );
    assert!(
        source.contains("bounded_reads::find_first_non_archived_entity_id")
            && source.contains("bounded_reads::POINT_LOOKUP_TOP"),
        "ArtifactBatch lookups should use the shared bounded read helper"
    );
    assert!(
        source.contains("wasm_helpers::bounded_reads"),
        "ArtifactBatch should route Archived filtering through the shared helper"
    );
}

#[test]
fn skill_file_lookups_use_bounded_lossless_filters() {
    for path in [
        "os-apps/paw-skills/wasm/skill_installer/src/lib.rs",
        "os-apps/paw-agent/wasm/context_preparer/src/lib.rs",
    ] {
        let source = repo_file(path);
        assert!(
            !source.contains("Status ne 'Archived'"),
            "{path} must not put Status ne 'Archived' in Files filters; it causes QueryTooLarge at tenant scale"
        );
        assert!(
            source.contains("$top=20")
                || source.contains("$top={top}")
                || source.contains("bounded_reads::bounded_collection_query_url")
                || source.contains("bounded_reads::find_first_non_archived_entity_id"),
            "{path} should keep Files path/index lookups bounded"
        );
    }

    let context_preparer = repo_file("os-apps/paw-agent/wasm/context_preparer/src/lib.rs");
    assert!(
        context_preparer.contains("bounded_reads::entity_is_archived(item)"),
        "context_preparer should filter archived skill/mode files through the shared helper after bounded queries"
    );
}

#[test]
fn monty_pawfs_point_lookups_use_shared_bounded_helper() {
    let source = repo_file("os-apps/paw-agent/wasm/monty_repl/src/entity_ops.rs");
    assert!(
        source.contains("bounded_reads::bounded_collection_query_path"),
        "Monty PawFS point lookups should use the shared bounded collection helper"
    );
    assert!(
        source.contains("bounded_reads::POINT_LOOKUP_TOP"),
        "Monty PawFS point lookups should share the bounded point-lookup page size"
    );
}

#[test]
fn paw_fs_declares_artifact_batch_and_usage_bucket_entities() {
    let usage_bucket = repo_file("os-apps/paw-fs/specs/workspace_usage_bucket.ioa.toml");
    assert!(usage_bucket.contains("name = \"WorkspaceUsageBucket\""));
    assert!(usage_bucket.contains("name = \"ApplyDelta\""));

    let artifact_batch = repo_file("os-apps/paw-fs/specs/artifact_batch.ioa.toml");
    assert!(artifact_batch.contains("name = \"ArtifactBatch\""));
    for action in ["Submit", "Apply", "RecordFileApplied", "Complete", "Fail"] {
        assert!(
            artifact_batch.contains(&format!("name = \"{action}\"")),
            "ArtifactBatch missing {action}"
        );
    }
}

#[test]
fn monty_exposes_write_many_for_artifact_sets() {
    let source = repo_file("os-apps/paw-agent/wasm/monty_repl/src/entity_ops.rs");
    assert!(
        source.contains("write_many"),
        "Monty should expose temper.write_many(files, opts) for artifact sets"
    );
    assert!(
        source.contains("ArtifactBatch"),
        "write_many should be backed by the ArtifactBatch workflow"
    );
}

#[test]
fn default_agent_tool_allowlists_include_write_many() {
    for path in [
        "crates/temperpaw/src/startup.rs",
        "crates/temperpaw/src/setup_api.rs",
        "os-apps/paw-agent/wasm/plan_approval_handler/src/lib.rs",
        "os-apps/paw-agent/wasm/tool-catalog/src/lib.rs",
    ] {
        let source = repo_file(path);
        assert!(
            source.contains("temper_write_many"),
            "{path} should expose temper_write_many in default tool allowlists"
        );
    }
}

#[test]
fn artifact_batch_apply_has_wasm_host_authorization() {
    let policy = repo_file("os-apps/paw-fs/policies/wasm.cedar");
    assert!(
        policy.matches("artifact_batch_apply").count() >= 2,
        "artifact_batch_apply must be authorized for both http_call and access_secret host capabilities"
    );
}

#[test]
fn paw_fs_hot_path_entities_allow_odata_collection_create() {
    for (path, entity_type) in [
        ("os-apps/paw-fs/policies/workspace.cedar", "Workspace"),
        ("os-apps/paw-fs/policies/workspace.cedar", "Directory"),
        ("os-apps/paw-fs/policies/file.cedar", "File"),
        (
            "os-apps/paw-fs/policies/artifact_batch.cedar",
            "ArtifactBatch",
        ),
        (
            "os-apps/paw-fs/policies/workspace_usage_bucket.cedar",
            "WorkspaceUsageBucket",
        ),
    ] {
        let policy = repo_file(path);
        assert!(
            policy.contains("Action::\"create\"")
                && policy.contains(&format!("resource is {entity_type}")),
            "{path} must allow OData collection create for {entity_type}; Temper authorizes POST /tdata/* with lowercase create before bound Create actions"
        );
    }
}

#[test]
fn paw_fs_hot_path_entities_allow_agent_read_and_list_queries() {
    for (path, entity_type) in [
        ("os-apps/paw-fs/policies/workspace.cedar", "Directory"),
        ("os-apps/paw-fs/policies/file.cedar", "File"),
        (
            "os-apps/paw-fs/policies/artifact_batch.cedar",
            "ArtifactBatch",
        ),
        (
            "os-apps/paw-fs/policies/workspace_usage_bucket.cedar",
            "WorkspaceUsageBucket",
        ),
    ] {
        let policy = repo_file(path);
        assert!(
            policy.contains("Action::\"read\"")
                && policy.contains("Action::\"list\"")
                && policy.contains(&format!("resource is {entity_type}")),
            "{path} must allow direct PawFS hot-path agents to query {entity_type} entities without routing reads through Workspace"
        );
    }
}

#[test]
fn paw_fs_file_policy_permits_the_session_read_alias_family() {
    // temper.read from sessions authorizes content access through a family
    // of capitalized action names relayed as service:wasm-runtime. Dropping
    // any of them silently re-breaks every session file read (the session
    // dies instead of pausing — found live, hindcast surveyor, wall 13).
    let policy = repo_file("os-apps/paw-fs/policies/file.cedar");
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
            policy.contains(&format!("Action::\"{action}\"")),
            "file.cedar must permit the {action} read alias for the wasm-runtime relay"
        );
    }
    assert!(
        policy.contains("Agent::\"service:wasm-runtime\""),
        "the read alias family is scoped to the session relay principal"
    );
    // The scoping is only real if the any-principal permit stays narrow: a
    // revert that folds the aliases back into it would still satisfy the
    // substring above via the write permit.
    assert!(
        policy.contains("action in [Action::\"read\", Action::\"list\"]"),
        "the any-principal File permit must stay exactly lowercase read/list"
    );
}

#[test]
fn paw_fs_file_policy_allows_value_upload_update_on_direct_hot_path() {
    let policy = repo_file("os-apps/paw-fs/policies/file.cedar");
    assert!(
        policy.contains("Action::\"update\""),
        "PUT Files(...)/$value is authorized as lowercase update before File.StreamUpdated"
    );
    assert!(
        policy.contains("principal.agent_type == \"system\""),
        "system agent workflows such as ArtifactBatch and Monty writes need direct File update authority"
    );
}

#[test]
fn artifact_batch_apply_returns_explicit_success_to_wasm_host() {
    let source = repo_file("os-apps/paw-fs/wasm/artifact_batch_apply/src/lib.rs");
    assert!(
        source.contains("set_success_result"),
        "artifact_batch_apply must report a successful invocation after completing the batch"
    );
}

#[test]
fn packaged_wasm_sdk_pins_match_temper_dependency_revision() {
    let root = repo_root();
    let mut manifests = Vec::new();
    collect_cargo_tomls(&root.join("os-apps"), &mut manifests);

    let mut stale = Vec::new();
    for manifest in manifests {
        let source = fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        if source.contains("temper-wasm-sdk")
            && (source.contains(OLD_TEMPER_REV) || !source.contains(EXPECTED_TEMPER_REV))
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
        "packaged WASM manifests must pin temper-wasm-sdk to {EXPECTED_TEMPER_REV}; stale: {stale:?}"
    );
}
