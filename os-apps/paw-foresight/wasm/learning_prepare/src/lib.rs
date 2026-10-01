use foresight_learning_core::{Example, MAX_EXAMPLES, prepare};
use foresight_learning_io::{field, get_json, model, safe_id};
use temper_wasm_sdk::prelude::*;
fn execute(ctx: &Context) -> Result<(), String> {
    let fields = &ctx.entity_state;
    let world_id = safe_id(field(fields, "world_id"))?;
    let mode = field(fields, "mode");
    let as_of = field(fields, "as_of");
    let world = get_json(ctx, &format!("Worlds('{world_id}')"))?;
    let world_mode = match field(&world, "learning_mode") {
        "" => "observed",
        other => other,
    };
    if mode != world_mode {
        return Err("Learning mode must equal the world's immutable learning_mode".into());
    }
    let expected = field(&world, "model_json");
    let incumbent = model(expected, mode)?;
    let raw = field(fields, "dataset_json");
    if raw.len() > 1_000_000 {
        return Err("dataset exceeds 1 MB".into());
    }
    let data: Vec<Example> = if raw.trim().is_empty() || raw.trim() == "[]" {
        let rows = get_json(
            ctx,
            &format!(
                "Forecasts?$filter=world_id eq '{world_id}'&$top={}",
                MAX_EXAMPLES + 1
            ),
        )?;
        if rows.get("@odata.nextLink").is_some() {
            return Err("forecast history exceeds query bound; cannot silently truncate".into());
        }
        let rows = rows
            .get("value")
            .and_then(Value::as_array)
            .ok_or("forecast response missing value array")?;
        if rows.len() > MAX_EXAMPLES {
            return Err("forecast history exceeds 512 rows; no truncation permitted".into());
        }
        rows.iter()
            .filter(|r| matches!(field(r, "status"), "Scored" | "Resolved"))
            .map(|row| {
                let outcome = match field(row, "outcome") {
                    "yes" => 1,
                    "no" => 0,
                    _ => 2,
                };
                let probability = field(row, "base_probability").parse().unwrap_or(f64::NAN);
                let source_refs =
                    serde_json::from_str(field(row, "outcome_source_refs")).unwrap_or_default();
                Example {
                    event_id: field(row, "event_node_id").into(),
                    base_probability: probability,
                    outcome,
                    registered_at: field(row, "registered_at").into(),
                    resolved_at: field(row, "resolved_at").into(),
                    evidence_kind: match field(row, "outcome_evidence_kind") {
                        "" => field(row, "evidence_kind"),
                        kind => kind,
                    }
                    .into(),
                    source_refs,
                }
            })
            .collect()
    } else {
        if mode == "observed" {
            return Err(
                "Observed runs must use recorded Forecast outcomes, not an uploaded dataset".into(),
            );
        }
        serde_json::from_str(raw).map_err(|error| format!("invalid dataset: {error}"))?
    };
    let prepared = prepare(data, mode, as_of, &incumbent)?;
    set_success_result(
        "Prepared",
        &json!({"prepared_json":serde_json::to_string(&prepared).map_err(|e|e.to_string())?,
        "incumbent_model_json":serde_json::to_string(&incumbent).map_err(|e|e.to_string())?,
        "expected_model_json":expected}),
    );
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
