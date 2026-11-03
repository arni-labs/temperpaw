//! Pure functions that assemble OTS trajectory JSON from Session state.
//!
//! Kept separate from `lib.rs` so the mapping logic can be unit-tested without
//! a live Temper context. The resulting JSON shape matches the `OTSTrajectory`
//! serde schema in `temper/crates/temper-ots/src/models/trajectory.rs` and is
//! versioned at "0.1.0".
//!
//! Turn reconstruction (ARN-109): turns come from the SessionEntry tree, not
//! from a synthetic single turn. Every assistant entry closes one LLM cycle;
//! the user / tool-result / steering / compaction entries that precede it are
//! that turn's prompt side. Decisions come from the assistant's `tool_use`
//! blocks, are answered by the `tool_result` blocks of the following turn, and
//! are enriched with wall-clock duration from the per-session tool-span JSONL.
//!
//! Payload discipline: message bodies that already live in TemperFS are emitted
//! as file references, never inlined, and inline text is bounded per message and
//! per trajectory. Inlining full bodies once cost ~300MB of a 491MB database
//! (.proofs/061); the budget below is the guard against a repeat.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use wasm_helpers::TranscriptPresence;

/// Value of `metadata.harness` — identifies the runtime that produced the run.
pub const HARNESS: &str = "temperpaw";
/// Tag prefix naming evidence this trajectory was built without.
///
/// It lives in `metadata.tags` because that is kernel-modeled: a completeness
/// marker is the last thing that may vanish when a consumer re-serializes a
/// stored row, since losing it turns a partial record into an apparently
/// complete one.
pub const DEGRADED_TAG_PREFIX: &str = "degraded:";
/// OTS schema version emitted by this module.
pub const OTS_VERSION: &str = "0.1.0";

// The JCS contract fields — `metadata.harness`, `metadata.spec_version`, the
// per-turn token-level RL signals and `decisions[].cause_id` — are modeled by
// the pinned `temper-ots` structs and ride natively. A consumer that
// deserializes a stored row into `OTSTrajectory` and writes it back keeps them.
//
// They did not always. Until the pin reached the revision that added them, each
// one travelled through a field the kernel did model — tag mirrors for the
// provenance pair, an inventory in `context.entities[]` for the signals — and a
// test failed the moment a bump made those carriers dead weight. That is what
// removed them; `kernel_round_trip_keeps_the_jcs_contract_fields` now holds the
// native path, and would fail if a carrier were reintroduced or the pin rolled
// back.
//
// `metadata.trajectory_id` stays unmodeled by design: the server's POST handler
// reads it from there before any struct is involved, and the kernel models the
// top-level `trajectory_id` that mirrors it.

/// Largest inline text body attached to a single OTS message.
pub const MAX_MESSAGE_INLINE_CHARS: usize = 4_000;
/// Largest total inline text across the whole trajectory document.
pub const MAX_TRAJECTORY_INLINE_CHARS: usize = 64_000;
/// Largest `consequence.result_summary`.
pub const MAX_RESULT_SUMMARY_CHARS: usize = 500;
/// Largest serialized `choice.arguments` payload.
pub const MAX_ARGUMENTS_CHARS: usize = 4_000;
/// Largest inlined system prompt.
pub const MAX_SYSTEM_PROMPT_CHARS: usize = 2_000;
/// Largest task description taken from the user message.
pub const MAX_TASK_DESCRIPTION_CHARS: usize = 500;
/// Largest total serialized size of token-level signals in one trajectory.
///
/// These arrays scale with completion length and are the only payload in the
/// document the character budgets above do not touch, so a long session could
/// otherwise carry many megabytes of them. Signals past the ceiling are dropped
/// visibly rather than silently.
pub const MAX_TOKEN_SIGNAL_BYTES: usize = 1_048_576;

/// Every token-level signal name, in the shape OTS turns use. The prompt-side
/// ids stand alone; the rest are the positionally aligned completion set.
/// `token_signal_fields_cover_every_signal` keeps the two in step.
pub const TOKEN_SIGNAL_FIELDS: &[&str] = &[
    "prompt_token_ids",
    "completion_token_ids",
    "response_mask",
    "logprobs",
];

const EPOCH: &str = "1970-01-01T00:00:00Z";

/// Everything the emitter knows about a finished session.
pub struct TrajectoryInputs<'a> {
    /// Stable trajectory id (`trj-<session_id>`) — the idempotency key.
    pub trajectory_id: &'a str,
    /// Session entity id.
    pub session_id: &'a str,
    /// Owning agent entity id.
    pub agent_id: &'a str,
    /// Terminal session status (`Completed` / `Failed` / `Cancelled`).
    pub status: &'a str,
    /// Session entity `fields` object.
    pub fields: &'a Value,
    /// Session tree as JSONL (one entry per line), oldest first.
    pub session_jsonl: &'a str,
    /// Per-session tool-span JSONL written by `monty_repl`.
    pub tool_spans_jsonl: &'a str,
    /// Full entity state (used for the event log).
    pub entity_state: &'a Value,
    /// Identity of the governing actor spec (`<app>@<version>`).
    pub spec_version: &'a str,
    /// Whether the transcript behind `session_jsonl` was actually there.
    pub transcript: TranscriptPresence,
    /// Whether a declared tool-span file could not be found.
    pub tool_spans_missing: bool,
}

/// Map the Session's terminal state + has_result flag to an OTS `OutcomeType`.
///
/// The OTS enum serializes as snake_case; returned strings match the on-wire
/// representation exactly.
pub fn derive_outcome(status: &str, has_result: bool) -> &'static str {
    match (status, has_result) {
        ("Completed", true) => "success",
        ("Completed", false) => "partial_success",
        ("Failed", _) => "failure",
        ("Cancelled", _) => "partial_success",
        _ => "failure",
    }
}

/// Classify a tool-call error message into an OTS `consequence.error_type`.
///
/// Categories match the conventions described in ADR-0035 decision section 4.
pub fn classify_error(error_text: &str) -> &'static str {
    let lowered = error_text.to_ascii_lowercase();
    if lowered.is_empty() {
        "unknown_error"
    } else if lowered.contains("timeout") || lowered.contains("timed out") {
        "tool_timeout"
    } else if lowered.contains("cedar") || lowered.contains("denied") {
        "cedar_denied"
    } else {
        "tool_error"
    }
}

/// Truncate a string to at most `max_chars` characters (not bytes). Keeps UTF-8 safe.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Format milliseconds since the Unix epoch as an RFC-3339 UTC timestamp.
///
/// The WASM guests have no chrono dependency, so this implements the civil-date
/// conversion directly. Negative inputs clamp to the epoch — a trajectory with a
/// pre-1970 timestamp is a corrupted clock, not signal worth preserving.
pub fn rfc3339_from_millis(millis: i64) -> String {
    let millis = millis.max(0);
    let total_secs = millis / 1_000;
    let ms = (millis % 1_000) as u32;
    let days = total_secs / 86_400;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z"
    )
}

/// Howard Hinnant's `civil_from_days` — days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// A parsed session-tree entry (JSONL line or materialized SessionEntry row).
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Entry id (`u-*`, `a-*`, `t-*`, `c-*`, `s-*`, `h-*`).
    pub id: String,
    /// Parent entry id, absent for the header.
    pub parent_id: Option<String>,
    /// `header` | `message` | `compaction` | `steering`.
    pub entry_type: String,
    /// `user` | `assistant` | empty.
    pub role: String,
    /// Inline content when the entry was not externalized.
    pub content: Option<Value>,
    /// TemperFS file id when the body was externalized.
    pub content_file_id: Option<String>,
    /// Immutable file version for stable historical reads.
    pub content_file_version_id: Option<String>,
    /// Token estimate recorded with the entry.
    pub tokens: u64,
    /// The whole line, so extras (`ts_ms`, token signals, usage) stay reachable.
    pub raw: Value,
}

impl TreeEntry {
    fn from_value(value: Value) -> Option<Self> {
        let id = value.get("id").and_then(Value::as_str)?.to_string();
        if id.is_empty() {
            return None;
        }
        Some(TreeEntry {
            parent_id: value
                .get("parentId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            entry_type: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message")
                .to_string(),
            role: value
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            content: value.get("content").cloned(),
            content_file_id: value
                .get("content_file_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            content_file_version_id: value
                .get("content_file_version_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            tokens: value.get("tokens").and_then(Value::as_u64).unwrap_or(0),
            id,
            raw: value,
        })
    }

    fn is_assistant(&self) -> bool {
        self.role == "assistant"
    }

    fn is_header(&self) -> bool {
        self.entry_type == "header"
    }

    /// Wall-clock time the entry was recorded, when the writer stamped one.
    fn recorded_at(&self) -> Option<String> {
        if let Some(ms) = self.raw.get("ts_ms").and_then(json_i64) {
            return Some(rfc3339_from_millis(ms));
        }
        self.raw
            .get("timestamp")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// Content blocks when the entry carries an Anthropic-style block array.
    fn blocks(&self) -> Option<&Vec<Value>> {
        self.content.as_ref().and_then(Value::as_array)
    }
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f as i64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

/// Parse session-tree JSONL into ordered entries. Invalid lines are skipped —
/// a corrupted line must not cost the whole trajectory.
pub fn parse_session_entries(session_jsonl: &str) -> Vec<TreeEntry> {
    session_jsonl
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(TreeEntry::from_value)
        .collect()
}

/// A root→leaf chain, and whether the session's own leaf produced it.
pub struct ResolvedChain {
    /// Entry indices from root to leaf.
    pub chain: Vec<usize>,
    /// False when a recorded `session_leaf_id` could not be walked and the
    /// chain came from a fallback. The fallback keeps the trajectory usable,
    /// but the newest turns are the ones missing from it, so a consumer has to
    /// be told. An empty `session_leaf_id` claims nothing and stays true.
    pub from_recorded_leaf: bool,
}

/// Resolve the root→leaf chain the session actually executed.
///
/// Prefers the recorded `session_leaf_id`. When that leaf is missing or its
/// parent chain is broken (continuation/recovery races can leave the Session
/// field ahead of durable rows), falls back to the newest walkable entry and
/// finally to raw file order, so a damaged tree still yields real turns.
pub fn resolve_chain(entries: &[TreeEntry], leaf_id: &str) -> ResolvedChain {
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        by_id.insert(entry.id.as_str(), index);
    }

    // `None` means this leaf did not yield a walkable chain — either an ancestor
    // is missing, or the parent pointers loop. A cycle is not a chain that
    // merely stops early: everything above the loop is unreachable, so treating
    // the fragment as a resolved chain would report a truncated history as the
    // whole one.
    let walk = |leaf: &str| -> Option<Vec<usize>> {
        let mut chain = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut cursor = Some(leaf.to_string());
        while let Some(id) = cursor {
            let index = *by_id.get(id.as_str())?;
            if !seen.insert(entries[index].id.as_str()) {
                return None; // cycle guard — malformed parent pointer
            }
            chain.push(index);
            cursor = entries[index].parent_id.clone();
        }
        chain.reverse();
        Some(chain)
    };

    let has_message = |chain: &[usize]| chain.iter().any(|i| !entries[*i].is_header());

    if !leaf_id.is_empty()
        && let Some(chain) = walk(leaf_id)
        && has_message(&chain)
    {
        return ResolvedChain {
            chain,
            from_recorded_leaf: true,
        };
    }

    // Try the newest entries only. Walking every entry would be quadratic on a
    // long session, and a tree whose last hundred leaves are all unwalkable is
    // damaged far past the point where a smarter search would help.
    const FALLBACK_LEAF_ATTEMPTS: usize = 100;
    let from_recorded_leaf = leaf_id.is_empty();
    for index in (0..entries.len()).rev().take(FALLBACK_LEAF_ATTEMPTS) {
        if let Some(chain) = walk(&entries[index].id)
            && has_message(&chain)
        {
            return ResolvedChain {
                chain,
                from_recorded_leaf,
            };
        }
    }

    // Raw file order is not a walk. No leaf — recorded or not — produced a
    // chain with a message in it, so the parent structure is unusable and the
    // order is a guess at what followed what. That holds whether or not a leaf
    // was recorded, so it is reported even when none was: a session with no
    // recorded leaf and dangling parents is still missing its shape. An empty
    // transcript is the one case with nothing to have resolved, and the
    // presence reasons already speak for it.
    ResolvedChain {
        chain: (0..entries.len()).collect(),
        from_recorded_leaf: entries.is_empty(),
    }
}

/// One reconstructed LLM cycle: the prompt-side entries plus the assistant
/// entry that closed it. The final turn may have no assistant entry when the
/// session was cancelled or failed mid-cycle.
#[derive(Debug, Clone)]
pub struct TurnDraft {
    /// Indices of prompt-side entries, in order.
    pub prompt: Vec<usize>,
    /// Index of the assistant entry that closed the turn.
    pub assistant: Option<usize>,
}

/// Group a root→leaf chain into turns at assistant-message boundaries.
pub fn group_turns(entries: &[TreeEntry], chain: &[usize]) -> Vec<TurnDraft> {
    let mut turns: Vec<TurnDraft> = Vec::new();
    let mut prompt: Vec<usize> = Vec::new();

    for index in chain {
        let entry = &entries[*index];
        if entry.is_header() {
            continue;
        }
        if entry.is_assistant() {
            turns.push(TurnDraft {
                prompt: std::mem::take(&mut prompt),
                assistant: Some(*index),
            });
        } else {
            prompt.push(*index);
        }
    }

    if !prompt.is_empty() {
        turns.push(TurnDraft {
            prompt,
            assistant: None,
        });
    }

    turns
}

/// Bounded inline-text accounting shared across the whole trajectory.
struct InlineBudget {
    remaining: usize,
}

impl InlineBudget {
    fn new(total: usize) -> Self {
        InlineBudget { remaining: total }
    }

    /// Take up to `per_message_max` characters, respecting the global budget.
    /// Returns the text plus the number of characters dropped.
    fn take(&mut self, text: &str, per_message_max: usize) -> (String, usize) {
        let cap = per_message_max.min(self.remaining);
        let total = text.chars().count();
        if cap >= total {
            self.remaining = self.remaining.saturating_sub(total);
            return (text.to_string(), 0);
        }
        let taken: String = text.chars().take(cap).collect();
        self.remaining = self.remaining.saturating_sub(cap);
        (taken, total - cap)
    }
}

/// A tool call the assistant chose, as recovered from a `tool_use` block.
#[derive(Debug, Clone)]
struct ToolCall {
    id: String,
    name: String,
    arguments: Option<Value>,
}

/// The observation a tool call produced, as recovered from a `tool_result` block.
#[derive(Debug, Clone, Default)]
struct Observation {
    is_error: bool,
    text: String,
}

fn tool_calls_from_entry(entry: &TreeEntry) -> Vec<ToolCall> {
    let Some(blocks) = entry.blocks() else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| {
            let id = block.get("id").and_then(Value::as_str)?.to_string();
            Some(ToolCall {
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                arguments: block.get("input").cloned(),
                id,
            })
        })
        .collect()
}

fn observations_from_entry(entry: &TreeEntry) -> Vec<(String, Observation)> {
    let Some(blocks) = entry.blocks() else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| {
            let id = block.get("tool_use_id").and_then(Value::as_str)?.to_string();
            let text = match block.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => serde_json::to_string(other).unwrap_or_default(),
                None => String::new(),
            };
            Some((
                id,
                Observation {
                    is_error: block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    text,
                },
            ))
        })
        .collect()
}

