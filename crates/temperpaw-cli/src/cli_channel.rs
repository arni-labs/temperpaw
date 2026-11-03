use paw_transport::PawApiClient;
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

const CONNECT_WAIT_ATTEMPTS: usize = 30;
const CONNECT_WAIT_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliChannel {
    pub entity_id: String,
    pub channel_id: String,
}

pub async fn ensure_cli_channel(api: &PawApiClient, profile: &str) -> Result<CliChannel, String> {
    let channel_id = cli_channel_id(profile);
    let escaped_channel = escape_odata(&channel_id);
    let mut existing = api
        .query_entities(
            "Channels",
            &format!("channel_type eq 'cli' and channel_id eq '{escaped_channel}'"),
            20,
        )
        .await?;
    if existing.is_empty() {
        existing = api
            .query_entities(
                "Channels",
                &format!("ChannelType eq 'cli' and ChannelId eq '{escaped_channel}'"),
                20,
            )
            .await?;
    }

    if let Some(channel) = select_cli_channel(existing, &channel_id) {
        let entity_id = entity_id(&channel)
            .ok_or_else(|| "existing cli Channel is missing an entity id".to_string())?
            .to_string();
        ensure_connected(api, &entity_id, &channel_id, channel_status(&channel)).await?;
        return Ok(CliChannel {
            entity_id,
            channel_id,
        });
    }

    let created = api
        .create_entity("Channels", json!({ "ChannelType": "cli" }))
        .await?;
    let entity_id = entity_id(&created)
        .ok_or_else(|| "created cli Channel did not return an entity id".to_string())?
        .to_string();

    api.dispatch_action(
        "Channels",
        &entity_id,
        "Paw.Channel.Configure",
        json!({
            "channel_type": "cli",
            "channel_id": channel_id,
            "guild_id": "",
            "default_agent_config": "{}",
            "webhook_secret": "",
            "webhook_url": "",
        }),
    )
    .await?;

    api.dispatch_action("Channels", &entity_id, "Paw.Channel.Connect", json!({}))
        .await?;
    wait_for_connected(api, &entity_id).await?;

    Ok(CliChannel {
        entity_id,
        channel_id,
    })
}

async fn ensure_connected(
    api: &PawApiClient,
    entity_id: &str,
    channel_id: &str,
    status: &str,
) -> Result<(), String> {
    match status {
        "Connected" => Ok(()),
        "Disconnected" => {
            api.dispatch_action("Channels", entity_id, "Paw.Channel.Reconnect", json!({}))
                .await?;
            wait_for_connected(api, entity_id).await
        }
        "Created" => {
            api.dispatch_action(
                "Channels",
                entity_id,
                "Paw.Channel.Configure",
                json!({
                    "channel_type": "cli",
                    "channel_id": channel_id,
                    "guild_id": "",
                    "default_agent_config": "{}",
                    "webhook_secret": "",
                    "webhook_url": "",
                }),
            )
            .await?;
            api.dispatch_action("Channels", entity_id, "Paw.Channel.Connect", json!({}))
                .await?;
            wait_for_connected(api, entity_id).await
        }
        "Connecting" => wait_for_connected(api, entity_id).await,
        other => Err(format!(
            "cli Channel {entity_id} is in unsupported state {other}"
        )),
    }
}

async fn wait_for_connected(api: &PawApiClient, entity_id: &str) -> Result<(), String> {
    for _ in 0..CONNECT_WAIT_ATTEMPTS {
        let channel = api.get_entity("Channels", entity_id).await?;
        if channel_status(&channel) == "Connected" {
            return Ok(());
        }
        sleep(CONNECT_WAIT_DELAY).await;
    }

    Err(format!(
        "timed out waiting for cli Channel {entity_id} to become Connected"
    ))
}

pub fn receive_message_params(
    message_id: &str,
    author_id: &str,
    thread_id: &str,
    content: &str,
    command: &str,
) -> Value {
    json!({
        "message_id": message_id,
        "author_id": author_id,
        "thread_id": thread_id,
        "content": content,
        "command": command,
        "gen_ai_parent_trace_id": "",
        "gen_ai_parent_span_id": "",
    })
}

pub fn cli_channel_id(profile: &str) -> String {
    let profile = profile.trim();
    if profile.is_empty() {
        "cli:local".to_string()
    } else {
        format!("cli:{profile}")
    }
}

fn entity_id(value: &Value) -> Option<&str> {
    field_str(value, &["entity_id", "Id", "id"])
}

fn channel_status(value: &Value) -> &str {
    field_str(value, &["Status", "status"]).unwrap_or("")
}

fn channel_type(value: &Value) -> &str {
    field_str(value, &["channel_type", "ChannelType"]).unwrap_or("")
}

fn channel_external_id(value: &Value) -> &str {
    field_str(value, &["channel_id", "ChannelId"]).unwrap_or("")
}

fn field_str<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .or_else(|| value.get("fields")?.get(*name)?.as_str())
    })
}

fn select_cli_channel(channels: Vec<Value>, channel_id: &str) -> Option<Value> {
    let mut candidates = channels.into_iter().filter(|channel| {
        channel_type(channel) == "cli"
            && channel_external_id(channel) == channel_id
            && channel_status(channel) != "Archived"
    });

    let first = candidates.next()?;
    if channel_status(&first) == "Connected" {
        return Some(first);
    }

    candidates
        .find(|channel| channel_status(channel) == "Connected")
        .or(Some(first))
}

fn escape_odata(value: &str) -> String {
    value.replace('\'', "''")
}
