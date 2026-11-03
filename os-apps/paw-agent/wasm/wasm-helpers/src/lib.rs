//! Shared helper functions for Agent WASM modules.
//!
//! Provides common TemperFS I/O, field extraction, URL resolution,
//! and sandbox provider abstraction to eliminate duplication across
//! WASM integration modules.

pub mod sandbox;

use std::collections::BTreeMap;

use temper_wasm_sdk::prelude::*;

pub const SESSION_ENTRIES_REF_PREFIX: &str = "session-entries:";
const TEMPERFS_READ_ATTEMPTS: usize = 10;
const TEMPERFS_WRITE_ATTEMPTS: usize = 5;
const TEMPERFS_BATCH_READ_ATTEMPTS: usize = 3;
const SESSION_ENTRIES_PAGE_SIZE: usize = 1000;
const SESSION_ENTRIES_MAX_PAGES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTextFileReadItem {
    pub file_id: String,
    pub found: bool,
    pub content_hash: String,
    pub mime_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTextFileVersionReadItem {
    pub file_version_id: String,
    pub found: bool,
    pub content_hash: String,
    pub mime_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedContentFileRef {
    pub file_id: String,
    pub file_version_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSessionEntry {
    pub entity_id: String,
    pub entry_id: String,
}

#[derive(Clone, Copy)]
struct SessionEntryCreateSpec<'a> {
    session_id: &'a str,
    entry_id: &'a str,
    parent_entry_id: Option<&'a str>,
    sequence: i64,
    entry_type: &'a str,
    role: Option<&'a str>,
    content: Option<&'a Value>,
    content_file_id: Option<&'a str>,
    content_file_version_id: Option<&'a str>,
    extra_json: Option<&'a Value>,
    tokens: usize,
}

/// Current wall-clock time as a millis-since-epoch string.
///
/// Used as the value for OData fields shaped `last_*_at` (e.g.
/// `last_heartbeat_at`, `last_progress_at`, `last_message_at`). Historically
/// these fields were populated with sentinel words ("alive", "resumed",
/// "created") because no chrono dep existed in the WASM crates; this helper
/// gives ops a real, sortable, machine-parseable value without pulling in
/// chrono. Consumers that want an ISO timestamp can divide by 1000 and
/// format with any standard library — the string is the canonical i64
/// decimal representation of the host's wall clock.
pub fn timestamp_millis_string() -> String {
    Context::get_time_millis().to_string()
}

pub fn session_entries_ref(session_id: &str) -> String {
    format!("{SESSION_ENTRIES_REF_PREFIX}{session_id}")
}

pub fn session_id_from_entries_ref(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(SESSION_ENTRIES_REF_PREFIX)
        .filter(|session_id| !session_id.is_empty())
}

pub fn is_session_entries_ref(reference: &str) -> bool {
    session_id_from_entries_ref(reference).is_some()
}

pub fn next_session_entry_id(prefix: &str, parent_entry_id: &str) -> (String, i64) {
    let next_sequence = parent_session_entry_sequence(parent_entry_id).unwrap_or(0) + 1;
    (format!("{prefix}-{next_sequence}"), next_sequence)
}

fn parent_session_entry_sequence(parent_entry_id: &str) -> Option<i64> {
    let suffix = parent_entry_id
        .rsplit('-')
        .next()
        .and_then(|value| value.parse::<i64>().ok())?;
    if parent_entry_id.starts_with("u-ss-") {
        suffix.checked_mul(2)?.checked_add(1)
    } else {
        Some(suffix)
    }
}

fn read_temperfs_value_with_retry(
    ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    label: &str,
) -> Result<String, String> {
    read_temperfs_value_or_absent(ctx, url, headers, label).map(Option::unwrap_or_default)
}

/// Same read, with a missing file reported as `None` instead of an empty body.
///
/// A writer starting a new tree treats both the same way, which is why the
/// wrapper above collapses them. A reader deciding whether a record is complete
/// cannot: an absent file is missing history, an empty one is a session that has
/// not written history yet.
fn read_temperfs_value_or_absent(
    ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    label: &str,
) -> Result<Option<String>, String> {
    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..TEMPERFS_READ_ATTEMPTS {
        let resp = ctx.http_call("GET", url, headers, "")?;
        if resp.status == 200 {
            return Ok(Some(resp.body));
        }
        if resp.status == 404 {
            return Ok(None);
        }

        last_status = resp.status;
        last_body = resp.body;

        if (500..600).contains(&resp.status) && attempt + 1 < TEMPERFS_READ_ATTEMPTS {
            ctx.log(
                "warn",
                &format!(
                    "{label}: transient read failure (HTTP {}), retry {}/{}",
                    resp.status,
                    attempt + 2,
                    TEMPERFS_READ_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "{label} (HTTP {}): {}",
        last_status,
        error_excerpt(&last_body, 200)
    ))
}

fn is_retriable_write_failure(status: u16, body: &str) -> bool {
    (500..600).contains(&status) || body.contains("BlobUploadFailed") || body.contains("HTTP -1")
}

pub fn write_temperfs_value_with_retry(
    ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    label: &str,
) -> Result<(), String> {
    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..TEMPERFS_WRITE_ATTEMPTS {
        let resp = ctx.http_call("PUT", url, headers, body)?;
        if (200..300).contains(&resp.status) {
            return Ok(());
        }

        last_status = resp.status;
        last_body = resp.body;

        if is_retriable_write_failure(resp.status, &last_body)
            && attempt + 1 < TEMPERFS_WRITE_ATTEMPTS
        {
            ctx.log(
                "warn",
                &format!(
                    "{label}: transient write failure (HTTP {}), retry {}/{}",
                    resp.status,
                    attempt + 2,
                    TEMPERFS_WRITE_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "{label} (HTTP {}): {}",
        last_status,
        error_excerpt(&last_body, 200)
    ))
}

/// Resolve the Temper API URL from entity fields or context config,
/// falling back to localhost.
pub fn resolve_temper_api_url(ctx: &Context, fields: &Value) -> String {
    fields
        .get("temper_api_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            ctx.config
                .get("temper_api_url")
                .filter(|s| !s.is_empty())
                .cloned()
        })
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

/// Why a session transcript read produced no entries — or that it produced some.
///
/// A transcript has several ways of being empty and they are not the same fact.
/// A consumer that stores the result as a record of what an agent did has to be
/// able to tell "nothing has been written yet" from "what was written is gone".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptPresence {
    /// The read returned at least one transcript line.
    Present,
    /// The transcript is SessionEntry rows and none are materialized yet — a
    /// first-turn session that has not written one.
    PendingFirstTurn,
    /// The transcript is SessionEntry rows, materialization is on, and the
    /// query returned nothing.
    NoEntries,
    /// The legacy TemperFS file the session points at does not exist.
    MissingFile,
    /// The legacy TemperFS file exists and holds nothing.
    EmptyFile,
    /// The session declares no transcript reference at all.
    Undeclared,
}

impl TranscriptPresence {
    /// True only when the read produced transcript content.
    pub fn is_present(self) -> bool {
        matches!(self, TranscriptPresence::Present)
    }

    /// Stable wire name, used in logs and in the emitted trajectory.
    pub fn as_str(self) -> &'static str {
        match self {
            TranscriptPresence::Present => "present",
            TranscriptPresence::PendingFirstTurn => "pending_first_turn",
            TranscriptPresence::NoEntries => "no_entries",
            TranscriptPresence::MissingFile => "missing_file",
            TranscriptPresence::EmptyFile => "empty_file",
            TranscriptPresence::Undeclared => "undeclared",
        }
    }
}

/// A session transcript plus whether it is actually there.
#[derive(Debug, Clone)]
pub struct TranscriptRead {
    /// Transcript JSONL, empty for every non-`Present` presence.
    pub jsonl: String,
    /// Whether the transcript was found, and if not, why not.
    pub presence: TranscriptPresence,
}

/// Read session JSONL from TemperFS by file ID.
///
/// Every "no transcript" case collapses to an empty string, which is what a
/// writer resuming a tree wants. Readers that have to distinguish absence from
/// emptiness use [`read_session_transcript`].
pub fn read_session_from_temperfs(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
) -> Result<String, String> {
    read_session_transcript(ctx, temper_api_url, tenant, fields, file_id)
        .map(|read| read.jsonl)
}

/// Read session JSONL and report whether the transcript exists.
pub fn read_session_transcript(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
) -> Result<TranscriptRead, String> {
    if let Some(session_id) = session_id_from_entries_ref(file_id) {
        if !session_entries_materialized(fields) {
            ctx.log(
                "info",
                &format!(
                    "read_session_from_temperfs: virtual first-turn SessionEntries ref for {session_id}; materialization is false"
                ),
            );
            return Ok(TranscriptRead {
                jsonl: String::new(),
                presence: TranscriptPresence::PendingFirstTurn,
            });
        }
        let jsonl = read_session_from_entries(ctx, temper_api_url, tenant, fields, session_id)?;
        return Ok(TranscriptRead {
            presence: transcript_presence(&jsonl, TranscriptPresence::NoEntries),
            jsonl,
        });
    }

    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = runtime_headers(ctx, tenant, fields, None, None);
    match read_temperfs_value_or_absent(ctx, &url, &headers, "TemperFS session read failed")? {
        Some(jsonl) => Ok(TranscriptRead {
            presence: transcript_presence(&jsonl, TranscriptPresence::EmptyFile),
            jsonl,
        }),
        None => Ok(TranscriptRead {
            jsonl: String::new(),
            presence: TranscriptPresence::MissingFile,
        }),
    }
}

/// A transcript of nothing but whitespace carries no entries, so it counts as
/// the caller's "empty" case rather than as content.
fn transcript_presence(jsonl: &str, when_empty: TranscriptPresence) -> TranscriptPresence {
    if jsonl.lines().any(|line| !line.trim().is_empty()) {
        TranscriptPresence::Present
    } else {
        when_empty
    }
}

/// Write session JSONL to TemperFS by file ID.
pub fn write_session_to_temperfs(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
    jsonl: &str,
) -> Result<(), String> {
    if let Some(session_id) = session_id_from_entries_ref(file_id) {
        return sync_session_entries_from_jsonl(
            ctx,
            temper_api_url,
            tenant,
            fields,
            session_id,
            jsonl,
        );
    }

    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = runtime_headers(ctx, tenant, fields, Some("text/plain"), None);
    write_temperfs_value_with_retry(ctx, &url, &headers, jsonl, "TemperFS session write failed")
}

pub fn create_session_entry(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    entry_id: &str,
    parent_entry_id: Option<&str>,
    sequence: i64,
    entry_type: &str,
    role: Option<&str>,
    content: Option<&Value>,
    content_file_id: Option<&str>,
    content_file_version_id: Option<&str>,
    extra_json: Option<&Value>,
    tokens: usize,
) -> Result<CreatedSessionEntry, String> {
    // Every entry carries its own wall-clock stamp. The entity event log is a
    // hot tail (older events vanish at snapshot boundaries), so it cannot be
    // trusted to date turns in a long session; the entry can.
    let extra_json = stamp_recorded_at(extra_json, Context::get_time_millis());
    let extra_json = Some(&extra_json);
    let spec = SessionEntryCreateSpec {
        session_id,
        entry_id,
        parent_entry_id,
        sequence,
        entry_type,
        role,
        content,
        content_file_id,
        content_file_version_id,
        extra_json,
        tokens,
    };
    let body = session_entry_create_body(&spec)?;
    let url = format!("{temper_api_url}/tdata/SessionEntries");
    let headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "SessionEntry creation failed (HTTP {}): {}",
            resp.status,
            error_excerpt(&resp.body, 300)
        ));
    }
    let created = parse_created_session_entry_ack(&resp.body, session_id, entry_id)?;
    maybe_verify_session_entries(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        &[entry_id],
        "create_session_entry",
    )?;

    Ok(created)
}

pub fn create_initial_session_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    user_message: &str,
) -> Result<(CreatedSessionEntry, CreatedSessionEntry), String> {
    let header_id = format!("h-{session_id}");
    let user_entry_id = format!("u-{session_id}-0");
    let header_extra = json!({ "version": 1 });
    let user_content = json!(user_message);
    let specs = [
        SessionEntryCreateSpec {
            session_id,
            entry_id: &header_id,
            parent_entry_id: None,
            sequence: 0,
            entry_type: "header",
            role: None,
            content: None,
            content_file_id: None,
            content_file_version_id: None,
            extra_json: Some(&header_extra),
            tokens: 0,
        },
        SessionEntryCreateSpec {
            session_id,
            entry_id: &user_entry_id,
            parent_entry_id: Some(&header_id),
            sequence: 1,
            entry_type: "message",
            role: Some("user"),
            content: Some(&user_content),
            content_file_id: None,
            content_file_version_id: None,
            extra_json: None,
            tokens: user_message.len() / 4,
        },
    ];
    let created =
        create_session_entry_batch(ctx, temper_api_url, tenant, fields, &specs, "initial")?;

    maybe_verify_session_entries(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        &[&header_id, &user_entry_id],
        "create_initial_session_entries",
    )?;

    if created.len() != 2 {
        return Err(format!(
            "initial SessionEntry batch returned {} created entries for 2 specs",
            created.len()
        ));
    }
    Ok((created[0].clone(), created[1].clone()))
}

