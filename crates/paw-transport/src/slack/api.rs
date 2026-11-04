//! Slack Web API helpers — auth, messaging, message updates.
//!
//! All REST API calls use the Bot Token (xoxb-...) for authentication.

use super::types::*;

/// Slack API base URL.
pub(crate) const SLACK_API_BASE: &str = "https://slack.com/api";

/// Call `auth.test` to verify the bot token and get the bot's user ID.
pub(crate) async fn auth_test(
    http: &reqwest::Client,
    bot_token: &str,
) -> Result<AuthTestResponse, String> {
    let resp = http
        .post(format!("{SLACK_API_BASE}/auth.test"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .send()
        .await
        .map_err(|e| format!("auth.test failed: {e}"))?;

    let body: AuthTestResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse auth.test response: {e}"))?;

    if !body.ok {
        return Err(format!(
            "auth.test failed: {}",
            body.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(body)
}

/// Send a plain text message to a Slack channel.
///
/// Automatically splits messages exceeding 4000 characters.
pub async fn send_slack_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
) -> Result<(), String> {
    let chunks = split_message(text, 4000);
    for chunk in chunks {
        let body = serde_json::json!({
            "channel": channel,
            "text": chunk,
        });

        let resp = http
            .post(format!("{SLACK_API_BASE}/chat.postMessage"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("chat.postMessage failed: {e}"))?;

        let api_resp: SlackApiResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse chat.postMessage response: {e}"))?;

        if !api_resp.ok {
            return Err(format!(
                "chat.postMessage failed: {}",
                api_resp.error.unwrap_or_else(|| "unknown".to_string())
            ));
        }
    }
    Ok(())
}

/// Send a message with Block Kit blocks to a Slack channel.
///
/// Returns the API response (includes channel + ts for later updates).
pub async fn send_slack_message_with_blocks(
    http: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
    blocks: &[SlackBlock],
) -> Result<SlackApiResponse, String> {
    let body = serde_json::json!({
        "channel": channel,
        "text": text,
        "blocks": blocks,
    });

    let resp = http
        .post(format!("{SLACK_API_BASE}/chat.postMessage"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("chat.postMessage failed: {e}"))?;

    let api_resp: SlackApiResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse chat.postMessage response: {e}"))?;

    if !api_resp.ok {
        return Err(format!(
            "chat.postMessage failed: {}",
            api_resp.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(api_resp)
}

/// Update an existing Slack message (e.g., to remove buttons after a decision).
pub async fn update_slack_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    ts: &str,
    text: &str,
    blocks: Option<&[SlackBlock]>,
) -> Result<(), String> {
    let mut body = serde_json::json!({
        "channel": channel,
        "ts": ts,
        "text": text,
    });

    if let Some(blocks) = blocks {
        body["blocks"] = serde_json::to_value(blocks).unwrap_or_default();
    } else {
        // Empty blocks array removes all blocks.
        body["blocks"] = serde_json::json!([]);
    }

    let resp = http
        .post(format!("{SLACK_API_BASE}/chat.update"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("chat.update failed: {e}"))?;

    let api_resp: SlackApiResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse chat.update response: {e}"))?;

    if !api_resp.ok {
        return Err(format!(
            "chat.update failed: {}",
            api_resp.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    Ok(())
}

/// Split a message into chunks that fit within Slack's character limit.
/// UTF-8 safe — uses floor_char_boundary to avoid splitting multi-byte chars.
fn split_message(content: &str, max_len: usize) -> Vec<&str> {
    if content.len() <= max_len {
        return vec![content];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }

        let boundary = remaining.floor_char_boundary(max_len);
        let split_at = remaining[..boundary].rfind('\n').unwrap_or(boundary);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk);
        remaining = rest.trim_start_matches('\n');
    }

    chunks
}
