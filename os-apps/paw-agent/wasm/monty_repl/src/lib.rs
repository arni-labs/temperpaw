//! Monty REPL — WASM module embedding Pydantic's Monty Python sandbox.
//!
//! Replaces tool_runner with a true persistent REPL. Agents write Python
//! code using `temper.*` and `sandbox.*` objects; Monty interprets it and
//! dispatches method calls to the Temper API or sandbox via host functions.
//!
//! REPL state (heap, globals, intern table) persists across LLM turns via
//! Monty's dump()/load() serialization, stored in a `repl_file_id` TemperFS
//! file for the session.
//!
//! Build: `cargo build --target wasm32-wasip1 --release`

// getrandom 0.3+ requires an explicit backend for wasm32-unknown-unknown.
// ahash (via monty/jiter) uses it for hash randomisation — not crypto.
// We provide a simple LCG-based fallback so hash seeds are non-deterministic
// within a single WASM invocation.
use std::sync::atomic::{AtomicU64, Ordering};

static SEED_CTR: AtomicU64 = AtomicU64::new(0x5EED_C0DE_CAFE_F00D);

#[unsafe(no_mangle)]
unsafe extern "C" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> u32 {
    let buf = unsafe { core::slice::from_raw_parts_mut(dest, len) };
    let mut state = SEED_CTR.fetch_add(1, Ordering::Relaxed);
    for chunk in buf.chunks_mut(8) {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bytes = state.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    0 // success
}

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use temper_wasm_sdk::prelude::*;

mod convert;
mod datadog;
mod dispatch;
mod entity_ops;
mod railway;
mod session;
mod vercel;

use monty::{
    DictPairs, ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl,
    PrintWriter, PrintWriterCallback, ReplProgress, ResourceLimits,
};

const MAX_TOOL_RESULT_BYTES: usize = 16 * 1024;
// 512 KB default. Empirically the Monty dump after a typical katagami-style
// turn (specs lookup + a handful of entity reads) is ~180 KB, so 512 KB gives
// comfortable headroom. Persistence is not optional — without it, every
// `temper.execute` invocation re-creates the REPL from scratch and the agent
// loses every variable, import, and intermediate result, which turns a 5-turn
// task into a 50+ turn rediscovery loop.
const DEFAULT_NORMAL_REPL_STATE_MAX_BYTES: usize = 512 * 1024;

/// Captures `print()` output without allowing Monty to grow an unbounded host-side buffer.
///
/// The original implementation used `PrintWriter::Collect(String)` and only truncated after
/// execution completed. Large or pathological output therefore forced repeated reallocations
/// inside the daemon and could balloon RSS before we ever returned a tool result.
struct BoundedOutputCollector {
    buf: String,
    max_bytes: usize,
    total_bytes_seen: usize,
    truncated: bool,
}

impl BoundedOutputCollector {
    fn new(max_bytes: usize) -> Self {
        Self {
            buf: String::with_capacity(max_bytes.min(1024)),
            max_bytes,
            total_bytes_seen: 0,
            truncated: false,
        }
    }

    fn append_str(&mut self, output: &str) {
        self.total_bytes_seen += output.len();
        if self.buf.len() >= self.max_bytes {
            self.truncated = true;
            return;
        }

        let remaining = self.max_bytes - self.buf.len();
        if output.len() <= remaining {
            self.buf.push_str(output);
            return;
        }

        self.truncated = true;
        self.buf
            .push_str(prefix_at_char_boundary(output, remaining));
    }

    fn append_char(&mut self, ch: char) {
        self.total_bytes_seen += ch.len_utf8();
        if self.buf.len() >= self.max_bytes {
            self.truncated = true;
            return;
        }

        if self.buf.len() + ch.len_utf8() <= self.max_bytes {
            self.buf.push(ch);
        } else {
            self.truncated = true;
        }
    }

    fn into_string(self) -> String {
        if !self.truncated {
            return self.buf;
        }

        let captured_bytes = self.buf.len();
        let total_bytes_seen = self.total_bytes_seen;
        let buf = self.buf;
        format!(
            "{}...\n[print output truncated, captured {} of {} bytes]",
            buf, captured_bytes, total_bytes_seen,
        )
    }
}

impl PrintWriterCallback for BoundedOutputCollector {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.append_str(&output);
        Ok(())
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.append_char(end);
        Ok(())
    }
}