/// Bound a tool-call argument payload, charging it to the trajectory's inline
/// budget. Arguments are the largest attacker-shaped field in a decision — a
/// file-write tool call carries the whole file — so they are capped per call
/// and counted against the same global ceiling as message text.
fn bound_arguments(arguments: Option<Value>, budget: &mut InlineBudget) -> Option<Value> {
    let arguments = arguments?;
    if arguments.is_null() {
        return None;
    }
    let serialized = serde_json::to_string(&arguments).unwrap_or_default();
    let (preview, dropped) = budget.take(&serialized, MAX_ARGUMENTS_CHARS);
    if dropped == 0 {
        return Some(arguments);
    }
    Some(json!({
        "_truncated": true,
        "_original_chars": serialized.chars().count(),
        "_preview": preview,
    }))
}

fn parse_arguments_field(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(s)) => Some(
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.clone())),
        ),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.clone()),
    }
}

/// Build a decision from a tool call plus whatever evidence exists for it.
fn build_decision(
    call: &ToolCall,
    observation: Option<&Observation>,
    span: Option<&Value>,
    budget: &mut InlineBudget,
) -> Value {
    let name = if call.name.is_empty() || call.name == "unknown" {
        span.and_then(|s| s.get("tool_name"))
            .and_then(Value::as_str)
            .unwrap_or(&call.name)
            .to_string()
    } else {
        call.name.clone()
    };

    let arguments = call
        .arguments
        .clone()
        .filter(|value| !value.is_null())
        .or_else(|| parse_arguments_field(span.and_then(|s| s.get("arguments"))));

    let mut choice = json!({ "action": if name.is_empty() { "unknown".to_string() } else { name } });
    if let Some(arguments) = bound_arguments(arguments, budget) {
        choice["arguments"] = arguments;
    }

    let (is_error, result_text) = match observation {
        Some(observation) => (observation.is_error, observation.text.clone()),
        None => (
            span.and_then(|s| s.get("is_error"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            span.and_then(|s| s.get("result"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
    };

    let mut consequence = json!({
        "success": !is_error,
        "result_summary": truncate_chars(&result_text, MAX_RESULT_SUMMARY_CHARS),
    });
    if is_error {
        consequence["error_type"] = json!(classify_error(&result_text));
    }

    let mut decision = json!({
        "decision_id": call.id,
        "decision_type": "tool_selection",
        // cause_id links the decision to the observation it caused: the
        // tool_result block carrying the same tool_call_id, which lands in the
        // next turn's prompt side.
        "cause_id": call.id,
        "choice": choice,
        "consequence": consequence,
    });
    if let Some(duration) = span.and_then(|s| s.get("duration_ms")).and_then(json_u64) {
        decision["_duration_ms"] = json!(duration);
    }
    decision
}

/// Convert a single tool-span JSON object (as emitted by
/// `monty_repl::emit_tool_call_telemetry`) into an `OTSDecision` JSON value.
///
/// Used for spans that no assistant entry claims — the session tree body was
/// externalized to TemperFS, so the span is the only surviving evidence of the
/// call. `_duration_ms` is preserved as a non-standard field for the evaluation
/// agents; the OTS schema has no home for tool wall-clock time.
fn span_to_decision(span: &Value, budget: &mut InlineBudget) -> Value {
    let call = ToolCall {
        id: span
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        name: span
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        arguments: None,
    };
    build_decision(&call, None, Some(span), budget)
}

/// Reserved tool name `monty_repl` writes when the span document hits its size
/// ceiling. It marks an incomplete record, not a decision the agent made.
pub const TOOL_SPANS_TRUNCATED_MARKER: &str = "_tool_spans_truncated";

fn is_truncation_marker(span: &Value) -> bool {
    span.get("tool_name").and_then(Value::as_str) == Some(TOOL_SPANS_TRUNCATED_MARKER)
}

/// A parsed tool-span document.
pub struct ToolSpanDocument {
    /// Real spans, in execution order.
    pub spans: Vec<Value>,
    /// True when the document was sealed at its size ceiling, so the decision
    /// record for this session is knowingly incomplete.
    pub truncated: bool,
    /// Lines that did not parse. Skipping them keeps one bad append from
    /// costing every span, but they are still tool calls with no evidence left,
    /// so the count travels rather than the loss being silent.
    pub unparsed_lines: usize,
}

/// Parse a tool-span JSONL document into span values, preserving execution
/// order and separating out the truncation marker.
///
/// Invalid lines are skipped silently — tool-span persistence is best-effort and
/// the emitter must not fail on a corrupted span. Truncation is decided from the
/// reserved `tool_name` of a parsed record, never from a substring search: a
/// tool that reads or greps the emitter's own source returns the marker literal
/// in its result, and that must not make a complete run look partial.
pub fn parse_tool_span_document(tool_spans_jsonl: &str) -> ToolSpanDocument {
    let mut spans = Vec::new();
    let mut truncated = false;
    let mut unparsed_lines = 0;
    for line in tool_spans_jsonl.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let Ok(span) = serde_json::from_str::<Value>(line) else {
            unparsed_lines += 1;
            continue;
        };
        if is_truncation_marker(&span) {
            truncated = true;
        } else {
            spans.push(span);
        }
    }
    ToolSpanDocument {
        spans,
        truncated,
        unparsed_lines,
    }
}

/// Extract first and last event timestamps from the entity event log.
///
/// Returns `(first, last)` as ISO-8601 strings. Falls back to `"1970-01-01T00:00:00Z"`
/// when no events exist — keeps the schema populated with a legal value rather
/// than failing deserialization.
pub fn extract_event_bookends(entity_state: &Value) -> (String, String) {
    let events = entity_state.get("events").and_then(|v| v.as_array());
    let Some(events) = events else {
        return (EPOCH.to_string(), EPOCH.to_string());
    };
    let first = events
        .first()
        .and_then(|e| e.get("timestamp"))
        .and_then(|v| v.as_str())
        .unwrap_or(EPOCH)
        .to_string();
    let last = events
        .last()
        .and_then(|e| e.get("timestamp"))
        .and_then(|v| v.as_str())
        .unwrap_or(EPOCH)
        .to_string();
    (first, last)
}

/// Timestamps of the events that close an LLM cycle, oldest first.
///
/// The entity event log is a hot tail (older events are dropped at snapshot
/// boundaries), so this is a fallback for entries written before per-entry
/// timestamps were recorded — never the primary source.
fn turn_boundary_event_timestamps(entity_state: &Value) -> Vec<String> {
    const CYCLE_CLOSING_ACTIONS: &[&str] = &[
        "ProcessToolCalls",
        "CheckSteering",
        "RecordResult",
        "RecordResultNoReply",
        "RecordResultInlineReply",
    ];
    entity_state
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event
                        .get("action")
                        .and_then(Value::as_str)
                        .is_some_and(|action| CYCLE_CLOSING_ACTIONS.contains(&action))
                })
                .filter_map(|event| event.get("timestamp").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn field_str<'a>(fields: &'a Value, key: &str) -> &'a str {
    fields.get(key).and_then(Value::as_str).unwrap_or("")
}

fn message_role(entry: &TreeEntry) -> &'static str {
    if entry.is_assistant() {
        return "assistant";
    }
    if entry.entry_type == "compaction" {
        return "system";
    }
    let has_tool_results = entry
        .blocks()
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        });
    if has_tool_results || entry.id.starts_with("t-") {
        return "tool";
    }
    "user"
}

/// Build the OTS message for one session entry, honoring the inline budget.
fn build_message(entry: &TreeEntry, timestamp: &str, budget: &mut InlineBudget) -> Value {
    let role = message_role(entry);
    let mut message = json!({
        "message_id": entry.id,
        "role": role,
        "timestamp": entry.recorded_at().unwrap_or_else(|| timestamp.to_string()),
    });

    // Externalized bodies are referenced, never fetched and never inlined.
    if let Some(file_id) = &entry.content_file_id {
        let mut data = json!({
            "content_file_id": file_id,
            "externalized": true,
            "tokens": entry.tokens,
        });
        if let Some(version_id) = &entry.content_file_version_id {
            data["content_file_version_id"] = json!(version_id);
        }
        message["content"] = json!({
            "type": content_type_for_role(role),
            "data": data,
        });
        return message;
    }

    let mut content = json!({ "type": "text" });
    let mut data = Map::new();

    match entry.content.clone() {
        Some(Value::String(text)) => {
            let (text, dropped) = budget.take(&text, MAX_MESSAGE_INLINE_CHARS);
            content["text"] = json!(text);
            if dropped > 0 {
                data.insert("truncated_chars".to_string(), json!(dropped));
            }
        }
        Some(Value::Array(blocks)) => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut reasoning_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<Value> = Vec::new();
            let mut tool_results: Vec<Value> = Vec::new();

            for block in &blocks {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            text_parts.push(text.to_string());
                        }
                    }
                    "thinking" | "redacted_thinking" => {
                        if let Some(text) = block
                            .get("thinking")
                            .or_else(|| block.get("text"))
                            .and_then(Value::as_str)
                        {
                            reasoning_parts.push(text.to_string());
                        }
                    }
                    "tool_use" => {
                        // Identity only. The arguments live on the decision for
                        // this call; duplicating them here would double the
                        // largest field in the document for no new information.
                        tool_calls.push(json!({
                            "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                            "name": block.get("name").and_then(Value::as_str).unwrap_or(""),
                        }));
                    }
                    "tool_result" => {
                        let text = match block.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => serde_json::to_string(other).unwrap_or_default(),
                            None => String::new(),
                        };
                        let (text, dropped) = budget.take(&text, MAX_MESSAGE_INLINE_CHARS);
                        let mut result = json!({
                            "tool_call_id": block
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or(""),
                            "is_error": block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            "content": text,
                        });
                        if dropped > 0 {
                            result["truncated_chars"] = json!(dropped);
                        }
                        tool_results.push(result);
                    }
                    _ => {}
                }
            }

            if !text_parts.is_empty() {
                let (text, dropped) = budget.take(&text_parts.join("\n"), MAX_MESSAGE_INLINE_CHARS);
                content["text"] = json!(text);
                if dropped > 0 {
                    data.insert("truncated_chars".to_string(), json!(dropped));
                }
            }
            if !reasoning_parts.is_empty() {
                let (reasoning, _) =
                    budget.take(&reasoning_parts.join("\n"), MAX_MESSAGE_INLINE_CHARS);
                message["reasoning"] = json!(reasoning);
            }
            if !tool_calls.is_empty() {
                content["type"] = json!("tool_call");
                data.insert("tool_calls".to_string(), json!(tool_calls));
            }
            if !tool_results.is_empty() {
                content["type"] = json!("tool_response");
                data.insert("tool_results".to_string(), json!(tool_results));
            }
        }
        Some(other) if !other.is_null() => {
            let serialized = serde_json::to_string(&other).unwrap_or_default();
            let (text, dropped) = budget.take(&serialized, MAX_MESSAGE_INLINE_CHARS);
            content["text"] = json!(text);
            if dropped > 0 {
                data.insert("truncated_chars".to_string(), json!(dropped));
            }
        }
        _ => {}
    }

    // Compaction entries keep their summary in an extra field, not `content`.
    if entry.entry_type == "compaction"
        && let Some(summary) = entry.raw.get("summary").and_then(Value::as_str)
    {
        let (text, dropped) = budget.take(summary, MAX_MESSAGE_INLINE_CHARS);
        content["text"] = json!(text);
        data.insert("compaction".to_string(), json!(true));
        if dropped > 0 {
            data.insert("truncated_chars".to_string(), json!(dropped));
        }
    }

    if !data.is_empty() {
        content["data"] = Value::Object(data);
    }
    message["content"] = content;
    message
}

fn content_type_for_role(role: &str) -> &'static str {
    match role {
        "tool" => "tool_response",
        _ => "text",
    }
}

/// Shape check for a token-signal array before it is written to a turn.
type SignalValidator = fn(&Value) -> bool;

/// Completion-side token signals, in the order they are emitted. Element *i* of
/// each one describes the same generated token, so they are positionally
/// aligned with one another.
const COMPLETION_TOKEN_SIGNALS: &[(&str, SignalValidator)] = &[
    ("completion_token_ids", is_u32_array),
    ("response_mask", is_mask_array),
    ("logprobs", is_f64_array),
];

/// The completion-side signal the other two index into. The kernel requires it
/// to be present whenever they are, so the emitter does too.
const COMPLETION_ANCHOR: &str = "completion_token_ids";

/// Whole-trajectory ceiling on token-signal bytes, spent in write order.
struct TokenSignalBudget {
    remaining: usize,
}

impl TokenSignalBudget {
    fn new(total: usize) -> Self {
        TokenSignalBudget { remaining: total }
    }

    /// Serialized size of `value`, or `None` when it does not fit what is left.
    fn take(&mut self, value: &Value) -> Option<usize> {
        let size = serde_json::to_string(value).ok()?.len();
        if size > self.remaining {
            return None;
        }
        self.remaining -= size;
        Some(size)
    }
}

