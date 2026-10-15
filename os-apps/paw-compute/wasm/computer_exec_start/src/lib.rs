//! computer_exec_start — launch a command on a Computer's sandbox and return
//! immediately (ARN-443 D, async exec).
//!
//! Fires on Exec.Run. Resolves the target Computer (Ready-gated), wraps the
//! command in a long sandbox-side `timeout` (the async path has no 120s cap — the
//! Poll loop spans invocations), starts it via `sandbox_exec_start`, and reports
//! ExecStarted(run_id, started_at_ms). The Poll loop (computer_exec_poll) then
//! drives it to completion. On failure the trigger's on_failure routes to
//! RunFailed.
//!
//! (NOTE: the small computer-resolution helpers below are duplicated from
//! computer_exec / computer_exec_poll; DRY-ing them into a shared wasm-helpers
//! module is a tracked follow-up, kept out of this first async pass.)
//!
//! Build: `cargo build --target wasm32-wasip1 --release`.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::sandbox::{
    self, computer_is_runnable, ensure_sandbox_awake, normalize_sandbox_provider, SandboxHandle,
};
use wasm_helpers::{bounded_reads, entity_field_str, odata_headers, resolve_temper_api_url};

/// Hard wall-clock limit for an async command, enforced sandbox-side by `timeout`
/// so a runaway is killed even though the WASM never blocks on it. The poll loop's
/// deadline (`deadline_at_ms`, set here) is this plus a margin — computed ONCE, on
/// the row, so the poll never re-derives it (single source of truth).
const MAX_RUN_SECS: u64 = 1800;
/// Extra wall-clock the poll waits past the sandbox-side `timeout` before it gives
/// up — covers the poll cadence and a slow final rc write.
const POLL_DEADLINE_MARGIN_MS: i64 = 60_000;
/// Bytes of stdout/stderr tail captured on the box (must match the poll's read
/// bound). The wrapper truncates to this at the source, so the poll never pulls a
/// full body — a huge output cannot OOM the module.
const OUTPUT_TAIL_BYTES: usize = 262_144;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let computer_id =
            field(&fields, "computer_id").ok_or("computer_exec_start: missing computer_id")?;
        let command = field(&fields, "command").ok_or("computer_exec_start: missing command")?;

        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let computer = fetch_computer(&ctx, &temper_api_url, &fields, &computer_id)?;
        let handle = handle_from_computer(&computer)
            .map_err(|e| format!("computer_exec_start: computer {computer_id}: {e}"))?;
        let status = entity_field_str(&computer, &["Status", "status"]).unwrap_or("");
        ensure_sandbox_awake(&ctx, &handle, status)
            .map_err(|e| format!("computer_exec_start: resume {computer_id} from {status}: {e}"))?;

        let wrapped = format!(
            "timeout -k 5s {MAX_RUN_SECS}s bash -c {}",
            shell_single_quote(&command)
        );
        // Deterministic, retry-stable run_id (one exec per Exec row): a re-fired
        // start reuses it and the idempotent launch dedups, so a retry cannot leave
        // a duplicate orphan process.
        let run_id = sandbox::deterministic_run_id(&ctx.entity_id);
        sandbox::sandbox_exec_start(&ctx, &handle, &wrapped, &run_id, OUTPUT_TAIL_BYTES)?;
        let now_ms = Context::get_time_millis();
        let deadline_at_ms = now_ms + (MAX_RUN_SECS as i64) * 1000 + POLL_DEADLINE_MARGIN_MS;

        ctx.log(
            "info",
            &format!(
                "computer_exec_start: started {} on {computer_id} (run_id {run_id})",
                ctx.entity_id
            ),
        );

        set_success_result(
            "ExecStarted",
            &json!({
                "run_id": run_id,
                "started_at_ms": now_ms.to_string(),
                "deadline_at_ms": deadline_at_ms.to_string(),
            }),
        );
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn field(fields: &Value, key: &str) -> Option<String> {
    entity_field_str(fields, &[key])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

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
    bounded_reads::get_json(ctx, temper_api_url, &path, &headers, "computer_exec_start")
}

/// Build a SandboxHandle from a Computer row. Fails CLOSED: only a runnable
/// computer with a sandbox_url is exec-able. Runnable = Ready (source),
/// Leased (governed copy), or Sleeping (source that will be resumed before
/// the command starts). Every other state is refused.
fn handle_from_computer(computer: &Value) -> Result<SandboxHandle, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_requires_live_computer() {
        // Ready (a source) and Leased (a governed copy) are both exec-able.
        let ready = json!({"Status": "Ready", "fields": {"sandbox_url": "https://x.sandbox.tensorlake.ai", "machine_id": "x"}});
        assert!(handle_from_computer(&ready).is_ok());
        let leased = json!({"Status": "Leased", "fields": {"sandbox_url": "https://x.sandbox.tensorlake.ai", "machine_id": "x"}});
        assert!(handle_from_computer(&leased).is_ok());
        let sleeping = json!({"Status": "Sleeping", "fields": {"sandbox_url": "https://x.sandbox.tensorlake.ai", "machine_id": "x"}});
        assert!(handle_from_computer(&sleeping).is_ok());
        // Any non-runnable state fails closed.
        for st in ["Created", "Provisioning", "Terminating", "Destroyed"] {
            let c =
                json!({"Status": st, "fields": {"sandbox_url": "https://x.sandbox.tensorlake.ai"}});
            assert!(
                handle_from_computer(&c).is_err(),
                "{st} must not be exec-able"
            );
        }
    }

    #[test]
    fn quotes_command_for_bash() {
        assert_eq!(shell_single_quote("echo 'hi'"), r"'echo '\''hi'\'''");
    }
}