/// Thread-local flag set by `dispatch_success` / `dispatch_error` whenever
/// this invocation has handed the Session a follow-up action to run. Read
/// by `run()`'s outer match to enforce the "every WASM exit dispatches an
/// action on the Session" invariant (temperpaw ADR-0039 Sub-Decision 3a).
///
/// Reset at the top of each `run()` call so re-used WASM instances don't
/// carry state between invocations.
thread_local! {
    static ACTION_DISPATCHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn dispatch_success(action: &str, params: &Value) {
    ACTION_DISPATCHED.with(|flag| flag.set(true));
    set_success_result(action, params);
}

fn dispatch_error(error: &str) {
    ACTION_DISPATCHED.with(|flag| flag.set(true));
    set_error_result(error);
}

/// Classifies the outcome of `run()`'s body closure into a WASM exit code,
/// flagging the invariant-violation case ("closure returned Ok but no
/// Session action was dispatched") as an error so the integration's
/// `on_failure = "Fail"` hook fires instead of leaving the Session stuck
/// in its intermediate state.
///
/// Pure helper, unit-testable. The actual FFI `dispatch_error` call for
/// the violation path is wired in `run()` after consulting this function.
fn classify_run_outcome(
    closure_result: &Result<(), String>,
    action_dispatched: bool,
) -> RunOutcome {
    match (closure_result, action_dispatched) {
        (Ok(()), true) => RunOutcome::Success,
        (Ok(()), false) => RunOutcome::InvariantViolation,
        (Err(_), _) => RunOutcome::PropagateError,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RunOutcome {
    /// Closure returned Ok AND at least one action was dispatched.
    Success,
    /// Closure returned Ok with no action dispatched — structural
    /// invariant violation. Caller must dispatch a synthesized
    /// `dispatch_error` so `on_failure="Fail"` fires.
    InvariantViolation,
    /// Closure returned Err; caller dispatches `dispatch_error(&msg)`.
    PropagateError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolProgressBoundary {
    Start,
    End,
}

fn run_with_tool_progress<T>(
    mut emit_progress: impl FnMut(ToolProgressBoundary),
    run_tool: impl FnOnce() -> T,
) -> T {
    emit_progress(ToolProgressBoundary::Start);
    let result = run_tool();
    emit_progress(ToolProgressBoundary::End);
    result
}

fn start_tool_guest_span(
    ctx: &Context,
    tool_name: &str,
    tool_call_id: &str,
    tool_arguments_json: &str,
) -> Option<WasmSpan> {
    let attrs = json!({
        "tool.operation": "execute",
        "tool.name": tool_name,
        "tool.call_id": tool_call_id,
        "tool.arguments": tool_arguments_json,
    });
    match ctx.start_span(&format!("tool.{tool_name}"), &attrs) {
        Ok(span) => Some(span),
        Err(err) => {
            ctx.log(
                "warn",
                &format!("monty_repl: failed to start tool guest span tool_name={tool_name} tool_call_id={tool_call_id}: {err}"),
            );
            None
        }
    }
}

fn finish_tool_guest_span(
    ctx: &Context,
    tool_name: &str,
    span: &mut Option<WasmSpan>,
    result: &Result<Value, String>,
    duration_ms: u64,
) {
    if let Some(span) = span.take() {
        let _ = ctx.emit_metric(
            "temperpaw.wasm_tool_call.duration_ms",
            duration_ms as f64,
            &json!({
                "tool.name": tool_name,
                "tool.success": if result.is_ok() { "true" } else { "false" },
            }),
            Some("histogram"),
        );
        let event_attrs = match result {
            Ok(value) => json!({
                "tool.duration_ms": duration_ms,
                "tool.result.is_error": false,
                "tool.result.preview": prefix_at_char_boundary(&value.to_string(), 1024),
            }),
            Err(error) => json!({
                "tool.duration_ms": duration_ms,
                "tool.result.is_error": true,
                "error.message": prefix_at_char_boundary(error, 1024),
            }),
        };
        let final_attrs = match result {
            Ok(_) => json!({
                "tool.duration_ms": duration_ms,
                "tool.result.is_error": false,
            }),
            Err(error) => json!({
                "tool.duration_ms": duration_ms,
                "tool.result.is_error": true,
                "error.message": prefix_at_char_boundary(error, 1024),
            }),
        };
        let _ = span.add_event("tool.result", &event_attrs);
        let _ = span.set_attributes(&final_attrs);
        match result {
            Ok(_) => {
                let _ = span.end_ok(&json!({}));
            }
            Err(error) => {
                let _ = span.end_error("tool_error", error, &json!({}));
            }
        }
    }
}

fn batch_window_len(start_index: usize, total_calls: usize, checkpoint_every_n: usize) -> usize {
    let remaining = total_calls.saturating_sub(start_index);
    let next_boundary = ((start_index / checkpoint_every_n) + 1) * checkpoint_every_n;
    let until_boundary = if next_boundary < total_calls {
        next_boundary.saturating_sub(start_index)
    } else {
        remaining
    };
    remaining.min(until_boundary.max(1))
}

fn batchable_run_len(tool_calls: &[Value], start_index: usize, max_batch_len: usize) -> usize {
    let upper_bound = tool_calls
        .len()
        .min(start_index.saturating_add(max_batch_len));
    let mut run_len = 0usize;
    for call in &tool_calls[start_index..upper_bound] {
        let code = call
            .get("input")
            .and_then(|input| input.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if dispatch::batchable_tool_plan_from_code(code).is_none() {
            break;
        }
        run_len += 1;
    }
    run_len
}

fn collect_batchable_tool_calls(
    tool_calls: &[Value],
    start_index: usize,
    run_len: usize,
) -> Vec<dispatch::BatchableToolCall> {
    tool_calls[start_index..start_index + run_len]
        .iter()
        .filter_map(|call| {
            let tool_call_id = call.get("id").and_then(Value::as_str)?;
            let code = call
                .get("input")
                .and_then(|input| input.get("code"))
                .and_then(Value::as_str)?;
            let plan = dispatch::batchable_tool_plan_from_code(code)?;
            Some(dispatch::BatchableToolCall {
                tool_call_id: tool_call_id.to_string(),
                plan,
            })
        })
        .collect()
}

fn push_batch_tool_result(
    ctx: &Context,
    tool_results: &mut Vec<Value>,
    tool_id: &str,
    result: &Result<Value, String>,
) {
    match result {
        Ok(value) => {
            let expr_val = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
            let content = if expr_val == "null" || expr_val.is_empty() {
                "(no output)".to_string()
            } else {
                truncate_output(&expr_val)
            };
            ctx.log(
                "info",
                &format!(
                    "monty_repl: batched tool completed {tool_id}, expr_bytes={}, result_bytes={}, is_error=false",
                    expr_val.len(),
                    content.len()
                ),
            );
            tool_results.push(make_tool_result(tool_id, &content, false));
        }
        Err(error) => {
            let content = truncate_output(error);
            ctx.log(
                "info",
                &format!(
                    "monty_repl: batched tool completed {tool_id}, error_bytes={}, result_bytes={}, is_error=true",
                    error.len(),
                    content.len()
                ),
            );
            tool_results.push(make_tool_result(tool_id, &content, true));
        }
    }
}

const INVARIANT_VIOLATION_MSG: &str = "monty_repl exited without dispatching any Session action — invariant violation (ADR-0039 Sub-Decision 3a)";

/// Entry point — invoked by the Temper WASM engine on the `run_tools` trigger.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    // Reset the invariant flag. Must come before anything that could call
    // dispatch_success / dispatch_error so a prior invocation's state
    // doesn't leak into this one.
    ACTION_DISPATCHED.with(|flag| flag.set(false));
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        ctx.log("info", "monty_repl: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let temper_api_url = ctx
            .config
            .get("temper_api_url")
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        let tenant = &ctx.tenant;
        let mut sandbox_url = fields
            .get("sandbox_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let workdir = fields
            .get("workdir")
            .and_then(|v| v.as_str())
            .unwrap_or("/workspace");

        // Cedar resume mode: if we're resuming after a Cedar approval, the
        // remaining tool calls and previously-completed results are stored in
        // the entity's `pending_tool_context` field (set during PauseForApproval).
        let pending_ctx_str = fields
            .get("pending_tool_context")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let is_cedar_resume = pending_ctx_str.is_some();
        let (tool_calls, mut prior_results): (Vec<Value>, Vec<Value>) =
            if let Some(ctx_json) = pending_ctx_str {
                ctx.log("info", "monty_repl: resuming from Cedar approval");
                let pause_ctx: Value = serde_json::from_str(ctx_json)
                    .map_err(|e| format!("failed to parse pending_tool_context: {e}"))?;
                let remaining = pause_ctx
                    .get("remaining_tool_calls")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let completed = pause_ctx
                    .get("completed_results")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                (remaining, completed)
            } else {
                // Normal mode: read pending tool calls from trigger params
                let tool_calls_json = ctx
                    .trigger_params
                    .get("pending_tool_calls")
                    .and_then(|v| v.as_str())
                    .unwrap_or("[]");
                let calls: Vec<Value> = serde_json::from_str(tool_calls_json)
                    .map_err(|e| format!("failed to parse pending_tool_calls: {e}"))?;
                (calls, Vec::new())
            };

        ctx.log(
            "info",
            &format!(
                "monty_repl: executing {} tool calls (prior_results={})",
                tool_calls.len(),
                prior_results.len()
            ),
        );

        // Load REPL state from TemperFS file (not from entity fields — avoids
        // context JSON bloat that causes WASM memory exhaustion after ~26 turns).
        let workspace_id = fields
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let repl_file_id = fields
            .get("repl_file_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let repl_state_b64 = if !repl_file_id.is_empty() {
            session::read_temperfs_file_safe(&ctx, &temper_api_url, tenant, repl_file_id)
                .unwrap_or_default()
        } else {
            // Fallback: check entity field for backward compatibility
            fields
                .get("repl_state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        ctx.log(
            "info",
            &format!(
                "monty_repl: loaded repl state bytes={} from_file={}",
                repl_state_b64.len(),
                !repl_file_id.is_empty()
            ),
        );

        let mut repl = load_or_create_repl(&repl_state_b64, &ctx)?;

        // Execute each tool call
        let mut tool_results: Vec<Value> = Vec::new();
        let mut tool_span_events: Vec<Value> = Vec::new();

        // Tool-batch checkpoint boundary (TemperPaw Track 1 Phase 3 / temperpaw#66).
        // Chunk size is fixed at 20; empirical fuel cost is ~400M per
        // `temper.get`, so 20 × 400M = 8B sits comfortably under the 120B
        // ceiling. Raise/lower by editing this constant; a future phase may
        // promote it to integration.config.
        const CHECKPOINT_EVERY_N: usize = 20;
        // Runaway guard: fail the session if checkpoints accumulate past this
        // threshold in a single turn (~1000 tool calls at chunk=20).
        const MAX_CHECKPOINTS_PER_TURN: u64 = 50;

        let mut i = 0usize;
        while i < tool_calls.len() {
            // Checkpoint check (before executing the i-th call). Skip on i=0
            // because we just entered; only fire at whole chunk boundaries and
            // only when there is actually more work after us.
            if i > 0 && i % CHECKPOINT_EVERY_N == 0 && i < tool_calls.len() {
                let current_ckpt: u64 = fields
                    .get("checkpoint_count")
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    })
                    .unwrap_or(0);

                if current_ckpt >= MAX_CHECKPOINTS_PER_TURN {
                    ctx.log("error", &format!(
                        "monty_repl: checkpoint runaway guard tripped at count={current_ckpt}, failing session"
                    ));
                    dispatch_success(
                        "Fail",
                        &json!({
                            "error_message": format!(
                                "tool-call checkpoint budget exhausted ({current_ckpt} checkpoints in one turn; max {MAX_CHECKPOINTS_PER_TURN})"
                            ),
                        }),
                    );
                    return Ok(());
                }

                ctx.log(
                    "info",
                    &format!(
                        "monty_repl: tool-batch checkpoint at i={i}, completed={}, remaining={}, checkpoint_count={current_ckpt}",
                        prior_results.len() + tool_results.len(),
                        tool_calls.len() - i
                    ),
                );

                // Save REPL state to TemperFS so the re-entered invocation
                // restarts from the same interpreter heap.
                // Graceful: if save fails, skip this checkpoint — the agent
                // gets a fresh REPL on resume but the session stays alive.
                let saved_state = match save_repl_state(&repl) {
                    Ok(s) => s,
                    Err(e) => {
                        ctx.log(
                            "warn",
                            &format!(
                                "monty_repl: checkpoint repl save failed, skipping checkpoint: {e}"
                            ),
                        );
                        i += 1;
                        continue;
                    }
                };
                let new_repl_file_id = match session::save_repl_to_file(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    workspace_id,
                    repl_file_id,
                    &saved_state,
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        ctx.log(
                            "warn",
                            &format!(
                                "monty_repl: checkpoint file save failed, skipping checkpoint: {e}"
                            ),
                        );
                        i += 1;
                        continue;
                    }
                };

                // Build checkpoint context — same shape the Cedar pause path
                // uses, so the existing resume branch at the top of this
                // function reads it back transparently.
                let mut all_completed = prior_results.clone();
                all_completed.append(&mut tool_results);
                let remaining: Vec<Value> = tool_calls[i..].to_vec();
                let ckpt_ctx = json!({
                    "completed_results": all_completed,
                    "remaining_tool_calls": remaining,
                });

                let session_leaf_id = fields
                    .get("session_leaf_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let params = json!({
                    "pending_tool_calls": serde_json::to_string(&remaining)
                        .unwrap_or_else(|_| "[]".to_string()),
                    "pending_tool_context": serde_json::to_string(&ckpt_ctx)
                        .unwrap_or_else(|_| "{}".to_string()),
                    "repl_file_id": new_repl_file_id,
                    "session_leaf_id": session_leaf_id,
                });

                dispatch_success("CheckpointToolBatch", &params);
                return Ok(());
            }

            let max_batch_len = batch_window_len(i, tool_calls.len(), CHECKPOINT_EVERY_N);
            let batch_len = batchable_run_len(&tool_calls, i, max_batch_len);
            if batch_len >= 2 {
                let batch_calls = collect_batchable_tool_calls(&tool_calls, i, batch_len);
                ctx.log(
                    "info",
                    &format!(
                        "monty_repl: batching {batch_len} read-only tool calls starting at i={i}"
                    ),
                );

                let batch_started_ms = Context::get_time_millis();
                let batch_results = run_with_tool_progress(
                    |boundary| {
                        ctx.log(
                            "debug",
                            &format!(
                                "monty_repl: tool progress boundary={boundary:?} tool_name=batch tool_call_id=batch:{i}"
                            ),
                        );
                        session::send_progress(&ctx, &temper_api_url, tenant);
                    },
                    || {
                        dispatch::execute_batchable_tool_calls(
                            &ctx,
                            &temper_api_url,
                            tenant,
                            &batch_calls,
                            || session::send_progress(&ctx, &temper_api_url, tenant),
                        )
                    },
                );

                for (offset, result) in batch_results.into_iter().enumerate() {
                    let call = &tool_calls[i + offset];
                    let tool_id = call.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let input = call.get("input").cloned().unwrap_or(json!({}));
                    let duration_ms = (Context::get_time_millis() - batch_started_ms).max(0) as u64;
                    let tool_arguments_json = serde_json::to_string(&input).unwrap_or_default();

                    tool_span_events.push(emit_tool_call_telemetry(
                        &ctx,
                        &batch_calls[offset].plan.tool_name,
                        &batch_calls[offset].tool_call_id,
                        &tool_arguments_json,
                        &result,
                        duration_ms,
                    ));
                    push_batch_tool_result(&ctx, &mut tool_results, tool_id, &result);
                }

                i += batch_len;
                continue;
            }

            let call = &tool_calls[i];

            let tool_id = call.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let tool_name = call
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("python");
            let input = call.get("input").cloned().unwrap_or(json!({}));
            let code = input.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let tool_arguments_json = serde_json::to_string(&input).unwrap_or_default();
            let tool_started_ms = Context::get_time_millis();
            let mut outer_guest_span =
                start_tool_guest_span(&ctx, tool_name, tool_id, &tool_arguments_json);

            ctx.log(
                "info",
                &format!(
                    "monty_repl: executing code for {tool_id}, code_bytes={}",
                    code.len()
                ),
            );

            // Execute snippets directly in the shared Monty namespace so globals
            // and helper definitions survive across turns as intended.
            let snippet = code.trim();

            // Execute via REPL feed_start() using a bounded collector so pathological
            // print output cannot force runaway reallocations inside the daemon.
            let mut printed = BoundedOutputCollector::new(MAX_TOOL_RESULT_BYTES);
            let print = PrintWriter::Callback(&mut printed);
            let progress = match repl.feed_start(snippet, vec![], print) {
                Ok(p) => p,
                Err(e) => {
                    let e = *e;
                    repl = e.repl;
                    let mut combined = printed.into_string();
                    let msg = format_monty_exception(&e.error);
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(&msg);
                    tool_results.push(make_tool_result(tool_id, &truncate_output(&combined), true));
                    let duration_ms = (Context::get_time_millis() - tool_started_ms).max(0) as u64;
                    let outer_result = Err(msg);
                    finish_tool_guest_span(
                        &ctx,
                        tool_name,
                        &mut outer_guest_span,
                        &outer_result,
                        duration_ms,
                    );
                    i += 1;
                    continue;
                }
            };

            // Drive the event loop (continues collecting print output)
            let _ = dispatch::take_dispatch_image_results();
            let (result, returned_repl, tool_events) = drive_repl_loop(
                &ctx,
                &temper_api_url,
                tenant,
                &sandbox_url,
                workdir,
                progress,
                &mut printed,
            );
            repl = returned_repl;
            tool_span_events.extend(tool_events);

            // If lazy sandbox was provisioned during this tool call, update
            // sandbox_url for subsequent tool calls in this invocation (ADR-0022).
            if let Some(url) = dispatch::peek_lazy_sandbox_url() {
                sandbox_url = url;
            }

            // --- Cedar denial: clean pause ---
            // Check the thread-local flag (set by dispatch even if Monty catches
            // the exception). If a Cedar denial occurred, save state and pause.
            if let Some(cedar_ctx_json) = dispatch::take_cedar_denial() {
                ctx.log(
                    "info",
                    "monty_repl: Cedar denial detected, pausing for approval",
                );

                // Parse the Cedar context to extract decision_id
                let cedar_ctx: Value = serde_json::from_str(&cedar_ctx_json)
                    .unwrap_or(json!({"decision_id": "unknown"}));
                let decision_id = cedar_ctx
                    .get("decision_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                // Save REPL state before pausing.
                // Graceful: if save fails, agent gets a fresh REPL on resume.
                let repl_file_id = match save_repl_state(&repl).and_then(|saved_state| {
                    session::save_repl_to_file(
                        &ctx,
                        &temper_api_url,
                        tenant,
                        workspace_id,
                        repl_file_id,
                        &saved_state,
                    )
                }) {
                    Ok(id) => id,
                    Err(e) => {
                        ctx.log("warn", &format!(
                            "monty_repl: Cedar pause repl save failed (agent gets fresh REPL on resume): {e}"
                        ));
                        repl_file_id.to_string()
                    }
                };

                // Build pause context: all completed results (prior + current) +
                // remaining tool calls (current denied call + any after it)
                let mut all_completed = prior_results.clone();
                all_completed.append(&mut tool_results);
                let remaining: Vec<Value> = tool_calls[i..].to_vec();

                let pause_ctx = json!({
                    "completed_results": all_completed,
                    "remaining_tool_calls": remaining,
                    "tool_context": cedar_ctx,
                });

                // Dispatch PauseForApproval — Session transitions to WaitingForApproval
                let mut params = json!({
                    "pending_decision_id": decision_id,
                    "pending_tool_context": serde_json::to_string(&pause_ctx)
                        .unwrap_or_else(|_| "{}".to_string()),
                    "repl_file_id": repl_file_id,
                });

                // Preserve sandbox state if lazily provisioned (ADR-0022)
                if let Some((url, id, provider)) = dispatch::take_lazy_sandbox() {
                    params["sandbox_url"] = json!(url);
                    params["sandbox_id"] = json!(id);
                    params["sandbox_provider"] = json!(provider);
                }

                dispatch_success("PauseForApproval", &params);
                return Ok(());
            }

            let printed = printed.into_string();
            let dispatch_images =
                image_results_from_values(&dispatch::take_dispatch_image_results());

            // Combine print output + expression value
            let outer_result = match result {
                Ok(expr_val) => {
                    let expr_image = extract_image_result(&expr_val);
                    let mut image_results = Vec::new();
                    if let Some(image_result) = expr_image.clone() {
                        image_results.push(image_result);
                    }
                    append_unique_image_results(&mut image_results, dispatch_images);

                    let expr_len = expr_val.len();
                    let mut combined = printed;
                    if expr_image.is_none() && expr_val != "null" && !expr_val.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&expr_val);
                    }
                    // If the dispatch function stored important output (e.g.
                    // submit_specs success message), always surface it — even
                    // if Python printed something, the dispatch message is the
                    // authoritative result the LLM needs to see.
                    if let Some(dispatch_msg) = dispatch::take_dispatch_output() {
                        if combined.is_empty() {
                            combined.push_str(&dispatch_msg);
                        } else {
                            combined.push('\n');
                            combined.push_str(&dispatch_msg);
                        }
                    }

                    if !image_results.is_empty() {
                        for image_result in &image_results {
                            if !combined.is_empty() {
                                combined.push('\n');
                            }
                            combined.push_str(&format!(
                                "[Image available at {}]",
                                image_result.source_path
                            ));
                        }
                        let content = truncate_output(&combined);
                        ctx.log(
                            "info",
                            &format!(
                                "monty_repl: tool completed {tool_id}, image_count={image_count}, printed_bytes={}, expr_bytes={}, result_bytes={}, is_error=false",
                                combined.len().saturating_sub(expr_len),
                                expr_len,
                                content.len(),
                                image_count = image_results.len(),
                            ),
                        );
                        tool_results.push(make_tool_result_multimodal_many(
                            tool_id,
                            &content,
                            &image_results,
                            false,
                        ));
                        Ok(json!({
                            "tool.result.preview": content,
                            "tool.result.media_type": image_results[0].media_type,
                        }))
                    } else {
                        if combined.is_empty() {
                            combined.push_str("(no output)");
                        }
                        let content = truncate_output(&combined);
                        ctx.log(
                            "info",
                            &format!(
                                "monty_repl: tool completed {tool_id}, printed_bytes={}, expr_bytes={}, result_bytes={}, is_error=false",
                                combined.len().saturating_sub(expr_len),
                                expr_len,
                                content.len()
                            ),
                        );
                        tool_results.push(make_tool_result(tool_id, &content, false));
                        Ok(json!({ "tool.result.preview": content }))
                    }
                }
                Err(e) => {
                    let error_len = e.len();
                    let mut combined = printed;
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(&e);
                    let content = truncate_output(&combined);
                    ctx.log(
                        "info",
                        &format!(
                            "monty_repl: tool completed {tool_id}, printed_bytes={}, error_bytes={}, result_bytes={}, is_error=true",
                            combined.len().saturating_sub(error_len),
                            error_len,
                            content.len()
                        ),
                    );
                    tool_results.push(make_tool_result(tool_id, &content, true));
                    Err(e)
                }
            };
            let duration_ms = (Context::get_time_millis() - tool_started_ms).max(0) as u64;
            finish_tool_guest_span(
                &ctx,
                tool_name,
                &mut outer_guest_span,
                &outer_result,
                duration_ms,
            );

            i += 1;
        }

        // Merge prior results from Cedar resume (if any) with newly collected results
        if !prior_results.is_empty() {
            prior_results.append(&mut tool_results);
            tool_results = prior_results;
        }

        // Save small REPL states between provider turns, but do not route the
        // normal hot path through a large versioned TemperFS rewrite. Checkpoint
        // and approval pauses still persist state above because they need exact
        // mid-batch recovery.
        // Graceful: REPL state save failure should not kill the session.
        let repl_file_id = match save_repl_state(&repl) {
            Ok(saved_state) => {
                ctx.log(
                    "info",
                    &format!(
                        "monty_repl: saving repl state bytes={}, tool_results={}",
                        saved_state.len(),
                        tool_results.len()
                    ),
                );
                let max_normal_repl_state_bytes = normal_repl_state_max_bytes(&ctx);
                if max_normal_repl_state_bytes > 0
                    && saved_state.len() <= max_normal_repl_state_bytes
                {
                    match session::save_repl_to_file(
                        &ctx,
                        &temper_api_url,
                        tenant,
                        workspace_id,
                        repl_file_id,
                        &saved_state,
                    ) {
                        Ok(id) => id,
                        Err(e) => {
                            ctx.log(
                                "warn",
                                &format!("monty_repl: end-of-batch file save failed: {e}"),
                            );
                            repl_file_id.to_string()
                        }
                    }
                } else {
                    ctx.log(
                        "warn",
                        &format!(
                            "monty_repl: skipping normal repl state persist bytes={} max_bytes={}",
                            saved_state.len(),
                            max_normal_repl_state_bytes
                        ),
                    );
                    String::new()
                }
            }
            Err(e) => {
                ctx.log(
                    "warn",
                    &format!("monty_repl: end-of-batch repl save failed: {e}"),
                );
                repl_file_id.to_string()
            }
        };

        // ProgressMade: a tool batch completed — this is real forward progress,
        // so reset the Executing state_timeout (not just ping liveness).
        session::send_progress(&ctx, &temper_api_url, tenant);

        // Check if agent signaled completion via temper.done(result)
        let done_result = dispatch::take_done_result();

        // Persist results (without repl_state in entity params).
        // Graceful: fall back to inline JSON if file persistence fails.
        let mut params = match session::persist_results(
            &ctx,
            &temper_api_url,
            tenant,
            &fields,
            &tool_results,
            &repl_file_id,
        ) {
            Ok(p) => p,
            Err(e) => {
                ctx.log(
                    "warn",
                    &format!("monty_repl: persist_results failed, falling back to inline: {e}"),
                );
                let results_json = serde_json::to_string(&tool_results).unwrap_or_default();
                json!({"pending_tool_calls": results_json, "repl_file_id": repl_file_id})
            }
        };

        // If a sandbox was lazily provisioned during this invocation,
        // include it in the callback params so it persists to entity state (ADR-0022).
        if let Some((url, id, provider)) = dispatch::take_lazy_sandbox() {
            params["sandbox_url"] = json!(url);
            params["sandbox_id"] = json!(id);
            params["sandbox_provider"] = json!(provider);
        }
        if persist_tool_spans_file(&ctx) && !tool_span_events.is_empty() {
            let existing_tool_spans_file_id = fields
                .get("tool_spans_file_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match session::append_tool_spans_to_file(
                &ctx,
                &temper_api_url,
                tenant,
                workspace_id,
                existing_tool_spans_file_id,
                &tool_span_events,
            ) {
                Ok(id) if !id.is_empty() => {
                    params["tool_spans_file_id"] = json!(id);
                }
                Ok(_) => {}
                Err(e) => {
                    // The tool calls happened; only the record of them is gone.
                    // An empty tool_spans_file_id reads exactly like a session
                    // that called no tools, so the loss is recorded on the
                    // entity and the emitter degrades the trajectory for it.
                    params["tool_spans_write_failed"] = json!("true");
                    ctx.log(
                        "warn",
                        &format!("monty_repl: tool_spans append failed: {e}"),
                    );
                }
            }
        } else if !tool_span_events.is_empty() {
            ctx.log(
                "debug",
                &format!(
                    "monty_repl: skipping tool_spans file persist events={}",
                    tool_span_events.len()
                ),
            );
        }
        attach_llmobs_tool_spans(&mut params, &tool_span_events);

        // Clear Cedar approval state after successful resume so the next
        // run_tools invocation doesn't erroneously re-enter resume mode.
        if is_cedar_resume {
            params["pending_tool_context"] = json!("");
            params["pending_decision_id"] = json!("");
        }

        if let Some(result_text) = done_result {
            // Agent called temper.done() — complete the session
            let mut done_params = params.clone();
            done_params["result"] = json!(result_text);
            done_params["pending_tool_calls"] = json!("");
            done_params["pending_tool_context"] = json!("");
            done_params["pending_decision_id"] = json!("");
            dispatch_success("RecordResult", &done_params);
        } else {
            dispatch_success("HandleToolResults", &params);
        }
        Ok(())
    })();

    let dispatched = ACTION_DISPATCHED.with(|f| f.get());
    match classify_run_outcome(&result, dispatched) {
        RunOutcome::Success => 0,
        RunOutcome::InvariantViolation => {
            // Closure returned Ok but never dispatched a Session action.
            // Under the current code this means a silent code-path slipped
            // through — fire on_failure so the Session transitions to
            // Failed rather than staying stuck in Executing.
            dispatch_error(INVARIANT_VIOLATION_MSG);
            1
        }
        RunOutcome::PropagateError => {
            let e = result.err().unwrap_or_default();
            dispatch_error(&e);
            1
        }
    }
}

