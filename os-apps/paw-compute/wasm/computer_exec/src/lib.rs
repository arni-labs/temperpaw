//! computer_exec — WASM module executing one shell command on a Computer's sandbox.
//!
//! Runs on the Exec entity's Run action (and LatencyDiag's RunScan). Resolves the
//! target Computer row via the Temper API loopback, builds a SandboxHandle from
//! its recorded fields (provider, sandbox_url, machine_id), and executes the
//! command through the shared sandbox provider abstraction in wasm-helpers.
//! stdout and stderr are captured to separate per-exec log files on the sandbox;
//! a bounded tail of each is written back to the entity so the audit row stays
//! small regardless of what the command prints.
//!
//! Reports back to the state machine: RunSucceeded (exit_code, stdout_tail,
//! stderr_tail, stdout_path, stdout_bytes) or RunFailed (error + cleared result
//! fields, via `set_failure_result`).
//!
//! Build: `cargo build --target wasm32-wasip1 --release` (WASI target — the host
//! wires wasi_snapshot_preview1; wasm32-unknown-unknown is forbidden, it links
//! wasm-bindgen via chrono and fails host instantiation).

use temper_wasm_sdk::prelude::*;
use wasm_helpers::sandbox::{
    self, computer_is_runnable, ensure_sandbox_awake, normalize_sandbox_provider, ExecResult,
    SandboxHandle,
};
use wasm_helpers::{bounded_reads, entity_field_str, odata_headers, resolve_temper_api_url};

/// Keep at most this many bytes of stdout/stderr on the row. The full output is
/// persisted to per-exec log files on the sandbox (see [`wrap_command`]); this
/// bounds only the tails carried back on the row.
const OUTPUT_TAIL_BYTES: usize = 262_144;

/// Hard wall-clock limit for the command on the sandbox, enforced by `timeout` so
/// a runaway command is killed and cannot outlive the exec (no orphans). Budget
/// math against the 120s WASM invocation cap (temper-wasm `WasmResourceLimits`):
/// command ≤90s, then the tensorlake helper polls to ~100s (outliving this), then
/// the output reads + callback fit in the remaining ~20s. Must stay below the
/// helper's poll budget. Longer runs need the async exec path (ARN-443 D).
const EXEC_TIMEOUT_SECS: u64 = 90;

/// Marker (fixed line, wrapper-emitted) announcing the stdout log path.
const EXEC_LOG_MARKER: &str = "__EXEC_LOG_PATH";
/// Marker (fixed line, wrapper-emitted) carrying the timed-out flag (0/1).
const EXEC_TO_MARKER: &str = "__EXEC_TIMED_OUT";

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let computer_id = config_field_or_param(&ctx, &fields, "computer_id")
            .ok_or("computer_exec: missing computer_id")?;
        let command = config_field_or_param(&ctx, &fields, "command")
            .ok_or("computer_exec: missing command")?;

        ctx.log(
            "info",
            &format!(
                "computer_exec: exec {} on computer {computer_id} ({} byte command)",
                ctx.entity_id,
                command.len()
            ),
        );

        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let computer = fetch_computer(&ctx, &temper_api_url, &fields, &computer_id)?;
        let handle = sandbox_handle_from_computer(&computer)
            .map_err(|e| format!("computer_exec: computer {computer_id}: {e}"))?;
        let status = entity_field_str(&computer, &["Status", "status"]).unwrap_or("");
        ensure_sandbox_awake(&ctx, &handle, status)
            .map_err(|e| format!("computer_exec: resume {computer_id} from {status}: {e}"))?;

        let wrapped = wrap_command(&command, &ctx.entity_id);
        let result = sandbox::sandbox_exec(&ctx, &handle, &wrapped, "/")?;
        let captured = parse_captured_output(&result.stdout);
        ctx.log(
            "info",
            &format!(
                "computer_exec: command exited {} (timed_out={}, stderr {} bytes)",
                result.exit_code,
                captured.timed_out,
                result.stderr.len()
            ),
        );

        // A `timeout`-killed command is a FAILURE, not a result: the done marker
        // is absent (the child never reached it), which distinguishes an
        // outer-timeout from a command that legitimately exits 124. The process
        // is already gone (timeout killed its group on the sandbox — no orphan).
        if captured.timed_out {
            set_failure_result(&format!(
                "command exceeded the {EXEC_TIMEOUT_SECS}s limit and was terminated on the sandbox"
            ));
            return Ok(());
        }

        set_success_result(
            "RunSucceeded",
            &success_params(&captured, &result, OUTPUT_TAIL_BYTES),
        );
        Ok(())
    })();

    if let Err(e) = result {
        set_failure_result(&e);
    }
    0
}

