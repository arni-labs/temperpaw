use std::collections::HashMap;
use std::fs;
use std::path::Path;

use temper_authz::{AuthzEngine, SecurityContext};

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn agent_context(id: &str, agent_type: &str) -> SecurityContext {
    SecurityContext::from_resolved_identity(id, agent_type, None)
}

fn resource_attrs(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

#[test]
fn inline_reply_delivered_policy_authorizes_only_inline_channels() {
    let policy =
        fs::read_to_string(repo_root().join("os-apps/paw-channels/policies/channels.cedar"))
            .expect("channels policy should exist");
    let engine = AuthzEngine::new(&policy).expect("channels.cedar should parse");
    let agent = agent_context("agent-1", "agent");
    let cli_channel = resource_attrs(&[
        ("id", serde_json::json!("ch-cli")),
        ("ChannelType", serde_json::json!("cli")),
    ]);
    let discord_channel = resource_attrs(&[
        ("id", serde_json::json!("ch-discord")),
        ("ChannelType", serde_json::json!("discord")),
    ]);

    assert!(
        engine
            .authorize(&agent, "ReplyDelivered", "Channel", &cli_channel)
            .is_allowed(),
        "agent should be allowed to record direct ReplyDelivered on inline cli channels"
    );
    assert!(
        !engine
            .authorize(&agent, "ReplyDelivered", "Channel", &discord_channel)
            .is_allowed(),
        "agent must not be allowed to bypass send_reply on webhook-backed channels"
    );
    assert!(
        engine
            .authorize(&agent, "SendReply", "Channel", &discord_channel)
            .is_allowed(),
        "agent should still use SendReply for webhook-backed channels"
    );
}
