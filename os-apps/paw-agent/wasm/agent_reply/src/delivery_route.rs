use temper_wasm_sdk::prelude::*;
use wasm_helpers::entity_field_str;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryRoute {
    pub(crate) channel_id: String,
    pub(crate) thread_id: String,
    pub(crate) channel_entity_id: Option<String>,
    pub(crate) channel_type: Option<String>,
}

pub(crate) struct ChannelSessionLookup {
    pub(crate) filter: String,
    pub(crate) bound_id: String,
}

pub(crate) fn delivery_route_from_session_fields(fields: &Value) -> Option<DeliveryRoute> {
    let channel_id = entity_field_str(fields, &["reply_channel_id", "ReplyChannelId"])
        .filter(|value| !value.trim().is_empty())?;
    let thread_id = entity_field_str(fields, &["reply_thread_id", "ReplyThreadId"])
        .filter(|value| !value.trim().is_empty())?;
    let channel_entity_id =
        entity_field_str(fields, &["reply_channel_entity_id", "ReplyChannelEntityId"])
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
    let channel_type = entity_field_str(fields, &["reply_channel_type", "ReplyChannelType"])
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    Some(DeliveryRoute {
        channel_id: channel_id.to_string(),
        thread_id: thread_id.to_string(),
        channel_entity_id,
        channel_type,
    })
}

pub(crate) fn delivery_route_from_channel_session(session: &Value) -> Option<DeliveryRoute> {
    let channel_id = entity_field_str(session, &["ChannelId", "channel_id"])
        .filter(|value| !value.trim().is_empty())?;
    let thread_id = entity_field_str(session, &["ThreadId", "thread_id"])
        .filter(|value| !value.trim().is_empty())?;

    Some(DeliveryRoute {
        channel_id: channel_id.to_string(),
        thread_id: thread_id.to_string(),
        channel_entity_id: None,
        channel_type: None,
    })
}

pub(crate) fn should_skip_channel_session_lookup(
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> bool {
    let current_session_id = current_session_id.trim();
    !current_session_id.is_empty()
        && current_session_id == agent_id.trim()
        && parent_session_id.trim().is_empty()
}

pub(crate) fn channel_session_lookup_candidates(
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Vec<ChannelSessionLookup> {
    let mut candidates = Vec::new();
    let current_session_id = current_session_id.trim();
    let parent_session_id = parent_session_id.trim();
    let agent_id = agent_id.trim();

    if should_skip_channel_session_lookup(current_session_id, agent_id, parent_session_id) {
        return candidates;
    }

    if !current_session_id.is_empty() {
        let escaped = escape_odata(current_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: agent_id.to_string(),
        });
    }

    if !parent_session_id.is_empty() && parent_session_id != current_session_id {
        let escaped = escape_odata(parent_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: parent_session_id.to_string(),
        });
    }

    if !agent_id.is_empty() {
        let escaped = escape_odata(agent_id);
        candidates.push(ChannelSessionLookup {
            filter: format!("$filter=Status eq 'Active' and agent_entity_id eq '{escaped}'&$top=1"),
            bound_id: agent_id.to_string(),
        });
        candidates.push(ChannelSessionLookup {
            filter: format!("$filter=agent_entity_id eq '{escaped}'&$top=1"),
            bound_id: agent_id.to_string(),
        });
    }

    candidates
}

fn escape_odata(value: &str) -> String {
    value.replace('\'', "''")
}
