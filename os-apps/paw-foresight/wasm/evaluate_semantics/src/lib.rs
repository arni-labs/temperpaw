//! Bounded Jev shadow evaluation. No scoring, pruning, forecast or outcome writes.
use sha2::{Digest, Sha256};
use temper_wasm_sdk::prelude::*;
const MODEL: &str = "jev-1.13.0";
mod packet_contract {
    include!("../../evaluation_packet.rs");
}
const QUESTIONS: [&str; 4] = ["contradiction", "incentive", "lag", "miracle"];
const CAP: usize = 128 * 1024;

fn request(state: Value) -> Result<Value, String> {
    if state.to_string().len() > CAP {
        return Err("snapshot_exceeds_128k_bytes".into());
    }
    let questions: Value = serde_json::from_str(include_str!("../../evaluation_contract.json"))
        .map_err(|_| "invalid_evaluation_contract")?;
    Ok(json!({"model":MODEL,"state":state,"questions":questions}))
}

fn validate(response: &Value) -> Result<(), String> {
    if response["model"].as_str() != Some(MODEL) {
        return Err("model_mismatch".into());
    }
    let answers = response["answers"].as_object().ok_or("missing_answers")?;
    if answers.len() != QUESTIONS.len() {
        return Err("answer_count_mismatch".into());
    }
    for key in QUESTIONS {
        let a = &response["answers"][key];
        if a["type"] != "choice" {
            return Err("wrong_answer_type".into());
        }
        let p = a["probabilities"]
            .as_object()
            .ok_or("missing_distribution")?;
        if p.len() != 3 {
            return Err("invalid_distribution".into());
        }
        let mut sum = 0.0;
        let mut max: f64 = 0.0;
        for choice in ["clear", "defect", "unknown"] {
            let n = p
                .get(choice)
                .and_then(Value::as_f64)
                .ok_or("invalid_probability")?;
            if !n.is_finite() || !(0.0..=1.0).contains(&n) {
                return Err("invalid_probability".into());
            }
            sum += n;
            max = max.max(n);
        }
        // Live Jev responses round probabilities to two decimal places.
        // Three independently rounded options can differ from one by 0.015.
        // Preserve the raw response; scoring normalizes the accepted distribution.
        if (sum - 1.0).abs() > 0.015 + f64::EPSILON {
            return Err("distribution_not_normalized".into());
        }
        let choice = a["choice"].as_str().ok_or("missing_choice")?;
        if p.get(choice).and_then(Value::as_f64) != Some(max) {
            return Err("choice_not_argmax".into());
        }
        let confidence = a["confidence"].as_f64().ok_or("missing_confidence")?;
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err("invalid_confidence".into());
        }
    }
    if response["usage"]["input_tokens"].as_u64().is_none() {
        return Err("missing_usage".into());
    }
    Ok(())
}

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get("fields")
        .unwrap_or(v)
        .get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
}
fn evaluate(ctx: &Context) -> Result<Option<Value>, String> {
    let raw = field(&ctx.entity_state, "challenge_packet_json");
    let packet_hash = field(&ctx.entity_state, "challenge_packet_sha256");
    let packet = packet_contract::checked_packet(raw, packet_hash)?;
    if packet["mode"] == "off" {
        return Ok(None);
    }
    let key = ctx
        .config
        .get("typesafe_api_key")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("missing_typesafe_api_key")?;
    let request = request(packet["state"].clone())?;
    let encoded = request.to_string();
    let hash = format!("{:x}", Sha256::digest(encoded.as_bytes()));
    let started = Context::get_time_millis();
    let response = ctx
        .http_call(
            "POST",
            "https://api.typesafe.ai/v1/systemone",
            &[
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), format!("Bearer {key}")),
            ],
            &encoded,
        )
        .map_err(|_| "provider_transport_error")?;
    if !(200..300).contains(&response.status) {
        return Err(format!("provider_http_{}", response.status));
    }
    if response.body.len() > 32 * 1024 {
        return Err("provider_response_exceeds_limit".into());
    }
    let result: Value =
        serde_json::from_str(&response.body).map_err(|_| "invalid_provider_json")?;
    validate(&result)?;
    let elapsed = Context::get_time_millis() - started;
    Ok(Some(
        json!({"schema_version":"foresight-shadow-v2","packet_sha256":packet_hash,"mode":"shadow",
        "input_sha256":hash,"request":request,"response":result,
        "routing":"existing_critic_unchanged","provider_elapsed_ms":elapsed,"forecast_probability":null}),
    ))
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    match Context::from_host() {
        Ok(ctx) => {
            let record = match evaluate(&ctx) {
                Ok(None) => {
                    set_success_result("Skip", &json!({}));
                    return 0;
                }
                Ok(Some(record)) => record,
                Err(reason) => {
                    set_success_result("Fail", &json!({"error_message":reason}));
                    return 0;
                }
            };
            set_success_result("Record", &json!({"result_json":record.to_string()}));
            0
        }
        Err(error) => {
            set_error_result(&error);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response() -> Value {
        let mut answers = json!({});
        for key in QUESTIONS {
            answers[key] = json!({"type":"choice","choice":"clear",
                "probabilities":{"clear":0.96,"defect":0.02,"unknown":0.02},"confidence":0.9});
        }
        json!({"model":MODEL,"answers":answers,"usage":{"input_tokens":120,"output_tokens":0}})
    }
    #[test]
    fn bounded_snapshot_is_evaluated_with_pinned_model() {
        let r = request(json!({"repair":"a","evidence":[{"statement":"b"}]})).unwrap();
        assert_eq!(r["model"], MODEL);
        assert_eq!(r["questions"].as_object().unwrap().len(), 4);
    }
    #[test]
    fn accepts_complete_distribution() {
        assert!(validate(&response()).is_ok());
    }
    #[test]
    fn accepts_observed_rounding_but_rejects_larger_mass_errors() {
        let mut r = response();
        // Numerical values from the saved live decomposition_batched response;
        // labels are mapped solely to exercise the three-option wire contract.
        r["answers"]["lag"]["probabilities"] = json!({"clear":0.81,"defect":0.10,"unknown":0.08});
        assert!(validate(&r).is_ok());
        r["answers"]["lag"]["probabilities"]["unknown"] = json!(0.06);
        assert!(validate(&r).is_err());
    }
    #[test]
    fn rejects_missing_or_inconsistent_answers() {
        let mut r = response();
        r["answers"].as_object_mut().unwrap().remove("lag");
        assert!(validate(&r).is_err());
        let mut r = response();
        r["answers"]["lag"]["probabilities"]["clear"] = json!(0.2);
        assert!(validate(&r).is_err());
        let mut r = response();
        r["answers"]["lag"]["choice"] = json!("defect");
        assert!(validate(&r).is_err());
    }
    #[test]
    fn rejects_model_drift_and_bad_confidence() {
        let mut r = response();
        r["model"] = json!("jev-latest");
        assert!(validate(&r).is_err());
        let mut r = response();
        r["answers"]["contradiction"]["confidence"] = json!(1.1);
        assert!(validate(&r).is_err());
    }
    #[test]
    fn rejects_oversized_snapshot_instead_of_silently_truncating() {
        assert!(request(json!({"repair":"x".repeat(200_000)})).is_err());
    }
}