/// Load a persistent REPL from serialized state, or create a fresh one.
///
/// At c9802b5 (v0.0.9), MontyRepl::new() creates a bare REPL; globals are
/// injected via feed_start() which we drive to completion.
fn load_or_create_repl(
    repl_state_b64: &str,
    ctx: &Context,
) -> Result<MontyRepl<LimitedTracker>, String> {
    let limits = ResourceLimits::new()
        .max_duration(Duration::from_secs(300))
        .max_memory(64 * 1024 * 1024)
        .max_allocations(250_000);
    let tracker = LimitedTracker::new(limits);

    if repl_state_b64.is_empty() {
        // Create bare REPL, then inject temper + sandbox globals via feed_start
        let repl = MontyRepl::new("init.py", tracker);

        let inputs = vec![
            (
                "temper".to_string(),
                MontyObject::Dataclass {
                    name: "Temper".to_string(),
                    type_id: 1,
                    field_names: vec![],
                    attrs: DictPairs::from(Vec::<(MontyObject, MontyObject)>::new()),
                    frozen: true,
                },
            ),
            (
                "sandbox".to_string(),
                MontyObject::Dataclass {
                    name: "Sandbox".to_string(),
                    type_id: 2,
                    field_names: vec![],
                    attrs: DictPairs::from(Vec::<(MontyObject, MontyObject)>::new()),
                    frozen: true,
                },
            ),
            (
                "json".to_string(),
                MontyObject::Dataclass {
                    name: "Json".to_string(),
                    type_id: 3,
                    field_names: vec![],
                    attrs: DictPairs::from(Vec::<(MontyObject, MontyObject)>::new()),
                    frozen: true,
                },
            ),
        ];

        let print = PrintWriter::Disabled;
        let progress = repl
            .feed_start("pass", inputs, print)
            .map_err(|e| format_monty_exception(&e.error))?;

        let repl = drive_init_to_completion(progress)?;

        ctx.log(
            "info",
            "monty_repl: created fresh REPL with temper + sandbox + json objects",
        );
        Ok(repl)
    } else {
        let bytes = base64_decode(repl_state_b64)?;
        MontyRepl::load(&bytes).map_err(|e| format!("failed to deserialize REPL state: {e}"))
    }
}