/// Copy token-id / mask / logprob signals onto the turn when the serving stack
/// recorded them. Absent otherwise — the emitter never fabricates them and never
/// makes a provider round-trip to fetch them.
///
/// The completion-side signals are emitted only when the ones that are present
/// agree on length. Arrays of different lengths would hand an RL consumer
/// probabilities and mask bits belonging to the wrong tokens, which is worse
/// than having none: the misalignment is invisible downstream. A dropped set is
/// recorded as `_token_signals_misaligned` so the loss is not silent, and a set
/// dropped for exceeding `MAX_TOKEN_SIGNAL_BYTES` as `_token_signals_dropped`.
///
/// Returns an inventory of what was written: signal name -> element count, plus
/// any drop marker. The caller reads it to tell whether the turn's record came
/// up short.
fn attach_token_signals(
    turn: &mut Value,
    source: &Value,
    budget: &mut TokenSignalBudget,
) -> Map<String, Value> {
    let mut inventory = Map::new();

    // Signals the SessionEntry writer already refused, before the emitter ever
    // saw them. Without carrying these forward, a turn whose signals were all
    // dropped at capture is indistinguishable from a provider that sent none.
    for field in TOKEN_SIGNAL_FIELDS {
        let marker = format!("{field}_dropped_bytes");
        if let Some(size) = source.get(&marker).and_then(json_u64) {
            record_signal_drop_size(turn, &mut inventory, field, size);
        }
    }

    // The entry's extras were themselves cut to fit the field ceiling. Which
    // members went is no longer knowable — only how many — but the turn's
    // record is short either way.
    if let Some(dropped) = source.get("_extra_json_dropped_members").and_then(json_u64)
        && dropped > 0
    {
        turn["_turn_extras_dropped_members"] = json!(dropped);
        inventory.insert("_turn_extras_dropped_members".to_string(), json!(dropped));
    }

    // Prompt-side ids describe the prompt, which the completion signals do not
    // index into, so they stand on their own.
    if let Some(value) = source
        .get("prompt_token_ids")
        .filter(|value| is_u32_array(value))
    {
        if budget.take(value).is_some() {
            inventory.insert(
                "prompt_token_ids".to_string(),
                json!(value.as_array().map(Vec::len).unwrap_or(0)),
            );
            turn["prompt_token_ids"] = value.clone();
        } else {
            record_signal_drop(turn, &mut inventory, "prompt_token_ids", value);
        }
    }

    let present: Vec<(&str, &Value)> = COMPLETION_TOKEN_SIGNALS
        .iter()
        .filter_map(|(field, validator)| {
            source
                .get(*field)
                .filter(|value| validator(value))
                .map(|value| (*field, value))
        })
        .collect();
    if present.is_empty() {
        return inventory;
    }

    // `completion_token_ids` is the anchor the other two index into, and the
    // kernel enforces exactly that: `OTSTurn::validate_token_signals` rejects a
    // turn carrying `response_mask` or `logprobs` without it. A rejected
    // document is not a partial write — the POST answers 400, the emission is
    // recorded failed, and `build_trajectory` is deterministic, so every retry
    // rebuilds the identical rejected document and the row is lost for good.
    //
    // Anchorless sets reach here for real rather than in theory: the
    // SessionEntry writer bounds each signal independently, and the ids are
    // several times the size of the mask, so a long completion loses the ids
    // while the mask survives.
    if !present.iter().any(|(field, _)| *field == COMPLETION_ANCHOR) {
        for (field, value) in present {
            record_signal_drop(turn, &mut inventory, field, value);
        }
        return inventory;
    }

    let mut lengths = Map::new();
    for (field, value) in &present {
        lengths.insert(
            (*field).to_string(),
            json!(value.as_array().map(Vec::len).unwrap_or(0)),
        );
    }
    let aligned = lengths
        .values()
        .map(|length| length.as_u64().unwrap_or(0))
        .collect::<BTreeSet<u64>>()
        .len()
        <= 1;

    if !aligned {
        turn["_token_signals_misaligned"] = Value::Object(lengths.clone());
        inventory.insert("_token_signals_misaligned".to_string(), Value::Object(lengths));
        return inventory;
    }

    // The aligned set travels or is dropped whole: keeping a mask without the
    // ids it indexes leaves a consumer with signal it cannot use.
    let set = Value::Array(present.iter().map(|(_, value)| (*value).clone()).collect());
    if budget.take(&set).is_none() {
        for (field, value) in present {
            record_signal_drop(turn, &mut inventory, field, value);
        }
        return inventory;
    }
    for (field, value) in present {
        inventory.insert(
            field.to_string(),
            json!(value.as_array().map(Vec::len).unwrap_or(0)),
        );
        turn[field] = value.clone();
    }
    inventory
}

/// Record a signal the trajectory budget refused, by element count. A dropped
/// signal that leaves a trace is debuggable; a silent one reads as a turn the
/// serving stack never produced signals for.
fn record_signal_drop(
    turn: &mut Value,
    inventory: &mut Map<String, Value>,
    field: &str,
    value: &Value,
) {
    record_signal_drop_size(
        turn,
        inventory,
        field,
        value.as_array().map(Vec::len).unwrap_or(0) as u64,
    );
}

/// Same record, from a size the caller already knows — the SessionEntry writer
/// reports bytes rather than elements when it refuses a signal at capture.
fn record_signal_drop_size(
    turn: &mut Value,
    inventory: &mut Map<String, Value>,
    field: &str,
    size: u64,
) {
    let entry = json!(size);
    match turn
        .get_mut("_token_signals_dropped")
        .and_then(Value::as_object_mut)
    {
        Some(existing) => {
            existing.insert(field.to_string(), entry.clone());
        }
        None => {
            let mut map = Map::new();
            map.insert(field.to_string(), entry.clone());
            turn["_token_signals_dropped"] = Value::Object(map);
        }
    }
    let carried = inventory
        .entry("_token_signals_dropped".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(map) = carried.as_object_mut() {
        map.insert(field.to_string(), entry);
    }
}

fn is_u32_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|item| item.as_u64().is_some_and(|n| n <= u32::MAX as u64)))
}

/// `response_mask` is a per-token loss switch: every entry is 0 or 1. The kernel
/// rejects the whole turn over a single entry above 1, so a looser check here
/// would hand it a document it refuses — and a refused document is lost, not
/// degraded (`validate_token_signals`).
fn is_mask_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|item| matches!(item.as_u64(), Some(0 | 1))))
}

fn is_f64_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|item| item.as_f64().is_some()))
}

/// Take the earliest span carrying `id` that no decision has claimed yet.
///
/// Repeated ids are matched in execution order, so the first `tool_1` call gets
/// the first `tool_1` span. Without this, a document-wide `id -> span` map hands
/// every call sharing an id the same duration and result.
fn claim_span(
    span_queue_by_id: &BTreeMap<&str, Vec<usize>>,
    span_claimed: &mut [bool],
    id: &str,
) -> Option<usize> {
    let queue = span_queue_by_id.get(id)?;
    let index = *queue.iter().find(|index| !span_claimed[**index])?;
    span_claimed[index] = true;
    Some(index)
}

/// Prefix marking a `session_file_id` that points at SessionEntry rows rather
/// than a TemperFS file. Mirrors `wasm_helpers::session_entries_ref`.
const SESSION_ENTRIES_REF_PREFIX: &str = "session-entries:";

fn file_resource(resources: &mut Vec<Value>, kind: &str, file_id: &str) {
    if file_id.is_empty() {
        return;
    }
    // The session tree is entity-backed in production, so its "file id" is a
    // SessionEntries reference. Pointing a consumer at Files('session-entries:…')
    // would send it somewhere that does not exist.
    let uri = match file_id.strip_prefix(SESSION_ENTRIES_REF_PREFIX) {
        Some(session_id) => format!("temper://SessionEntries?SessionId={session_id}"),
        None => format!("temperfs://Files('{file_id}')"),
    };
    resources.push(json!({ "type": kind, "uri": uri }));
}