/// Read a value from the trigger config FIRST, then entity state, then trigger
/// params.
///
/// The trigger config is spec-defined and NOT influenced by the request body, so
/// a value pinned there (LatencyDiag's canned command / computer_id) can never be
/// overridden by a caller. Temper merges arbitrary request-body keys into entity
/// fields, so the field is NOT a safe source for a constrained value — hence
/// config wins. For Exec nothing is pinned in config, so the caller's Run params
/// (written to fields) are used.
fn config_field_or_param(ctx: &Context, fields: &Value, key: &str) -> Option<String> {
    ctx.config
        .get(key)
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| {
            entity_field_str(fields, &[key])
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            ctx.trigger_params
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

/// Emit the on-failure callback with the error AND cleared result fields, so a
/// re-run that fails (e.g. LatencyDiag rescanning) never leaves a previous run's
/// exit code / output on the row. `success: false` routes it to the spec's
/// `on_failure` action (RunFailed); the cleared params overwrite stale values.
fn set_failure_result(error: &str) {
    let result = json!({
        "action": "callback",
        "params": {
            "error": error,
            "exit_code": "",
            "stdout_tail": "",
            "stderr_tail": "",
        },
        "success": false,
        "error": error,
    });
    let json = result.to_string();
    unsafe {
        temper_wasm_sdk::host::host_set_result(json.as_ptr() as i32, json.len() as i32);
    }
}

/// Fetch the Computer row by id via the Temper API loopback.
fn fetch_computer(
    ctx: &Context,
    temper_api_url: &str,
    fields: &Value,
    computer_id: &str,
) -> Result<Value, String> {
    let headers = odata_headers(ctx, &ctx.tenant, fields);
    let path = format!(
        "/tdata/Computers('{}')",
        bounded_reads::odata_escape(computer_id)
    );
    bounded_reads::get_json(ctx, temper_api_url, &path, &headers, "computer_exec")
}

/// Build a SandboxHandle from a Computer row's recorded fields.
///
/// The Computer must be runnable (Ready, Leased, or Sleeping) with a
/// sandbox_url. Fail CLOSED: missing/empty or any other status is refused.
fn sandbox_handle_from_computer(computer: &Value) -> Result<SandboxHandle, String> {
    let status = entity_field_str(computer, &["Status", "status"]).unwrap_or("");
    if !computer_is_runnable(status) {
        let shown = if status.is_empty() {
            "(no status)"
        } else {
            status
        };
        return Err(format!(
            "computer is {shown}, not Ready, Leased, or Sleeping"
        ));
    }

    let sandbox_url = entity_field_str(computer, &["SandboxUrl", "sandbox_url"])
        .map(str::trim)
        .unwrap_or("");
    if sandbox_url.is_empty() {
        return Err("no sandbox_url recorded — provision the computer first".to_string());
    }

    let sandbox_id = entity_field_str(computer, &["MachineId", "machine_id"])
        .filter(|s| !s.is_empty())
        .or_else(|| entity_field_str(computer, &["Name", "name"]).filter(|s| !s.is_empty()))
        .unwrap_or("computer-sandbox");

    let provider = entity_field_str(computer, &["Provider", "provider"])
        .filter(|s| !s.is_empty())
        .map(normalize_sandbox_provider)
        .unwrap_or_else(|| "tensorlake".to_string());

    Ok(SandboxHandle {
        sandbox_url: sandbox_url.to_string(),
        sandbox_id: sandbox_id.to_string(),
        provider,
    })
}

/// Wrap the user command so its full stdout and stderr are persisted to separate
/// per-exec log files on the sandbox while the row carries a bounded tail of each.
///
/// - The user command runs in a CHILD `bash -c` under `timeout`. The OUTER
///   script's epilogue (rc + markers + tails) ALWAYS runs afterward — nothing the
///   command does (`exit`, `exec`, background spawns) can skip it, because the
///   command never runs in the wrapper's own shell.
/// - Timed-out is read from `timeout`'s own exit status (124), NOT a marker the
///   command could skip — so `exec long-running` is reported by its real outcome,
///   not misclassified. (A command that itself exits exactly 124 is then
///   classified as timed-out: an accepted residual — it is the caller's own audit
///   record, and WHO ran WHAT stays kernel-stamped and unforgeable; the security
///   boundary is Cedar + kernel identity, not this observability bit.)
/// - `timeout -k` kills the whole child group past `EXEC_TIMEOUT_SECS` (no
///   orphan). stdout → `<id>.log`, stderr → `<id>.err` (separate streams). The
///   wrapper prints the stdout byte count, the log-path marker, the timed-out
///   marker, and the stdout tail to its own stdout; the stderr tail to its own
///   stderr (`>&2`) — so no command-controllable delimiter lives in the stdout
///   data. `exit $__rc` surfaces the true status.
fn wrap_command(command: &str, exec_id: &str) -> String {
    let id = exec_log_id(exec_id);
    let log = format!("~/.exec-out/{id}.log");
    let err = format!("~/.exec-out/{id}.err");
    let q = shell_single_quote(command);
    format!(
        "mkdir -p ~/.exec-out ; \
         timeout -k 5s {EXEC_TIMEOUT_SECS}s bash -c {q} > {log} 2> {err} ; __rc=$? ; \
         if [ \"$__rc\" -eq 124 ]; then __to=1 ; else __to=0 ; fi ; \
         wc -c < {log} ; echo \"{EXEC_LOG_MARKER} {log}\" ; echo \"{EXEC_TO_MARKER} $__to\" ; \
         tail -c {OUTPUT_TAIL_BYTES} {log} ; tail -c {OUTPUT_TAIL_BYTES} {err} >&2 ; exit $__rc"
    )
}

/// POSIX single-quote a string so it survives as one `bash -c` argument.
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// FNV-1a 32-bit digest of the full id, so a bounded (truncated) exec-log id
/// stays distinguishing between distinct ids.
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Injective, filename-safe, bounded encoding of the exec id for `~/.exec-out`.
/// Alphanumerics and `-` pass through; every other byte — `_` included — becomes
/// `_` + two hex digits (so `_` only marks an escape and distinct ids never
/// alias, e.g. `a/b` vs `a?b`). Capped at 32 encoded chars + an 8-hex FNV hash of
/// the FULL id, so the filename stays bounded while distinct ids stay distinct.
/// Empty falls back to `exec`.
fn exec_log_id(exec_id: &str) -> String {
    let mut enc = String::with_capacity(exec_id.len());
    for b in exec_id.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' {
            enc.push(b as char);
        } else {
            enc.push('_');
            enc.push_str(&format!("{b:02x}"));
        }
    }
    let head: String = enc.chars().take(32).collect();
    let head = if head.is_empty() {
        "exec".to_string()
    } else {
        head
    };
    format!("{head}-{:08x}", fnv1a_32(exec_id.as_bytes()))
}