/// Drive a simple init snippet (just "pass") to completion, recovering the REPL.
fn drive_init_to_completion(
    mut progress: ReplProgress<LimitedTracker>,
) -> Result<MontyRepl<LimitedTracker>, String> {
    loop {
        match progress {
            ReplProgress::Complete { repl, .. } => return Ok(repl),
            ReplProgress::FunctionCall(call) => {
                let ext_result = ExtFunctionResult::Error(MontyException::new(
                    ExcType::RuntimeError,
                    Some("function calls not allowed during init".into()),
                ));
                let print = PrintWriter::Disabled;
                progress = call
                    .resume(ext_result, print)
                    .map_err(|e| format_monty_exception(&e.error))?;
            }
            ReplProgress::ResolveFutures(state) => {
                let print = PrintWriter::Disabled;
                progress = state
                    .resume(vec![], print)
                    .map_err(|e| format_monty_exception(&e.error))?;
            }
            ReplProgress::NameLookup(lookup) => {
                let print = PrintWriter::Disabled;
                progress = lookup
                    .resume(monty::NameLookupResult::Undefined, print)
                    .map_err(|e| format_monty_exception(&e.error))?;
            }
            ReplProgress::OsCall(os_call) => {
                let ext_result = ExtFunctionResult::Error(MontyException::new(
                    ExcType::RuntimeError,
                    Some("OS calls not allowed during init".into()),
                ));
                let print = PrintWriter::Disabled;
                progress = os_call
                    .resume(ext_result, print)
                    .map_err(|e| format_monty_exception(&e.error))?;
            }
        }
    }
}