/// Evidence the finished document was built without, newest-first in the order
/// the tags were added. Empty for a complete trajectory.
///
/// Read back off the document rather than recomputed from the inputs, so the
/// stored row and the Session entity cannot disagree about what is missing.
pub fn degradations(trajectory: &Value) -> Vec<String> {
    trajectory["metadata"]["tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .filter_map(|tag| tag.strip_prefix(DEGRADED_TAG_PREFIX))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assemble a complete `OTSTrajectory` JSON document.
pub fn build_trajectory(inputs: &TrajectoryInputs<'_>) -> Value {
    let TrajectoryInputs {
        trajectory_id,
        session_id,
        agent_id,
        status,
        fields,
        session_jsonl,
        tool_spans_jsonl,
        entity_state,
        spec_version,
        transcript,
        tool_spans_missing,
    } = *inputs;

    let (event_start, timestamp_end) = extract_event_bookends(entity_state);
    let entries = parse_session_entries(session_jsonl);
    // A transcript that is there but does not parse is missing history just as
    // surely as one that is absent, and `parse_session_entries` drops bad lines
    // deliberately so a single corrupted line cannot cost the whole trajectory.
    // Judging completeness by whether bytes arrived would let a truncated or
    // corrupted file store as a complete record.
    let unparsed_lines = session_jsonl
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        .saturating_sub(entries.len());
    let resolved = resolve_chain(&entries, field_str(fields, "session_leaf_id"));
    let chain = resolved.chain;
    let turn_drafts = group_turns(&entries, &chain);

    let timestamp_start = chain
        .iter()
        .find_map(|index| entries[*index].recorded_at())
        .unwrap_or(event_start);

    let has_result = fields
        .get("has_result")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            fields
                .get("has_result")
                .and_then(|v| v.as_str())
                .map(|s| s == "true")
        })
        .unwrap_or(false);
    let outcome = derive_outcome(status, has_result);

    let task_description = truncate_chars(
        field_str(fields, "user_message"),
        MAX_TASK_DESCRIPTION_CHARS,
    );

    let mut tags: Vec<String> = Vec::new();
    for key in ["model", "provider", "session_mode"] {
        let value = field_str(fields, key);
        if !value.is_empty() {
            tags.push(value.to_string());
        }
    }
    // What the document was built without. `metadata.tags` is the carrier
    // because a consumer that loses a completeness marker reads a partial
    // record as a whole one — and unlike run provenance, which the kernel now
    // models directly, there is no field of its own for this.
    //
    // Absence is only the loudest way a transcript can be short. It can also
    // arrive corrupted, arrive without the newest turns (the recorded leaf does
    // not resolve, so the fallback chain is an older one), or arrive with
    // entries that yield no turn at all. Each of those produces a document that
    // reads as a smaller session than the one that ran, and each is reported.
    //
    // `turn_count` is deliberately not one of these. It counts continuations
    // (tool results, steering, plan resumes), not assistant messages, so it
    // does not equal the reconstructed turn count even on a healthy session —
    // comparing them would mark almost every trajectory degraded. It travels as
    // `_session_turn_count` for a consumer that wants to weigh the two.
    let mut transcript_reasons: Vec<&str> = Vec::new();
    if !transcript.is_present() {
        transcript_reasons.push(transcript.as_str());
    } else {
        if unparsed_lines > 0 {
            transcript_reasons.push("unparseable");
        }
        if !resolved.from_recorded_leaf {
            transcript_reasons.push("leaf_unresolved");
        }
        if turn_drafts.is_empty() {
            transcript_reasons.push("no_turns");
        }
    }
    for reason in &transcript_reasons {
        tags.push(format!("{DEGRADED_TAG_PREFIX}transcript_{reason}"));
    }
    let span_document = parse_tool_span_document(tool_spans_jsonl);
    if span_document.truncated {
        tags.push(format!("{DEGRADED_TAG_PREFIX}tool_spans_truncated"));
    }
    if span_document.unparsed_lines > 0 {
        tags.push(format!("{DEGRADED_TAG_PREFIX}tool_spans_unparseable"));
    }
    if tool_spans_missing {
        tags.push(format!("{DEGRADED_TAG_PREFIX}tool_spans_missing_file"));
    }
    // The session recorded that an append failed. Its tool calls happened and
    // their record did not survive, which an empty span file cannot distinguish
    // from a session that called nothing.
    let tool_spans_write_failed = field_str(fields, "tool_spans_write_failed") == "true";
    if tool_spans_write_failed {
        tags.push(format!("{DEGRADED_TAG_PREFIX}tool_spans_write_failed"));
    }

    // Tool-call ids are unique only within a turn. Providers that omit them get
    // synthetic ids that restart with each response, and a model can repeat one,
    // so a document-wide index would let a later turn's call overwrite an
    // earlier one — both decisions would then carry the last call's consequence
    // and duration. Observations and spans are therefore attributed per turn.
    //
    // A tool_result on turn N's prompt answers a call made in turn N-1, so the
    // observations parsed from turn K's prompt belong to turn K-1. Ones landing
    // on the first turn answer a call made before this chain begins and have no
    // decision to attach to.
    let mut observations_by_turn: Vec<Vec<(String, Observation)>> =
        vec![Vec::new(); turn_drafts.len()];
    for (turn_index, draft) in turn_drafts.iter().enumerate() {
        let Some(answered_turn) = turn_index.checked_sub(1) else {
            continue;
        };
        for prompt_index in &draft.prompt {
            observations_by_turn[answered_turn]
                .extend(observations_from_entry(&entries[*prompt_index]));
        }
    }

    let spans = span_document.spans;
    // Spans are appended in execution order, so repeated ids are matched to
    // calls first-come-first-served rather than collapsed onto one record.
    let mut span_queue_by_id: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, span) in spans.iter().enumerate() {
        if let Some(id) = span.get("tool_call_id").and_then(Value::as_str) {
            span_queue_by_id.entry(id).or_default().push(index);
        }
    }
    let mut span_claimed: Vec<bool> = vec![false; spans.len()];

    let boundary_timestamps = turn_boundary_event_timestamps(entity_state);
    let mut budget = InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS);
    let mut signal_budget = TokenSignalBudget::new(MAX_TOKEN_SIGNAL_BYTES);
    let mut lost_token_signals = false;
    let mut turns: Vec<Value> = Vec::new();

    for (turn_index, draft) in turn_drafts.iter().enumerate() {
        let assistant = draft.assistant.map(|index| &entries[index]);
        let timestamp = assistant
            .and_then(|entry| entry.recorded_at())
            .or_else(|| boundary_timestamps.get(turn_index).cloned())
            .unwrap_or_else(|| timestamp_start.clone());

        let mut messages: Vec<Value> = Vec::new();
        for prompt_index in &draft.prompt {
            messages.push(build_message(&entries[*prompt_index], &timestamp, &mut budget));
        }
        if let Some(entry) = assistant {
            messages.push(build_message(entry, &timestamp, &mut budget));
        }

        // Decisions the assistant made in this cycle, in call order. Everything
        // below is scoped to this turn, because ids repeat across turns.
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut named: BTreeSet<String> = BTreeSet::new();
        if let Some(entry) = assistant {
            for call in tool_calls_from_entry(entry) {
                named.insert(call.id.clone());
                calls.push(call);
            }
        }
        // Calls whose only evidence is the observation — the assistant body was
        // externalized, so the tree cannot name them — still deserve a decision.
        // `build_decision` recovers the tool name from the span when one exists.
        for (id, _) in &observations_by_turn[turn_index] {
            if named.insert(id.clone()) {
                calls.push(ToolCall {
                    id: id.clone(),
                    name: "unknown".to_string(),
                    arguments: None,
                });
            }
        }

        let mut observation_by_id: BTreeMap<&str, &Observation> = BTreeMap::new();
        for (id, observation) in &observations_by_turn[turn_index] {
            observation_by_id.entry(id.as_str()).or_insert(observation);
        }

        let mut decisions: Vec<Value> = Vec::new();
        let mut turn_error = false;
        let mut turn_duration_ms: u64 = 0;
        for call in &calls {
            let span_index = claim_span(&span_queue_by_id, &mut span_claimed, &call.id);
            let decision = build_decision(
                call,
                observation_by_id.get(call.id.as_str()).copied(),
                span_index.map(|index| &spans[index]),
                &mut budget,
            );
            if decision["consequence"]["success"] == json!(false) {
                turn_error = true;
            }
            if let Some(duration) = decision.get("_duration_ms").and_then(json_u64) {
                turn_duration_ms += duration;
            }
            decisions.push(decision);
        }

        let span_id = assistant
            .map(|entry| format!("{session_id}:{}", entry.id))
            .unwrap_or_else(|| format!("{session_id}:turn-{}", turn_index + 1));

        let mut turn = json!({
            "turn_id": (turn_index + 1) as i64,
            "span_id": span_id,
            "timestamp": timestamp,
            "error": turn_error,
            "messages": messages,
            "decisions": decisions,
        });
        if turn_duration_ms > 0 {
            turn["duration_ms"] = json!(turn_duration_ms as f64);
        }

        let prompt_tokens: u64 = assistant
            .and_then(|entry| entry.raw.get("input_tokens").and_then(json_u64))
            .unwrap_or_else(|| {
                draft
                    .prompt
                    .iter()
                    .map(|index| entries[*index].tokens)
                    .sum()
            });
        let completion_tokens: u64 = assistant
            .and_then(|entry| {
                entry
                    .raw
                    .get("output_tokens")
                    .and_then(json_u64)
                    .or(Some(entry.tokens))
            })
            .unwrap_or(0);
        turn["_prompt_tokens"] = json!(prompt_tokens);
        turn["_completion_tokens"] = json!(completion_tokens);

        if let Some(entry) = assistant {
            let inventory = attach_token_signals(&mut turn, &entry.raw, &mut signal_budget);
            // Misalignment discards the completion-side set exactly as a budget
            // drop does, and extras cut to fit the entry ceiling take turn facts
            // with them. All three are the record coming up short.
            lost_token_signals |= inventory.contains_key("_token_signals_dropped")
                || inventory.contains_key("_token_signals_misaligned")
                || inventory.contains_key("_turn_extras_dropped_members");
        }

        turns.push(turn);
    }

    // Spans no turn claimed (whole tree unavailable, or a call the tree never
    // recorded) still carry real decisions — attach them to the last turn so no
    // evidence is silently dropped. Claiming is tracked by span position, so two
    // spans sharing an id are two decisions, not one.
    let orphan_decisions: Vec<Value> = spans
        .iter()
        .enumerate()
        .filter(|(index, _)| !span_claimed[*index])
        .map(|(_, span)| span_to_decision(span, &mut budget))
        .collect();

    if !orphan_decisions.is_empty() {
        if turns.is_empty() {
            turns.push(json!({
                "turn_id": 1_i64,
                "span_id": format!("{session_id}:turn-1"),
                "timestamp": timestamp_start,
                "error": orphan_decisions
                    .iter()
                    .any(|d| d["consequence"]["success"] == json!(false)),
                "messages": Vec::<Value>::new(),
                "decisions": orphan_decisions,
            }));
        } else {
            let last = turns.len() - 1;
            if orphan_decisions
                .iter()
                .any(|d| d["consequence"]["success"] == json!(false))
            {
                turns[last]["error"] = json!(true);
            }
            if let Some(existing) = turns[last]["decisions"].as_array_mut() {
                existing.extend(orphan_decisions);
            }
        }
    }

    // Every session produces at least one turn, so an empty tree plus zero
    // spans still yields a schema-valid document rather than an empty array.
    if turns.is_empty() {
        turns.push(json!({
            "turn_id": 1_i64,
            "span_id": format!("{session_id}:turn-1"),
            "timestamp": timestamp_start,
            "error": status == "Failed",
            "messages": Vec::<Value>::new(),
            "decisions": Vec::<Value>::new(),
        }));
    }

    // Announced in a kernel-modeled field so a consumer working from a
    // re-serialized row knows the stored row holds signals its structs cannot
    // represent. Only when a signal was actually written: a turn whose signals
    // were all dropped carries the record of the drop, not a signal to read.
    // A signal the writer or this budget refused is evidence the row was built
    // without, which is what degraded means — so it reaches the Session status
    // the same way a missing transcript does, rather than only the document.
    if lost_token_signals {
        tags.push(format!("{DEGRADED_TAG_PREFIX}token_signals_dropped"));
    }

    // trajectory_id is duplicated inside `metadata` because Temper's server-side
    // POST handler at temper-server/src/observe/evolution/trajectories.rs reads
    // it from metadata.trajectory_id (not the OTS top-level field). Emitting in
    // both places keeps OTS schema compliance AND lets the Turso row use the same
    // id my module stored on the Session entity — which is what makes
    // INSERT OR REPLACE-based retry idempotency actually work.
    let mut metadata = json!({
        "trajectory_id": trajectory_id,
        "task_description": task_description,
        "domain": "temperpaw-agent",
        "timestamp_start": timestamp_start,
        "timestamp_end": timestamp_end,
        "agent_id": agent_id,
        "framework": "temperpaw",
        "harness": HARNESS,
        "environment": "production",
        "outcome": outcome,
        "tags": tags,
    });
    if !spec_version.is_empty() {
        metadata["spec_version"] = json!(spec_version);
    }

    let mut resources: Vec<Value> = Vec::new();
    file_resource(&mut resources, "session_tree", field_str(fields, "session_file_id"));
    file_resource(&mut resources, "tool_spans", field_str(fields, "tool_spans_file_id"));
    file_resource(
        &mut resources,
        "prepared_context",
        field_str(fields, "prepared_context_file_id"),
    );
    file_resource(
        &mut resources,
        "provider_response",
        field_str(fields, "provider_response_file_id"),
    );
    file_resource(
        &mut resources,
        "system_prompt",
        field_str(fields, "system_prompt_file_id"),
    );

    let mut trajectory = json!({
        "trajectory_id": trajectory_id,
        "version": OTS_VERSION,
        "metadata": metadata,
        "turns": turns,
        "_token_usage": {
            "input_tokens": fields.get("input_tokens").and_then(json_u64).unwrap_or(0),
            "output_tokens": fields.get("output_tokens").and_then(json_u64).unwrap_or(0),
            "context_tokens": fields.get("context_tokens").and_then(json_u64).unwrap_or(0),
        },
        "_session_turn_count": fields.get("turn_count").and_then(json_u64).unwrap_or(0),
    });

    if span_document.truncated {
        // The span document hit its ceiling, so some tool timings are missing.
        // A consumer training on this must know the record is partial.
        trajectory["_tool_spans_truncated"] = json!(true);
    }

    if span_document.unparsed_lines > 0 {
        // Tool calls whose only evidence was a span line that did not parse.
        trajectory["_tool_spans_unparsed_lines"] = json!(span_document.unparsed_lines);
    }

    if !transcript_reasons.is_empty() {
        // The turn structure that makes a trajectory readable was not all
        // there. Each reason is a different way of being short of the session
        // that actually ran.
        let mut marker = json!({
            "present": transcript.is_present(),
            "reasons": transcript_reasons,
        });
        if unparsed_lines > 0 {
            marker["unparsed_lines"] = json!(unparsed_lines);
        }
        trajectory["_transcript"] = marker;
    }
    if tool_spans_missing {
        trajectory["_tool_spans_missing"] = json!(true);
    }
    if tool_spans_write_failed {
        trajectory["_tool_spans_write_failed"] = json!(true);
    }

    if !resources.is_empty() {
        trajectory["context"] = json!({ "resources": resources });
    }

    let system_prompt = field_str(fields, "system_prompt");
    if !system_prompt.is_empty() {
        // OTSSystemMessage only has { content, timestamp } — no `role` field.
        trajectory["system_message"] = json!({
            "content": truncate_chars(system_prompt, MAX_SYSTEM_PROMPT_CHARS),
            "timestamp": trajectory["metadata"]["timestamp_start"].clone(),
        });
    }

    trajectory
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact set of emitted fields the pinned `temper-ots` structs do not
    /// model, and therefore drop on a deserialize/re-serialize round trip.
    /// Pinned by `kernel_round_trip_drops_exactly_the_unmodeled_extensions`,
    /// which fails the day the kernel starts modeling one of them.
    const KERNEL_UNMODELED_FIELDS: &[&str] = &[
        // Read by the server's POST handler before any struct is involved, and
        // repeated at the top level where the kernel does model it.
        "metadata.trajectory_id",
    ];

    fn inputs<'a>(
        fields: &'a Value,
        session_jsonl: &'a str,
        tool_spans_jsonl: &'a str,
        entity_state: &'a Value,
        status: &'a str,
    ) -> TrajectoryInputs<'a> {
        TrajectoryInputs {
            trajectory_id: "trj-ss-1",
            session_id: "ss-1",
            agent_id: "aj-1",
            status,
            fields,
            session_jsonl,
            tool_spans_jsonl,
            entity_state,
            spec_version: "paw-agent@0.1.0",
            // Fixtures that pass a transcript describe a session whose
            // transcript was read; the degradation paths set this explicitly.
            transcript: if session_jsonl.trim().is_empty() {
                TranscriptPresence::NoEntries
            } else {
                TranscriptPresence::Present
            },
            tool_spans_missing: false,
        }
    }

    fn entity_state_with_events() -> Value {
        json!({
            "events": [
                { "action": "Created", "timestamp": "2026-01-01T00:00:00Z" },
                { "action": "Cancel",  "timestamp": "2026-01-01T00:00:01Z" },
            ]
        })
    }

    /// Two real LLM cycles: user -> assistant(tool_use) -> tool_result ->
    /// assistant(final text).
    fn two_turn_session_jsonl() -> String {
        let lines = [
            json!({"id":"h-ss-1","parentId":null,"type":"header","tokens":0}),
            json!({
                "id":"u-ss-1-0","parentId":"h-ss-1","type":"message","role":"user",
                "content":"find the bug","tokens":3,"ts_ms":1_767_225_600_000_i64
            }),
            json!({
                "id":"a-1","parentId":"u-ss-1-0","type":"message","role":"assistant",
                "content":[
                    {"type":"thinking","thinking":"I should grep first"},
                    {"type":"text","text":"Looking now"},
                    {"type":"tool_use","id":"tc-1","name":"temper.bash","input":{"cmd":"rg TODO"}}
                ],
                "tokens":42,"ts_ms":1_767_225_601_000_i64,
                "input_tokens":100,"output_tokens":42
            }),
            json!({
                "id":"t-2","parentId":"a-1","type":"message","role":"user",
                "content":[
                    {"type":"tool_result","tool_use_id":"tc-1","content":"src/lib.rs:12: TODO","is_error":false}
                ],
                "tokens":9,"ts_ms":1_767_225_602_000_i64
            }),
            json!({
                "id":"a-3","parentId":"t-2","type":"message","role":"assistant",
                "content":[{"type":"text","text":"Found it on line 12"}],
                "tokens":11,"ts_ms":1_767_225_603_000_i64,
                "input_tokens":160,"output_tokens":11
            }),
        ];
        lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn two_turn_fields() -> Value {
        json!({
            "user_message": "find the bug",
            "model": "claude-sonnet-4-6",
            "provider": "anthropic",
            "session_mode": "execute",
            "has_result": true,
            "session_leaf_id": "a-3",
            "session_file_id": "session-entries:ss-1",
            "tool_spans_file_id": "file-spans-1",
            "turn_count": 2,
            "input_tokens": 260,
            "output_tokens": 53,
            "context_tokens": 160,
        })
    }

    #[test]
    fn derive_outcome_matrix() {
        assert_eq!(derive_outcome("Completed", true), "success");
        assert_eq!(derive_outcome("Completed", false), "partial_success");
        assert_eq!(derive_outcome("Failed", true), "failure");
        assert_eq!(derive_outcome("Failed", false), "failure");
        assert_eq!(derive_outcome("Cancelled", true), "partial_success");
        assert_eq!(derive_outcome("Cancelled", false), "partial_success");
        assert_eq!(derive_outcome("Thinking", true), "failure"); // defensive
    }

    #[test]
    fn classify_error_categories() {
        assert_eq!(classify_error(""), "unknown_error");
        assert_eq!(classify_error("request timed out after 30s"), "tool_timeout");
        assert_eq!(classify_error("Cedar denied action"), "cedar_denied");
        assert_eq!(classify_error("permission denied by policy"), "cedar_denied");
        assert_eq!(classify_error("subprocess failed"), "tool_error");
    }

    #[test]
    fn truncate_chars_respects_utf8() {
        let s = "héllo wörld";
        assert_eq!(truncate_chars(s, 5), "héllo");
        assert_eq!(truncate_chars(s, 100), s);
    }

    #[test]
    fn rfc3339_from_millis_formats_utc() {
        assert_eq!(rfc3339_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_from_millis(-5), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            rfc3339_from_millis(1_767_225_600_000),
            "2026-01-01T00:00:00.000Z"
        );
        // Leap day, with sub-second precision retained.
        assert_eq!(
            rfc3339_from_millis(1_709_209_845_123),
            "2024-02-29T12:30:45.123Z"
        );
    }

    #[test]
    fn span_to_decision_success_shape() {
        let span = json!({
            "tool_name": "temper.read",
            "tool_call_id": "tc-123",
            "arguments": "{\"path\":\"/tmp/foo\"}",
            "result": "ok",
            "duration_ms": 42,
            "is_error": false,
        });
        let dec = span_to_decision(&span, &mut InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS));
        assert_eq!(dec["decision_id"], "tc-123");
        assert_eq!(dec["decision_type"], "tool_selection");
        assert_eq!(dec["cause_id"], "tc-123");
        assert_eq!(dec["choice"]["action"], "temper.read");
        assert_eq!(dec["choice"]["arguments"]["path"], "/tmp/foo");
        assert_eq!(dec["consequence"]["success"], true);
        assert_eq!(dec["consequence"]["result_summary"], "ok");
        assert!(dec["consequence"].get("error_type").is_none());
        assert_eq!(dec["_duration_ms"], 42);
    }

    #[test]
    fn span_to_decision_error_classifies() {
        let span = json!({
            "tool_name": "temper.bash",
            "tool_call_id": "tc-err",
            "arguments": "{}",
            "result": "Cedar denied bash execution",
            "duration_ms": 5,
            "is_error": true,
        });
        let dec = span_to_decision(&span, &mut InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS));
        assert_eq!(dec["consequence"]["success"], false);
        assert_eq!(dec["consequence"]["error_type"], "cedar_denied");
    }

    #[test]
    fn span_to_decision_keeps_non_json_arguments_as_string() {
        let span = json!({
            "tool_name": "raw",
            "tool_call_id": "tc-raw",
            "arguments": "not-valid-json",
            "result": "",
            "duration_ms": 0,
            "is_error": false,
        });
        let dec = span_to_decision(&span, &mut InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS));
        assert_eq!(dec["choice"]["arguments"], "not-valid-json");
    }

    #[test]
    fn span_to_decision_truncates_long_result() {
        let long_result = "x".repeat(1000);
        let span = json!({
            "tool_name": "temper.bash",
            "tool_call_id": "tc-long",
            "arguments": "{}",
            "result": long_result,
            "duration_ms": 1,
            "is_error": false,
        });
        let dec = span_to_decision(&span, &mut InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS));
        let summary = dec["consequence"]["result_summary"].as_str().unwrap();
        assert_eq!(summary.chars().count(), MAX_RESULT_SUMMARY_CHARS);
    }

    #[test]
    fn parse_tool_spans_skips_invalid_lines() {
        let jsonl = "{\"tool_call_id\":\"a\",\"tool_name\":\"x\",\"result\":\"\",\"duration_ms\":0,\"is_error\":false}\nINVALID\n{\"tool_call_id\":\"b\",\"tool_name\":\"y\",\"result\":\"\",\"duration_ms\":0,\"is_error\":false}\n";
        let mut budget = InlineBudget::new(MAX_TRAJECTORY_INLINE_CHARS);
        let decisions: Vec<Value> = parse_tool_span_document(jsonl)
            .spans
            .iter()
            .map(|span| span_to_decision(span, &mut budget))
            .collect();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0]["decision_id"], "a");
        assert_eq!(decisions[1]["decision_id"], "b");
    }

    #[test]
    fn resolve_chain_prefers_recorded_leaf() {
        let entries = parse_session_entries(&two_turn_session_jsonl());
        let resolved = resolve_chain(&entries, "a-3");
        let ids: Vec<&str> = resolved
            .chain
            .iter()
            .map(|i| entries[*i].id.as_str())
            .collect();
        assert_eq!(ids, vec!["h-ss-1", "u-ss-1-0", "a-1", "t-2", "a-3"]);
        assert!(resolved.from_recorded_leaf);
    }

    #[test]
    fn resolve_chain_falls_back_when_leaf_is_missing() {
        let entries = parse_session_entries(&two_turn_session_jsonl());
        let resolved = resolve_chain(&entries, "a-999-never-written");
        let ids: Vec<&str> = resolved
            .chain
            .iter()
            .map(|i| entries[*i].id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["h-ss-1", "u-ss-1-0", "a-1", "t-2", "a-3"],
            "a leaf ahead of durable rows must not empty the trajectory"
        );
        assert!(
            !resolved.from_recorded_leaf,
            "the fallback chain is missing the newest turns, and the caller has to know"
        );
    }

    /// The names the carrier and the drop markers iterate must stay the same
    /// set the turn writer emits, or a signal added later travels silently.
    #[test]
    fn token_signal_fields_cover_every_signal() {
        let mut expected: Vec<&str> = vec!["prompt_token_ids"];
        expected.extend(COMPLETION_TOKEN_SIGNALS.iter().map(|(field, _)| *field));
        let mut expected: Vec<&str> = expected;
        expected.sort_unstable();
        let mut declared: Vec<&str> = TOKEN_SIGNAL_FIELDS.to_vec();
        declared.sort_unstable();
        assert_eq!(declared, expected);
    }

    /// A cycle is not a chain that stops early: everything above the loop is
    /// unreachable, so accepting the fragment would report a truncated history
    /// as the recorded leaf's own.
    #[test]
    fn resolve_chain_reports_a_cyclic_ancestry_as_unresolved() {
        let jsonl = [
            json!({"id":"u-0","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({"id":"a-0","parentId":"u-0","type":"message","role":"assistant","content":"ok"}),
            json!({"id":"a","parentId":"b","type":"message","role":"assistant","content":"x"}),
            json!({"id":"b","parentId":"a","type":"message","role":"user","content":"y"}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let entries = parse_session_entries(&jsonl);
        let resolved = resolve_chain(&entries, "a");
        assert!(
            !resolved.from_recorded_leaf,
            "a leaf whose ancestry loops has not resolved"
        );

        let fields = json!({ "session_leaf_id": "a", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));
        assert!(
            degradations(&t).contains(&"transcript_leaf_unresolved".to_string()),
            "tags: {:?}",
            t["metadata"]["tags"]
        );
    }

    #[test]
    fn resolve_chain_survives_parent_cycle() {
        let jsonl = [
            json!({"id":"a","parentId":"b","type":"message","role":"assistant","content":"x"}),
            json!({"id":"b","parentId":"a","type":"message","role":"user","content":"y"}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let entries = parse_session_entries(&jsonl);
        let resolved = resolve_chain(&entries, "a");
        assert!(resolved.chain.len() <= entries.len());
    }

    #[test]
    fn build_trajectory_reconstructs_real_turns() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"arguments\":\"{}\",\"result\":\"src/lib.rs:12: TODO\",\"duration_ms\":137,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let turns = t["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 2, "two assistant messages means two turns");
        assert_eq!(turns[0]["turn_id"], 1);
        assert_eq!(turns[1]["turn_id"], 2);
        assert_eq!(turns[0]["span_id"], "ss-1:a-1");
        assert_eq!(turns[1]["span_id"], "ss-1:a-3");
        assert_eq!(turns[0]["timestamp"], "2026-01-01T00:00:01.000Z");
        assert_eq!(turns[1]["timestamp"], "2026-01-01T00:00:03.000Z");

        // Turn 1 carries the user prompt plus the assistant reply.
        let turn1_messages = turns[0]["messages"].as_array().unwrap();
        assert_eq!(turn1_messages.len(), 2);
        assert_eq!(turn1_messages[0]["role"], "user");
        assert_eq!(turn1_messages[0]["content"]["text"], "find the bug");
        assert_eq!(turn1_messages[1]["role"], "assistant");
        assert_eq!(turn1_messages[1]["content"]["type"], "tool_call");
        assert_eq!(turn1_messages[1]["content"]["text"], "Looking now");
        assert_eq!(turn1_messages[1]["reasoning"], "I should grep first");

        // Turn 2's prompt side is the tool result observation.
        let turn2_messages = turns[1]["messages"].as_array().unwrap();
        assert_eq!(turn2_messages.len(), 2);
        assert_eq!(turn2_messages[0]["role"], "tool");
        assert_eq!(turn2_messages[0]["content"]["type"], "tool_response");
        assert_eq!(
            turn2_messages[0]["content"]["data"]["tool_results"][0]["tool_call_id"],
            "tc-1"
        );
    }

    #[test]
    fn build_trajectory_populates_decisions_with_cause_id() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"arguments\":\"{}\",\"result\":\"ignored\",\"duration_ms\":137,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let decisions = t["turns"][0]["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0]["decision_id"], "tc-1");
        assert_eq!(decisions[0]["cause_id"], "tc-1");
        assert_eq!(decisions[0]["decision_type"], "tool_selection");
        assert_eq!(decisions[0]["choice"]["action"], "temper.bash");
        assert_eq!(decisions[0]["choice"]["arguments"]["cmd"], "rg TODO");
        assert_eq!(decisions[0]["consequence"]["success"], true);
        assert_eq!(
            decisions[0]["consequence"]["result_summary"], "src/lib.rs:12: TODO",
            "the tool_result observation wins over the span result"
        );
        assert_eq!(decisions[0]["_duration_ms"], 137);
        assert_eq!(t["turns"][0]["duration_ms"], 137.0);
        assert_eq!(t["turns"][0]["error"], false);
        assert!(
            t["turns"][1]["decisions"].as_array().unwrap().is_empty(),
            "the final text-only turn makes no tool decisions"
        );
    }

    #[test]
    fn build_trajectory_marks_turn_error_on_failed_tool_call() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"tool_use","id":"tc-9","name":"temper.bash","input":{}}]
            }),
            json!({
                "id":"t-2","parentId":"a-1","type":"message","role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tc-9","content":"Cedar denied bash","is_error":true}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "t-2", "has_result": false });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Failed"));

        assert_eq!(t["turns"][0]["error"], true);
        assert_eq!(t["turns"][0]["decisions"][0]["consequence"]["success"], false);
        assert_eq!(
            t["turns"][0]["decisions"][0]["consequence"]["error_type"],
            "cedar_denied"
        );
    }

    #[test]
    fn build_trajectory_references_externalized_content_instead_of_inlining() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content_file_id":"file-abc","content_file_version_id":"ver-1","tokens":9000
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let assistant = &t["turns"][0]["messages"][1];
        assert_eq!(assistant["content"]["data"]["content_file_id"], "file-abc");
        assert_eq!(
            assistant["content"]["data"]["content_file_version_id"],
            "ver-1"
        );
        assert_eq!(assistant["content"]["data"]["externalized"], true);
        assert!(
            assistant["content"].get("text").is_none(),
            "externalized bodies must never be inlined"
        );
    }

    #[test]
    fn build_trajectory_bounds_inline_text() {
        let huge = "x".repeat(MAX_MESSAGE_INLINE_CHARS * 3);
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":huge}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"ok"}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let user = &t["turns"][0]["messages"][0];
        let text = user["content"]["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), MAX_MESSAGE_INLINE_CHARS);
        assert_eq!(
            user["content"]["data"]["truncated_chars"],
            json!(MAX_MESSAGE_INLINE_CHARS * 2)
        );
    }

    #[test]
    fn build_trajectory_bounds_total_inline_text_across_messages() {
        let chunk = "y".repeat(MAX_MESSAGE_INLINE_CHARS);
        let mut lines: Vec<Value> = vec![json!({
            "id":"u-0","parentId":null,"type":"message","role":"user","content":"start"
        })];
        let mut parent = "u-0".to_string();
        // 40 assistant/user pairs of full-size bodies far exceed the global budget.
        for index in 1..40 {
            let assistant = format!("a-{index}");
            lines.push(json!({
                "id": assistant, "parentId": parent, "type": "message", "role": "assistant",
                "content": [{"type":"text","text": chunk}]
            }));
            let user = format!("u-{index}");
            lines.push(json!({
                "id": user, "parentId": assistant, "type": "message", "role": "user",
                "content": chunk
            }));
            parent = user;
        }
        let jsonl = lines
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let fields = json!({ "session_leaf_id": parent, "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let inline_chars: usize = t["turns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|turn| turn["messages"].as_array().unwrap())
            .filter_map(|message| message["content"].get("text").and_then(Value::as_str))
            .map(|text| text.chars().count())
            .sum();
        assert!(
            inline_chars <= MAX_TRAJECTORY_INLINE_CHARS,
            "inline text budget exceeded: {inline_chars}"
        );
    }

    #[test]
    fn build_trajectory_carries_token_signals_when_recorded() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "input_tokens": 12, "output_tokens": 3,
                "prompt_token_ids":[1,2,3],
                "completion_token_ids":[4,5],
                "response_mask":[1,1],
                "logprobs":[-0.25,-1.5]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let turn = &t["turns"][0];
        assert_eq!(turn["prompt_token_ids"], json!([1, 2, 3]));
        assert_eq!(turn["completion_token_ids"], json!([4, 5]));
        assert_eq!(turn["response_mask"], json!([1, 1]));
        assert_eq!(turn["logprobs"], json!([-0.25, -1.5]));
        assert_eq!(turn["_prompt_tokens"], 12);
        assert_eq!(turn["_completion_tokens"], 3);
    }

    #[test]
    fn build_trajectory_omits_malformed_token_signals() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "prompt_token_ids":["not","numbers"],
                "response_mask":[7000],
                "logprobs":"nope"
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let turn = &t["turns"][0];
        assert!(turn.get("prompt_token_ids").is_none());
        assert!(turn.get("response_mask").is_none());
        assert!(turn.get("logprobs").is_none());
    }

    #[test]
    fn build_trajectory_sets_contract_metadata() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(t["trajectory_id"], "trj-ss-1");
        assert_eq!(t["version"], "0.1.0");
        assert_eq!(t["metadata"]["trajectory_id"], "trj-ss-1");
        assert_eq!(t["metadata"]["harness"], "temperpaw");
        assert_eq!(t["metadata"]["spec_version"], "paw-agent@0.1.0");
        assert_eq!(t["metadata"]["framework"], "temperpaw");
        assert_eq!(t["metadata"]["agent_id"], "aj-1");
        assert_eq!(t["metadata"]["outcome"], "success");
        assert_eq!(t["metadata"]["domain"], "temperpaw-agent");
        assert_eq!(t["metadata"]["task_description"], "find the bug");
        assert_eq!(
            t["metadata"]["timestamp_start"], "2026-01-01T00:00:00.000Z",
            "the first entry's own timestamp beats the event hot tail"
        );
        assert_eq!(t["metadata"]["timestamp_end"], "2026-01-01T00:00:01Z");
        let tags = t["metadata"]["tags"].as_array().unwrap();
        assert!(tags.iter().any(|v| v == "claude-sonnet-4-6"));
        assert!(tags.iter().any(|v| v == "anthropic"));
        assert!(tags.iter().any(|v| v == "execute"));
        assert_eq!(t["_session_turn_count"], 2);
        assert_eq!(t["_token_usage"]["input_tokens"], 260);
        assert_eq!(t["_token_usage"]["output_tokens"], 53);
        assert_eq!(t["_token_usage"]["context_tokens"], 160);
    }

    #[test]
    fn build_trajectory_references_session_artifacts_as_resources() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let resources = t["context"]["resources"].as_array().unwrap();
        let kinds: Vec<&str> = resources
            .iter()
            .map(|r| r["type"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"session_tree"));
        assert!(kinds.contains(&"tool_spans"));
        assert_eq!(
            resources
                .iter()
                .find(|r| r["type"] == "tool_spans")
                .unwrap()["uri"],
            "temperfs://Files('file-spans-1')"
        );
        assert_eq!(
            resources
                .iter()
                .find(|r| r["type"] == "session_tree")
                .unwrap()["uri"],
            "temper://SessionEntries?SessionId=ss-1",
            "an entity-backed transcript must not be advertised as a TemperFS file"
        );
    }

    #[test]
    fn build_trajectory_reports_a_sealed_span_document_without_faking_a_decision() {
        let spans = concat!(
            "{\"tool_call_id\":\"tc-a\",\"tool_name\":\"read\",\"result\":\"ok\",\"duration_ms\":1,\"is_error\":false}\n",
            "{\"tool_call_id\":\"\",\"tool_name\":\"_tool_spans_truncated\",\"result\":\"tool span file size ceiling reached\",\"duration_ms\":0,\"is_error\":false}\n",
        );
        let fields = json!({ "user_message": "x", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", spans, &state, "Completed"));

        assert_eq!(t["_tool_spans_truncated"], true);
        let decisions = t["turns"][0]["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 1, "the marker must not become a decision");
        assert_eq!(decisions[0]["decision_id"], "tc-a");
    }

    #[test]
    fn build_trajectory_keeps_unclaimed_spans_as_decisions() {
        // Assistant body externalized: the tree cannot name the tool call, so
        // the span is the only evidence and must still become a decision.
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content_file_id":"file-xyz","tokens":900
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let spans = "{\"tool_call_id\":\"tc-orphan\",\"tool_name\":\"temper.read\",\"arguments\":\"{}\",\"result\":\"ok\",\"duration_ms\":4,\"is_error\":false}\n";
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let decisions = t["turns"][0]["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0]["decision_id"], "tc-orphan");
        assert_eq!(decisions[0]["cause_id"], "tc-orphan");
    }

    #[test]
    fn build_trajectory_without_session_tree_falls_back_to_spans() {
        let fields = json!({ "user_message": "x", "has_result": false });
        let spans = "{\"tool_call_id\":\"tc-a\",\"tool_name\":\"read\",\"arguments\":\"{}\",\"result\":\"ok\",\"duration_ms\":2,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", spans, &state, "Cancelled"));

        assert_eq!(t["metadata"]["outcome"], "partial_success");
        let turns = t["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["decisions"].as_array().unwrap().len(), 1);
        assert_eq!(turns[0]["decisions"][0]["decision_id"], "tc-a");
    }

    #[test]
    fn build_trajectory_empty_session_still_emits_one_turn() {
        let fields = json!({ "user_message": "", "has_result": false });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", "", &state, "Failed"));

        assert_eq!(t["metadata"]["outcome"], "failure");
        let turns = t["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["turn_id"], 1);
        assert_eq!(turns[0]["error"], true);
        assert!(turns[0]["decisions"].as_array().unwrap().is_empty());
        assert!(
            turns[0]["timestamp"].as_str().is_some_and(|s| !s.is_empty()),
            "turn timestamp must be non-empty"
        );
    }

    #[test]
    fn build_trajectory_includes_system_message_when_present() {
        let fields = json!({ "user_message": "", "system_prompt": "you are a helpful agent" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", "", &state, "Completed"));
        // OTSSystemMessage has { content, timestamp } only — no `role` field.
        assert_eq!(t["system_message"]["content"], "you are a helpful agent");
        assert!(
            t["system_message"]["timestamp"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "system_message.timestamp must be non-empty"
        );
        assert!(
            t["system_message"].get("role").is_none(),
            "role must be omitted"
        );
    }

    #[test]
    fn build_trajectory_skips_system_message_when_empty() {
        let fields = json!({ "user_message": "", "system_prompt": "" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", "", &state, "Completed"));
        assert!(t.get("system_message").is_none());
    }

    #[test]
    fn build_trajectory_missing_events_emits_epoch_fallback() {
        let fields = json!({ "user_message": "x" });
        let t = build_trajectory(&inputs(&fields, "", "", &json!({}), "Completed"));
        assert_eq!(t["metadata"]["timestamp_start"], "1970-01-01T00:00:00Z");
        assert_eq!(t["metadata"]["timestamp_end"], "1970-01-01T00:00:00Z");
    }

    #[test]
    fn build_trajectory_uses_event_log_when_entries_lack_timestamps() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let state = json!({
            "events": [
                { "action": "Created", "timestamp": "2026-03-01T00:00:00Z" },
                { "action": "RecordResult", "timestamp": "2026-03-01T00:00:09Z" },
            ]
        });
        let fields = json!({ "session_leaf_id": "a-1" });
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));
        assert_eq!(t["turns"][0]["timestamp"], "2026-03-01T00:00:09Z");
    }

    /// The wire contract with the kernel. If a field name or type drifts, the
    /// POST at `/api/ots/trajectories` stores a row that no consumer can read;
    /// this catches it at build time instead.
    #[test]
    fn build_trajectory_deserializes_as_a_kernel_ots_trajectory() {
        use temper_ots::models::{ContentType, DecisionType, MessageRole, OTSTrajectory, OutcomeType};

        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"arguments\":\"{}\",\"result\":\"src/lib.rs:12: TODO\",\"duration_ms\":137,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let document = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let trajectory: OTSTrajectory = serde_json::from_value(document)
            .expect("emitted document must deserialize as the kernel OTSTrajectory");

        assert_eq!(trajectory.trajectory_id, "trj-ss-1");
        assert_eq!(trajectory.version, OTS_VERSION);
        assert_eq!(trajectory.metadata.outcome, OutcomeType::Success);
        assert_eq!(trajectory.metadata.agent_id, "aj-1");
        assert_eq!(trajectory.metadata.framework.as_deref(), Some("temperpaw"));
        assert_eq!(trajectory.turns.len(), 2);

        let first = &trajectory.turns[0];
        assert_eq!(first.turn_id, 1);
        assert_eq!(first.messages.len(), 2);
        assert_eq!(first.messages[0].role, MessageRole::User);
        assert_eq!(first.messages[1].role, MessageRole::Assistant);
        assert_eq!(first.messages[1].content.content_type, ContentType::ToolCall);
        assert_eq!(
            first.messages[1].reasoning.as_deref(),
            Some("I should grep first")
        );
        assert_eq!(first.decisions.len(), 1);
        assert_eq!(first.decisions[0].decision_id, "tc-1");
        assert_eq!(first.decisions[0].decision_type, DecisionType::ToolSelection);
        assert_eq!(first.decisions[0].choice.action, "temper.bash");
        assert!(first.decisions[0].consequence.success);
        assert_eq!(first.duration_ms, Some(137.0));

        let second = &trajectory.turns[1];
        assert_eq!(second.turn_id, 2);
        assert_eq!(second.messages[0].role, MessageRole::Tool);
        assert_eq!(
            second.messages[0].content.content_type,
            ContentType::ToolResponse
        );

        // Session artifacts survive as context resources rather than payloads.
        assert!(
            trajectory
                .context
                .resources
                .iter()
                .any(|resource| resource.resource_type == "tool_spans"),
            "tool span file must be referenced in the trajectory context"
        );
    }

    /// Terminal states other than success have to survive the same round trip —
    /// a failed run is training signal, not a row the consumer can skip.
    #[test]
    fn failed_and_cancelled_trajectories_also_deserialize() {
        use temper_ots::models::{OTSTrajectory, OutcomeType};

        let fields = json!({ "user_message": "x", "has_result": false });
        let state = entity_state_with_events();
        for (status, expected) in [
            ("Failed", OutcomeType::Failure),
            ("Cancelled", OutcomeType::PartialSuccess),
        ] {
            let document = build_trajectory(&inputs(&fields, "", "", &state, status));
            let trajectory: OTSTrajectory = serde_json::from_value(document)
                .unwrap_or_else(|err| panic!("{status} document must deserialize: {err}"));
            assert_eq!(trajectory.metadata.outcome, expected);
            assert_eq!(trajectory.turns.len(), 1);
        }
    }

    /// Tool arguments are the largest field a decision can carry — a file-write
    /// call holds the whole file — so many of them must not inflate the document
    /// past the global ceiling.
    #[test]
    fn build_trajectory_counts_tool_arguments_against_the_global_budget() {
        let payload = "q".repeat(MAX_ARGUMENTS_CHARS);
        let mut lines: Vec<Value> = vec![json!({
            "id":"u-0","parentId":null,"type":"message","role":"user","content":"go"
        })];
        let mut parent = "u-0".to_string();
        for turn in 1..40 {
            let assistant = format!("a-{turn}");
            lines.push(json!({
                "id": assistant, "parentId": parent, "type": "message", "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": format!("tc-{turn}"),
                    "name": "temper.write",
                    "input": {"body": payload}
                }]
            }));
            let tool = format!("t-{turn}");
            lines.push(json!({
                "id": tool, "parentId": assistant, "type": "message", "role": "user",
                "content": [{"type":"tool_result","tool_use_id":format!("tc-{turn}"),"content":"ok"}]
            }));
            parent = tool;
        }
        let jsonl = lines
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let fields = json!({ "session_leaf_id": parent, "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let argument_chars: usize = t["turns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|turn| turn["decisions"].as_array().unwrap())
            .filter_map(|decision| decision["choice"].get("arguments"))
            .map(|arguments| {
                serde_json::to_string(arguments)
                    .unwrap_or_default()
                    .chars()
                    .count()
            })
            .sum();
        assert!(
            argument_chars <= MAX_TRAJECTORY_INLINE_CHARS * 2,
            "tool arguments escaped the inline budget: {argument_chars} chars"
        );
        assert_eq!(
            t["turns"][30]["decisions"][0]["choice"]["arguments"]["_truncated"],
            json!(true),
            "late decisions must degrade to a preview once the budget is spent"
        );
        assert!(
            t["turns"][0]["messages"][1]["content"]["data"]["tool_calls"][0]
                .get("arguments")
                .is_none(),
            "the message must not duplicate the decision's arguments"
        );
    }

    /// Providers that omit tool-call ids get synthetic ones that restart at
    /// every response, so two turns can both call `tool_1`. Indexing spans and
    /// observations across the whole document made the second call overwrite the
    /// first: both decisions then carried the second call's result and duration.
    #[test]
    fn build_trajectory_keeps_repeated_tool_call_ids_apart_per_turn() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"tool_use","id":"tool_1","name":"temper.read","input":{"path":"/first"}}]
            }),
            json!({
                "id":"t-1","parentId":"a-1","type":"message","role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tool_1","content":"first result","is_error":false}]
            }),
            json!({
                "id":"a-2","parentId":"t-1","type":"message","role":"assistant",
                "content":[{"type":"tool_use","id":"tool_1","name":"temper.bash","input":{"cmd":"second"}}]
            }),
            json!({
                "id":"t-2","parentId":"a-2","type":"message","role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tool_1","content":"second failed","is_error":true}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let spans = concat!(
            "{\"tool_call_id\":\"tool_1\",\"tool_name\":\"temper.read\",\"result\":\"first result\",\"duration_ms\":11,\"is_error\":false}\n",
            "{\"tool_call_id\":\"tool_1\",\"tool_name\":\"temper.bash\",\"result\":\"second failed\",\"duration_ms\":22,\"is_error\":true}\n",
        );
        let fields = json!({ "session_leaf_id": "t-2", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let first = &t["turns"][0]["decisions"][0];
        let second = &t["turns"][1]["decisions"][0];
        assert_eq!(first["choice"]["action"], "temper.read");
        assert_eq!(first["consequence"]["result_summary"], "first result");
        assert_eq!(first["consequence"]["success"], true);
        assert_eq!(first["_duration_ms"], 11);
        assert_eq!(second["choice"]["action"], "temper.bash");
        assert_eq!(second["consequence"]["result_summary"], "second failed");
        assert_eq!(second["consequence"]["success"], false);
        assert_eq!(second["_duration_ms"], 22);
        assert_eq!(t["turns"][0]["error"], false);
        assert_eq!(t["turns"][1]["error"], true);
        assert_eq!(
            t["turns"][0]["duration_ms"], 11.0,
            "a repeated id must not double-count one span onto two turns"
        );
    }

    /// Two spans sharing an id and no transcript to claim them are two calls,
    /// not one — collapsing them would erase a whole tool execution.
    #[test]
    fn build_trajectory_keeps_repeated_ids_among_unclaimed_spans() {
        let spans = concat!(
            "{\"tool_call_id\":\"tool_1\",\"tool_name\":\"read\",\"result\":\"a\",\"duration_ms\":1,\"is_error\":false}\n",
            "{\"tool_call_id\":\"tool_1\",\"tool_name\":\"bash\",\"result\":\"b\",\"duration_ms\":2,\"is_error\":false}\n",
        );
        let fields = json!({ "user_message": "x", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", spans, &state, "Completed"));

        let decisions = t["turns"][0]["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0]["choice"]["action"], "read");
        assert_eq!(decisions[1]["choice"]["action"], "bash");
    }

    /// A tool that reads or greps the emitter's own source returns the marker
    /// literal in its result. That must not make a complete run look partial.
    #[test]
    fn build_trajectory_does_not_read_a_quoted_truncation_marker_as_truncation() {
        let span = json!({
            "tool_call_id": "tc-a",
            "tool_name": "temper.read",
            "result": format!("pub const MARKER: &str = \"{TOOL_SPANS_TRUNCATED_MARKER}\";"),
            "duration_ms": 1,
            "is_error": false,
        });
        let spans = format!("{span}\n");
        let fields = json!({ "user_message": "x", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, "", &spans, &state, "Completed"));

        assert!(
            t.get("_tool_spans_truncated").is_none(),
            "truncation is a reserved tool_name, not a substring of any result"
        );
        assert_eq!(t["turns"][0]["decisions"].as_array().unwrap().len(), 1);
        assert!(!parse_tool_span_document(&spans).truncated);
    }

    /// Completion-side signals are positionally aligned with one another.
    /// Emitting them at different lengths would silently pair probabilities and
    /// mask bits with the wrong tokens.
    #[test]
    fn build_trajectory_drops_misaligned_completion_token_signals() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "prompt_token_ids":[1,2,3],
                "completion_token_ids":[4,5,6],
                "response_mask":[1,1,1],
                "logprobs":[-0.1,-0.2]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let turn = &t["turns"][0];
        assert_eq!(
            turn["prompt_token_ids"],
            json!([1, 2, 3]),
            "the prompt side does not index into the completion and still travels"
        );
        for field in ["completion_token_ids", "response_mask", "logprobs"] {
            assert!(
                turn.get(field).is_none(),
                "{field} must not ship in a misaligned set"
            );
        }
        assert_eq!(turn["_token_signals_misaligned"]["logprobs"], json!(2));
        assert_eq!(turn["_token_signals_misaligned"]["response_mask"], json!(3));
    }

    /// A completion-side signal with no `completion_token_ids` to index into is
    /// dropped, not carried. The kernel rejects that turn outright, and a
    /// rejected document is lost rather than degraded: the POST answers 400 and
    /// every retry rebuilds the identical document.
    ///
    /// This is reachable, not hypothetical — the SessionEntry writer bounds each
    /// signal on its own, and the ids are several times the size of the mask, so
    /// a long completion loses the ids and keeps the mask.
    #[test]
    fn build_trajectory_drops_completion_signals_with_no_anchor() {
        for (name, signal) in [
            ("logprobs", json!([-0.1, -0.2])),
            ("response_mask", json!([1, 1])),
        ] {
            let mut assistant = json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
            });
            assistant[name] = signal;
            let jsonl = [
                json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
                assistant,
            ]
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
            let fields = json!({ "session_leaf_id": "a-1" });
            let state = entity_state_with_events();
            let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

            assert!(
                t["turns"][0].get(name).is_none(),
                "{name} has nothing to align against and must not be written"
            );
            assert!(
                t["turns"][0]["_token_signals_dropped"][name].is_number(),
                "the drop must be recorded: {}",
                t["turns"][0]
            );
            assert!(
                degradations(&t).contains(&"token_signals_dropped".to_string()),
                "and must reach the Session status"
            );
        }
    }

    /// The anchor itself travels alone: it is what the others index into, so
    /// nothing is missing.
    #[test]
    fn build_trajectory_keeps_a_lone_completion_token_ids_signal() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "completion_token_ids":[7,8]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(t["turns"][0]["completion_token_ids"], json!([7, 8]));
        assert!(t["turns"][0].get("_token_signals_dropped").is_none());
    }

    /// `response_mask` is a loss switch: the kernel rejects a turn over a single
    /// entry above 1, so a mask carrying one is not written at all.
    #[test]
    fn build_trajectory_refuses_a_response_mask_that_is_not_binary() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "completion_token_ids":[7,8],
                "response_mask":[1,2]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert!(t["turns"][0].get("response_mask").is_none());
        assert_eq!(
            t["turns"][0]["completion_token_ids"],
            json!([7, 8]),
            "the anchor is still valid on its own"
        );
    }

    /// The whole point of the three rules above: whatever the emitter builds,
    /// the pinned kernel accepts it. A rejected document is not a partial write
    /// — it is a row that never lands and that no retry can change.
    #[test]
    fn every_asymmetric_signal_shape_still_deserializes_as_a_kernel_turn() {
        use temper_ots::models::OTSTrajectory;

        let shapes = [
            json!({"logprobs": [-0.1, -0.2]}),
            json!({"response_mask": [1, 1]}),
            json!({"response_mask": [1, 2], "completion_token_ids": [7, 8]}),
            json!({"logprobs": [-0.1], "completion_token_ids": [7, 8]}),
            json!({"response_mask": [1, 1], "logprobs": [-0.1, -0.2]}),
            json!({"completion_token_ids": [7, 8], "response_mask": [1, 1], "logprobs": [-0.1, -0.2]}),
            json!({"prompt_token_ids": [1, 2, 3], "response_mask": [1, 1]}),
        ];
        for shape in shapes {
            let mut assistant = json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
            });
            for (key, value) in shape.as_object().unwrap() {
                assistant[key] = value.clone();
            }
            let jsonl = [
                json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
                assistant,
            ]
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
            let fields = json!({ "session_leaf_id": "a-1" });
            let state = entity_state_with_events();
            let document = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

            serde_json::from_value::<OTSTrajectory>(document.clone()).unwrap_or_else(|err| {
                panic!(
                    "the kernel refuses a document this emitter built from {shape}: {err}\n\
                     the POST would answer 400 and every retry would rebuild it\n{document}"
                )
            });
        }
    }

    /// The decision-to-observation join must not depend on a field the kernel
    /// drops. `cause_id` mirrors `decision_id`, which the kernel does model, so
    /// a consumer working from re-serialized rows can still join.
    #[test]
    fn cause_id_mirrors_the_kernel_modeled_decision_id() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"result\":\"ok\",\"duration_ms\":3,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let mut checked = 0;
        for turn in t["turns"].as_array().unwrap() {
            for decision in turn["decisions"].as_array().unwrap() {
                assert_eq!(
                    decision["cause_id"], decision["decision_id"],
                    "the join key must stay a kernel-modeled field"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the fixture must contain decisions");
    }

    /// Session JSONL whose single assistant turn carries every token signal.
    fn token_signal_session_jsonl(tokens: usize) -> String {
        [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"done"}],
                "prompt_token_ids": vec![7_u64; tokens],
                "completion_token_ids": vec![3_u64; tokens],
                "response_mask": vec![1_u64; tokens],
                "logprobs": vec![-0.5_f64; tokens]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// Token signals are the one payload the character budgets do not bound, so
    /// a long session could otherwise carry many megabytes of them.
    #[test]
    fn build_trajectory_bounds_token_signals_across_the_document() {
        // Two turns of roughly a megabyte of signals each: the first fits, the
        // second cannot, and the drop has to be visible on both sides.
        let per_turn = MAX_TOKEN_SIGNAL_BYTES / 8;
        let mut lines: Vec<Value> = vec![
            json!({"id":"u-0","parentId":null,"type":"message","role":"user","content":"go"}),
        ];
        let mut parent = "u-0".to_string();
        for turn in 1..=2 {
            let assistant = format!("a-{turn}");
            lines.push(json!({
                "id": assistant, "parentId": parent, "type": "message", "role": "assistant",
                "content": [{"type":"text","text":"ok"}],
                "completion_token_ids": vec![1234_u64; per_turn],
                "response_mask": vec![1_u64; per_turn],
            }));
            parent = assistant;
        }
        let jsonl = lines
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let fields = json!({ "session_leaf_id": parent, "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let signal_bytes: usize = t["turns"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|turn| {
                ["prompt_token_ids", "completion_token_ids", "response_mask", "logprobs"]
                    .into_iter()
                    .filter_map(|field| turn.get(field))
                    .map(|value| value.to_string().len())
            })
            .sum();
        assert!(
            signal_bytes <= MAX_TOKEN_SIGNAL_BYTES,
            "token signals must stay under the trajectory ceiling, got {signal_bytes}"
        );

        let dropped = &t["turns"][1]["_token_signals_dropped"];
        assert_eq!(
            dropped["completion_token_ids"], per_turn as u64,
            "a dropped signal must name itself and its length: {dropped}"
        );
        assert_eq!(
            t["turns"][1]["_token_signals_dropped"]["response_mask"], per_turn as u64,
            "every refused signal on the turn must name itself"
        );
    }

    /// Misaligned completion signals are discarded whole — the same loss a
    /// budget drop is — so the row is degraded, not merely annotated.
    #[test]
    fn build_trajectory_marks_misaligned_signals_as_degraded() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"ok"}],
                "completion_token_ids":[7, 8, 9],
                "response_mask":[1, 1],
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert!(t["turns"][0]["_token_signals_misaligned"].is_object());
        assert!(
            t["turns"][0].get("completion_token_ids").is_none(),
            "the misaligned set is discarded whole"
        );
        assert!(
            degradations(&t).contains(&"token_signals_dropped".to_string()),
            "the discard must reach the Session status, not only the document: {:?}",
            t["metadata"]["tags"]
        );
    }

    /// The tag says a consumer can read token signals off this row. A turn whose
    /// signals were all dropped carries a record of the drop and no signal, so
    /// tagging it would send a consumer looking for data that is not there.
    #[test]
    fn build_trajectory_does_not_advertise_signals_it_dropped() {
        let oversized = MAX_TOKEN_SIGNAL_BYTES;
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"ok"}],
                "completion_token_ids": vec![123456_u64; oversized],
                "response_mask": vec![1_u64; oversized],
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert!(
            t["turns"][0].get("completion_token_ids").is_none(),
            "the signal must not have been written"
        );
        assert!(
            t["turns"][0]["_token_signals_dropped"].is_object(),
            "the drop itself still has to be recorded on the turn"
        );
        assert!(
            degradations(&t).contains(&"token_signals_dropped".to_string()),
            "a row built without signals it was offered is degraded: {:?}",
            t["metadata"]["tags"]
        );
    }

    /// When the entry's extras were themselves cut to fit the field ceiling,
    /// which members went is no longer knowable — only how many. The turn's
    /// record is short either way, and the row must say so.
    #[test]
    fn build_trajectory_marks_turn_extras_cut_to_fit_as_degraded() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"ok"}],
                "_extra_json_dropped_members": 12
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(t["turns"][0]["_turn_extras_dropped_members"], 12);
        assert!(
            degradations(&t).contains(&"token_signals_dropped".to_string()),
            "extras cut to fit take turn facts with them, so the row is short: {:?}",
            t["metadata"]["tags"]
        );
    }

    /// The SessionEntry writer refuses signals that would push the entry over
    /// its own ceiling and leaves `<signal>_dropped_bytes` behind. Without
    /// carrying that forward, a turn whose signals were all refused at capture
    /// looks exactly like a provider that never sent any.
    #[test]
    fn build_trajectory_carries_capture_stage_signal_drops() {
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"ok"}],
                "completion_token_ids_dropped_bytes": 40_000,
                "logprobs_dropped_bytes": 52_000
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(
            t["turns"][0]["_token_signals_dropped"]["completion_token_ids"],
            40_000
        );
        assert_eq!(
            t["turns"][0]["_token_signals_dropped"]["logprobs"], 52_000,
            "a refusal at capture must still be legible on the turn"
        );
        assert!(
            degradations(&t).contains(&"token_signals_dropped".to_string()),
            "and must reach the Session status: {:?}",
            t["metadata"]["tags"]
        );
    }

    /// The JCS contract fields ride natively now: the pinned `temper-ots`
    /// models every one, so a consumer that deserializes a stored row and
    /// writes it back keeps them, and the carriers that stood in for them are
    /// gone.
    ///
    /// This is what those carriers were removed against, so it fails if either
    /// half comes back. Each field must be emitted, survive the round trip with
    /// its value intact, and be mirrored nowhere: no `harness:` /
    /// `spec_version:` / `token_signals:present` tag, no `context.entities`
    /// inventory.
    #[test]
    fn kernel_round_trip_keeps_the_jcs_contract_fields() {
        use temper_ots::models::OTSTrajectory;

        let fields = two_turn_fields();
        let state = entity_state_with_events();
        let jsonl = token_signal_session_jsonl(2);
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.read\",\"result\":\"ok\",\"duration_ms\":1,\"is_error\":false}\n";
        let emitted = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        let trajectory: OTSTrajectory =
            serde_json::from_value(emitted.clone()).expect("document deserializes");
        let round_tripped = serde_json::to_value(&trajectory).expect("re-serializes");

        let contract_fields: [(&str, &Value, &Value); 7] = [
            (
                "metadata.harness",
                &emitted["metadata"]["harness"],
                &round_tripped["metadata"]["harness"],
            ),
            (
                "metadata.spec_version",
                &emitted["metadata"]["spec_version"],
                &round_tripped["metadata"]["spec_version"],
            ),
            (
                "turns[].prompt_token_ids",
                &emitted["turns"][0]["prompt_token_ids"],
                &round_tripped["turns"][0]["prompt_token_ids"],
            ),
            (
                "turns[].completion_token_ids",
                &emitted["turns"][0]["completion_token_ids"],
                &round_tripped["turns"][0]["completion_token_ids"],
            ),
            (
                "turns[].response_mask",
                &emitted["turns"][0]["response_mask"],
                &round_tripped["turns"][0]["response_mask"],
            ),
            (
                "turns[].logprobs",
                &emitted["turns"][0]["logprobs"],
                &round_tripped["turns"][0]["logprobs"],
            ),
            (
                "turns[].decisions[].cause_id",
                &emitted["turns"][0]["decisions"][0]["cause_id"],
                &round_tripped["turns"][0]["decisions"][0]["cause_id"],
            ),
        ];
        for (path, before, after) in contract_fields {
            assert!(
                !before.is_null(),
                "the emitter stopped producing {path}; this test only means \
                 something while every contract field is emitted"
            );
            assert_eq!(
                before, after,
                "{path} did not survive the kernel round trip. Either the pin \
                 rolled back to a revision that does not model it, or the field \
                 drifted — do not answer that by reintroducing a carrier"
            );
        }

        // Typed access, not just JSON shape: the fields are on the structs.
        assert_eq!(trajectory.metadata.harness.as_deref(), Some(HARNESS));
        assert_eq!(
            trajectory.metadata.spec_version.as_deref(),
            Some("paw-agent@0.1.0")
        );
        assert_eq!(
            trajectory.turns[0].decisions[0].cause_id.as_deref(),
            Some("tc-1")
        );

        // The carriers are gone and must stay gone.
        for tag in trajectory.metadata.tags.iter() {
            assert!(
                !tag.starts_with("harness:")
                    && !tag.starts_with("spec_version:")
                    && !tag.starts_with("token_signals:"),
                "a provenance mirror came back as {tag:?}; the kernel models \
                 these directly now"
            );
        }
        assert!(
            round_tripped["context"].get("entities").is_none(),
            "the turn_token_signals inventory came back: {}",
            round_tripped["context"]
        );
    }

    /// Serde ignores unknown fields, so a round trip says nothing about a field
    /// the kernel does not model. One is left by design — the POST handler reads
    /// `metadata.trajectory_id` before any struct is involved — and this pins
    /// that it is the only one, so a new extension cannot appear unnoticed.
    #[test]
    fn kernel_round_trip_drops_exactly_the_unmodeled_extensions() {
        use temper_ots::models::OTSTrajectory;

        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"tool_use","id":"tc-1","name":"temper.read","input":{}}],
                "prompt_token_ids":[1,2],
                "completion_token_ids":[3,4],
                "response_mask":[1,1],
                "logprobs":[-0.1,-0.2]
            }),
            json!({
                "id":"t-2","parentId":"a-1","type":"message","role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tc-1","content":"ok"}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "t-2", "has_result": true });
        let state = entity_state_with_events();
        let emitted = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let trajectory: OTSTrajectory =
            serde_json::from_value(emitted.clone()).expect("document deserializes");
        let round_tripped = serde_json::to_value(&trajectory).expect("re-serializes");

        let mut dropped: Vec<String> = Vec::new();
        collect_dropped_paths(&emitted, &round_tripped, "", &mut dropped);
        let dropped: BTreeSet<String> = dropped
            .into_iter()
            // The `_`-prefixed fields are declared non-standard by ADR-0035 and
            // are expected to be kernel-invisible; the ones without the prefix
            // read like OTS fields and are the ones worth pinning.
            .filter(|path| !path.split('.').any(|segment| segment.starts_with('_')))
            .collect();
        let expected: BTreeSet<String> = KERNEL_UNMODELED_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect();

        assert_eq!(
            dropped, expected,
            "the set of emitter fields the pinned kernel drops has changed; \
             update KERNEL_UNMODELED_FIELDS and ADR-0035 deliberately"
        );
    }

    /// Rows written before these fields existed must keep deserializing — the
    /// additions are additive, and an evaluation worker reads old rows too.
    #[test]
    fn rows_without_the_new_fields_still_deserialize() {
        use temper_ots::models::{OTSTrajectory, OutcomeType};

        let old_row = json!({
            "trajectory_id": "trj-old",
            "version": "0.1.0",
            "metadata": {
                "trajectory_id": "trj-old",
                "task_description": "an older run",
                "domain": "temperpaw-agent",
                "timestamp_start": "2026-01-01T00:00:00Z",
                "timestamp_end": "2026-01-01T00:00:05Z",
                "agent_id": "aj-old",
                "framework": "temperpaw",
                "environment": "production",
                "outcome": "success",
                "tags": ["claude-sonnet-4-6"],
            },
            "turns": [{
                "turn_id": 1,
                "span_id": "ss-old:a-1",
                "timestamp": "2026-01-01T00:00:01Z",
                "error": false,
                "messages": [],
                "decisions": [{
                    "decision_id": "tc-old",
                    "decision_type": "tool_selection",
                    "choice": { "action": "temper.read" },
                    "consequence": { "success": true, "result_summary": "ok" },
                }],
            }],
        });

        let trajectory: OTSTrajectory = serde_json::from_value(old_row)
            .expect("a pre-ARN-109 row must still deserialize as an OTSTrajectory");
        assert_eq!(trajectory.metadata.outcome, OutcomeType::Success);
        assert_eq!(trajectory.turns[0].decisions[0].decision_id, "tc-old");
        assert_eq!(trajectory.metadata.tags, vec!["claude-sonnet-4-6"]);
    }

    fn is_empty_collection(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.is_empty(),
            Value::Object(fields) => fields.is_empty(),
            Value::Null => true,
            _ => false,
        }
    }

    /// Record every leaf path present in `emitted` but absent from
    /// `round_tripped`, using the field paths `KERNEL_UNMODELED_FIELDS` names.
    fn collect_dropped_paths(
        emitted: &Value,
        round_tripped: &Value,
        path: &str,
        dropped: &mut Vec<String>,
    ) {
        match emitted {
            Value::Object(fields) => {
                for (key, value) in fields {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    match round_tripped.get(key) {
                        Some(other) => {
                            collect_dropped_paths(value, other, &child_path, dropped)
                        }
                        // An empty collection the kernel omits on write carries
                        // no data, so its absence is formatting, not loss.
                        None if is_empty_collection(value) => {}
                        None => dropped.push(child_path),
                    }
                }
            }
            Value::Array(items) => {
                let others = round_tripped.as_array();
                for (index, item) in items.iter().enumerate() {
                    let Some(other) = others.and_then(|others| others.get(index)) else {
                        continue;
                    };
                    collect_dropped_paths(item, other, &format!("{path}[]"), dropped);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn build_trajectory_bounds_oversized_tool_arguments() {
        let big_argument = "z".repeat(MAX_ARGUMENTS_CHARS * 2);
        let jsonl = [
            json!({"id":"u-1","parentId":null,"type":"message","role":"user","content":"go"}),
            json!({
                "id":"a-1","parentId":"u-1","type":"message","role":"assistant",
                "content":[{"type":"tool_use","id":"tc-big","name":"temper.write","input":{"body":big_argument}}]
            }),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "session_leaf_id": "a-1" });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        let arguments = &t["turns"][0]["decisions"][0]["choice"]["arguments"];
        assert_eq!(arguments["_truncated"], true);
        assert!(
            arguments["_preview"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= MAX_ARGUMENTS_CHARS
        );
    }

    /// A transcript that is not there produces a spans-only document. Storing it
    /// as if it were complete is the failure this marks: the row is written once
    /// and the session is marked emitted, so nothing downstream ever revisits it.
    #[test]
    fn build_trajectory_marks_an_absent_transcript_as_degraded() {
        let fields = json!({ "has_result": true, "tool_spans_file_id": "file-spans-1" });
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"result\":\"ok\",\"duration_ms\":3,\"is_error\":false}\n";
        let state = entity_state_with_events();

        for (presence, expected) in [
            (TranscriptPresence::MissingFile, "transcript_missing_file"),
            (TranscriptPresence::EmptyFile, "transcript_empty_file"),
            (
                TranscriptPresence::PendingFirstTurn,
                "transcript_pending_first_turn",
            ),
            (TranscriptPresence::NoEntries, "transcript_no_entries"),
            (TranscriptPresence::Undeclared, "transcript_undeclared"),
        ] {
            let mut input = inputs(&fields, "", spans, &state, "Completed");
            input.transcript = presence;
            let t = build_trajectory(&input);

            assert_eq!(
                degradations(&t),
                vec![expected.to_string()],
                "a {} transcript must be reported as degraded",
                presence.as_str()
            );
            assert_eq!(t["_transcript"]["present"], false);
            assert_eq!(t["_transcript"]["reasons"], json!([presence.as_str()]));
        }
    }

    /// A transcript that arrived but does not parse is missing history just as
    /// surely as one that never arrived. Judging completeness by whether bytes
    /// showed up would store a corrupted file as a complete record — the same
    /// false-complete row an absent transcript used to produce, reached through
    /// corruption instead of a 404.
    #[test]
    fn build_trajectory_marks_an_unparseable_transcript_as_degraded() {
        let mut lines: Vec<String> = two_turn_session_jsonl()
            .lines()
            .map(str::to_string)
            .collect();
        lines.push("{\"id\":\"a-4\",\"parentId\":\"a-3\",\"type\"".to_string()); // write cut mid-line
        let jsonl = lines.join("\n");
        let fields = two_turn_fields();
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(degradations(&t), vec!["transcript_unparseable".to_string()]);
        assert_eq!(t["_transcript"]["present"], true);
        assert_eq!(t["_transcript"]["unparsed_lines"], 1);
        assert_eq!(
            t["turns"].as_array().unwrap().len(),
            2,
            "the readable turns are still kept — one bad line must not cost the trajectory"
        );
    }

    /// The recorded leaf is the session's own claim about where its history
    /// ends. When it does not resolve, the fallback chain is an older one — the
    /// newest turns are exactly what is missing — and the row must not pass as
    /// the whole session.
    #[test]
    fn build_trajectory_marks_an_unresolved_leaf_as_degraded() {
        let mut fields = two_turn_fields();
        fields["session_leaf_id"] = json!("a-5-never-written");
        let jsonl = two_turn_session_jsonl();
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(
            degradations(&t),
            vec!["transcript_leaf_unresolved".to_string()]
        );
        assert_eq!(t["_transcript"]["present"], true);
        assert_eq!(
            t["turns"].as_array().unwrap().len(),
            2,
            "the recoverable turns are still emitted"
        );
    }

    /// A transcript whose entries yield no turn at all produces the same
    /// synthetic single-turn document as an empty one, and must be labelled the
    /// same way rather than passing as a session that genuinely did nothing.
    #[test]
    fn build_trajectory_marks_a_transcript_without_turns_as_degraded() {
        let jsonl = json!({"id":"h-ss-1","parentId":null,"type":"header","tokens":0}).to_string();
        let fields = json!({ "session_leaf_id": "h-ss-1", "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert!(
            degradations(&t).contains(&"transcript_no_turns".to_string()),
            "tags: {:?}",
            t["metadata"]["tags"]
        );
        assert_eq!(t["turns"].as_array().unwrap().len(), 1);
    }

    /// Span lines are skipped when they do not parse, so a partially written
    /// append leaves tool calls with no evidence and nothing saying so.
    #[test]
    fn build_trajectory_marks_unparseable_tool_spans_as_degraded() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = concat!(
            "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"result\":\"ok\",\"duration_ms\":3,\"is_error\":false}\n",
            "{\"tool_call_id\":\"tc-2\",\"tool_name\":\"temper.re\n"
        );
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        assert_eq!(
            degradations(&t),
            vec!["tool_spans_unparseable".to_string()]
        );
        assert_eq!(t["_tool_spans_unparsed_lines"], 1);
    }

    /// A complete run must not be labelled degraded — the marker is only useful
    /// if it means something.
    #[test]
    fn build_trajectory_reports_no_degradation_for_a_complete_run() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let spans = "{\"tool_call_id\":\"tc-1\",\"tool_name\":\"temper.bash\",\"result\":\"ok\",\"duration_ms\":3,\"is_error\":false}\n";
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, spans, &state, "Completed"));

        assert!(degradations(&t).is_empty(), "tags: {:?}", t["metadata"]["tags"]);
        assert!(t.get("_transcript").is_none());
        assert!(t.get("_tool_spans_missing").is_none());
    }

    /// A declared span file that 404s is missing evidence, not an absence of
    /// tool calls, and a truncated span document is missing tool timings.
    #[test]
    fn build_trajectory_marks_missing_and_truncated_tool_spans() {
        let fields = two_turn_fields();
        let jsonl = two_turn_session_jsonl();
        let state = entity_state_with_events();

        let mut input = inputs(&fields, &jsonl, "", &state, "Completed");
        input.tool_spans_missing = true;
        let t = build_trajectory(&input);
        assert_eq!(degradations(&t), vec!["tool_spans_missing_file".to_string()]);
        assert_eq!(t["_tool_spans_missing"], true);

        let sealed = format!(
            "{}\n",
            json!({"tool_name": TOOL_SPANS_TRUNCATED_MARKER, "tool_call_id":"", "result":"", "duration_ms":0, "is_error":false})
        );
        let t = build_trajectory(&inputs(&fields, &jsonl, &sealed, &state, "Completed"));
        assert_eq!(t["_tool_spans_truncated"], true);
        assert_eq!(degradations(&t), vec!["tool_spans_truncated".to_string()]);
    }

    /// A span write that failed leaves `tool_spans_file_id` empty, which reads
    /// exactly like a session that called no tools. The session records the
    /// failure so the two can be told apart.
    #[test]
    fn build_trajectory_marks_a_failed_span_write_as_degraded() {
        let mut fields = two_turn_fields();
        fields["tool_spans_file_id"] = json!("");
        fields["tool_spans_write_failed"] = json!("true");
        let jsonl = two_turn_session_jsonl();
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert_eq!(
            degradations(&t),
            vec!["tool_spans_write_failed".to_string()]
        );
        assert_eq!(t["_tool_spans_write_failed"], true);
    }

    /// Raw file order is not a walk. When no leaf resolves — recorded or not —
    /// the parent structure is unusable and the order is a guess, so the
    /// document must not present it as the session's shape.
    #[test]
    fn build_trajectory_marks_raw_order_fallback_as_degraded() {
        // Every entry points at a parent that is not in the transcript, so no
        // leaf walks, and no leaf was recorded either.
        let jsonl = [
            json!({"id":"u-1","parentId":"missing-a","type":"message","role":"user","content":"go"}),
            json!({"id":"a-1","parentId":"missing-b","type":"message","role":"assistant","content":"ok"}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let fields = json!({ "has_result": true });
        let state = entity_state_with_events();
        let t = build_trajectory(&inputs(&fields, &jsonl, "", &state, "Completed"));

        assert!(
            degradations(&t).contains(&"transcript_leaf_unresolved".to_string()),
            "tags: {:?}",
            t["metadata"]["tags"]
        );

        let resolved = resolve_chain(&parse_session_entries(&jsonl), "");
        assert!(!resolved.from_recorded_leaf);
    }

    /// The completeness marker is the last thing that may be lost on a round
    /// trip: without it a partial record reads as a whole one.
    #[test]
    fn degradation_markers_survive_the_kernel_round_trip() {
        use temper_ots::models::OTSTrajectory;

        let fields = json!({ "has_result": false });
        let state = entity_state_with_events();
        let mut input = inputs(&fields, "", "", &state, "Failed");
        input.transcript = TranscriptPresence::MissingFile;
        input.tool_spans_missing = true;
        let document = build_trajectory(&input);

        let trajectory: OTSTrajectory =
            serde_json::from_value(document).expect("document deserializes");
        let round_tripped = serde_json::to_value(&trajectory).expect("re-serializes");

        assert_eq!(
            degradations(&round_tripped),
            vec![
                "transcript_missing_file".to_string(),
                "tool_spans_missing_file".to_string()
            ],
            "tags: {:?}",
            trajectory.metadata.tags
        );
    }
}
