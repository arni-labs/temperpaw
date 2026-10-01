//! Session transcript access follows the authenticated runtime identity.
use std::collections::HashMap;
use std::path::Path;
use temper_authz::{AuthzEngine, SecurityContext};

#[test]
fn trusted_runtime_can_append_and_read_session_entries_without_mutating_history() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps/paw-agent/policies/session_entry.cedar");
    let policy = std::fs::read_to_string(path).expect("read session entry policy");
    let engine = AuthzEngine::new(&policy).expect("parse session entry policy");
    let runtime =
        SecurityContext::from_resolved_identity("service:wasm-runtime", "wasm-runtime", None);
    let other = SecurityContext::from_resolved_identity("other-runtime", "wasm-runtime", None);
    let resource = HashMap::new();
    for action in ["create", "read", "list"] {
        assert!(
            engine
                .authorize(&runtime, action, "SessionEntry", &resource)
                .is_allowed(),
            "trusted session runtime must be able to {action} transcript records"
        );
        assert!(
            !engine
                .authorize(&other, action, "SessionEntry", &resource)
                .is_allowed(),
            "a matching agent type alone must not grant {action}"
        );
        assert!(
            !engine
                .authorize(
                    &SecurityContext::anonymous(),
                    action,
                    "SessionEntry",
                    &resource
                )
                .is_allowed()
        );
    }
    for action in ["update", "delete", "PATCH", "PUT", "DELETE"] {
        assert!(
            !engine
                .authorize(&runtime, action, "SessionEntry", &resource)
                .is_allowed(),
            "runtime must not rewrite existing transcript history through {action}"
        );
    }
}
