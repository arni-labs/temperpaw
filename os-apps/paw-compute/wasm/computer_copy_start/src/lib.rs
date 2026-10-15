//! Start or reconcile one named provider copy for a child Computer.
//!
//! ProvisionFromCopy may submit a copy request after checking the exact name.
//! ReconcileCopy performs GET-only recovery after an uncertain response. The
//! child retains its source machine ID until a verified destination callback
//! lands. Readiness is then polled by computer_copy_poll in Copying.
//!
//! Build: `cargo build --target wasm32-wasip1 --release`.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::entity_field_str;
use wasm_helpers::sandbox::{self, normalize_sandbox_provider};

/// Wall-clock budget (ms) for the copy to become ready, stamped on the row so the
/// poll reads a single deadline. A live-copy of a real box takes minutes.
const COPY_READY_BUDGET_MS: i64 = 300_000;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let provider = provider_from_fields(&fields);
        let source_machine_id = entity_field_str(&fields, &["machine_id", "MachineId"])
            .filter(|s| !s.is_empty())
            .ok_or("computer_copy_start: no source machine_id to copy from")?
            .to_string();

        let name = sandbox::sandbox_copy_name(&ctx.entity_id, &source_machine_id)?;
        let mode = copy_mode(&ctx.trigger_action)?;
        ctx.log(
            "info",
            &format!("computer_copy_start: {mode:?} for child {}", ctx.entity_id),
        );
        let handle =
            sandbox::sandbox_copy(&ctx, &provider, &source_machine_id, &ctx.entity_id, mode)?;

        let deadline_at_ms = Context::get_time_millis() + COPY_READY_BUDGET_MS;
        set_success_result(
            "CopyStarted",
            &json!({
                "machine_id": handle.sandbox_id,
                "sandbox_url": handle.sandbox_url,
                "source_machine_id": source_machine_id,
                "name": name,
                "copy_deadline_at_ms": deadline_at_ms.to_string(),
            }),
        );
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn copy_mode(action: &str) -> Result<sandbox::CopyMode, String> {
    match action {
        "ProvisionFromCopy" => Ok(sandbox::CopyMode::Start),
        "ReconcileCopy" => Ok(sandbox::CopyMode::Reconcile),
        _ => Err("computer_copy_start requires a native start or reconciliation action".into()),
    }
}

/// The provider recorded on the row, normalized; defaults to tensorlake.
fn provider_from_fields(fields: &Value) -> String {
    entity_field_str(fields, &["provider", "Provider"])
        .filter(|s| !s.is_empty())
        .map(normalize_sandbox_provider)
        .unwrap_or_else(|| "tensorlake".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_defaults_and_normalizes() {
        assert_eq!(provider_from_fields(&json!({})), "tensorlake");
        assert_eq!(
            provider_from_fields(&json!({"provider": "tl"})),
            "tensorlake"
        );
        assert_eq!(provider_from_fields(&json!({"provider": "modal"})), "modal");
    }

    #[test]
    fn copy_names_preserve_the_complete_native_child_id() {
        let first = "01a0778b-09f1-75e3-84d1-2eda36a64f6b";
        let second = "01a0778b-09f1-75e3-84d1-2eda36a64f6c";
        let source = "lw947cgusmggtko7l4mgz";
        assert_eq!(
            sandbox::sandbox_copy_name(first, source).unwrap(),
            format!("copy-{first}-{source}")
        );
        assert_eq!(sandbox::sandbox_copy_name(first, source).unwrap().len(), 63);
        assert_ne!(
            sandbox::sandbox_copy_name(first, source).unwrap(),
            sandbox::sandbox_copy_name(second, source).unwrap()
        );
        assert_ne!(
            sandbox::sandbox_copy_name(first, source).unwrap(),
            sandbox::sandbox_copy_name(first, "different-source").unwrap()
        );
        for invalid in ["", "child/name", "Uppercase", "bad_underscore"] {
            assert!(sandbox::sandbox_copy_name(invalid, source).is_err());
            assert!(sandbox::sandbox_copy_name(first, invalid).is_err());
        }
        assert!(sandbox::sandbox_copy_name(first, &"a".repeat(22)).is_err());
    }

    #[test]
    fn only_native_start_and_reconcile_actions_choose_a_copy_mode() {
        assert_eq!(
            copy_mode("ProvisionFromCopy").unwrap(),
            sandbox::CopyMode::Start
        );
        assert_eq!(
            copy_mode("ReconcileCopy").unwrap(),
            sandbox::CopyMode::Reconcile
        );
        for action in ["", "Copy", "CopyStarted", "Retry"] {
            assert!(copy_mode(action).is_err());
        }
    }
}
