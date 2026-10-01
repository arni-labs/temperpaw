use serde_json::json;
use std::sync::Arc;
use temper_authz::{AuthzEngine, SecurityContext};
use temper_jit::TransitionTable;
use temper_runtime::scheduler::SimActorHandler;
use temper_server::entity_actor::sim_handler::EntityActorHandler;
fn source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/specs/world.ioa.toml"),
    )
    .unwrap()
}
fn invoke(
    table: TransitionTable,
    action: &str,
    attempt: u64,
    id: &str,
) -> Result<serde_json::Value, String> {
    let mut w = EntityActorHandler::new("World", "world", Arc::new(table));
    w.init().unwrap();
    w.handle_message("Seed", "{}").unwrap();
    w.handle_message("ResearchSessionStarted",&json!({"research_session_id":"current","expected_research_attempt":1,"research_session_attempt":"1"}).to_string()).unwrap();
    let mut p = json!({"expected_research_attempt":attempt,"expected_research_session_id":id,"error_message":"test"});
    if action == "ResearchIncomplete" {
        p["research_session_attempt"] = json!("1");
    }
    w.handle_message(action, &p.to_string())
}
#[test]
fn stale_callbacks_reject_and_mutant_accepts() {
    for a in [
        "ResearchPending",
        "ResearchIncomplete",
        "ResearchFailed",
        "ResearchMonitorUnavailable",
    ] {
        for (param, attempt, id) in [
            ("expected_research_attempt", 0, "current"),
            ("expected_research_session_id", 1, "stale"),
        ] {
            assert!(
                invoke(TransitionTable::from_ioa_source(&source()), a, attempt, id).is_err(),
                "{a} {param}"
            );
            let mut m = TransitionTable::from_ioa_source(&source());
            m.action_contracts
                .get_mut(a)
                .unwrap()
                .constraints
                .retain(|c| c.param() != param);
            assert!(invoke(m, a, attempt, id).is_ok(), "mutant {a} {param}");
        }
        let valid = invoke(TransitionTable::from_ioa_source(&source()), a, 1, "current").unwrap();
        if a == "ResearchIncomplete" {
            assert_eq!(valid["counters"]["research_attempt"], 2);
            assert_eq!(valid["fields"]["research_session_attempt"], "1");
        }
    }
}
#[test]
fn forged_callbacks_denied_and_policy_mutant_accepts() {
    let p = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/policies/foresight.cedar"),
    )
    .unwrap();
    let engine = AuthzEngine::new(&p).unwrap();
    let wasm = SecurityContext::from_resolved_identity("service:wasm-runtime", "service", None);
    let operator = SecurityContext::from_resolved_identity("operator", "operator", None);
    let ordinary = SecurityContext::from_resolved_identity("session", "agent", None);
    let attrs = std::collections::HashMap::new();
    for a in [
        "ResearchPending",
        "ResearchIncomplete",
        "ResearchFailed",
        "ResearchMonitorUnavailable",
    ] {
        assert!(engine.authorize(&wasm, a, "World", &attrs).is_allowed());
        for principal in [&operator, &ordinary] {
            assert!(!engine.authorize(principal, a, "World", &attrs).is_allowed());
        }
    }
    let start = p.rfind("// The app monitor").unwrap();
    let mutant = format!(
        "{}\npermit(principal,action in [Action::\"ResearchPending\",Action::\"ResearchIncomplete\",Action::\"ResearchFailed\",Action::\"ResearchMonitorUnavailable\"],resource is World);",
        &p[..start]
    );
    let m = AuthzEngine::new(&mutant).unwrap();
    for a in [
        "ResearchPending",
        "ResearchIncomplete",
        "ResearchFailed",
        "ResearchMonitorUnavailable",
    ] {
        assert!(m.authorize(&operator, a, "World", &attrs).is_allowed());
    }
}

#[test]
fn unavailable_preserves_seeding_blocks_checks_but_allows_completion() {
    let mut w = EntityActorHandler::new(
        "World",
        "world",
        Arc::new(TransitionTable::from_ioa_source(&source())),
    );
    w.init().unwrap();
    w.handle_message("Seed", "{}").unwrap();
    w.handle_message("ResearchSessionStarted",&json!({"research_session_id":"current","expected_research_attempt":1,"research_session_attempt":"1"}).to_string()).unwrap();
    let state=w.handle_message("ResearchMonitorUnavailable",&json!({"expected_research_attempt":1,"expected_research_session_id":"current","error_message":"authorization denied"}).to_string()).unwrap();
    assert_eq!(state["status"], "Seeding");
    assert_eq!(state["fields"]["error_message"], "authorization denied");
    assert!(w.handle_message("CheckResearchSession", "{}").is_err());
    let done = w
        .handle_message(
            "SeedComplete",
            &json!({"skeleton_node_count":"1","graph_snapshot_file_id":"","uncertainty_axes":"[]"})
                .to_string(),
        )
        .unwrap();
    assert_eq!(done["status"], "Active");
}