/// Serialize the REPL state to base64 for storage in a TemperFS file.
fn save_repl_state(repl: &MontyRepl<LimitedTracker>) -> Result<String, String> {
    let bytes = repl
        .dump()
        .map_err(|e| format!("failed to serialize REPL state: {e}"))?;
    Ok(base64_encode(&bytes))
}

fn normal_repl_state_max_bytes(ctx: &Context) -> usize {
    // Treat an explicitly-zero config the same as missing — fall back to the
    // default. PR #162 shipped `"0"` as a temporary kill-switch and that
    // value is currently baked into the deployed spec; we don't want a stale
    // override to silently disable persistence (and the resulting agent
    // re-discovery loop) until the next Docker bake.
    let configured = ctx
        .config
        .get("normal_repl_state_max_bytes")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if configured > 0 {
        configured
    } else {
        DEFAULT_NORMAL_REPL_STATE_MAX_BYTES
    }
}

/// Whether to persist this batch's tool spans to the session's span file.
///
/// Defaults to ON. The spans are the only record of tool-call wall-clock time,
/// and the OTS emitter reads them to complete each decision — a missing config
/// key silently emptying every stored trajectory (exactly what happened in
/// production, ARN-109) is a worse failure than the write cost, which is
/// bounded by `session::encode_tool_spans_jsonl`. Set the key to a false-y
/// value to opt a deployment out.
fn persist_tool_spans_file(ctx: &Context) -> bool {
    ctx.config
        .get("persist_tool_spans_file")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn attach_llmobs_tool_spans(params: &mut Value, tool_span_events: &[Value]) {
    if tool_span_events.is_empty() {
        return;
    }
    let Some(object) = params.as_object_mut() else {
        return;
    };
    object.insert(
        "_dd_llmobs_tool_spans".to_string(),
        Value::Array(tool_span_events.to_vec()),
    );
}

/// Drive the Monty REPL event loop to completion.
///
/// Accepts a print buffer (from the initial `feed_start()` call) and continues
/// collecting `print()` output throughout execution. Returns:
/// - expression result (Ok/Err)
/// - the repl (for state persistence)
fn drive_repl_loop(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    sandbox_url: &str,
    workdir: &str,
    mut progress: ReplProgress<LimitedTracker>,
    print_buf: &mut BoundedOutputCollector,
) -> (
    Result<String, String>,
    MontyRepl<LimitedTracker>,
    Vec<Value>,
) {
    let mut pending_results: BTreeMap<u32, ExtFunctionResult> = BTreeMap::new();
    let mut tool_span_events = Vec::new();

    loop {
        match progress {
            ReplProgress::Complete { repl, value } => {
                let json_value = convert::monty_object_to_json(&value);
                let result = serde_json::to_string(&json_value)
                    .map_err(|e| format!("failed to serialize result: {e}"));
                return (result, repl, tool_span_events);
            }

            ReplProgress::FunctionCall(call) => {
                if !call.method_call {
                    let msg = format!(
                        "sandbox denied function call '{}'. Only temper.<method> and sandbox.<method> calls are allowed.",
                        call.function_name
                    );
                    let ext_result = ExtFunctionResult::Error(MontyException::new(
                        ExcType::RuntimeError,
                        Some(msg.clone()),
                    ));
                    let print = PrintWriter::Callback(print_buf);
                    match call.resume(ext_result, print) {
                        Ok(p) => {
                            progress = p;
                            continue;
                        }
                        Err(e) => return (Err(msg), e.repl, tool_span_events),
                    }
                }

                let _call_id = call.call_id;
                let fn_name = call.function_name.clone();
                let args = call.args.clone();
                let kwargs = call.kwargs.clone();

                let (obj_name, user_args) = classify_method_call(&args);

                let json_args: Vec<Value> = user_args
                    .iter()
                    .map(|a| convert::monty_object_to_json(a))
                    .collect();
                let json_kwargs: Vec<(Value, Value)> = kwargs
                    .iter()
                    .map(|(k, v)| {
                        (
                            convert::monty_object_to_json(k),
                            convert::monty_object_to_json(v),
                        )
                    })
                    .collect();
                let tool_name = format!("{obj_name}.{fn_name}");
                let tool_call_id = call.call_id.to_string();
                let tool_arguments_json = if json_kwargs.is_empty() {
                    serde_json::to_string(&json_args).unwrap_or_default()
                } else {
                    serde_json::to_string(&json!({
                        "args": json_args,
                        "kwargs": json_kwargs,
                    }))
                    .unwrap_or_default()
                };
                let started_ms = Context::get_time_millis();
                let mut guest_span =
                    start_tool_guest_span(ctx, &tool_name, &tool_call_id, &tool_arguments_json);

                let result = run_with_tool_progress(
                    |boundary| {
                        ctx.log(
                            "debug",
                            &format!(
                                "monty_repl: tool progress boundary={boundary:?} tool_name={tool_name} tool_call_id={tool_call_id}"
                            ),
                        );
                        session::send_progress(ctx, temper_api_url, tenant);
                    },
                    || {
                        dispatch::dispatch(
                            ctx,
                            temper_api_url,
                            tenant,
                            sandbox_url,
                            workdir,
                            &obj_name,
                            &fn_name,
                            Some(tool_call_id.as_str()),
                            &json_args,
                            &json_kwargs,
                        )
                    },
                );
                let duration_ms = (Context::get_time_millis() - started_ms).max(0) as u64;
                finish_tool_guest_span(ctx, &tool_name, &mut guest_span, &result, duration_ms);

                tool_span_events.push(emit_tool_call_telemetry(
                    ctx,
                    &tool_name,
                    &tool_call_id,
                    &tool_arguments_json,
                    &result,
                    duration_ms,
                ));

                let ext_result = match result {
                    Ok(value) => ExtFunctionResult::Return(convert::json_to_monty_object(&value)),
                    Err(message) => {
                        // Tool-disabled errors must surface to the LLM even if Python
                        // code catches the exception — store in DISPATCH_OUTPUT so the
                        // REPL always includes it in the tool result.
                        if message.contains("is not enabled for this session")
                            || message.contains("is not configured for this session")
                        {
                            dispatch::set_dispatch_output(&message);
                        }
                        ExtFunctionResult::Error(MontyException::new(
                            ExcType::RuntimeError,
                            Some(message),
                        ))
                    }
                };

                // Resume with the result directly — we have it now, no need for
                // the async resume_pending() → ResolveFutures roundtrip.
                let print = PrintWriter::Callback(print_buf);
                match call.resume(ext_result, print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return (
                            Err(format_monty_exception(&e.error)),
                            e.repl,
                            tool_span_events,
                        );
                    }
                }
            }

            ReplProgress::ResolveFutures(state) => {
                let mut ready: Vec<(u32, ExtFunctionResult)> = Vec::new();
                for call_id in state.pending_call_ids() {
                    if let Some(result) = pending_results.remove(call_id) {
                        ready.push((*call_id, result));
                    }
                }

                let print = PrintWriter::Callback(print_buf);
                match state.resume(ready, print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return (
                            Err(format_monty_exception(&e.error)),
                            e.repl,
                            tool_span_events,
                        );
                    }
                }
            }

            ReplProgress::NameLookup(lookup) => {
                let print = PrintWriter::Callback(print_buf);
                match lookup.resume(monty::NameLookupResult::Undefined, print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return (
                            Err(format_monty_exception(&e.error)),
                            e.repl,
                            tool_span_events,
                        );
                    }
                }
            }

            ReplProgress::OsCall(os_call) => {
                let ext_result = ExtFunctionResult::Error(MontyException::new(
                    ExcType::RuntimeError,
                    Some(
                        "sandbox blocked OS access. Use sandbox.bash() for shell commands.".into(),
                    ),
                ));
                let print = PrintWriter::Callback(print_buf);
                match os_call.resume(ext_result, print) {
                    Ok(p) => progress = p,
                    Err(e) => {
                        return (
                            Err(format_monty_exception(&e.error)),
                            e.repl,
                            tool_span_events,
                        );
                    }
                }
            }
        }
    }
}

