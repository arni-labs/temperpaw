use foresight_learning_core::{valid_mode, valid_time};
use foresight_learning_io::field;
use temper_wasm_sdk::prelude::*;
fn execute(ctx: &Context) -> Result<(), String> {
    let fields = &ctx.entity_state;
    let outcome = match field(fields, "outcome") {
        "yes" => 1.0,
        "no" => 0.0,
        _ => return Err("outcome must be yes or no".into()),
    };
    let resolved = field(fields, "resolved_at");
    if !valid_time(resolved)
        || !valid_time(field(fields, "registered_at"))
        || resolved <= field(fields, "registered_at")
    {
        return Err("resolution must be UTC seconds after preregistration".into());
    }
    let probability: f64 = field(fields, "probability")
        .parse()
        .map_err(|_| "probability is not numeric")?;
    if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
        return Err("probability must be finite and between zero and one".into());
    }
    let mode = field(fields, "evidence_kind");
    if !valid_mode(mode) {
        return Err("forecast has no supported evidence provenance".into());
    }
    if mode == "observed" {
        let recorded = ctx
            .entity_state
            .get("events")
            .and_then(Value::as_array)
            .and_then(|events| events.last())
            .and_then(|event| event.get("timestamp"))
            .and_then(Value::as_str)
            .ok_or("Observed resolution needs its host-recorded event timestamp")?;
        let now = if recorded.len() >= 19 {
            format!("{}Z", &recorded[..19])
        } else {
            String::new()
        };
        if !valid_time(&now) || resolved > now.as_str() {
            return Err("An observed outcome cannot resolve in the future".into());
        }
    }
    let sources: Vec<String> = serde_json::from_str(field(fields, "outcome_source_refs"))
        .map_err(|_| "sources must be a JSON array")?;
    if sources.is_empty()
        || sources.len() > 16
        || sources.iter().any(|s| {
            s.is_empty() || s.len() > 2048 || (mode != "simulated" && !s.starts_with("https://"))
        })
    {
        return Err(
            "outcome requires bounded sources; empirical evidence needs HTTPS references".into(),
        );
    }
    let action = if ctx.trigger_action == "ResolveRevision" {
        "RevisionVerified"
    } else {
        "OutcomeVerified"
    };
    set_success_result(
        action,
        &json!({"brier":(probability-outcome).powi(2).to_string(),"error_message":"", "outcome_evidence_kind":mode}),
    );
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    match Context::from_host() {
        Ok(ctx) => {
            if let Err(error) = execute(&ctx) {
                set_success_result("OutcomeFailed", &json!({"error_message":error}));
            }
            0
        }
        Err(error) => {
            set_error_result(&error);
            1
        }
    }
}
