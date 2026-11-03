//! Slack Socket Mode WebSocket lifecycle — connect, acknowledge, receive.
//!
//! Pure platform I/O. No Paw business logic here.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::types::*;

/// Slack API base URL.
pub(crate) const SLACK_API_BASE: &str = "https://slack.com/api";

#[derive(Clone, Copy)]
struct SocketEventLog<'a> {
    outcome: &'a str,
    envelope_id: &'a str,
    envelope_type: &'a str,
    error: &'a str,
}

fn log_socket_event(event: SocketEventLog<'_>) {
    tracing::info!(
        observability_event = "temperpaw.transport",
        transport.name = "slack",
        transport.operation = "socket_mode",
        transport.outcome = event.outcome,
        slack.envelope_id = event.envelope_id,
        slack.envelope_type = event.envelope_type,
        error.message = event.error,
        "slack socket mode event"
    );
}

// Type aliases kept for reference if needed for observer or other extensions.
// pub(crate) type WsSink = futures_util::stream::SplitSink<...>;
// pub(crate) type WsStream = futures_util::stream::SplitStream<...>;

/// Fetch a Socket Mode WebSocket URL from Slack.
///
/// POST `https://slack.com/api/apps.connections.open` with the App-Level Token.
/// Returns the WebSocket URL to connect to.
pub(crate) async fn fetch_socket_url(
    http: &reqwest::Client,
    app_token: &str,
) -> Result<String, String> {
    let resp = http
        .post(format!("{SLACK_API_BASE}/apps.connections.open"))
        .header("Authorization", format!("Bearer {app_token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await
        .map_err(|e| format!("Failed to open Socket Mode connection: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("apps.connections.open returned {status}: {body}"));
    }

    let body: ConnectionsOpenResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse connections.open response: {e}"))?;

    if !body.ok {
        return Err(format!(
            "apps.connections.open failed: {}",
            body.error.unwrap_or_else(|| "unknown error".to_string())
        ));
    }

    body.url
        .ok_or_else(|| "No WebSocket URL in response".to_string())
}

/// Connect to the Socket Mode WebSocket and run the event loop.
///
/// Receives envelopes, sends acknowledgments, and forwards events to the
/// provided handler callback. Returns on disconnect.
pub(crate) async fn connect_and_run<F, Fut>(url: &str, mut on_envelope: F) -> Result<(), String>
where
    F: FnMut(SlackEnvelope) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("Socket Mode WebSocket connect failed: {e}"))?;

    let (mut write, mut read) = ws.split();

    log_socket_event(SocketEventLog {
        outcome: "connected",
        envelope_id: "",
        envelope_type: "",
        error: "",
    });

    loop {
        let frame = match read.next().await {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => {
                return Err(format!("Socket Mode WebSocket read error: {e}"));
            }
            None => {
                log_socket_event(SocketEventLog {
                    outcome: "closed",
                    envelope_id: "",
                    envelope_type: "",
                    error: "",
                });
                return Ok(());
            }
        };

        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8(b.to_vec())
                .map_err(|e| format!("Invalid UTF-8 in Socket Mode frame: {e}"))?,
            Message::Ping(data) => {
                let _ = write.send(Message::Pong(data)).await;
                continue;
            }
            Message::Close(_) => {
                log_socket_event(SocketEventLog {
                    outcome: "close_frame",
                    envelope_id: "",
                    envelope_type: "",
                    error: "",
                });
                return Ok(());
            }
            _ => continue,
        };

        // Parse the envelope.
        let envelope: SlackEnvelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                log_socket_event(SocketEventLog {
                    outcome: "parse_error",
                    envelope_id: "",
                    envelope_type: "",
                    error: &e.to_string(),
                });
                continue;
            }
        };

        // Acknowledge immediately (must respond within 3 seconds).
        let ack = EnvelopeAck {
            envelope_id: envelope.envelope_id.clone(),
        };
        let ack_json = serde_json::to_string(&ack).unwrap_or_default();
        if let Err(e) = write.send(Message::Text(ack_json.into())).await {
            log_socket_event(SocketEventLog {
                outcome: "ack_failed",
                envelope_id: &envelope.envelope_id,
                envelope_type: &envelope.envelope_type,
                error: &e.to_string(),
            });
        }

        // Process the envelope.
        on_envelope(envelope).await;
    }
}
