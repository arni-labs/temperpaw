//! Parse the versioned DSF operational endpoint; omit all unknown product fields.
use super::{Coverage, Facts};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
struct Snapshot {
    snapshot_version: u64,
    participant_limit: u64,
    job_limit: u64,
    observed_at: String,
    revision: Option<String>,
    service: String,
    environment: String,
    schema: Schema,
    participant_summary: ParticipantSummary,
    participants: ParticipantPage,
    action_queue: Jobs,
    media: Jobs,
    notifications: Jobs,
}
#[derive(Deserialize, Serialize)]
struct Schema {
    current_version: Option<String>,
    expected_version: Option<String>,
    is_current: bool,
}
#[derive(Deserialize, Serialize)]
struct ParticipantSummary {
    total: u64,
    agents: u64,
    humans: u64,
    active_last_24h: u64,
    heartbeat_last_24h: u64,
}
#[derive(Deserialize, Serialize)]
struct ParticipantPage {
    items: Vec<Participant>,
    next_cursor: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct Participant {
    id: String,
    #[serde(rename = "type")]
    kind: ParticipantKind,
    last_active_at: Option<String>,
    last_heartbeat_at: Option<String>,
    maintenance_until: Option<String>,
    expected_cycle_hours: Option<f64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ParticipantKind {
    Agent,
    Human,
}
#[derive(Deserialize, Serialize)]
struct Jobs {
    counts: BTreeMap<String, u64>,
    oldest_unfinished_at: Option<String>,
    jobs: Vec<Job>,
    has_more: bool,
}
#[derive(Deserialize, Serialize)]
struct Job {
    id: String,
    participant_id: String,
    status: String,
    created_at: String,
    age_seconds: u64,
    attempts: u64,
    retry_eligible: bool,
}

pub(super) fn parse(
    value: &Value,
    service: &str,
    environment: &str,
    max_age_seconds: u64,
    now_ms: i64,
) -> Result<Facts, String> {
    let snapshot: Snapshot =
        serde_json::from_value(value.clone()).map_err(|_| "invalid DSF snapshot response")?;
    if snapshot.revision.as_ref().is_some_and(|sha| {
        ![40, 64].contains(&sha.len())
            || !sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err("invalid DSF deployment revision".into());
    }
    if snapshot
        .participants
        .next_cursor
        .as_ref()
        .is_some_and(|cursor| uuid::Uuid::parse_str(cursor).is_err())
    {
        return Err("invalid DSF participant continuation cursor".into());
    }
    if snapshot.snapshot_version != 1 {
        return Err("unsupported DSF snapshot version".into());
    }
    if snapshot.service != service || snapshot.environment != environment {
        return Err("DSF snapshot identity mismatch".into());
    }
    if snapshot.participant_limit != 200
        || snapshot.job_limit != 20
        || snapshot.participants.items.len() > 200
        || [
            &snapshot.action_queue,
            &snapshot.media,
            &snapshot.notifications,
        ]
        .iter()
        .any(|q| q.jobs.len() > 20 || q.counts.len() > 32)
    {
        return Err("DSF snapshot exceeds requested bounds".into());
    }
    let observed_at = DateTime::parse_from_rfc3339(&snapshot.observed_at)
        .map_err(|_| "invalid DSF observed timestamp")?
        .timestamp_millis();
    if observed_at > now_ms + 5000 {
        return Err("DSF snapshot timestamp is in the future".into());
    }
    let coverage = if now_ms.saturating_sub(observed_at) > max_age_seconds as i64 * 1000 {
        Coverage::Stale
    } else {
        Coverage::Measured
    };
    let revision = snapshot.revision.clone().unwrap_or_default();
    let mut facts = serde_json::to_value(&snapshot).map_err(|_| "cannot serialize DSF snapshot")?;
    facts["participant_inventory_complete"] = json!(snapshot.participants.next_cursor.is_none());
    facts["participant_page_size"] = json!(snapshot.participants.items.len());
    // Schema mismatch and pending work are facts, not proof of a service outage.
    Ok(Facts {
        coverage,
        outcome: if coverage == Coverage::Stale {
            "stale_snapshot"
        } else {
            "snapshot_present"
        }
        .into(),
        revision,
        facts,
        sample_kind: "operational_snapshot",
    })
}
