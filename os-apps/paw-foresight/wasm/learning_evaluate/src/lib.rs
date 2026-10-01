use foresight_learning_core::{Prepared, evaluate};
use foresight_learning_io::{field, model};
use temper_wasm_sdk::prelude::*;
fn execute(ctx: &Context) -> Result<(), String> {
    let fields = &ctx.entity_state;
    let prepared: Prepared =
        serde_json::from_str(field(fields, "prepared_json")).map_err(|e| e.to_string())?;
    let mode = field(fields, "mode");
    let incumbent = model(field(fields, "incumbent_model_json"), mode)?;
    let candidate = model(field(fields, "candidate_model_json"), mode)?;
    let report = evaluate(
        &prepared,
        &incumbent,
        &candidate,
        mode,
        field(fields, "as_of"),
    );
    let action = if report["decision"] == "candidate_passed" {
        "CandidatePassed"
    } else {
        "Reject"
    };
    set_success_result(action, &json!({"report_json":report.to_string()}));
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