pub fn materialize_initial_session_entries_with_assistant(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    user_message: &str,
    assistant_content: &Value,
    assistant_tokens: usize,
    assistant_extra_json: Option<&Value>,
) -> Result<CreatedSessionEntry, String> {
    let user_entry_id = format!("u-{session_id}-0");
    let (assistant_entry_id, assistant_sequence) = next_session_entry_id("a", &user_entry_id);
    let user_content = json!(user_message);
    let specs = [
        SessionEntryCreateSpec {
            session_id,
            entry_id: &user_entry_id,
            parent_entry_id: None,
            sequence: 1,
            entry_type: "message",
            role: Some("user"),
            content: Some(&user_content),
            content_file_id: None,
            content_file_version_id: None,
            extra_json: None,
            tokens: user_message.len() / 4,
        },
        SessionEntryCreateSpec {
            session_id,
            entry_id: &assistant_entry_id,
            parent_entry_id: Some(&user_entry_id),
            sequence: assistant_sequence,
            entry_type: "message",
            role: Some("assistant"),
            content: Some(assistant_content),
            content_file_id: None,
            content_file_version_id: None,
            extra_json: assistant_extra_json,
            tokens: assistant_tokens,
        },
    ];
    let created = create_session_entry_batch(
        ctx,
        temper_api_url,
        tenant,
        fields,
        &specs,
        "materialize initial SessionEntry",
    )?;
    maybe_verify_session_entries(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        &[&user_entry_id, &assistant_entry_id],
        "materialize_initial_session_entries_with_assistant",
    )?;

    created
        .into_iter()
        .find(|entry| entry.entry_id == assistant_entry_id)
        .ok_or_else(|| {
            format!(
                "materialize initial SessionEntry batch did not return assistant entry {assistant_entry_id}"
            )
        })
}

fn create_session_entry_batch(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    specs: &[SessionEntryCreateSpec<'_>],
    label: &str,
) -> Result<Vec<CreatedSessionEntry>, String> {
    let create_headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    let create_url = format!("{temper_api_url}/tdata/SessionEntries");
    // Same wall-clock stamp as the single-entry path — see create_session_entry.
    let now_ms = Context::get_time_millis();
    let stamped: Vec<Value> = specs
        .iter()
        .map(|spec| stamp_recorded_at(spec.extra_json, now_ms))
        .collect();
    let create_requests = specs
        .iter()
        .zip(stamped.iter())
        .map(|(spec, extra_json)| {
            let spec = SessionEntryCreateSpec {
                extra_json: Some(extra_json),
                ..*spec
            };
            let body = session_entry_create_body(&spec)?;
            Ok(HttpRequest {
                method: "POST".to_string(),
                url: create_url.clone(),
                headers: create_headers.clone(),
                body: body.to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let create_responses = ctx.http_call_batch(&create_requests)?;
    if create_responses.len() != specs.len() {
        return Err(format!(
            "{label} batch returned {} responses for {} requests",
            create_responses.len(),
            specs.len()
        ));
    }

    let mut created = Vec::with_capacity(specs.len());
    for (spec, resp) in specs.iter().zip(create_responses.iter()) {
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "{label} {} creation failed (HTTP {}): {}",
                spec.entry_id,
                resp.status,
                error_excerpt(&resp.body, 300)
            ));
        }
        created.push(parse_created_session_entry_ack(
            &resp.body,
            spec.session_id,
            spec.entry_id,
        )?);
    }
    Ok(created)
}

fn maybe_verify_session_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    entry_ids: &[&str],
    label: &str,
) -> Result<(), String> {
    let config_value = ctx
        .config
        .get("session_entry_create_verify_readback")
        .map(String::as_str);
    if session_entry_create_verify_readback_enabled(fields, config_value) {
        return verify_session_entries(
            ctx,
            temper_api_url,
            tenant,
            fields,
            session_id,
            entry_ids,
            label,
        );
    }

    ctx.log(
        "info",
        &format!(
            "{label}: SessionEntry create response ack verified; strict read-back skipped (SessionId={session_id}, entries={})",
            entry_ids.join(",")
        ),
    );
    Ok(())
}

fn verify_session_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    entry_ids: &[&str],
    label: &str,
) -> Result<(), String> {
    const VERIFY_ATTEMPTS: u32 = 4;
    let verify_headers = runtime_headers(ctx, tenant, fields, None, Some("application/json"));
    let verify_urls = session_entries_verify_urls(temper_api_url, session_id, entry_ids);
    let mut last_status = 0_i64;
    let mut last_err = String::new();
    for attempt in 0..VERIFY_ATTEMPTS {
        let mut missing = Vec::new();
        for (entry_id, verify_url) in &verify_urls {
            match ctx.http_call("GET", verify_url, &verify_headers, "") {
                Ok(resp) if resp.status == 200 => {
                    last_status = resp.status as i64;
                    if !session_entry_verify_response_visible(&resp.body) {
                        missing.push(entry_id.clone());
                    }
                }
                Ok(resp) => {
                    last_status = resp.status as i64;
                    missing.push(entry_id.clone());
                }
                Err(err) => {
                    last_status = -1;
                    last_err = err.to_string();
                    missing.push(entry_id.clone());
                }
            }
        }

        if missing.is_empty() {
            if attempt > 0 {
                ctx.log(
                    "info",
                    &format!(
                        "{label}: read-back visible on attempt {} (SessionId={session_id})",
                        attempt + 1
                    ),
                );
            }
            return Ok(());
        } else if last_err.is_empty() {
            last_err = format!("missing entries: {}", missing.join(","));
        }

        if attempt + 1 < VERIFY_ATTEMPTS {
            let target_delay_ms = 50_i64 << attempt;
            let until = Context::get_time_millis() + target_delay_ms;
            while Context::get_time_millis() < until {}
        }
    }

    ctx.log(
        "error",
        &format!(
            "{label}: WRITE LOST — batch POST 2xx for SessionId={session_id} but read-back missed after {VERIFY_ATTEMPTS} attempts (last_status={last_status} last_err={last_err:?})"
        ),
    );
    Err(format!(
        "{label} entries acknowledged but read-back missed after {VERIFY_ATTEMPTS} attempts: SessionId={session_id}"
    ))
}

/// Add `ts_ms` to a SessionEntry's extra JSON without disturbing what the
/// caller already put there. An explicit `ts_ms` from the caller wins.
fn stamp_recorded_at(extra_json: Option<&Value>, now_ms: i64) -> Value {
    let mut extra = match extra_json {
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(other) if !other.is_null() => json!({ "extra": other.clone() }),
        _ => json!({}),
    };
    if let Some(object) = extra.as_object_mut()
        && !object.contains_key("ts_ms")
    {
        object.insert("ts_ms".to_string(), json!(now_ms));
    }
    extra
}

/// Ceiling on a SessionEntry's stored `extra_json`.
///
/// The same 131072 twice over: it is what `session_entry.ioa.toml` declares as
/// `overflow_inline_max_bytes` on the `extra_json` state variable, and it is
/// also the kernel's own `DEFAULT_FIELD_INLINE_MAX` when no per-field override
/// applies to a write path. The bound therefore holds whichever of the two the
/// kernel ends up measuring against.
pub const MAX_ENTRY_EXTRA_BYTES: usize = 131_072;

/// Per-turn facts the OTS emitter reads back off an entry. They are small, and
/// losing them costs a turn its date, its model and its token counts, so they
/// are the last thing dropped rather than the first.
const ENTRY_EXTRA_ESSENTIALS: &[&str] = &[
    "ts_ms",
    "provider",
    "model",
    "stop_reason",
    "input_tokens",
    "output_tokens",
];

/// The markers a token-signal refusal leaves behind. Bounded to one per signal,
/// and the emitter reads them to tell "the serving stack sent none" from "they
/// were sent and refused" — so the hard floor keeps them.
const TOKEN_SIGNAL_DROP_MARKERS: &[&str] = &[
    "prompt_token_ids_dropped_bytes",
    "completion_token_ids_dropped_bytes",
    "response_mask_dropped_bytes",
    "logprobs_dropped_bytes",
];

/// Size of `value` as the kernel measures the stored field.
///
/// String-typed state variables hold JSON as text, so what the overflow ceiling
/// sees is this JSON encoded *again* as a JSON string: every quote and
/// backslash gains an escape byte, plus the two enclosing quotes. (`serde_json`
/// output carries no raw control characters, so those are not a case.)
pub fn stored_json_len(value: &Value) -> usize {
    let Ok(json) = serde_json::to_string(value) else {
        return usize::MAX;
    };
    json.len()
        + json
            .bytes()
            .filter(|byte| matches!(byte, b'"' | b'\\'))
            .count()
        + 2
}