// --- Helpers ---

fn classify_method_call(args: &[MontyObject]) -> (String, Vec<MontyObject>) {
    if args.is_empty() {
        return ("unknown".to_string(), vec![]);
    }
    let obj_name = match &args[0] {
        MontyObject::Dataclass { name, .. } => match name.as_str() {
            "Temper" => "temper",
            "Sandbox" => "sandbox",
            "Json" => "json",
            _ => "unknown",
        },
        _ => "unknown",
    };
    (obj_name.to_string(), args[1..].to_vec())
}

fn format_monty_exception(exception: &MontyException) -> String {
    if exception.traceback().is_empty() {
        exception.summary()
    } else {
        exception.to_string()
    }
}

fn truncate_output(output: &str) -> String {
    if output.len() > MAX_TOOL_RESULT_BYTES {
        let shown = prefix_at_char_boundary(output, MAX_TOOL_RESULT_BYTES);
        format!(
            "{}...\n[truncated, showing {} of {} bytes]",
            shown,
            shown.len(),
            output.len()
        )
    } else {
        output.to_string()
    }
}

fn prefix_at_char_boundary(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    if max_bytes == 0 {
        return "";
    }

    let mut end = 0;
    for (idx, ch) in input.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &input[..end]
}

fn make_tool_result(tool_id: &str, content: &str, is_error: bool) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_id,
        "content": content,
        "is_error": is_error,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ImageResult {
    media_type: String,
    base64_data: String,
    source_path: String,
    file_id: String,
    file_version_id: String,
    path: String,
    media_generation_id: String,
}

