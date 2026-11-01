//! Who may move the OpenAI Codex sign-in, under every policy paw-agent installs.
//!
//! The setup routes make each OpenAICodexAuth move as their caller: Admin from
//! the dashboard, or agent_type "system" when provider_auth_gate asks for a
//! fresh login during a Session. With no permit for the type, staging refused
//! even a read with "no matching permit policy".
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
use temper_authz::{AuthzDecision, AuthzEngine, Principal, PrincipalKind, SecurityContext};

/// Every paw-agent policy, joined in file-name order as the server loads them.
fn paw_agent_policies() -> AuthzEngine {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-agent/policies");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("paw-agent policies")
        .map(|entry| entry.expect("policy entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "cedar"))
        .collect();
    files.sort();
    let text = files
        .iter()
        .map(|path| fs::read_to_string(path).expect("policy text"))
        .collect::<Vec<_>>()
        .join("\n\n");
    AuthzEngine::new(&text).expect("paw-agent policies parse")
}

fn admin() -> SecurityContext {
    SecurityContext {
        principal: Principal {
            id: "admin".to_string(),
            kind: PrincipalKind::Admin,
            role: None,
            acting_for: None,
            agent_type: None,
            attributes: HashMap::new(),
        },
        context_attrs: HashMap::new(),
        correlation_id: "openai-codex-auth-permits".to_string(),
        cause: None,
        credential: None,
    }
}

fn allowed(engine: &AuthzEngine, ctx: &SecurityContext, action: &str) -> bool {
    matches!(
        engine.authorize(
            ctx,
            action,
            "OpenAICodexAuth",
            &HashMap::from([
                ("id".into(), json!("openai-codex-auth")),
                ("Id".into(), json!("openai-codex-auth")),
                ("status".into(), json!("Disconnected")),
                ("Status".into(), json!("Disconnected")),
            ])
        ),
        AuthzDecision::Allow { .. }
    )
}

const MOVES: &[&str] = &[
    "read",
    "StartDeviceLogin",
    "DeviceCodeReady",
    "SignInRequired",
    "PollDeviceLogin",
    "LoginComplete",
    "Refresh",
    "EnsureFresh",
    "ForceRefresh",
    "Disconnect",
    "Fail",
];

#[test]
fn the_setup_routes_callers_may_move_the_sign_in() {
    let engine = paw_agent_policies();
    let callers = [
        ("Admin", admin()),
        (
            "provider_auth_gate (system)",
            SecurityContext::from_resolved_identity("paw-runtime", "system", None),
        ),
        (
            "a supervisor",
            SecurityContext::from_resolved_identity("lead-1", "supervisor", None),
        ),
        (
            "a person",
            SecurityContext::from_resolved_identity("person-1", "human", None),
        ),
    ];
    for (who, ctx) in &callers {
        for action in MOVES {
            assert!(
                allowed(&engine, ctx, action),
                "{who} should be allowed OpenAICodexAuth.{action}"
            );
        }
    }
}

#[test]
fn an_ordinary_agent_may_not_touch_the_sign_in() {
    let engine = paw_agent_policies();
    let agent = SecurityContext::from_resolved_identity("agent-1", "agent", None);
    for action in MOVES {
        assert!(
            !allowed(&engine, &agent, action),
            "an agent_type \"agent\" caller should be refused OpenAICodexAuth.{action}"
        );
    }
}