/// The stdout byte count, log path, timed-out flag, and stdout tail parsed out of
/// a wrapped command's stdout. (stderr arrives on a separate stream — see
/// [`success_params`].)
struct CapturedOutput {
    path: Option<String>,
    bytes: Option<u64>,
    timed_out: bool,
    tail: String,
}

/// Parse the wrapped-command stdout:
/// `<bytes>\n__EXEC_LOG_PATH <path>\n__EXEC_TIMED_OUT <0|1>\n<tail>`.
/// The markers sit on fixed lines the wrapper emits, so command output that
/// happens to contain the marker text stays in the tail and cannot be mistaken
/// for a delimiter. If the header is absent — e.g. the wrapper never ran
/// (provider error text) — the raw text is returned as the tail so nothing is
/// silently dropped.
fn parse_captured_output(stdout: &str) -> CapturedOutput {
    let mut parts = stdout.splitn(4, '\n');
    let first = parts.next().unwrap_or("");
    let second = parts.next().unwrap_or("");
    let third = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");

    let bytes = first.trim().parse::<u64>().ok();
    let path = second
        .strip_prefix(&format!("{EXEC_LOG_MARKER} "))
        .map(|s| s.trim().to_string());
    let to_flag = third.strip_prefix(&format!("{EXEC_TO_MARKER} "));

    if bytes.is_none() || path.is_none() || to_flag.is_none() {
        return CapturedOutput {
            path: None,
            bytes: None,
            timed_out: false,
            tail: stdout.to_string(),
        };
    }

    CapturedOutput {
        path,
        bytes,
        timed_out: to_flag.map(|s| s.trim() == "1").unwrap_or(false),
        tail: rest.to_string(),
    }
}