/// A char-safe prefix of a response body, for putting inside an error message.
///
/// Slicing by byte offset traps the guest whenever a multibyte character
/// straddles the cut — and it is the failure-reporting paths that do the
/// slicing, so the trap replaces the report: the module dies before it can say
/// what went wrong, and the entity keeps whatever status it already had. The
/// same class already cost `monty_repl` its span document (ADR-0035 §15).
pub fn error_excerpt(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Serialized size of `value` before the field encoding — what a dropped-member
/// marker reports, so the number means the same thing everywhere.
fn json_len(value: &Value) -> usize {
    serde_json::to_string(value)
        .map(|json| json.len())
        .unwrap_or(usize::MAX)
}

/// What one `"key":value,` member contributes to the stored field.
fn member_cost(key: &str, value: &Value) -> usize {
    stored_json_len(value) + key.len() + 4
}

/// Drop whatever does not fit under the entry's `extra_json` ceiling.
///
/// Past the ceiling the kernel replaces or externalizes the *whole* field, so
/// one oversized addition takes the per-turn facts with it. Writers that build
/// their own extras bound them by policy — which signal to sacrifice first —
/// but this is the invariant at the boundary every writer passes through,
/// including the JSONL sync path that re-materializes extras written before any
/// of those policies existed. Anything dropped leaves its size behind.
fn bound_entry_extra(extra: Value) -> Value {
    if stored_json_len(&extra) <= MAX_ENTRY_EXTRA_BYTES {
        return extra;
    }
    if !extra.is_object() {
        // Nothing to sacrifice member by member; record the size it had.
        return json!({ "_extra_json_dropped_bytes": json_len(&extra) });
    }

    let mut extra = extra;
    if let Some(fields) = extra.as_object_mut() {
        // Every member is measured once and the largest go first. Re-measuring
        // the whole object per drop would be quadratic, and this runs inside a
        // WASM guest on an object a corrupted transcript line can make wide.
        let mut droppable: Vec<(String, usize)> = fields
            .iter()
            .filter(|(key, _)| {
                !ENTRY_EXTRA_ESSENTIALS.contains(&key.as_str()) && !key.ends_with("_dropped_bytes")
            })
            .map(|(key, value)| (key.clone(), member_cost(key, value)))
            .collect();
        droppable.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

        let mut total = stored_json_len(&Value::Object(fields.clone()));
        for (key, cost) in droppable {
            if total <= MAX_ENTRY_EXTRA_BYTES {
                break;
            }
            let size = fields.remove(&key).map(|value| json_len(&value)).unwrap_or(0);
            let marker = format!("{key}_dropped_bytes");
            total = total.saturating_sub(cost) + member_cost(&marker, &json!(size));
            fields.insert(marker, json!(size));
        }

        // A per-turn fact can be oversized on its own — a provider is free to
        // return a huge `stop_reason`, and the JSONL sync path copies whatever
        // a transcript line put under that key — and nothing above would have
        // touched it, because the drop loop skips the essentials.
        bound_essential_values(fields);
    }

    // Hard floor. An extras object with thousands of small members leaves a
    // marker behind for each one, and the markers alone can hold the value over
    // the ceiling. The per-turn facts are worth more than the drop record, so
    // they are what survives, with a single count in place of the markers.
    if stored_json_len(&extra) > MAX_ENTRY_EXTRA_BYTES
        && let Some(fields) = extra.as_object_mut()
    {
        let before = fields.len();
        // The token-signal drop markers survive alongside the facts. They are
        // the emitter's only evidence that signals existed and were refused,
        // and there are at most four of them, so they cannot be what holds the
        // value over — unlike the arbitrary `<key>_dropped_bytes` markers this
        // floor exists to shed.
        fields.retain(|key, _| {
            ENTRY_EXTRA_ESSENTIALS.contains(&key.as_str())
                || TOKEN_SIGNAL_DROP_MARKERS.contains(&key.as_str())
        });
        let removed = before - fields.len();
        bound_essential_values(fields);
        fields.insert("_extra_json_dropped_members".to_string(), json!(removed));
    }

    // The invariant is unconditional: returning a value over the ceiling costs
    // the entire field, which is what this function exists to prevent. Nothing
    // above should be able to reach here, so if anything does, the size is the
    // one fact worth keeping.
    if stored_json_len(&extra) > MAX_ENTRY_EXTRA_BYTES {
        extra = json!({ "_extra_json_dropped_bytes": json_len(&extra) });
    }

    extra
}

/// Cut every member down to a size that cannot itself breach the entry ceiling,
/// recording each cut. Reached when nothing droppable is left, so it must handle
/// the per-turn facts themselves: they are small by construction only when the
/// writer is well behaved, and one of the writers copies a transcript line's
/// keys verbatim.
fn bound_essential_values(fields: &mut serde_json::Map<String, Value>) {
    /// Generous for a provider name, a model id or a stop reason, and small
    /// enough that the whole essential set fits many times over.
    const MAX_ESSENTIAL_STRING_CHARS: usize = 512;
    /// Anything not a short string or a scalar is not the fact this key names.
    const MAX_ESSENTIAL_VALUE_BYTES: usize = 4_096;

    let oversized: Vec<String> = fields
        .iter()
        .filter(|(key, value)| {
            !key.ends_with("_truncated_chars")
                && !key.ends_with("_dropped_bytes")
                && (value
                    .as_str()
                    .is_some_and(|text| text.chars().count() > MAX_ESSENTIAL_STRING_CHARS)
                    || json_len(value) > MAX_ESSENTIAL_VALUE_BYTES)
        })
        .map(|(key, _)| key.clone())
        .collect();

    for key in oversized {
        let Some(value) = fields.get(&key) else {
            continue;
        };
        match value.as_str() {
            Some(text) => {
                let original = text.chars().count();
                let cut: String = text.chars().take(MAX_ESSENTIAL_STRING_CHARS).collect();
                fields.insert(key.clone(), json!(cut));
                fields.insert(format!("{key}_truncated_chars"), json!(original));
            }
            // An array or object under a per-turn fact's key is corrupt data,
            // not the fact. Its size is the only part worth keeping.
            None => {
                let size = json_len(value);
                fields.remove(&key);
                fields.insert(format!("{key}_dropped_bytes"), json!(size));
            }
        }
    }
}

fn session_entry_create_body(spec: &SessionEntryCreateSpec<'_>) -> Result<Value, String> {
    let content_json = spec
        .content
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| format!("serialize SessionEntry content: {err}"))?
        .unwrap_or_default();
    let extra_json = spec
        .extra_json
        .map(|extra| serde_json::to_string(&bound_entry_extra(extra.clone())))
        .transpose()
        .map_err(|err| format!("serialize SessionEntry extra_json: {err}"))?
        .unwrap_or_else(|| "{}".to_string());

    Ok(json!({
        "SessionId": spec.session_id,
        "EntryId": spec.entry_id,
        "ParentEntryId": spec.parent_entry_id.unwrap_or(""),
        "Sequence": spec.sequence,
        "EntryType": spec.entry_type,
        "Role": spec.role.unwrap_or(""),
        "Content": content_json,
        "ContentFileId": spec.content_file_id.unwrap_or(""),
        "ContentFileVersionId": spec.content_file_version_id.unwrap_or(""),
        "ExtraJson": extra_json,
        "Tokens": spec.tokens as i64,
    }))
}

fn parse_created_session_entry_ack(
    body: &str,
    expected_session_id: &str,
    expected_entry_id: &str,
) -> Result<CreatedSessionEntry, String> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|err| format!("parse SessionEntry creation response: {err}"))?;
    let entity_id = entity_field_str(&parsed, &["entity_id", "Id"])
        .unwrap_or("")
        .to_string();
    if entity_id.is_empty() {
        return Err("SessionEntry creation response missing entity_id/Id".to_string());
    }

    let actual_session_id = entity_field_str(&parsed, &["SessionId", "session_id"]).unwrap_or("");
    let actual_entry_id = entity_field_str(&parsed, &["EntryId", "entry_id"]).unwrap_or("");
    if actual_session_id != expected_session_id || actual_entry_id != expected_entry_id {
        return Err(format!(
            "SessionEntry creation response ack mismatch: expected SessionId={expected_session_id} EntryId={expected_entry_id}, got SessionId={actual_session_id:?} EntryId={actual_entry_id:?}"
        ));
    }

    Ok(CreatedSessionEntry {
        entity_id,
        entry_id: expected_entry_id.to_string(),
    })
}

fn session_entry_create_verify_readback_enabled(
    fields: &Value,
    config_value: Option<&str>,
) -> bool {
    fields
        .get("session_entry_create_verify_readback")
        .and_then(boolish_json)
        .or_else(|| config_value.and_then(boolish_str))
        .unwrap_or(true)
}

fn boolish_json(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_str().and_then(boolish_str))
}

fn boolish_str(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn session_entries_materialized(fields: &Value) -> bool {
    fields
        .get("session_entries_materialized")
        .and_then(boolish_json)
        .or_else(|| {
            fields
                .get("SessionEntriesMaterialized")
                .and_then(boolish_json)
        })
    .or_else(|| {
        fields
            .get("fields")
            .and_then(|nested| nested.get("session_entries_materialized"))
            .and_then(boolish_json)
    })
    .or_else(|| {
        fields
            .get("fields")
            .and_then(|nested| nested.get("SessionEntriesMaterialized"))
            .and_then(boolish_json)
    })
    .or_else(|| {
        entity_field_str(
            fields,
            &["session_entries_materialized", "SessionEntriesMaterialized"],
        )
        .and_then(boolish_str)
    })
    .unwrap_or(true)
}

fn session_entry_verify_url(temper_api_url: &str, session_id: &str, entry_id: &str) -> String {
    format!(
        "{temper_api_url}/tdata/SessionEntries(SessionId='{}',EntryId='{}')",
        session_id.replace('\'', "''"),
        entry_id.replace('\'', "''"),
    )
}

fn session_entries_verify_urls(
    temper_api_url: &str,
    session_id: &str,
    entry_ids: &[&str],
) -> Vec<(String, String)> {
    entry_ids
        .iter()
        .map(|entry_id| {
            (
                (*entry_id).to_string(),
                session_entry_verify_url(temper_api_url, session_id, entry_id),
            )
        })
        .collect()
}

fn session_entries_list_url(
    temper_api_url: &str,
    session_id: &str,
    top: usize,
    skip: usize,
) -> String {
    format!(
        "{temper_api_url}/tdata/SessionEntries?$filter=SessionId%20eq%20%27{}%27&$top={top}&$skip={skip}",
        session_id.replace('\'', "''"),
    )
}

fn session_entry_verify_response_visible(body: &str) -> bool {
    let parsed: Value = serde_json::from_str(body).unwrap_or_else(|_| json!({"value": []}));
    if entity_field_str(&parsed, &["SessionId", "session_id"]).is_some()
        && entity_field_str(&parsed, &["EntryId", "entry_id"]).is_some()
    {
        return true;
    }
    parsed
        .get("value")
        .and_then(|value| value.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false)
}

/// Append one SessionEntry under `parent_entry_id`.
///
/// `extra_json` carries per-turn facts the OTS emitter needs later (provider,
/// model, stop reason, usage, token-level RL signals). It is merged into the
/// entry's ExtraJson alongside the wall-clock stamp; pass `None` when the
/// caller has nothing to add.
#[allow(clippy::too_many_arguments)]
pub fn append_session_entry_inline(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_ref: &str,
    parent_entry_id: &str,
    entry_prefix: &str,
    role: &str,
    content: &Value,
    tokens: usize,
    extra_json: Option<&Value>,
) -> Result<CreatedSessionEntry, String> {
    let session_id = session_id_from_entries_ref(session_ref)
        .ok_or("append_session_entry_inline requires session-entries:<session_id> ref")?;
    if parent_entry_id.is_empty() {
        return Err("append_session_entry_inline requires parent_entry_id".to_string());
    }
    let (entry_id, sequence) = next_session_entry_id(entry_prefix, parent_entry_id);
    create_session_entry(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        &entry_id,
        Some(parent_entry_id),
        sequence,
        "message",
        Some(role),
        Some(content),
        None,
        None,
        extra_json,
        tokens,
    )
}

pub fn read_session_from_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
) -> Result<String, String> {
    let entries = list_session_entries(ctx, temper_api_url, tenant, fields, session_id)?;
    Ok(session_entries_jsonl_from_entities(&entries))
}

