//! Pure webhook admission configuration, authentication, and identity helpers.

use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderName};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const HARD_MAX_BODY_BYTES: usize = 1024 * 1024;
pub(super) const MAX_DELIVERY_ID_BYTES: usize = 256;
const MAX_DELIVERIES_PER_MINUTE: u32 = 10_000;
const MAX_DEDUP_WINDOW_MINUTES: usize = 10_080;
pub(super) const MAX_IN_FLIGHT_ADMISSIONS: usize = 32;
pub(super) const RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(super) struct RateWindow {
    pub(super) started_at: Instant,
    pub(super) accepted: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WebhookAuthScheme {
    HmacSha256,
}

#[derive(Debug, Clone)]
pub(super) struct WebhookRouteSnapshot {
    pub(super) route_id: String,
    pub(super) route_key: String,
    pub(super) source_type: String,
    pub(super) target_entity_type: String,
    pub(super) target_action: String,
    pub(super) auth_scheme: WebhookAuthScheme,
    pub(super) secret_ref: String,
    pub(super) signature_header: HeaderName,
    pub(super) delivery_id_header: HeaderName,
    pub(super) max_body_bytes: usize,
    pub(super) max_deliveries_per_minute: u32,
    pub(super) monitor_resolution_enabled: String,
    pub(super) dedup_enabled: String,
    pub(super) dedup_window_minutes: String,
}

impl WebhookRouteSnapshot {
    pub(super) fn from_entity(entity: &Value) -> Result<Self, String> {
        let required = |name: &str| {
            route_field(entity, name)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("webhook route is missing {name}"))
        };

        let route_id = entity
            .get("entity_id")
            .or_else(|| entity.get("Id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "webhook route is missing its entity ID".to_string())?;
        let route_key = required("route_key")?;
        if !valid_token(&route_key, 128) {
            return Err("webhook route_key is not a valid route token".into());
        }
        let source_type = required("source_type")?;
        if !valid_token(&source_type, 128) {
            return Err("webhook source_type is not a valid source token".into());
        }
        let target_entity_type = required("target_entity_type")?;
        if !valid_identifier(&target_entity_type, 128) {
            return Err("webhook target_entity_type is not a valid identifier".into());
        }
        let target_action = required("target_action")?;
        if target_action.len() > 256
            || !target_action
                .split('.')
                .all(|segment| valid_identifier(segment, 64))
        {
            return Err("webhook target_action is not a valid qualified action".into());
        }

        let auth_scheme = match required("auth_scheme")?.as_str() {
            "hmac-sha256" => WebhookAuthScheme::HmacSha256,
            value => return Err(format!("unsupported webhook auth scheme '{value}'")),
        };
        let secret_ref = required("secret_ref")?;
        if secret_ref.len() > 128
            || !secret_ref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err("webhook secret_ref must be a vault key, not a value or template".into());
        }

        let signature_header = parse_header_name(&required("signature_header")?)?;
        let delivery_id_header = parse_header_name(&required("delivery_id_header")?)?;
        let max_body_bytes = parse_budget(
            route_field(entity, "max_body_bytes").unwrap_or("262144"),
            "max_body_bytes",
            HARD_MAX_BODY_BYTES,
        )?;
        let max_deliveries_per_minute = parse_budget(
            route_field(entity, "max_deliveries_per_minute").unwrap_or("120"),
            "max_deliveries_per_minute",
            MAX_DELIVERIES_PER_MINUTE as usize,
        )? as u32;
        let monitor_resolution_enabled = parse_bool(
            route_field(entity, "monitor_resolution_enabled").unwrap_or("false"),
            "monitor_resolution_enabled",
        )?;
        let dedup_enabled = parse_bool(
            route_field(entity, "dedup_enabled").unwrap_or("false"),
            "dedup_enabled",
        )?;
        let dedup_window_minutes = parse_budget(
            route_field(entity, "dedup_window_minutes").unwrap_or("60"),
            "dedup_window_minutes",
            MAX_DEDUP_WINDOW_MINUTES,
        )?
        .to_string();

        Ok(Self {
            route_id,
            route_key,
            source_type,
            target_entity_type,
            target_action,
            auth_scheme,
            secret_ref,
            signature_header,
            delivery_id_header,
            max_body_bytes,
            max_deliveries_per_minute,
            monitor_resolution_enabled,
            dedup_enabled,
            dedup_window_minutes,
        })
    }

    pub(super) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        update_hash_part(&mut hasher, b"hmac-sha256");
        for value in [
            self.route_id.as_str(),
            self.route_key.as_str(),
            self.source_type.as_str(),
            self.target_entity_type.as_str(),
            self.target_action.as_str(),
            self.secret_ref.as_str(),
            self.signature_header.as_str(),
            self.delivery_id_header.as_str(),
            self.monitor_resolution_enabled.as_str(),
            self.dedup_enabled.as_str(),
            self.dedup_window_minutes.as_str(),
        ] {
            update_hash_part(&mut hasher, value.as_bytes());
        }
        update_hash_part(&mut hasher, &self.max_body_bytes.to_be_bytes());
        update_hash_part(&mut hasher, &self.max_deliveries_per_minute.to_be_bytes());
        hex::encode(hasher.finalize())
    }
}

pub(super) fn route_field<'a>(entity: &'a Value, name: &str) -> Option<&'a str> {
    entity
        .get("fields")
        .and_then(|fields| fields.get(name))
        .or_else(|| entity.get(name))
        .and_then(Value::as_str)
}

fn parse_budget(value: &str, name: &str, maximum: usize) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("webhook route has invalid {name}"))?;
    if parsed == 0 || parsed > maximum {
        return Err(format!(
            "webhook route {name} is outside its supported budget"
        ));
    }
    Ok(parsed)
}

fn parse_header_name(value: &str) -> Result<HeaderName, String> {
    HeaderName::from_bytes(value.as_bytes())
        .map_err(|_| format!("webhook route has invalid header name '{value}'"))
}

fn parse_bool(value: &str, name: &str) -> Result<String, String> {
    match value {
        "true" | "false" => Ok(value.to_string()),
        _ => Err(format!("webhook route has invalid {name}")),
    }
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= maximum
        && (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn valid_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(super) fn required_header(
    headers: &HeaderMap,
    name: &HeaderName,
    purpose: &str,
) -> Result<String, String> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing webhook {purpose} header '{}'", name.as_str()))?;
    if values.next().is_some() {
        return Err(format!(
            "multiple webhook {purpose} headers '{}' are not allowed",
            name.as_str()
        ));
    }
    Ok(value)
}

pub(super) fn signature_matches(secret: &[u8], body: &[u8], provided: &str) -> bool {
    use hmac::{Hmac, Mac};

    let normalized = provided.trim().to_ascii_lowercase();
    let provided_hex = normalized
        .strip_prefix("sha256=")
        .unwrap_or(normalized.as_str())
        .trim();
    let Ok(provided_bytes) = hex::decode(provided_hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided_bytes).is_ok()
}

pub(super) fn webhook_event_id(tenant: &str, route_id: &str, delivery_id: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [
        b"temperpaw-webhook-v1".as_slice(),
        tenant.as_bytes(),
        route_id.as_bytes(),
        delivery_id.as_bytes(),
    ] {
        update_hash_part(&mut hasher, part);
    }
    format!("wh-{}", hex::encode(hasher.finalize()))
}

fn update_hash_part(hasher: &mut Sha256, part: &[u8]) {
    hasher.update(part.len().to_be_bytes());
    hasher.update(part);
}
