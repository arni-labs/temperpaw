//! computer_copy_poll — poll an async live-copy for readiness (ARN-443 C).
//!
//! Fires on Copy.CopyPoll (driven by the Copying state_timeout). The copy sandbox
//! already exists (computer_copy_start recorded its machine_id/sandbox_url); this
//! health-checks it:
//! - ready               → CopyComplete (Copying → Leased);
//! - not ready, in budget → report success with an EMPTY callback (no transition);
//!                          the Copying state_timeout (reset_on = ["CopyPoll"])
//!                          re-arms and we poll again;
//! - past the deadline    → set_failure → on_failure = CopyExpired (terminate the
//!                          leaked copy). A transient health-check error before the
//!                          deadline is treated as "not ready yet" (re-arm), so a
//!                          provider blip never tears down a copy that is just slow.
//!
//! Build: `cargo build --target wasm32-wasip1 --release`.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::entity_field_str;
use wasm_helpers::sandbox::{self, SandboxHandle, normalize_sandbox_provider};

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let ctx = match Context::from_host() {
        Ok(c) => c,
        Err(e) => {
            set_error_result(&e);
            return 0;
        }
    };
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

    let now = Context::get_time_millis();
    let deadline_at = entity_field_str(&fields, &["copy_deadline_at_ms", "CopyDeadlineAtMs"])
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let past_deadline = deadline_at != 0 && now > deadline_at;

    let handle = match handle_from_fields(&fields) {
        Ok(h) => h,
        Err(e) => {
            // Should not happen (CopyStarted set these) — treat as terminal.
            set_error_result(&format!("computer_copy_poll: {e}"));
            return 0;
        }
    };

    match sandbox::sandbox_health_check(&ctx, &handle) {
        Ok(true) => {
            ctx.log("info", &format!("computer_copy_poll: copy {} is ready", handle.sandbox_id));
            set_success_result("CopyComplete", &json!({}));
        }
        Ok(false) => {
            if past_deadline {
                set_error_result("copy never became ready before its deadline");
            } else {
                set_success_result("", &json!({})); // still booting — re-arm
            }
        }
        Err(e) => {
            // Transient provider error: retry until the deadline, then give up
            // (CopyExpired terminates the leaked copy).
            if past_deadline {
                set_error_result(&format!("copy readiness unknown at deadline: {e}"));
            } else {
                set_success_result("", &json!({}));
            }
        }
    }
    0
}

/// Build the copy's sandbox handle from the row (machine_id + sandbox_url are the
/// COPY's, set by computer_copy_start at CopyStarted).
fn handle_from_fields(fields: &Value) -> Result<SandboxHandle, String> {
    let sandbox_url = entity_field_str(fields, &["sandbox_url", "SandboxUrl"])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("no sandbox_url on the copy")?
        .to_string();
    let sandbox_id = entity_field_str(fields, &["machine_id", "MachineId"])
        .filter(|s| !s.is_empty())
        .ok_or("no machine_id on the copy")?
        .to_string();
    let provider = entity_field_str(fields, &["provider", "Provider"])
        .filter(|s| !s.is_empty())
        .map(normalize_sandbox_provider)
        .unwrap_or_else(|| "tensorlake".to_string());
    Ok(SandboxHandle {
        sandbox_url,
        sandbox_id,
        provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_needs_url_and_id() {
        assert!(handle_from_fields(&json!({"sandbox_url": "https://x", "machine_id": "m"})).is_ok());
        assert!(handle_from_fields(&json!({"machine_id": "m"})).is_err());
        assert!(handle_from_fields(&json!({"sandbox_url": "https://x"})).is_err());
    }
}