pub fn sync_session_entries_from_jsonl(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    jsonl: &str,
) -> Result<(), String> {
    let existing = list_session_entries(ctx, temper_api_url, tenant, fields, session_id)?;
    let existing_ids: std::collections::BTreeSet<String> = existing
        .iter()
        .filter_map(|entry| entity_field_str(entry, &["EntryId", "entry_id"]).map(str::to_string))
        .collect();

    for (sequence, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: Value = serde_json::from_str(line)
            .map_err(|err| format!("parse SessionTree JSONL line for SessionEntry sync: {err}"))?;
        let entry_id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or("SessionTree JSONL line missing id")?;
        if existing_ids.contains(entry_id) {
            continue;
        }

        create_session_entry_from_jsonl_value(
            ctx,
            temper_api_url,
            tenant,
            fields,
            session_id,
            &parsed,
            sequence as i64,
        )?;
    }

    Ok(())
}

pub fn session_entries_jsonl_from_entities(entries: &[Value]) -> String {
    let mut rows: Vec<(i64, String, String)> = entries
        .iter()
        .filter_map(|entry| {
            let sequence = entity_field_i64(entry, &["Sequence", "sequence"]).unwrap_or(0);
            let entry_id = entity_field_str(entry, &["EntryId", "entry_id"])
                .unwrap_or("")
                .to_string();
            if entry_id.is_empty() {
                return None;
            }
            session_entry_entity_to_jsonl(entry).map(|line| (sequence, entry_id, line))
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    rows.into_iter()
        .map(|(_, _, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn create_session_entry_from_jsonl_value(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
    entry: &Value,
    sequence: i64,
) -> Result<CreatedSessionEntry, String> {
    let entry_id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("SessionTree JSONL entry missing id")?;
    let parent_entry_id = entry.get("parentId").and_then(Value::as_str);
    let entry_type = entry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    let role = entry.get("role").and_then(Value::as_str);
    let tokens = entry.get("tokens").and_then(Value::as_u64).unwrap_or(0) as usize;
    let content = entry.get("content");
    let content_file_id = entry.get("content_file_id").and_then(Value::as_str);
    let content_file_version_id = entry.get("content_file_version_id").and_then(Value::as_str);
    let extra_json = session_entry_extra_json(entry);

    create_session_entry(
        ctx,
        temper_api_url,
        tenant,
        fields,
        session_id,
        entry_id,
        parent_entry_id,
        sequence,
        entry_type,
        role,
        content,
        content_file_id,
        content_file_version_id,
        Some(&extra_json),
        tokens,
    )
}

fn list_session_entries(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_id: &str,
) -> Result<Vec<Value>, String> {
    let headers = runtime_headers(ctx, tenant, fields, None, Some("application/json"));
    let mut entries = Vec::new();
    let mut skip = 0usize;

    for _ in 0..SESSION_ENTRIES_MAX_PAGES {
        let url =
            session_entries_list_url(temper_api_url, session_id, SESSION_ENTRIES_PAGE_SIZE, skip);
        let resp = ctx.http_call("GET", &url, &headers, "")?;
        if resp.status != 200 {
            return Err(format!(
                "SessionEntry list failed (HTTP {}): {}",
                resp.status,
                error_excerpt(&resp.body, 300)
            ));
        }
        let parsed: Value = serde_json::from_str(&resp.body)
            .map_err(|err| format!("parse SessionEntry list response: {err}"))?;
        let mut page = parsed
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = page.len();
        entries.append(&mut page);
        if page_len < SESSION_ENTRIES_PAGE_SIZE {
            return Ok(entries);
        }
        skip += page_len;
    }

    Err(format!(
        "SessionEntry list exceeded {} pages for session {session_id}; refusing incomplete context",
        SESSION_ENTRIES_MAX_PAGES
    ))
}

fn session_entry_entity_to_jsonl(entry: &Value) -> Option<String> {
    let entry_id = entity_field_str(entry, &["EntryId", "entry_id"])?;
    let parent_entry_id =
        entity_field_str(entry, &["ParentEntryId", "parent_entry_id"]).unwrap_or("");
    let entry_type = entity_field_str(entry, &["EntryType", "entry_type"]).unwrap_or("message");
    let role = entity_field_str(entry, &["Role", "role"]).unwrap_or("");
    let tokens = entity_field_i64(entry, &["Tokens", "tokens"]).unwrap_or(0);
    let content = entity_field_str(entry, &["Content", "content"]).unwrap_or("");
    let content_file_id =
        entity_field_str(entry, &["ContentFileId", "content_file_id"]).unwrap_or("");
    let content_file_version_id =
        entity_field_str(entry, &["ContentFileVersionId", "content_file_version_id"]).unwrap_or("");
    let extra_json = entity_field_str(entry, &["ExtraJson", "extra_json"]).unwrap_or("{}");

    let mut line = json!({
        "id": entry_id,
        "parentId": if parent_entry_id.is_empty() { Value::Null } else { json!(parent_entry_id) },
        "type": entry_type,
        "tokens": tokens,
    });

    if let Ok(extra) = serde_json::from_str::<Value>(extra_json) {
        if let (Some(target), Some(extra_obj)) = (line.as_object_mut(), extra.as_object()) {
            for (key, value) in extra_obj {
                if !is_session_entry_canonical_key(key) {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
    }

    if !role.is_empty() {
        line["role"] = json!(role);
    }
    if !content.is_empty() {
        line["content"] = serde_json::from_str::<Value>(content)
            .unwrap_or_else(|_| Value::String(content.to_string()));
    }
    if !content_file_id.is_empty() {
        line["content_file_id"] = json!(content_file_id);
    }
    if !content_file_version_id.is_empty() {
        line["content_file_version_id"] = json!(content_file_version_id);
    }

    serde_json::to_string(&line).ok()
}

fn session_entry_extra_json(entry: &Value) -> Value {
    let mut extra = serde_json::Map::new();
    let Some(obj) = entry.as_object() else {
        return json!({});
    };
    for (key, value) in obj {
        if !is_session_entry_canonical_key(key) {
            extra.insert(key.clone(), value.clone());
        }
    }
    Value::Object(extra)
}

fn is_session_entry_canonical_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "parentId"
            | "type"
            | "role"
            | "content"
            | "tokens"
            | "content_file_id"
            | "content_file_version_id"
    )
}

/// Read raw file content from TemperFS by file ID.
pub fn read_content_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
) -> Result<String, String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = runtime_headers(ctx, tenant, fields, None, None);
    read_temperfs_value_with_retry(ctx, &url, &headers, "TemperFS content file read failed")
}

pub fn read_content_file_version(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_version_id: &str,
) -> Result<String, String> {
    let results = read_text_file_versions_batch(
        ctx,
        temper_api_url,
        tenant,
        fields,
        &[file_version_id.to_string()],
    )?;
    Ok(results
        .get(file_version_id)
        .filter(|item| item.found)
        .map(|item| item.text.clone())
        .unwrap_or_default())
}

pub fn read_text_files_batch(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_ids: &[String],
) -> Result<BTreeMap<String, BatchTextFileReadItem>, String> {
    if file_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let url = format!("{temper_api_url}/api/files/read-text-batch");
    let headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    let body = json!({ "file_ids": file_ids }).to_string();

    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..TEMPERFS_BATCH_READ_ATTEMPTS {
        let resp = ctx.http_call("POST", &url, &headers, &body)?;
        if resp.status == 200 {
            return parse_batch_text_file_read_response(&resp.body);
        }

        last_status = resp.status;
        last_body = resp.body;

        if (500..600).contains(&last_status) && attempt + 1 < TEMPERFS_BATCH_READ_ATTEMPTS {
            ctx.log(
                "warn",
                &format!(
                    "TemperFS batch read transient failure (HTTP {}), retry {}/{}",
                    last_status,
                    attempt + 2,
                    TEMPERFS_BATCH_READ_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "TemperFS batch read failed (HTTP {}): {}",
        last_status,
        error_excerpt(&last_body, 200)
    ))
}

pub fn read_text_file_versions_batch(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_version_ids: &[String],
) -> Result<BTreeMap<String, BatchTextFileVersionReadItem>, String> {
    if file_version_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let url = format!("{temper_api_url}/api/files/read-version-text-batch");
    let headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    let body = json!({ "file_version_ids": file_version_ids }).to_string();

    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..TEMPERFS_BATCH_READ_ATTEMPTS {
        let resp = ctx.http_call("POST", &url, &headers, &body)?;
        if resp.status == 200 {
            return parse_batch_text_file_version_read_response(&resp.body);
        }

        last_status = resp.status;
        last_body = resp.body;

        if (500..600).contains(&last_status) && attempt + 1 < TEMPERFS_BATCH_READ_ATTEMPTS {
            ctx.log(
                "warn",
                &format!(
                    "TemperFS batch version read transient failure (HTTP {}), retry {}/{}",
                    last_status,
                    attempt + 2,
                    TEMPERFS_BATCH_READ_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "TemperFS batch version read failed (HTTP {}): {}",
        last_status,
        error_excerpt(&last_body, 200)
    ))
}

pub fn parse_batch_text_file_read_response(
    body: &str,
) -> Result<BTreeMap<String, BatchTextFileReadItem>, String> {
    let parsed: Value =
        serde_json::from_str(body).map_err(|e| format!("parse batch file read response: {e}"))?;
    let files = parsed
        .get("files")
        .and_then(Value::as_array)
        .ok_or("batch file read response missing files array")?;

    let mut by_id = BTreeMap::new();
    for file in files {
        let file_id = file
            .get("file_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if file_id.is_empty() {
            continue;
        }

        by_id.insert(
            file_id.clone(),
            BatchTextFileReadItem {
                file_id,
                found: file.get("found").and_then(Value::as_bool).unwrap_or(false),
                content_hash: file
                    .get("content_hash")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                mime_type: file
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                text: file
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }

    Ok(by_id)
}

pub fn parse_batch_text_file_version_read_response(
    body: &str,
) -> Result<BTreeMap<String, BatchTextFileVersionReadItem>, String> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| format!("parse batch file version read response: {e}"))?;
    let files = parsed
        .get("files")
        .and_then(Value::as_array)
        .ok_or("batch file version read response missing files array")?;

    let mut by_id = BTreeMap::new();
    for file in files {
        let file_version_id = file
            .get("file_version_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if file_version_id.is_empty() {
            continue;
        }

        by_id.insert(
            file_version_id.clone(),
            BatchTextFileVersionReadItem {
                file_version_id,
                found: file.get("found").and_then(Value::as_bool).unwrap_or(false),
                content_hash: file
                    .get("content_hash")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                mime_type: file
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                text: file
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }

    Ok(by_id)
}

/// Create a TemperFS file and write content into it.
pub fn create_content_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_name: &str,
    content: &str,
) -> Result<String, String> {
    create_content_file_ref(
        ctx,
        temper_api_url,
        tenant,
        workspace_id,
        file_name,
        content,
    )
    .map(|created| created.file_id)
}

/// Create a TemperFS file, write content into it, and resolve the immutable
/// file version produced by that write.
pub fn create_content_file_ref(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_name: &str,
    content: &str,
) -> Result<CreatedContentFileRef, String> {
    let headers = runtime_headers_with_workspace(
        ctx,
        tenant,
        &serde_json::json!({}),
        Some(workspace_id),
        None,
        Some("application/json"),
        None,
    );

    let file_body = serde_json::json!({
        "workspace_id": workspace_id,
        "name": file_name,
        "mime_type": "text/plain",
        "path": format!("/{file_name}")
    });
    let file_url = format!("{temper_api_url}/tdata/Files");
    let file_resp = ctx.http_call("POST", &file_url, &headers, &file_body.to_string())?;

    if file_resp.status < 200 || file_resp.status >= 300 {
        return Err(format!(
            "content file creation failed (HTTP {}): {}",
            file_resp.status,
            error_excerpt(&file_resp.body, 300)
        ));
    }

    let file_parsed: Value = serde_json::from_str(&file_resp.body)
        .map_err(|e| format!("parse content file response: {e}"))?;
    let file_id = file_parsed
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if file_id.is_empty() {
        return Err("content file created but entity_id missing".to_string());
    }

    let value_url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let value_headers = runtime_headers_with_workspace(
        ctx,
        tenant,
        &serde_json::json!({}),
        Some(workspace_id),
        None,
        Some("text/plain"),
        None,
    );
    write_temperfs_value_with_retry(
        ctx,
        &value_url,
        &value_headers,
        content,
        "content file write failed",
    )?;

    let file_state = read_content_file_head(ctx, temper_api_url, tenant, workspace_id, &file_id)?;
    let file_version_id = entity_field_str(&file_state, &["LastVersionId", "last_version_id"])
        .unwrap_or("")
        .to_string();
    let content_hash = entity_field_str(&file_state, &["ContentHash", "content_hash"])
        .unwrap_or("")
        .to_string();

    Ok(CreatedContentFileRef {
        file_id,
        file_version_id,
        content_hash,
    })
}

/// Build standard OData headers for tenant-scoped requests.
pub fn odata_headers(ctx: &Context, tenant: &str, fields: &Value) -> Vec<(String, String)> {
    runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    )
}

/// Build tenant-scoped runtime headers with an entity-derived principal.
pub fn runtime_headers(
    ctx: &Context,
    tenant: &str,
    fields: &Value,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    runtime_headers_with_workspace(ctx, tenant, fields, None, None, content_type, accept)
}

/// Build runtime headers while overriding the logical agent type.
pub fn runtime_headers_as(
    ctx: &Context,
    tenant: &str,
    fields: &Value,
    agent_type: &str,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    runtime_headers_with_workspace(
        ctx,
        tenant,
        fields,
        None,
        Some(agent_type),
        content_type,
        accept,
    )
}

/// Build runtime headers while explicitly supplying the workspace context.
pub fn runtime_headers_for_workspace(
    ctx: &Context,
    tenant: &str,
    fields: &Value,
    workspace_id: &str,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    runtime_headers_with_workspace(
        ctx,
        tenant,
        fields,
        Some(workspace_id),
        None,
        content_type,
        accept,
    )
}

fn runtime_headers_with_workspace(
    ctx: &Context,
    tenant: &str,
    fields: &Value,
    workspace_id_override: Option<&str>,
    agent_type_override: Option<&str>,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    headers.push(("x-tenant-id".to_string(), tenant.to_string()));
    headers.push(("x-temper-principal-kind".to_string(), "agent".to_string()));
    headers.push(("x-temper-principal-id".to_string(), ctx.entity_id.clone()));
    headers.push((
        "x-temper-agent-type".to_string(),
        agent_type_override
            .unwrap_or_else(|| default_agent_type(ctx))
            .to_string(),
    ));

    if let Some(content_type) = content_type {
        headers.push(("content-type".to_string(), content_type.to_string()));
    }
    if let Some(accept) = accept {
        headers.push(("accept".to_string(), accept.to_string()));
    }

    if let Some(soul_id) =
        entity_field_str(fields, &["soul_id", "SoulId"]).filter(|v| !v.is_empty())
    {
        headers.push(("x-temper-attr-soul_id".to_string(), soul_id.to_string()));
    }

    let workspace_id = workspace_id_override
        .filter(|value| !value.is_empty())
        .or_else(|| {
            entity_field_str(fields, &["workspace_id", "WorkspaceId"])
                .filter(|value| !value.is_empty())
        });
    if let Some(workspace_id) = workspace_id {
        headers.push((
            "x-temper-attr-workspaceid".to_string(),
            workspace_id.to_string(),
        ));
    }

    if let Some(key) = ctx.config.get("temper_api_key").filter(|k| !k.is_empty()) {
        headers.push(("authorization".to_string(), format!("Bearer {key}")));
    }

    headers
}

fn read_content_file_head(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    file_id: &str,
) -> Result<Value, String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')");
    let headers = runtime_headers_with_workspace(
        ctx,
        tenant,
        &serde_json::json!({}),
        Some(workspace_id),
        None,
        Some("application/json"),
        Some("application/json"),
    );
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status != 200 {
        return Err(format!(
            "content file head read failed (HTTP {}): {}",
            resp.status,
            error_excerpt(&resp.body, 200)
        ));
    }
    serde_json::from_str(&resp.body).map_err(|e| format!("parse content file head response: {e}"))
}

/// Derive agent type from entity type for WASM modules that construct their own headers.
/// Session integrations act on behalf of an agent; all others are platform-internal ("system")
/// dispatch callbacks. For modules covered by host-injected auth (ADR-0043), this is unused —
/// the host derives agent_type from WasmInvocationContext.entity_type directly.
fn default_agent_type(ctx: &Context) -> &'static str {
    if ctx.entity_type.eq_ignore_ascii_case("Session")
        || ctx.entity_type.eq_ignore_ascii_case("Sessions")
    {
        "agent"
    } else {
        "system"
    }
}

/// Look up a string field directly on a JSON value, trying multiple key names.
pub fn direct_field_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

/// Look up a string field on a JSON value, falling back to nested `fields` object.
pub fn entity_field_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    direct_field_str(value, keys).or_else(|| {
        value
            .get("fields")
            .and_then(|fields| direct_field_str(fields, keys))
    })
}

/// Look up an integer-ish field on a JSON value, accepting JSON numbers or strings.
pub fn entity_field_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .or_else(|| value.get("fields").and_then(|fields| fields.get(*key)))
            .and_then(|raw| {
                raw.as_i64()
                    .or_else(|| raw.as_u64().and_then(|num| i64::try_from(num).ok()))
                    .or_else(|| raw.as_str().and_then(|text| text.parse::<i64>().ok()))
            })
    })
}

pub mod bounded_reads {
    use serde_json::{Value, json};
    use temper_wasm_sdk::prelude::*;

    use super::{entity_field_str, error_excerpt};

    pub const POINT_LOOKUP_TOP: usize = 20;

    pub fn odata_escape(value: &str) -> String {
        value.replace('\'', "''")
    }

    pub fn odata_filter_urlencode(value: &str) -> String {
        value
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('&', "%26")
            .replace('=', "%3D")
            .replace('?', "%3F")
            .replace('#', "%23")
            .replace('\'', "%27")
    }

    pub fn bounded_collection_query_path(set_name: &str, filter: &str, top: usize) -> String {
        format!(
            "/tdata/{set_name}?$filter={}&$top={top}",
            odata_filter_urlencode(filter)
        )
    }

    pub fn bounded_collection_query_url(
        temper_api_url: &str,
        set_name: &str,
        filter: &str,
        top: usize,
    ) -> String {
        format!(
            "{}{}",
            temper_api_url.trim_end_matches('/'),
            bounded_collection_query_path(set_name, filter, top)
        )
    }

    pub fn composite_entity_path(set_name: &str, keys: &[(&str, &str)]) -> String {
        let key_expr = keys
            .iter()
            .map(|(key, value)| format!("{key}='{}'", odata_escape(value)))
            .collect::<Vec<_>>()
            .join(",");
        format!("/tdata/{set_name}({key_expr})")
    }

    pub fn composite_entity_url(temper_api_url: &str, set_name: &str, keys: &[(&str, &str)]) -> String {
        format!(
            "{}{}",
            temper_api_url.trim_end_matches('/'),
            composite_entity_path(set_name, keys)
        )
    }

    pub fn entity_id(value: &Value) -> Option<&str> {
        entity_field_str(value, &["entity_id", "Id", "id"])
    }

    pub fn entity_is_archived(value: &Value) -> bool {
        entity_field_str(value, &["Status", "status"]) == Some("Archived")
    }

    pub fn first_non_archived_entity(value: &Value) -> Option<&Value> {
        value
            .get("value")
            .and_then(Value::as_array)?
            .iter()
            .find(|item| !entity_is_archived(item))
    }

    pub fn first_non_archived_entity_id(value: &Value) -> Option<String> {
        first_non_archived_entity(value)
            .and_then(entity_id)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    pub fn get_json(
        ctx: &Context,
        temper_api_url: &str,
        path: &str,
        headers: &[(String, String)],
        label: &str,
    ) -> Result<Value, String> {
        let url = format!("{}{}", temper_api_url.trim_end_matches('/'), path);
        let resp = ctx.http_call("GET", &url, headers, "")?;
        if resp.status >= 400 {
            return Err(format!(
                "{label}: GET {path} failed (HTTP {}): {}",
                resp.status,
                error_excerpt(&resp.body, 300)
            ));
        }
        if resp.body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&resp.body).map_err(|error| format!("{label}: parse JSON: {error}"))
    }

    pub fn find_first_non_archived_entity(
        ctx: &Context,
        temper_api_url: &str,
        headers: &[(String, String)],
        set_name: &str,
        filter: &str,
        top: usize,
        label: &str,
    ) -> Result<Option<Value>, String> {
        let path = bounded_collection_query_path(set_name, filter, top);
        let resp = get_json(ctx, temper_api_url, &path, headers, label)?;
        Ok(first_non_archived_entity(&resp).cloned())
    }

    pub fn find_first_non_archived_entity_id(
        ctx: &Context,
        temper_api_url: &str,
        headers: &[(String, String)],
        set_name: &str,
        filter: &str,
        top: usize,
        label: &str,
    ) -> Result<Option<String>, String> {
        Ok(find_first_non_archived_entity(
            ctx,
            temper_api_url,
            headers,
            set_name,
            filter,
            top,
            label,
        )?
        .and_then(|value| entity_id(&value).map(str::to_string)))
    }
}

/// List entities returned by an OData collection URL.
pub fn list_entities(ctx: &Context, url: &str, tenant: &str) -> Result<Vec<Value>, String> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let headers = runtime_headers(ctx, tenant, &fields, None, Some("application/json"));
    let resp = ctx.http_call("GET", url, &headers, "")?;
    if resp.status != 200 {
        return Err(format!("GET {url} failed (HTTP {})", resp.status));
    }
    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({"value": []}));
    Ok(parsed
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Find the best available ChannelSession for a bound agent.
pub fn find_channel_session_by_agent(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    agent_id: &str,
) -> Result<Option<Value>, String> {
    Ok(find_channel_session_for_session_or_agent(
        ctx,
        temper_api_url,
        tenant,
        "",
        agent_id,
        "",
    )?
    .map(|(session, _)| session))
}

/// Find the best ChannelSession for a session-bound notification route.
///
/// Prefer the current Session's channel binding, then its parent Session, and
/// only then fall back to the newest agent-level binding. Older proof and perf
/// ChannelSessions may remain Active for the same agent, so the agent fallback
/// must not be the first lookup.
pub fn find_channel_session_for_session_or_agent(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Result<Option<(Value, String)>, String> {
    for candidate in channel_session_lookup_candidates(
        current_session_id,
        agent_id,
        parent_session_id,
    ) {
        let url = format!(
            "{temper_api_url}/tdata/ChannelSessions?{}",
            candidate.filter
        );
        if let Some(session) = list_entities(ctx, &url, tenant)?.into_iter().next() {
            return Ok(Some((session, candidate.bound_id)));
        }
    }

    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelSessionLookup {
    filter: String,
    bound_id: String,
}

fn channel_session_lookup_candidates(
    current_session_id: &str,
    agent_id: &str,
    parent_session_id: &str,
) -> Vec<ChannelSessionLookup> {
    let mut candidates = Vec::new();
    let current_session_id = current_session_id.trim();
    let parent_session_id = parent_session_id.trim();
    let agent_id = agent_id.trim();

    if !current_session_id.is_empty() {
        let escaped = escape_odata_value(current_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: agent_id.to_string(),
        });
    }

    if !parent_session_id.is_empty() && parent_session_id != current_session_id {
        let escaped = escape_odata_value(parent_session_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and session_entity_id eq '{escaped}'&$top=1"
            ),
            bound_id: parent_session_id.to_string(),
        });
    }

    if !agent_id.is_empty() {
        let escaped = escape_odata_value(agent_id);
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=Status eq 'Active' and agent_entity_id eq '{escaped}'&$orderby=last_message_at desc&$top=1"
            ),
            bound_id: agent_id.to_string(),
        });
        candidates.push(ChannelSessionLookup {
            filter: format!(
                "$filter=agent_entity_id eq '{escaped}'&$orderby=last_message_at desc&$top=1"
            ),
            bound_id: agent_id.to_string(),
        });
    }

    candidates
}