fn make_tool_result_multimodal_many(
    tool_id: &str,
    text: &str,
    images: &[ImageResult],
    is_error: bool,
) -> Value {
    let mut content_blocks: Vec<Value> = Vec::new();
    if !text.is_empty() {
        content_blocks.push(json!({
            "type": "text",
            "text": text
        }));
    }
    for image in images {
        let source = if image.base64_data.is_empty() {
            json!({
                "type": "pawfs_file",
                "media_type": image.media_type,
                "file_id": image.file_id,
                "path": image.path
            })
        } else {
            json!({
                "type": "base64",
                "media_type": image.media_type,
                "data": image.base64_data
            })
        };
        content_blocks.push(json!({
            "__temperpaw_image": true,
            "type": "image",
            "source": source,
            "media_type": image.media_type,
            "mime_type": image.media_type,
            "file_id": image.file_id,
            "file_version_id": image.file_version_id,
            "path": image.path,
            "media_generation_id": image.media_generation_id
        }));
    }
    json!({
        "type": "tool_result",
        "tool_use_id": tool_id,
        "content": content_blocks,
        "is_error": is_error,
    })
}

/// Check if a JSON-serialized expression value is an image result from dispatch.
fn extract_image_result(expr_val: &str) -> Option<ImageResult> {
    let v: Value = serde_json::from_str(expr_val).ok()?;
    extract_image_result_value(&v)
}

fn image_results_from_values(values: &[Value]) -> Vec<ImageResult> {
    values
        .iter()
        .filter_map(extract_image_result_value)
        .collect()
}

fn append_unique_image_results(images: &mut Vec<ImageResult>, additional: Vec<ImageResult>) {
    let mut seen = images
        .iter()
        .map(image_result_key)
        .filter(|key| !key.is_empty())
        .collect::<BTreeSet<_>>();
    for image in additional {
        let key = image_result_key(&image);
        if key.is_empty() || seen.insert(key) {
            images.push(image);
        }
    }
}

fn image_result_key(image: &ImageResult) -> String {
    if !image.file_id.is_empty() {
        image.file_id.clone()
    } else if !image.path.is_empty() {
        image.path.clone()
    } else {
        image.base64_data.clone()
    }
}

