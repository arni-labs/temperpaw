//! The platform operator reads FileVersions and removes the ones whose File is gone: File deletes from before
//! `FileVersion.File` cascaded (#590) left their versions behind. Cedar sees only a version's own fields, so it cannot
//! tell whether the File is still there. The version's `Orphan` move checks that (`File[file_id]` reads as the kernel's
//! tombstone of a deleted File), and the operator may delete only a version that made that move.
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
use temper_authz::{AuthzDecision, AuthzEngine, SecurityContext};
use temper_jit::table::{EvalContext, Related, TransitionTable};

fn os_app(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps/paw-fs")
        .join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

/// The version's `Orphan` move from `state`, with its File read as `file` (`None`: the read failed).
fn orphan(state: &str, file_id: &str, file: Vec<Option<&str>>) -> (bool, String) {
    let table = TransitionTable::from_ioa_source(&os_app("specs/file_version.ioa.toml"));
    let mut ctx = EvalContext::default();
    ctx.fields.insert("file_id".to_string(), json!(file_id));
    ctx.related.insert(
        ("File".to_string(), "file_id".to_string()),
        Related::Statuses(file.into_iter().map(|s| s.map(str::to_string)).collect()),
    );
    let result = table
        .evaluate_ctx(state, &ctx, "Orphan")
        .expect("FileVersion has an Orphan move");
    (result.success, result.new_state)
}

fn allowed(ctx: &SecurityContext, action: &str, status: &str) -> bool {
    let engine = AuthzEngine::new(&os_app("policies/file_version.cedar")).unwrap();
    matches!(
        engine.authorize(
            ctx,
            action,
            "FileVersion",
            &HashMap::from([
                ("id".into(), json!("fv-1")),
                ("Id".into(), json!("fv-1")),
                ("file_id".into(), json!("fl-1")),
                ("status".into(), json!(status)),
                ("Status".into(), json!(status)),
                ("has_spec".into(), json!(true)),
            ])
        ),
        AuthzDecision::Allow { .. }
    )
}

fn operator() -> SecurityContext {
    SecurityContext::from_resolved_identity("operator", "operator", None)
}

#[test]
fn an_operator_delete_of_a_version_whose_file_still_exists_is_refused() {
    // The File is there: the version cannot move to Orphaned, so it stays as it was.
    for file in ["Created", "Ready", "Locked", "Archived"] {
        for state in ["Current", "Superseded"] {
            assert_eq!(
                orphan(state, "fl-1", vec![Some(file)]),
                (false, state.to_string()),
                "{state}, File {file}"
            );
        }
    }
    for state in ["Current", "Superseded"] {
        // A File the kernel could not read is not taken for a deleted one. (A File id that was never
        // created reads as File's first state, Created, since the read starts that File.)
        assert_eq!(
            orphan(state, "fl-1", vec![None]),
            (false, state.to_string()),
            "{state}, File not read"
        );
        // And a version that did not move is not the operator's to delete.
        assert!(!allowed(&operator(), "delete", state), "{state}");
    }
}

#[test]
fn a_version_whose_file_is_gone_moves_to_orphaned_and_the_operator_may_delete_it() {
    for state in ["Current", "Superseded"] {
        // The kernel keeps a deleted File's tombstone, which reads as Deleted.
        assert_eq!(
            orphan(state, "fl-1", vec![Some("Deleted")]),
            (true, "Orphaned".to_string()),
            "{state}"
        );
    }
    // A version that names no File is not taken for one whose File is gone.
    assert!(!orphan("Current", "", vec![]).0);
    assert!(allowed(&operator(), "delete", "Orphaned"));
}

#[test]
fn the_operator_reads_and_lists_versions_and_moves_them() {
    for (action, status) in [
        ("read", "Current"),
        ("list", ""),
        ("read", "Superseded"),
        ("Orphan", "Current"),
        ("Orphan", "Superseded"),
    ] {
        assert!(allowed(&operator(), action, status), "{action} {status}");
    }
}

#[test]
fn only_the_verified_operator_reads_or_deletes_versions() {
    for ctx in [
        SecurityContext::anonymous().with_agent_context(Some("spoof"), None, Some("operator")),
        SecurityContext::from_resolved_identity("files", "file-service", None),
        SecurityContext::from_resolved_identity("worker", "worker", None),
    ] {
        for (action, status) in [
            ("read", "Current"),
            ("list", ""),
            ("Orphan", "Current"),
            ("delete", "Orphaned"),
        ] {
            assert!(!allowed(&ctx, action, status), "{action} {status}");
        }
    }
}