/// Build the RunSucceeded callback params, truncating each stream to a bounded
/// tail and surfacing the full-stdout log path/size. stderr comes from the
/// provider's separate stderr capture (the wrapper writes the stderr tail to its
/// own fd 2), so stdout and stderr never share a stream.
fn success_params(captured: &CapturedOutput, result: &ExecResult, tail_bytes: usize) -> Value {
    json!({
        "exit_code": result.exit_code.to_string(),
        "stdout_tail": output_tail(&captured.tail, tail_bytes),
        "stderr_tail": output_tail(&result.stderr, tail_bytes),
        "stdout_path": captured.path.clone().unwrap_or_default(),
        "stdout_bytes": captured.bytes.map(|b| b.to_string()).unwrap_or_default(),
    })
}

/// Keep the last `max_bytes` bytes of `text`, aligned to a char boundary, with a
/// marker noting how much was dropped. A leading replacement char left by a
/// byte-level `tail -c` on the sandbox is trimmed so the tail starts clean.
fn output_tail(text: &str, max_bytes: usize) -> String {
    let text = text.strip_prefix('\u{FFFD}').unwrap_or(text);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[... {} bytes truncated ...]\n{}", start, &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_computer() -> Value {
        json!({
            "Id": "computer-1",
            "Status": "Ready",
            "fields": {
                "name": "dd-computer",
                "provider": "tensorlake",
                "machine_id": "sbx-abc123",
                "sandbox_url": "https://sbx-abc123.sandbox.tensorlake.ai",
            }
        })
    }

    #[test]
    fn handle_from_ready_computer() {
        let handle = sandbox_handle_from_computer(&ready_computer()).unwrap();
        assert_eq!(
            handle.sandbox_url,
            "https://sbx-abc123.sandbox.tensorlake.ai"
        );
        assert_eq!(handle.sandbox_id, "sbx-abc123");
        assert_eq!(handle.provider, "tensorlake");
    }

    #[test]
    fn handle_normalizes_provider_alias() {
        let mut computer = ready_computer();
        computer["fields"]["provider"] = json!("tl");
        let handle = sandbox_handle_from_computer(&computer).unwrap();
        assert_eq!(handle.provider, "tensorlake");
    }

    #[test]
    fn handle_defaults_provider_to_tensorlake() {
        let mut computer = ready_computer();
        computer["fields"]["provider"] = json!("");
        let handle = sandbox_handle_from_computer(&computer).unwrap();
        assert_eq!(handle.provider, "tensorlake");
    }

    #[test]
    fn handle_falls_back_to_name_when_machine_id_missing() {
        let mut computer = ready_computer();
        computer["fields"]["machine_id"] = json!("");
        let handle = sandbox_handle_from_computer(&computer).unwrap();
        assert_eq!(handle.sandbox_id, "dd-computer");
    }

    #[test]
    fn handle_rejects_missing_sandbox_url() {
        let mut computer = ready_computer();
        computer["fields"]["sandbox_url"] = json!("");
        let err = sandbox_handle_from_computer(&computer).err().unwrap();
        assert!(err.contains("no sandbox_url"), "unexpected error: {err}");
    }

    #[test]
    fn handle_accepts_sleeping_and_leased() {
        let mut sleeping = ready_computer();
        sleeping["Status"] = json!("Sleeping");
        assert!(sandbox_handle_from_computer(&sleeping).is_ok());
        let mut leased = ready_computer();
        leased["Status"] = json!("Leased");
        assert!(sandbox_handle_from_computer(&leased).is_ok());
    }

    #[test]
    fn handle_rejects_non_runnable_computer() {
        let mut computer = ready_computer();
        computer["Status"] = json!("Terminating");
        let err = sandbox_handle_from_computer(&computer).err().unwrap();
        assert!(err.contains("Terminating"), "unexpected error: {err}");
    }

    #[test]
    fn handle_rejects_missing_status_fail_closed() {
        let computer = json!({
            "Id": "c",
            "fields": { "sandbox_url": "https://x.sandbox.tensorlake.ai", "machine_id": "x" }
        });
        let err = sandbox_handle_from_computer(&computer).err().unwrap();
        assert!(err.contains("no status"), "unexpected error: {err}");
    }

    #[test]
    fn exec_log_id_is_injective_and_bounded() {
        // Distinct ids that a lossy sanitizer would collapse must differ.
        assert_ne!(exec_log_id("a/b"), exec_log_id("a?b"));
        assert_ne!(exec_log_id("a_b"), exec_log_id("a/b"));
        // No path/space/quote can escape ~/.exec-out.
        let id = exec_log_id("../../etc/passwd '; rm -rf ~");
        assert!(
            !id.contains('/') && !id.contains(' ') && !id.contains('\''),
            "{id}"
        );
        // Bounded: 32 encoded head + '-' + 8 hex = <= 41.
        let long = exec_log_id(&"x".repeat(500));
        assert!(long.len() <= 41, "{} {}", long.len(), long);
        // Two long ids sharing the first 32 encoded chars still differ via hash.
        assert_ne!(
            exec_log_id(&("y".repeat(40) + "A")),
            exec_log_id(&("y".repeat(40) + "B"))
        );
        assert!(exec_log_id("").starts_with("exec-"));
    }

    #[test]
    fn tail_keeps_short_output_intact() {
        assert_eq!(output_tail("hello", 8192), "hello");
    }

    #[test]
    fn tail_truncates_long_output_with_marker() {
        let long = "x".repeat(10_000);
        let tail = output_tail(&long, 8192);
        assert!(tail.starts_with("[... 1808 bytes truncated ...]\n"));
        assert!(tail.ends_with('x'));
        assert_eq!(tail.len(), "[... 1808 bytes truncated ...]\n".len() + 8192);
    }

    #[test]
    fn tail_respects_char_boundaries() {
        let long = "€".repeat(4000);
        let tail = output_tail(&long, 8192);
        assert!(tail.contains("bytes truncated"));
        assert!(tail.ends_with('€'));
    }

    #[test]
    fn tail_trims_leading_replacement_char() {
        let text = format!("\u{FFFD}rest of the output");
        assert_eq!(output_tail(&text, 8192), "rest of the output");
    }

    #[test]
    fn default_tail_cap_is_256k() {
        assert_eq!(OUTPUT_TAIL_BYTES, 262_144);
    }

    // -- Command wrapping ----------------------------------------------------

    #[test]
    fn wrap_runs_command_in_a_timed_child_epilogue_outside() {
        let wrapped = wrap_command("echo hi", "exec-1");
        assert!(wrapped.contains("mkdir -p ~/.exec-out"));
        // Child (bash -c) under a hard timeout with a kill grace.
        assert!(wrapped.contains("timeout -k 5s 90s bash -c "), "{wrapped}");
        // Separate stdout and stderr files.
        assert!(wrapped.contains("2> ~/.exec-out/"));
        // Timed-out is derived from timeout's own exit (124) in the OUTER script —
        // no child-written marker to skip; correctly handles `exec`.
        assert!(wrapped.contains(r#"if [ "$__rc" -eq 124 ]"#), "{wrapped}");
        assert!(!wrapped.contains(".done"));
        assert!(wrapped.contains("__EXEC_TIMED_OUT $__to"));
        // stderr tail goes to the wrapper's own stderr (no in-band delimiter).
        assert!(wrapped.contains(">&2"));
        assert!(wrapped.trim_end().ends_with("exit $__rc"));
    }

    #[test]
    fn wrap_single_quotes_the_command() {
        let wrapped = wrap_command("echo 'hi'", "e");
        assert!(wrapped.contains(r"echo '\''hi'\''"), "{wrapped}");
    }

    #[test]
    fn parse_extracts_bytes_path_timeout_and_tail() {
        let stdout = "1234\n__EXEC_LOG_PATH ~/.exec-out/e.log\n__EXEC_TIMED_OUT 0\nhello\nworld\n";
        let cap = parse_captured_output(stdout);
        assert_eq!(cap.bytes, Some(1234));
        assert_eq!(cap.path.as_deref(), Some("~/.exec-out/e.log"));
        assert!(!cap.timed_out);
        assert_eq!(cap.tail, "hello\nworld\n");
    }

    #[test]
    fn parse_flags_timeout() {
        let stdout = "0\n__EXEC_LOG_PATH ~/.exec-out/e.log\n__EXEC_TIMED_OUT 1\n";
        let cap = parse_captured_output(stdout);
        assert!(cap.timed_out);
    }

    #[test]
    fn parse_keeps_tail_containing_marker_text() {
        // A command printing the marker text on a later line is not a delimiter.
        let stdout =
            "7\n__EXEC_LOG_PATH ~/.exec-out/e.log\n__EXEC_TIMED_OUT 0\n__EXEC_LOG_PATH nope\n";
        let cap = parse_captured_output(stdout);
        assert_eq!(cap.tail, "__EXEC_LOG_PATH nope\n");
    }

    #[test]
    fn parse_falls_back_to_raw_when_header_absent() {
        let stdout = "some unexpected provider output";
        let cap = parse_captured_output(stdout);
        assert_eq!(cap.bytes, None);
        assert_eq!(cap.path, None);
        assert_eq!(cap.tail, "some unexpected provider output");
    }

    #[test]
    fn success_params_carry_exit_code_stdout_and_stderr() {
        let result = ExecResult {
            stdout: "3\n__EXEC_LOG_PATH ~/.exec-out/e.log\n__EXEC_TIMED_OUT 0\nok\n".to_string(),
            stderr: "warn\n".to_string(),
            exit_code: 0,
        };
        let cap = parse_captured_output(&result.stdout);
        let params = success_params(&cap, &result, 8192);
        assert_eq!(params["exit_code"], "0");
        assert_eq!(params["stdout_tail"], "ok\n");
        assert_eq!(params["stderr_tail"], "warn\n");
        assert_eq!(params["stdout_path"], "~/.exec-out/e.log");
        assert_eq!(params["stdout_bytes"], "3");
    }

    #[test]
    fn success_params_stdout_tail_keeps_marker_lookalike() {
        // A command printing a marker-looking line stays in the stdout tail
        // intact — no in-band delimiter to corrupt it.
        let result = ExecResult {
            stdout: "20\n__EXEC_LOG_PATH ~/.exec-out/e.log\n__EXEC_TIMED_OUT 0\nline1\n__EXEC_TIMED_OUT 1\nline3\n"
                .to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
        let cap = parse_captured_output(&result.stdout);
        let params = success_params(&cap, &result, 8192);
        assert_eq!(params["stdout_tail"], "line1\n__EXEC_TIMED_OUT 1\nline3\n");
    }
}
