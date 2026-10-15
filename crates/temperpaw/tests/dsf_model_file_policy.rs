//! Registered model agents can maintain only the dedicated resource-config Files.
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
use temper_authz::{AuthzDecision, AuthzEngine, SecurityContext};

fn permits(ctx: &SecurityContext, action: &str, id: &str) -> bool {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps/dsf-twin/policies/model_investigation.cedar");
    let engine = AuthzEngine::new(&fs::read_to_string(path).unwrap()).unwrap();
    matches!(
        engine.authorize(
            ctx,
            action,
            "File",
            &HashMap::from([
                ("id".into(), json!(id)),
                ("status".into(), json!("")),
                ("has_spec".into(), json!(true)),
            ])
        ),
        AuthzDecision::Allow { .. }
    )
}

#[test]
fn factory_can_create_update_and_read_its_canonical_resource_configuration_files() {
    let ctx = SecurityContext::from_resolved_identity("factory", "dsf-factory", None);
    for action in ["create", "update", "read"] {
        assert!(
            permits(&ctx, action, "dsf-resource-config-production-api"),
            "{action}"
        );
    }
}

#[test]
fn configuration_file_capability_cannot_cross_prefix_or_delete() {
    let ctx = SecurityContext::from_resolved_identity("factory", "dsf-factory", None);
    for id in [
        "source-private",
        "dsf-resource-config-",
        "other-dsf-resource-config-api",
        "dsf-resource-config",
    ] {
        assert!(!permits(&ctx, "update", id), "{id}");
    }
    assert!(!permits(
        &ctx,
        "delete",
        "dsf-resource-config-production-api"
    ));
    assert!(!permits(&ctx, "list", ""));
}

#[test]
fn unverified_headers_and_other_registered_agents_cannot_use_config_file_capability() {
    for ctx in [
        SecurityContext::anonymous().with_agent_context(Some("spoof"), None, Some("dsf-factory")),
        SecurityContext::from_resolved_identity("worker", "worker", None),
        SecurityContext::from_resolved_identity("operator", "operator", None),
    ] {
        assert!(!permits(
            &ctx,
            "update",
            "dsf-resource-config-production-api"
        ));
    }
}

#[test]
fn ordinary_paw_reads_keep_their_permissions_on_a_bounded_worker_stack() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps/paw-patrol/policies/patrol.cedar");
    let engine = std::sync::Arc::new(AuthzEngine::new(&fs::read_to_string(path).unwrap()).unwrap());
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            let ctx = SecurityContext::from_resolved_identity("worker", "worker", None);
            for entity in ["WorkerRun", "PatrolRun", "WorkerAgent", "Ask", "Effort"] {
                assert!(
                    matches!(
                        engine.authorize(
                            &ctx,
                            "list",
                            entity,
                            &HashMap::from([
                                ("id".into(), json!("")),
                                ("status".into(), json!("")),
                                ("has_spec".into(), json!(true)),
                            ])
                        ),
                        AuthzDecision::Allow { .. }
                    ),
                    "{entity}"
                );
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
