//! computer_wake — resume the Computer's Tensorlake sandbox (ARN-466).
//!
//! Fires on Wake (Sleeping|Ready → Ready). POSTs /sandboxes/{id}/resume.
//! 200 if already running. Fail closed: empty machine_id or a provider
//! error trips WakeFailed (Ready → Sleeping) so Temper does not claim
//! Ready while the box is still suspended.
//!
//! Build: `cargo build --target wasm32-wasip1 --release`.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::entity_field_str;
use wasm_helpers::sandbox::{self, normalize_sandbox_provider};

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let provider = provider_from_fields(&fields);
        let machine_id = entity_field_str(&fields, &["machine_id", "MachineId"])
            .unwrap_or("")
            .to_string();

        if machine_id.is_empty() {
            return Err("computer_wake: no machine_id — cannot resume".to_string());
        }

        sandbox::sandbox_resume(&ctx, &provider, &machine_id)?;
        ctx.log("info", &format!("computer_wake: resumed {machine_id}"));
        set_success_result("", &json!({}));
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

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
    fn provider_defaults_to_tensorlake() {
        assert_eq!(provider_from_fields(&json!({})), "tensorlake");
        assert_eq!(
            provider_from_fields(&json!({"provider": "tl"})),
            "tensorlake"
        );
        assert_eq!(provider_from_fields(&json!({"provider": "modal"})), "modal");
    }
}
