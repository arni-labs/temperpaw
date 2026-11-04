//! Slack channel transport — Socket Mode WebSocket + Web API.
//!
//! Connects to Slack via Socket Mode (wss://wss-primary.slack.com), receives
//! message events, and dispatches them as Channel.ReceiveMessage actions via
//! the Paw OData API. Watches for Channel.SendReply events and delivers
//! replies via Slack's Web API.
//!
//! This is a Paw OData API client — no dependency on paw-server internals.

pub mod types;

pub(crate) mod api;
mod socket;
mod transport;

pub use api::{send_slack_message, send_slack_message_with_blocks, update_slack_message};
pub use transport::{SlackConfig, SlackTransport};
