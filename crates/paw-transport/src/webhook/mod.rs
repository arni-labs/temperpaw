//! Webhook trigger — HTTP endpoint that creates WebhookEvent entities.
//!
//! Receives HTTP POST requests at `/triggers/webhook/{route_key}`, creates
//! a WebhookEvent entity, dispatches the Received action, and returns the
//! event ID. ONE entity, ONE action — everything else is WASM integrations.
//!
//! This is a Paw OData API client — no dependency on paw-server internals.

mod admission;
mod trigger;

pub use trigger::{WebhookSecretResolver, WebhookTrigger, WebhookTriggerConfig, router};
