//! Discord channel transport — Gateway WebSocket + REST API.
//!
//! Connects to Discord's Gateway (wss://gateway.discord.gg), receives
//! MESSAGE_CREATE events, and dispatches them as Channel.ReceiveMessage
//! actions via the Paw OData API. Watches for Channel.SendReply events
//! and delivers replies via Discord's REST API.
//!
//! This is a Paw OData API client — no dependency on paw-server internals.

pub mod types;

pub(crate) mod gateway;
mod observer;
mod transport;

pub use gateway::{
    create_forum_post, edit_discord_message, edit_message, send_channel_message,
    send_discord_message, send_discord_message_with_components, send_thread_message,
};
pub use observer::{ObserverConfig, run_observer};
pub use transport::{DiscordConfig, DiscordTransport};
