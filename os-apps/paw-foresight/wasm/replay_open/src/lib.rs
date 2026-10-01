use foresight_learning_core::valid_time;
use foresight_learning_io::field;
use temper_wasm_sdk::prelude::*;
fn execute(ctx: &Context) -> Result<(), String> {
    let fields = &ctx.entity_state;
    if !matches!(field(fields, "learning_mode"), "historical" | "simulated") {
        return Err("OpenReplay requires historical or simulated learning_mode".into());
    }
    if !valid_time(field(fields, "last_ingest_date"))
        || !valid_time(field(fields, "frontier_date"))
        || field(fields, "frontier_date") <= field(fields, "last_ingest_date")
    {
        return Err("Replay dates must be UTC seconds with frontier after registration".into());
    }
    if field(fields, "domain").trim().is_empty() {
        return Err("Replay world needs a domain".into());
    }
    set_success_result("ReplayOpened", &json!({}));
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    match Context::from_host() {
        Ok(ctx) => {
            if let Err(error) = execute(&ctx) {
                set_success_result("Fail", &json!({"error_message":error}));
            }
            0
        }
        Err(error) => {
            set_error_result(&error);
            1
        }
    }
}