fn escape_odata_value(value: &str) -> String {
    value.replace('\'', "''")
}

/// Find the connected Channel entity for a platform-specific channel ID.
pub fn find_connected_channel_by_external_id(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    channel_id: &str,
) -> Result<Option<Value>, String> {
    let escaped = channel_id.replace('\'', "''");
    let filter = format!("$filter=Status eq 'Connected' and channel_id eq '{escaped}'&$top=1");
    let url = format!("{temper_api_url}/tdata/Channels?{filter}");
    Ok(list_entities(ctx, &url, tenant)?.into_iter().next())
}

/// Parse a basic ISO 8601 timestamp (YYYY-MM-DDTHH:MM:SSZ) to Unix epoch seconds.
/// Returns None if the format is unrecognized.
pub fn parse_iso8601_to_epoch_secs(s: &str) -> Option<u64> {
    // Supported formats: "2026-03-24T12:30:00Z", "2026-03-24T12:30:00.000Z"
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }

    let year: u64 = s.get(0..4)?.parse().ok()?;
    let month: u64 = s.get(5..7)?.parse().ok()?;
    let day: u64 = s.get(8..10)?.parse().ok()?;
    let hour: u64 = s.get(11..13)?.parse().ok()?;
    let minute: u64 = s.get(14..16)?.parse().ok()?;
    let second: u64 = s.get(17..19)?.parse().ok()?;

    if s.as_bytes().get(4) != Some(&b'-')
        || s.as_bytes().get(7) != Some(&b'-')
        || s.as_bytes().get(10) != Some(&b'T')
    {
        return None;
    }

    // Days in each month (non-leap)
    let days_in_month = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);

    // Days from epoch (1970-01-01) to start of `year`
    let mut days: u64 = 0;
    for y in 1970..year {
        let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        days += if leap { 366 } else { 365 };
    }

    // Days from start of year to start of month
    for m in 1..month {
        days += days_in_month[m as usize];
        if m == 2 && is_leap {
            days += 1;
        }
    }

    // Days within month (1-indexed)
    days += day - 1;

    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Past the `extra_json` ceiling the kernel replaces or externalizes the
    /// whole field, so one oversized member costs the turn its date, model and
    /// token counts as well. Writers bound their own extras by policy; this is
    /// the invariant every writer passes through, including the JSONL sync path
    /// that re-materializes extras written before any of those policies existed.
    #[test]
    fn entry_extra_is_bounded_at_the_write_boundary() {
        let oversized: Vec<Value> = (0..MAX_ENTRY_EXTRA_BYTES).map(|i| json!(i % 10)).collect();
        let spec = SessionEntryCreateSpec {
            session_id: "ss-1",
            entry_id: "a-1",
            parent_entry_id: Some("u-1"),
            sequence: 2,
            entry_type: "message",
            role: Some("assistant"),
            content: None,
            content_file_id: None,
            content_file_version_id: None,
            extra_json: Some(&json!({
                "ts_ms": 1_767_225_600_000_i64,
                "provider": "anthropic",
                "model": "claude-sonnet-4-6",
                "input_tokens": 120,
                "output_tokens": 34,
                "logprobs": oversized,
            })),
            tokens: 34,
        };

        let body = session_entry_create_body(&spec).expect("body builds");
        let stored = body["ExtraJson"].as_str().expect("ExtraJson is a string");
        assert!(
            stored_json_len(&Value::String(stored.to_string())) <= MAX_ENTRY_EXTRA_BYTES,
            "the stored field must fit the ceiling the kernel measures, got {} bytes",
            stored.len()
        );

        let extra: Value = serde_json::from_str(stored).expect("stored extra parses");
        assert_eq!(
            extra["ts_ms"], 1_767_225_600_000_i64,
            "the per-turn facts are the last thing sacrificed"
        );
        assert_eq!(extra["provider"], "anthropic");
        assert_eq!(extra["output_tokens"], 34);
        assert!(extra.get("logprobs").is_none());
        assert!(
            extra["logprobs_dropped_bytes"].as_u64().unwrap_or(0) > 0,
            "a dropped member must leave its size behind: {extra}"
        );
    }

    /// The bound has to hold even when nothing is droppable. A provider is free
    /// to return a huge `stop_reason`, and an extras object can carry thousands
    /// of small members whose drop markers alone exceed the ceiling. Returning
    /// an oversized value costs the entire field, which is the outcome the
    /// bound exists to prevent, so both cases end under the ceiling.
    #[test]
    fn entry_extra_bound_holds_when_nothing_is_droppable() {
        let huge_reason = "x".repeat(MAX_ENTRY_EXTRA_BYTES + 1024);
        let bounded = bound_entry_extra(json!({
            "ts_ms": 1_767_225_600_000_i64,
            "provider": "anthropic",
            "stop_reason": huge_reason,
            "output_tokens": 34,
        }));
        assert!(
            stored_json_len(&bounded) <= MAX_ENTRY_EXTRA_BYTES,
            "an oversized essential must be shortened, not passed through"
        );
        assert_eq!(bounded["ts_ms"], 1_767_225_600_000_i64);
        assert_eq!(bounded["output_tokens"], 34);
        assert!(bounded["stop_reason_truncated_chars"].as_u64().unwrap() > 0);

        // Not every oversized essential is a string: the JSONL sync path copies
        // a transcript line's keys verbatim, so a corrupted line can put an
        // array under one. Truncation does not apply, and the drop loop skips
        // essentials, so this used to sail past the bound entirely.
        let bounded = bound_entry_extra(json!({
            "ts_ms": 1_767_225_600_000_i64,
            "stop_reason": vec![7_u64; MAX_ENTRY_EXTRA_BYTES],
        }));
        assert!(
            stored_json_len(&bounded) <= MAX_ENTRY_EXTRA_BYTES,
            "an oversized non-string essential must be bounded too, got {} bytes",
            stored_json_len(&bounded)
        );
        assert_eq!(bounded["ts_ms"], 1_767_225_600_000_i64);
        assert!(bounded["stop_reason_dropped_bytes"].as_u64().unwrap() > 0);

        let mut crowded = serde_json::Map::new();
        crowded.insert("ts_ms".to_string(), json!(1_767_225_600_000_i64));
        crowded.insert("model".to_string(), json!("claude-sonnet-4-6"));
        for index in 0..20_000 {
            crowded.insert(format!("k{index}"), json!("payload"));
        }
        let bounded = bound_entry_extra(Value::Object(crowded));
        assert!(
            stored_json_len(&bounded) <= MAX_ENTRY_EXTRA_BYTES,
            "a marker per dropped member cannot be allowed to hold the value over"
        );
        assert_eq!(bounded["ts_ms"], 1_767_225_600_000_i64);
        assert_eq!(bounded["model"], "claude-sonnet-4-6");
        assert!(
            bounded["_extra_json_dropped_members"].as_u64().unwrap() > 0,
            "the count stands in for the markers it replaced: {bounded}"
        );
    }

    /// The hard floor sheds drop markers, but the token-signal ones are the
    /// emitter's only evidence that signals existed and were refused — losing
    /// them turns a refused signal back into "the serving stack sent none".
    /// There are at most four, so they cannot be what holds the value over.
    #[test]
    fn entry_extra_bound_keeps_the_token_signal_drop_markers() {
        let mut crowded = serde_json::Map::new();
        crowded.insert("ts_ms".to_string(), json!(1_767_225_600_000_i64));
        crowded.insert("logprobs_dropped_bytes".to_string(), json!(40_000));
        crowded.insert("completion_token_ids_dropped_bytes".to_string(), json!(35_000));
        for index in 0..20_000 {
            crowded.insert(format!("k{index}"), json!("payload"));
        }

        let bounded = bound_entry_extra(Value::Object(crowded));
        assert!(stored_json_len(&bounded) <= MAX_ENTRY_EXTRA_BYTES);
        assert_eq!(bounded["ts_ms"], 1_767_225_600_000_i64);
        assert_eq!(
            bounded["logprobs_dropped_bytes"], 40_000,
            "a refused signal must stay visible: {bounded}"
        );
        assert_eq!(bounded["completion_token_ids_dropped_bytes"], 35_000);
        assert!(bounded["_extra_json_dropped_members"].as_u64().unwrap() > 0);
    }

    /// A reader that stores what it read as a record of what an agent did has
    /// to be able to tell an empty transcript from a missing one. Collapsing
    /// both to `Ok("")` is right for a writer resuming a tree and wrong here.
    #[test]
    fn transcript_presence_separates_content_from_absence() {
        assert_eq!(
            transcript_presence("{\"id\":\"u-1\"}\n", TranscriptPresence::EmptyFile),
            TranscriptPresence::Present
        );
        assert_eq!(
            transcript_presence("", TranscriptPresence::EmptyFile),
            TranscriptPresence::EmptyFile
        );
        assert_eq!(
            transcript_presence("", TranscriptPresence::NoEntries),
            TranscriptPresence::NoEntries
        );
        assert_eq!(
            transcript_presence("  \n\n \n", TranscriptPresence::NoEntries),
            TranscriptPresence::NoEntries,
            "a transcript of nothing but whitespace carries no entries"
        );
    }

    /// The reasons travel into the stored trajectory, so they are a wire
    /// contract and cannot be renamed casually.
    #[test]
    fn transcript_presence_names_are_stable() {
        for (presence, name) in [
            (TranscriptPresence::Present, "present"),
            (TranscriptPresence::PendingFirstTurn, "pending_first_turn"),
            (TranscriptPresence::NoEntries, "no_entries"),
            (TranscriptPresence::MissingFile, "missing_file"),
            (TranscriptPresence::EmptyFile, "empty_file"),
            (TranscriptPresence::Undeclared, "undeclared"),
        ] {
            assert_eq!(presence.as_str(), name);
            assert_eq!(
                presence.is_present(),
                presence == TranscriptPresence::Present,
                "only Present may report as present"
            );
        }
    }

    #[test]
    fn bounded_read_helper_builds_point_lookup_urls() {
        let filter = format!(
            "Path eq '{}' and WorkspaceId eq '{}'",
            bounded_reads::odata_escape("/system/skills/we're-here.md"),
            bounded_reads::odata_escape("os-app-docs")
        );

        assert_eq!(
            bounded_reads::bounded_collection_query_path(
                "Files",
                &filter,
                bounded_reads::POINT_LOOKUP_TOP
            ),
            "/tdata/Files?$filter=Path%20eq%20%27/system/skills/we%27%27re-here.md%27%20and%20WorkspaceId%20eq%20%27os-app-docs%27&$top=20"
        );
    }

    #[test]
    fn bounded_read_helper_selects_first_non_archived_entity() {
        let response = json!({
            "value": [
                {"Id": "fl-archived", "Status": "Archived"},
                {"fields": {"Id": "fl-ready", "Status": "Ready"}}
            ]
        });

        assert_eq!(
            bounded_reads::first_non_archived_entity_id(&response),
            Some("fl-ready".to_string())
        );
    }

    #[test]
    fn bounded_read_helper_builds_composite_entity_urls() {
        assert_eq!(
            bounded_reads::composite_entity_url(
                "http://temper",
                "SessionEntries",
                &[("SessionId", "ss-1"), ("EntryId", "a'2")]
            ),
            "http://temper/tdata/SessionEntries(SessionId='ss-1',EntryId='a''2')"
        );
    }

    #[test]
    fn channel_session_lookup_prefers_session_routes_before_agent_fallback() {
        let candidates =
            channel_session_lookup_candidates("ss-current", "aj-agent", "ss-parent");
        let filters: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.filter.as_str())
            .collect();

        assert_eq!(
            filters,
            vec![
                "$filter=Status eq 'Active' and session_entity_id eq 'ss-current'&$top=1",
                "$filter=Status eq 'Active' and session_entity_id eq 'ss-parent'&$top=1",
                "$filter=Status eq 'Active' and agent_entity_id eq 'aj-agent'&$orderby=last_message_at desc&$top=1",
                "$filter=agent_entity_id eq 'aj-agent'&$orderby=last_message_at desc&$top=1",
            ]
        );
        assert_eq!(candidates[0].bound_id, "aj-agent");
        assert_eq!(candidates[1].bound_id, "ss-parent");
    }

    #[test]
    fn channel_session_lookup_escapes_odata_values() {
        let candidates = channel_session_lookup_candidates("ss'oops", "aj'agent", "");

        assert_eq!(
            candidates[0].filter,
            "$filter=Status eq 'Active' and session_entity_id eq 'ss''oops'&$top=1"
        );
        assert_eq!(
            candidates[1].filter,
            "$filter=Status eq 'Active' and agent_entity_id eq 'aj''agent'&$orderby=last_message_at desc&$top=1"
        );
    }

    #[test]
    fn discord_snowflake_guard_rejects_synthetic_threads() {
        assert!(is_discord_snowflake("1018228973869727785"));
        assert!(!is_discord_snowflake(""));
        assert!(!is_discord_snowflake("codex-live-proof-thread"));
        assert!(!is_discord_snowflake("123abc"));
    }

    #[test]
    fn parse_batch_text_file_read_response_deserializes_found_and_missing_items() {
        let by_id = parse_batch_text_file_read_response(
            r#"{
                "files": [
                    {
                        "file_id": "file-a",
                        "found": true,
                        "content_hash": "sha256:file-a",
                        "mime_type": "application/json",
                        "text": "{\"ok\":true}"
                    },
                    {
                        "file_id": "file-missing",
                        "found": false,
                        "content_hash": "",
                        "mime_type": "",
                        "text": ""
                    }
                ]
            }"#,
        )
        .expect("parse batch response");

        assert!(by_id["file-a"].found);
        assert_eq!(by_id["file-a"].text, "{\"ok\":true}");
        assert_eq!(by_id["file-a"].content_hash, "sha256:file-a");
        assert!(!by_id["file-missing"].found);
        assert_eq!(by_id["file-missing"].text, "");
    }

    #[test]
    fn test_parse_iso8601() {
        // 2026-03-24T12:00:00Z
        let secs = parse_iso8601_to_epoch_secs("2026-03-24T12:00:00Z");
        assert!(secs.is_some());
        let s = secs.unwrap();
        // Rough sanity: should be > 2025-01-01 (~1735689600) and < 2027-01-01
        assert!(s > 1_735_000_000);
        assert!(s < 1_800_000_000);
    }

    #[test]
    fn test_parse_iso8601_with_millis() {
        let secs = parse_iso8601_to_epoch_secs("2026-03-24T12:00:00.123Z");
        assert!(secs.is_some());
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_to_epoch_secs("").is_none());
        assert!(parse_iso8601_to_epoch_secs("not-a-date").is_none());
        assert!(parse_iso8601_to_epoch_secs("2026").is_none());
    }

    #[test]
    fn test_epoch_zero() {
        let secs = parse_iso8601_to_epoch_secs("1970-01-01T00:00:00Z");
        assert_eq!(secs, Some(0));
    }

    #[test]
    fn test_direct_field_str() {
        let val = serde_json::json!({"Name": "test", "id": "123"});
        assert_eq!(direct_field_str(&val, &["Name"]), Some("test"));
        assert_eq!(direct_field_str(&val, &["missing", "id"]), Some("123"));
        assert_eq!(direct_field_str(&val, &["missing"]), None);
    }

    #[test]
    fn test_entity_field_str() {
        let val = serde_json::json!({"fields": {"Status": "Active"}});
        assert_eq!(entity_field_str(&val, &["Status"]), Some("Active"));
    }

    #[test]
    fn test_session_entries_ref_round_trips_session_id() {
        let reference = session_entries_ref("ses-123");
        assert_eq!(reference, "session-entries:ses-123");
        assert_eq!(session_id_from_entries_ref(&reference), Some("ses-123"));
        assert!(is_session_entries_ref(&reference));
        assert_eq!(session_id_from_entries_ref("fl-123"), None);
    }

    #[test]
    fn virtual_first_turn_session_entries_are_not_materialized() {
        assert!(!session_entries_materialized(
            &json!({"session_entries_materialized": "false"})
        ));
        assert!(!session_entries_materialized(
            &json!({"fields": {"SessionEntriesMaterialized": false}})
        ));
        assert!(session_entries_materialized(&json!({
            "session_entries_materialized": "true"
        })));
        assert!(session_entries_materialized(&json!({})));
    }

    #[test]
    fn session_entries_list_url_pages_with_skip() {
        assert_eq!(
            session_entries_list_url("http://temper", "ss-1", 1000, 2000),
            "http://temper/tdata/SessionEntries?$filter=SessionId%20eq%20%27ss-1%27&$top=1000&$skip=2000"
        );
    }

    #[test]
    fn next_session_entry_id_advances_numeric_suffix() {
        assert_eq!(next_session_entry_id("a", "u-1"), ("a-2".to_string(), 2));
        assert_eq!(next_session_entry_id("t", "a-17"), ("t-18".to_string(), 18));
    }

    #[test]
    fn next_session_entry_id_maps_initial_session_user_turn_to_logical_sequence() {
        assert_eq!(
            next_session_entry_id("a", "u-ss-019dd16f-0da6-7863-932c-f5a477da4f00-0"),
            ("a-2".to_string(), 2)
        );
    }

    #[test]
    fn next_session_entry_id_defaults_to_first_child_when_parent_suffix_is_not_numeric() {
        assert_eq!(
            next_session_entry_id("a", "legacy-leaf"),
            ("a-1".to_string(), 1)
        );
    }

    #[test]
    fn test_session_entries_jsonl_from_entities_sorts_and_reconstructs() {
        let entities = vec![
            json!({
                "EntryId": "u-1",
                "ParentEntryId": "h-1",
                "Sequence": 1,
                "EntryType": "message",
                "Role": "user",
                "Content": "\"hello\"",
                "Tokens": 2,
                "ExtraJson": "{}"
            }),
            json!({
                "EntryId": "h-1",
                "ParentEntryId": "",
                "Sequence": 0,
                "EntryType": "header",
                "Role": "",
                "Content": "",
                "Tokens": 0,
                "ExtraJson": "{\"version\":1}"
            }),
        ];

        let jsonl = session_entries_jsonl_from_entities(&entities);
        let lines: Vec<Value> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect();

        assert_eq!(lines[0]["id"], "h-1");
        assert_eq!(lines[0]["version"], 1);
        assert_eq!(lines[1]["id"], "u-1");
        assert_eq!(lines[1]["parentId"], "h-1");
        assert_eq!(lines[1]["content"], "hello");
    }

    #[test]
    fn session_entries_jsonl_supports_headerless_root_user() {
        let entities = vec![
            json!({
                "EntryId": "a-2",
                "ParentEntryId": "u-ss-1-0",
                "Sequence": 2,
                "EntryType": "message",
                "Role": "assistant",
                "Content": "[{\"type\":\"text\",\"text\":\"hi\"}]",
                "Tokens": 3,
                "ExtraJson": "{}"
            }),
            json!({
                "EntryId": "u-ss-1-0",
                "ParentEntryId": "",
                "Sequence": 1,
                "EntryType": "message",
                "Role": "user",
                "Content": "\"hello\"",
                "Tokens": 2,
                "ExtraJson": "{}"
            }),
        ];

        let jsonl = session_entries_jsonl_from_entities(&entities);
        let lines: Vec<Value> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect();

        assert_eq!(lines[0]["id"], "u-ss-1-0");
        assert_eq!(lines[0]["parentId"], Value::Null);
        assert_eq!(lines[1]["id"], "a-2");
        assert_eq!(lines[1]["parentId"], "u-ss-1-0");
    }

    #[test]
    fn session_entries_jsonl_keeps_tail_entries_after_first_page() {
        let mut entities = Vec::new();
        entities.push(json!({
            "fields": {
                "EntryId": "h-ss-long",
                "ParentEntryId": "",
                "Sequence": 0,
                "EntryType": "header",
                "Role": "",
                "Content": "",
                "Tokens": 0,
                "ExtraJson": "{\"version\":1}"
            }
        }));

        let mut parent = "h-ss-long".to_string();
        for sequence in 1..=1005 {
            let entry_id = if sequence % 2 == 0 {
                format!("a-{sequence}")
            } else {
                format!("u-{sequence}")
            };
            let role = if entry_id.starts_with("a-") {
                "assistant"
            } else {
                "user"
            };
            entities.push(json!({
                "fields": {
                    "EntryId": entry_id,
                    "ParentEntryId": parent,
                    "Sequence": sequence,
                    "EntryType": "message",
                    "Role": role,
                    "Content": format!("\"entry {sequence}\""),
                    "Tokens": 1,
                    "ExtraJson": "{}"
                }
            }));
            parent = if sequence % 2 == 0 {
                format!("a-{sequence}")
            } else {
                format!("u-{sequence}")
            };
        }

        let jsonl = session_entries_jsonl_from_entities(&entities);
        let lines: Vec<Value> = jsonl
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect();

        assert_eq!(lines.len(), 1006);
        assert_eq!(lines[999]["id"], "u-999");
        assert_eq!(lines[1000]["id"], "a-1000");
        assert_eq!(lines[1005]["id"], "u-1005");
        assert_eq!(lines[1005]["parentId"], "a-1004");
    }

    #[test]
    fn stamp_recorded_at_adds_wall_clock_to_every_entry() {
        let stamped = stamp_recorded_at(None, 1_767_225_600_000);
        assert_eq!(stamped["ts_ms"], 1_767_225_600_000_i64);

        let existing = json!({ "version": 1 });
        let stamped = stamp_recorded_at(Some(&existing), 42);
        assert_eq!(stamped["version"], 1, "caller extras must survive");
        assert_eq!(stamped["ts_ms"], 42);
    }

    #[test]
    fn stamp_recorded_at_preserves_an_explicit_timestamp() {
        let existing = json!({ "ts_ms": 7 });
        let stamped = stamp_recorded_at(Some(&existing), 99);
        assert_eq!(stamped["ts_ms"], 7, "an explicit stamp is authoritative");
    }

    #[test]
    fn stamp_recorded_at_wraps_non_object_extras() {
        let existing = json!("legacy");
        let stamped = stamp_recorded_at(Some(&existing), 5);
        assert_eq!(stamped["extra"], "legacy");
        assert_eq!(stamped["ts_ms"], 5);
    }

    #[test]
    fn session_entry_create_body_shapes_header_and_user_entries() {
        let header_extra = json!({"version": 1});
        let header = session_entry_create_body(&SessionEntryCreateSpec {
            session_id: "ss-1",
            entry_id: "h-ss-1",
            parent_entry_id: None,
            sequence: 0,
            entry_type: "header",
            role: None,
            content: None,
            content_file_id: None,
            content_file_version_id: None,
            extra_json: Some(&header_extra),
            tokens: 0,
        })
        .expect("header body");
        let user_content = json!("hello");
        let user = session_entry_create_body(&SessionEntryCreateSpec {
            session_id: "ss-1",
            entry_id: "u-ss-1-0",
            parent_entry_id: Some("h-ss-1"),
            sequence: 1,
            entry_type: "message",
            role: Some("user"),
            content: Some(&user_content),
            content_file_id: None,
            content_file_version_id: None,
            extra_json: None,
            tokens: 1,
        })
        .expect("user body");

        assert_eq!(header["SessionId"], "ss-1");
        assert_eq!(header["EntryId"], "h-ss-1");
        assert_eq!(header["ParentEntryId"], "");
        assert_eq!(header["ExtraJson"], "{\"version\":1}");
        assert_eq!(user["EntryId"], "u-ss-1-0");
        assert_eq!(user["ParentEntryId"], "h-ss-1");
        assert_eq!(user["Role"], "user");
        assert_eq!(user["Content"], "\"hello\"");
    }

    #[test]
    fn session_entry_create_ack_accepts_matching_odata_state_response() {
        let created = parse_created_session_entry_ack(
            r#"{
                "entity_id": "se-123",
                "fields": {
                    "SessionId": "ss-1",
                    "EntryId": "a-2"
                }
            }"#,
            "ss-1",
            "a-2",
        )
        .expect("acknowledged create");

        assert_eq!(created.entity_id, "se-123");
        assert_eq!(created.entry_id, "a-2");
    }

    #[test]
    fn session_entry_create_ack_rejects_wrong_session_or_entry() {
        assert!(
            parse_created_session_entry_ack(
                r#"{"fields":{"SessionId":"ss-other","EntryId":"a-2"}}"#,
                "ss-1",
                "a-2",
            )
            .is_err()
        );
        assert!(
            parse_created_session_entry_ack(
                r#"{"fields":{"SessionId":"ss-1","EntryId":"a-3"}}"#,
                "ss-1",
                "a-2",
            )
            .is_err()
        );
    }

    #[test]
    fn session_entry_strict_readback_is_enabled_by_default() {
        assert!(session_entry_create_verify_readback_enabled(
            &json!({}),
            None,
        ));
        assert!(session_entry_create_verify_readback_enabled(
            &json!({"session_entry_create_verify_readback": "true"}),
            None,
        ));
        assert!(session_entry_create_verify_readback_enabled(
            &json!({}),
            Some("true"),
        ));
        assert!(!session_entry_create_verify_readback_enabled(
            &json!({"session_entry_create_verify_readback": "false"}),
            Some("true"),
        ));
    }

    #[test]
    fn session_entry_verify_response_requires_visible_row() {
        assert!(session_entry_verify_response_visible(
            r#"{"fields":{"SessionId":"ss-1","EntryId":"u-1"}}"#
        ));
        assert!(session_entry_verify_response_visible(
            r#"{"value":[{"EntryId":"u-1"}]}"#
        ));
        assert!(!session_entry_verify_response_visible(r#"{"value":[]}"#));
        assert!(!session_entry_verify_response_visible("not-json"));
    }

    #[test]
    fn session_entries_verify_urls_are_per_entry_and_bounded() {
        let urls = session_entries_verify_urls("http://temper", "ss-1", &["u-1", "a-2"]);

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].0, "u-1");
        assert_eq!(
            urls[0].1,
            "http://temper/tdata/SessionEntries(SessionId='ss-1',EntryId='u-1')"
        );
        assert_eq!(
            urls[1].1,
            "http://temper/tdata/SessionEntries(SessionId='ss-1',EntryId='a-2')"
        );
        assert!(!urls[0].1.contains("$filter"));
    }

    #[test]
    fn test_session_entry_extra_json_omits_canonical_fields() {
        let entry = json!({
            "id": "c-1",
            "parentId": "a-1",
            "type": "compaction",
            "tokens": 10,
            "summary": "short",
            "first_kept": "u-1"
        });
        let extra = session_entry_extra_json(&entry);
        assert!(extra.get("id").is_none());
        assert_eq!(extra["summary"], "short");
        assert_eq!(extra["first_kept"], "u-1");
    }
}