fn extract_image_result_value(v: &Value) -> Option<ImageResult> {
    let is_image = v
        .get("__temperpaw_image")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || v.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "image");
    if !is_image {
        return None;
    }

    let media_type = v
        .get("media_type")
        .or_else(|| v.get("mime_type"))
        .or_else(|| v.get("source").and_then(|source| source.get("media_type")))
        .and_then(Value::as_str)
        .unwrap_or("image/png")
        .to_string();
    let base64_data = v
        .get("base64_data")
        .or_else(|| v.get("source").and_then(|source| source.get("data")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let file_id = v
        .get("file_id")
        .or_else(|| v.get("source").and_then(|source| source.get("file_id")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let path = v
        .get("path")
        .or_else(|| v.get("source").and_then(|source| source.get("path")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if base64_data.is_empty() && file_id.is_empty() && path.is_empty() {
        return None;
    }

    let source_path = v
        .get("source_path")
        .or_else(|| v.get("path"))
        .or_else(|| v.get("source").and_then(|source| source.get("path")))
        .and_then(Value::as_str)
        .unwrap_or("(image)")
        .to_string();
    Some(ImageResult {
        media_type,
        base64_data,
        source_path,
        file_id,
        file_version_id: v
            .get("file_version_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        path,
        media_generation_id: v
            .get("media_generation_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn emit_tool_call_telemetry(
    ctx: &Context,
    tool_name: &str,
    tool_call_id: &str,
    tool_arguments_json: &str,
    result: &Result<Value, String>,
    duration_ms: u64,
) -> Value {
    let success = result.is_ok();
    let result_content = match result {
        Ok(value) => {
            // Don't log full base64 image data in telemetry
            if value
                .get("__temperpaw_image")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let path = value
                    .get("source_path")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let size = value
                    .get("base64_data")
                    .and_then(Value::as_str)
                    .map(|s| s.len())
                    .unwrap_or(0);
                format!("[image from {path}, base64_bytes={size}]")
            } else {
                truncate_output(&value.to_string())
            }
        }
        Err(message) => truncate_output(message),
    };
    // Successful tool dispatches are superseded by the `tool.<name>`
    // span (ADR-0037) and emitted at debug to reduce log volume.
    // Failed dispatches stay at warn so on-call still sees them in
    // default log streams.
    let log_level = if success { "debug" } else { "warn" };
    ctx.log(
        log_level,
        &format!(
            "tool dispatch complete tool_name={tool_name} tool_call_id={tool_call_id} duration_ms={duration_ms} success={success} result_preview={result_content}"
        ),
    );

    json!({
        "tool_name": tool_name,
        "tool_call_id": tool_call_id,
        "arguments": truncate_output(tool_arguments_json),
        "result": result_content,
        "duration_ms": duration_ms,
        "is_error": !success,
    })
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| format!("base64 decode error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ok_with_dispatch_is_success() {
        assert_eq!(classify_run_outcome(&Ok(()), true), RunOutcome::Success);
    }

    #[test]
    fn classify_ok_without_dispatch_is_invariant_violation() {
        // This is the orphan-creating path: closure returned Ok but never
        // fired a Session action. run() must convert this to an error so
        // on_failure="Fail" transitions the Session out of Executing.
        assert_eq!(
            classify_run_outcome(&Ok(()), false),
            RunOutcome::InvariantViolation
        );
    }

    #[test]
    fn classify_err_always_propagates() {
        assert_eq!(
            classify_run_outcome(&Err("boom".to_string()), true),
            RunOutcome::PropagateError
        );
        assert_eq!(
            classify_run_outcome(&Err("boom".to_string()), false),
            RunOutcome::PropagateError
        );
    }

    #[test]
    fn invariant_message_mentions_adr() {
        // Belt-and-suspenders regression guard: the error message must stay
        // greppable for future debugging. If someone "helpfully" rewrites it
        // to something bland, this test fails.
        assert!(INVARIANT_VIOLATION_MSG.contains("monty_repl"));
        assert!(INVARIANT_VIOLATION_MSG.contains("Session action"));
        assert!(INVARIANT_VIOLATION_MSG.contains("ADR-0039"));
    }

    #[test]
    fn tool_progress_wrapper_emits_start_and_end_on_success() {
        let mut events = Vec::new();

        let result = run_with_tool_progress(|event| events.push(event), || Ok::<_, String>(42));

        assert_eq!(result, Ok(42));
        assert_eq!(
            events,
            vec![ToolProgressBoundary::Start, ToolProgressBoundary::End]
        );
    }

    #[test]
    fn tool_progress_wrapper_emits_end_on_error() {
        let mut events = Vec::new();

        let result = run_with_tool_progress(
            |event| events.push(event),
            || Err::<(), _>("tool failed".to_string()),
        );

        assert_eq!(result, Err("tool failed".to_string()));
        assert_eq!(
            events,
            vec![ToolProgressBoundary::Start, ToolProgressBoundary::End]
        );
    }

    #[test]
    fn llmobs_tool_spans_are_attached_to_callback_params_when_present() {
        let mut params = json!({
            "pending_tool_calls": "[]",
        });
        let tool_span_events = vec![json!({
            "tool_name": "temper.list",
            "tool_call_id": "call-1",
            "arguments": "{\"entity_set\":\"Sessions\"}",
            "result": "{\"value\":[]}",
            "duration_ms": 42,
            "is_error": false,
        })];

        attach_llmobs_tool_spans(&mut params, &tool_span_events);

        assert_eq!(params["_dd_llmobs_tool_spans"], json!(tool_span_events));
    }

    #[test]
    fn file_only_temperpaw_image_results_are_extractable() {
        let expr = json!({
            "__temperpaw_image": true,
            "media_type": "image/png",
            "content_ref": "pawfs_file",
            "file_id": "fl-cat",
            "file_version_id": "fv-cat",
            "path": "/generated/images/cat.png",
            "media_generation_id": "en-cat"
        })
        .to_string();

        let image = extract_image_result(&expr).expect("file-only image should be detected");

        assert_eq!(image.base64_data, "");
        assert_eq!(image.file_id, "fl-cat");
        assert_eq!(image.source_path, "/generated/images/cat.png");
    }

    #[test]
    fn multimodal_tool_result_preserves_file_only_images_for_reply_attachments() {
        let image = ImageResult {
            media_type: "image/png".to_string(),
            base64_data: String::new(),
            source_path: "/generated/images/cat.png".to_string(),
            file_id: "fl-cat".to_string(),
            file_version_id: "fv-cat".to_string(),
            path: "/generated/images/cat.png".to_string(),
            media_generation_id: "en-cat".to_string(),
        };

        let result = make_tool_result_multimodal_many("call-cat", "generated", &[image], false);
        let image_block = &result["content"][1];

        assert_eq!(image_block["type"], "image");
        assert_eq!(image_block["file_id"], "fl-cat");
        assert_eq!(image_block["source"]["type"], "pawfs_file");
        assert_eq!(image_block["source"]["file_id"], "fl-cat");
    }

    #[test]
    fn batchable_run_len_stops_before_non_batchable_snippet() {
        let tool_calls = vec![
            json!({"input": {"code": "temper.web_search('terminal ui inspiration')"}}),
            json!({"input": {"code": "temper.web_fetch('https://example.com/guide')"}}),
            json!({"input": {"code": "result = temper.web_search(query)"}}),
            json!({"input": {"code": "temper.specs()"}}),
        ];

        assert_eq!(batchable_run_len(&tool_calls, 0, tool_calls.len()), 2);
    }

    #[test]
    fn batchable_run_len_respects_checkpoint_window_limit() {
        let tool_calls = vec![
            json!({"input": {"code": "temper.web_search('one')"}}),
            json!({"input": {"code": "temper.web_fetch('https://example.com/two')"}}),
            json!({"input": {"code": "temper.specs()"}}),
        ];

        assert_eq!(batchable_run_len(&tool_calls, 0, 2), 2);
    }
}