/// Send a typing indicator to Discord for the given agent.
/// Best-effort: silently ignores all errors.
///
/// Looks up ChannelSession → Channel → webhook_url, then POSTs to /typing.
pub fn send_typing_indicator(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    agent_entity_id: &str,
) {
    let _ = send_typing_indicator_inner(ctx, temper_api_url, tenant, agent_entity_id);
}

fn send_typing_indicator_inner(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    agent_entity_id: &str,
) -> Result<(), String> {
    if agent_entity_id.is_empty() {
        return Ok(());
    }

    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let headers = runtime_headers(ctx, tenant, &fields, None, Some("application/json"));
    let current_session_id = ctx.entity_id.as_str();
    let parent_session_id = entity_field_str(&fields, &["parent_session_id", "ParentSessionId"])
        .unwrap_or("");

    let session = find_channel_session_for_typing(
        ctx,
        temper_api_url,
        &headers,
        current_session_id,
        agent_entity_id,
        parent_session_id,
    )
    .ok_or("no session")?;

    let channel_id = entity_field_str(&session, &["ChannelId", "channel_id"]).unwrap_or("");
    let thread_id = entity_field_str(&session, &["ThreadId", "thread_id"]).unwrap_or("");
    if channel_id.is_empty() || thread_id.is_empty() {
        return Ok(());
    }

    // Find Channel to get webhook_url
    let escaped_ch = channel_id.replace('\'', "''");
    let channel_url = format!(
        "{temper_api_url}/tdata/Channels?$filter=Status eq 'Connected' and channel_id eq '{escaped_ch}'&$top=1"
    );
    let ch_resp = ctx.http_call("GET", &channel_url, &headers, "")?;
    if ch_resp.status != 200 {
        return Ok(());
    }
    let channels: Value =
        serde_json::from_str(&ch_resp.body).unwrap_or_else(|_| json!({"value": []}));
    let channel = channels
        .get("value")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .ok_or("no channel")?;

    let webhook_url = entity_field_str(channel, &["webhook_url", "WebhookUrl"]).unwrap_or("");
    if webhook_url.is_empty() {
        return Ok(());
    }

    let channel_type = entity_field_str(channel, &["ChannelType", "channel_type"]).unwrap_or("");
    if channel_type.eq_ignore_ascii_case("discord") && !is_discord_snowflake(thread_id) {
        ctx.log(
            "warn",
            &format!(
                "send_typing_indicator: skipping invalid Discord thread_id for channel_id={channel_id}"
            ),
        );
        return Ok(());
    }

    // POST to /typing endpoint
    let typing_url = format!(
        "{}/typing",
        webhook_url.trim_end_matches('/').trim_end_matches("/reply")
    );
    let body = json!({"thread_id": thread_id});
    let wh_headers = vec![("content-type".to_string(), "application/json".to_string())];
    let _ = ctx.http_call("POST", &typing_url, &wh_headers, &body.to_string());
    Ok(())
}

fn find_channel_session_for_typing(
    ctx: &Context,
    temper_api_url: &str,
    headers: &[(String, String)],
    current_session_id: &str,
    agent_entity_id: &str,
    parent_session_id: &str,
) -> Option<Value> {
    for candidate in channel_session_lookup_candidates(
        current_session_id,
        agent_entity_id,
        parent_session_id,
    ) {
        let url = format!(
            "{temper_api_url}/tdata/ChannelSessions?{}",
            candidate.filter
        );
        let resp = ctx.http_call("GET", &url, headers, "").ok()?;
        if resp.status != 200 {
            continue;
        }
        let sessions: Value =
            serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({"value": []}));
        if let Some(session) = sessions
            .get("value")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
        {
            return Some(session.clone());
        }
    }

    None
}

fn is_discord_snowflake(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.as_bytes().iter().all(u8::is_ascii_digit)
}
