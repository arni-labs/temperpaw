use sha2::{Digest, Sha256};
use temper_wasm_sdk::prelude::*;
mod core {
    include!("../../semantic_core.rs");
}
fn provider_error(status: u16, body: &str, secret: &str) -> String {
    let mut message = format!("Semantic provider HTTP {status}");
    if body.len() <= 128 * 1024
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
    {
        let detail = value["detail"]["message"]
            .as_str()
            .or_else(|| value["detail"]["reason"].as_str())
            .or_else(|| value["error"]["message"].as_str())
            .or_else(|| value["message"].as_str())
            .or_else(|| value["detail"].as_str())
            .or_else(|| value["error"].as_str());
        if let Some(code) = value["detail"]["error_type"].as_str() {
            message.push_str(": ");
            message.extend(
                if secret.is_empty() {
                    code.to_owned()
                } else {
                    code.replace(secret, "[redacted]")
                }
                .chars()
                .take(128),
            );
        }
        if let Some(detail) = detail {
            let safe = if secret.is_empty() {
                detail.to_owned()
            } else {
                detail.replace(secret, "[redacted]")
            };
            message.push_str(": ");
            message.extend(safe.chars().take(1024));
        }
    }
    message
}

fn is_token_overflow(status: u16, body: &str) -> bool {
    status == 400
        && body.len() <= 128 * 1024
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .is_some_and(|v| v["detail"]["error_type"] == "max_tokens_exceeded")
}

// A successfully parsed but invalid typed answer may be retried twice, never repaired.
fn validation_retry(program: &mut serde_json::Value) -> bool {
    let failures = program["validation_failures"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    program["validation_failures"] = json!(failures);
    failures <= 2
}
fn transient_http_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 520 | 522 | 524)
}
// Retries are checkpointed, counted attempts at the same cursor, not replacement results.
fn transient_retry(program: &mut serde_json::Value) -> bool {
    let failures = program["transient_provider_failures"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    program["transient_provider_failures"] = json!(failures);
    failures <= 2
}

fn safe_rejected_response(response: &serde_json::Value, secret: &str) -> serde_json::Value {
    let typed = json!({"model":response["model"],"answers":response["answers"]});
    let encoded = typed.to_string();
    let safe = if secret.is_empty() {
        encoded
    } else {
        encoded.replace(secret, "[redacted]")
    };
    // Valid provider bodies are already bounded to 128 KiB. Never persist arbitrary body text.
    serde_json::from_str(&safe).unwrap_or(json!({"error":"response redaction failed"}))
}

// Usage belongs to the HTTP request, not each answer in its fanout.
// Keep only documented token counts; absent/null usage is not a zero measurement.
fn record_provider_usage(
    entry: &mut serde_json::Value,
    response: &serde_json::Value,
    answer_offset: usize,
) {
    if answer_offset != 0 {
        return;
    }
    let mut usage = serde_json::Map::new();
    for key in ["input_tokens", "output_tokens"] {
        if let Some(count) = response["usage"][key].as_u64() {
            usage.insert(key.into(), json!(count));
        }
    }
    if !usage.is_empty() {
        entry["providerUsage"] = serde_json::Value::Object(usage);
    }
}

fn call(ctx: &Context) -> Result<(), String> {
    let mut p = core::parse(core::field(&ctx.entity_state, "program_json"))?;
    let mut trace = core::parse(core::field(&ctx.entity_state, "trace_json"))?;
    let snapshot = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
    let key = ctx
        .config
        .get("typesafe_api_key")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("Configure foresight_typesafe_api_key in Temper settings")?;
    // Record each question separately and checkpoint one independent HTTP batch.
    // The next invocation rebuilds from recorded answers before dependent estimates.
    let trace_bytes = trace.to_string().len();
    'batch: {
        core::skip_nonfuture_tasks(&mut p)?;
        let cursor = p["cursor"].as_u64().ok_or("Missing cursor")? as usize;
        if cursor >= p["tasks"].as_array().ok_or("Missing tasks")?.len() {
            break 'batch;
        }
        if trace.as_array().ok_or("Missing trace")?.len() >= core::call_limit(&p) {
            p["stop_reason"] = json!("call_budget");
            break 'batch;
        }
        // Reserve enough for the bounded response and metadata before spending a call.
        if trace_bytes + 192 * 1024 > core::MAX_TRACE_BYTES {
            p["stop_reason"] = json!("trace_budget");
            break 'batch;
        }
        if let Ok(started) = core::field(&ctx.entity_state, "started_at_ms").parse::<u64>()
            && (Context::get_time_millis() as u64).saturating_sub(started) >= core::time_limit(&p)
        {
            p["stop_reason"] = json!("time_budget");
            break 'batch;
        }
        let batch = core::batch::prepare(
            &snapshot,
            &p,
            core::call_limit(&p).saturating_sub(trace.as_array().unwrap().len()),
        )?;
        let request = &batch.request;
        let task = batch.tasks[0].clone();
        let node = task["nodeId"].as_str().ok_or("Missing node identity")?;
        let function = task["function"].as_str().ok_or("Missing function")?;
        let encoded = request.to_string();
        let started = Context::get_time_millis();
        let http_call = p["http_calls"]
            .as_u64()
            .unwrap_or(trace.as_array().unwrap().len() as u64)
            + 1;
        p["http_calls"] = json!(http_call);
        let mut token_overflow = false;
        let mut transient_error = false;
        let mut rejected_response = None;
        let response_result = (|| -> Result<serde_json::Value, String> {
            let r = ctx
                .http_call(
                    "POST",
                    "https://api.typesafe.ai/v1/systemone",
                    &[
                        ("content-type".into(), "application/json".into()),
                        ("authorization".into(), format!("Bearer {key}")),
                    ],
                    &encoded,
                )
                .map_err(|error| {
                    transient_error = !error.to_ascii_lowercase().contains("authorization denied");
                    "Semantic provider transport failed"
                })?;
            if !(200..300).contains(&r.status) {
                token_overflow = is_token_overflow(r.status, &r.body);
                transient_error = transient_http_status(r.status);
                return Err(provider_error(r.status, &r.body, key));
            }
            if r.body.len() > 128 * 1024 {
                return Err("Provider response exceeds bound".into());
            }
            let response = core::parse(&r.body)?;
            if let Err(error) = core::batch::answers(&batch, &response) {
                rejected_response = Some(response);
                return Err(error);
            }
            Ok(response)
        })();
        let response = match response_result {
            Ok(response) => response,
            Err(error) => {
                let error: String = error
                    .replace(key, "[redacted]")
                    .chars()
                    .take(2048)
                    .collect();
                let index = trace.as_array().unwrap().len();
                trace.as_array_mut().unwrap().push(json!({"index":index,"nodeId":node,"function":function,"requestHash":format!("{:x}",Sha256::digest(encoded.as_bytes())),"startedAtMs":started,"elapsedMs":Context::get_time_millis()-started,"error":error,"requestFormat":"failed-attempt-hash-only","requestBytes":encoded.len(),"taskCount":batch.tasks.len(),"tokenOverflow":token_overflow,"httpCallId":http_call,"task":task,"rejectedResponse":rejected_response.as_ref().map(|v| safe_rejected_response(v, key))}));
                p["stop_reason"] = if transient_error && transient_retry(&mut p) {
                    json!("provider_retry")
                } else if rejected_response.is_some() && validation_retry(&mut p) {
                    json!("validation_retry")
                } else if token_overflow && core::batch::reduce_cap(&mut p, &batch) {
                    json!("batch_repacking")
                } else {
                    json!("provider_error")
                };
                p["last_error"] = json!(error);
                break 'batch;
            }
        };
        p["validation_failures"] = json!(0);
        p["transient_provider_failures"] = json!(0);
        if matches!(
            p["stop_reason"].as_str(),
            Some("validation_retry" | "batch_repacking" | "provider_retry")
        ) {
            p["stop_reason"] = json!("");
        }
        let answers = core::batch::answers(&batch, &response)?;
        let provider_response = response;
        for (offset, (decision, mut evaluation, response)) in answers.into_iter().enumerate() {
            let task = &batch.tasks[offset];
            let node = core::field(task, "nodeId");
            let function = core::field(task, "function");
            let individual = &batch.individual[offset];
            let state = &individual["state"];
            let evidence_ids: Vec<_> = state["source_evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| n["Id"].clone())
                .collect();
            let mut context = json!({"round":p["round"],"world_revision":p["world_revision"],"world_pass":p["world_pass"],"evidence_ids":evidence_ids,"task":task,"branch_state":state["branch_state"]});
            if task["world_id"].is_string() && function != "estimate_likelihood" {
                context["audit_input_fingerprint"] = json!(core::search::audit_input_fingerprint(
                    &snapshot, task, individual
                ));
            }
            if function == "estimate_likelihood" {
                context["probability_comparison"] = core::coherence::receipt(state);
            }
            if let Some(basis) = core::evaluation::prerequisite_input_fingerprint(individual) {
                context["prerequisite_input_fingerprint"] = basis;
            }
            evaluation["context"] = context.clone();
            for key in ["results", "evaluations"] {
                if !p[key].is_object() {
                    p[key] = json!({});
                }
                if !p[key][node].is_object() {
                    p[key][node] = json!({});
                }
            }
            p["results"][node][function] = json!(decision);
            p["evaluations"][node][function] = evaluation.clone();
            p["cursor"] = json!(cursor + offset + 1);
            let index = trace.as_array().unwrap().len();
            let mut entry = json!({"index":index,"nodeId":node,"function":function,"task":task,"depth":task["depth"],"decision":decision,"startedAtMs":started,"elapsedMs":Context::get_time_millis()-started,"httpCallId":http_call,"questionKey":batch.question_key(offset),"requestHash":format!("{:x}",Sha256::digest(encoded.as_bytes())),"caseHash":format!("{:x}",Sha256::digest(individual.to_string().as_bytes())),"requestFormat":"fanout-case-v1","request":{"model":individual["model"],"questions":individual["questions"],"state_ref":{"nodeId":node,"worldId":snapshot["world"]["Id"],"context":context,"branch_state":state["branch_state"],"premise_judgments":state["premise_judgments"],"prerequisiteIds":state["prerequisites"].as_array().into_iter().flatten().map(|v|v["id"].clone()).collect::<Vec<_>>(),"prerequisiteAssessments":state["prerequisites"],"comparisonIds":state["comparisons"].as_array().into_iter().flatten().map(|v|v["Id"].clone()).collect::<Vec<_>>(),"assessment":state["assessment"],"evaluations":state["evaluations"],"context_encoding":state["context_encoding"],"evidence_sets":state["evidence_sets"]}},"response":response,"forecastProbability":evaluation["probability"]});
            record_provider_usage(&mut entry, &provider_response, offset);
            trace.as_array_mut().ok_or("Missing trace")?.push(entry);
        }
    }
    core::endpoints::refresh_changed_candidate_inputs(&snapshot, &mut p)?;
    set_success_result(
        "Recorded",
        &json!({"program_json":p.to_string(),"trace_json":trace.to_string()}),
    );
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host().and_then(|ctx| call(&ctx)) {
        Ok(()) => (),
        Err(e) => set_success_result("Fail", &json!({"error_message":e})),
    };
    0
}

#[cfg(test)]
#[test]
fn provider_errors_preserve_bounded_reason_without_secrets_or_raw_bodies() {
    assert_eq!(
        provider_error(
            400,
            r#"{"error":{"message":"state token limit exceeded"}}"#,
            "secret"
        ),
        "Semantic provider HTTP 400: state token limit exceeded"
    );
    assert!(!provider_error(400, r#"{"message":"bad secret"}"#, "secret").contains("secret"));
    assert_eq!(
        provider_error(400, "<html>raw upstream response</html>", "secret"),
        "Semantic provider HTTP 400"
    );
    let huge = serde_json::json!({"message":"x".repeat(3000)}).to_string();
    assert!(provider_error(400, &huge, "secret").len() < 1100);
}

#[test]
fn structured_token_limit_retains_code_and_redacts_reason() {
    let error = provider_error(
        400,
        r#"{"detail":{"error_type":"max_tokens_exceeded","reason":"state secret exceeds tokens"}}"#,
        "secret",
    );
    assert!(error.contains("max_tokens_exceeded"));
    assert!(error.contains("[redacted]"));
    assert!(!error.contains("secret"));
}

#[test]
fn only_confirmed_token_overflow_is_repackable() {
    let body = r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#;
    assert!(is_token_overflow(400, body));
    assert!(!is_token_overflow(403, body));
    assert!(!is_token_overflow(
        400,
        r#"{"detail":{"error_type":"invalid_request"}}"#
    ));
    assert!(!is_token_overflow(
        400,
        r#"{"message":"max_tokens_exceeded"}"#
    ));
}

#[test]
fn validation_retries_are_bounded_without_mutating_checkpoint() {
    let mut p = json!({"cursor":4,"tasks":["unchanged"],"results":{"prior":0.4}});
    assert!(validation_retry(&mut p));
    assert!(validation_retry(&mut p));
    assert!(!validation_retry(&mut p));
    assert!(!validation_retry(&mut p));
    assert_eq!(p["cursor"], 4);
    assert_eq!(p["results"]["prior"], 0.4);
    let safe = safe_rejected_response(
        &json!({"model":"jev-1.13.0","answers":{"result":{"choice":"secret"}},"unrelated":"omitted"}),
        "secret",
    );
    assert_eq!(safe["answers"]["result"]["choice"], "[redacted]");
    assert!(safe.get("unrelated").is_none());
}

#[cfg(test)]
#[test]
fn transient_failures_retry_twice_but_permissions_billing_and_invalid_requests_do_not() {
    for status in [408, 429, 500, 502, 503, 504, 520, 522, 524] {
        assert!(transient_http_status(status));
    }
    for status in [400, 401, 402, 403, 404, 422] {
        assert!(!transient_http_status(status));
    }
    let mut p = json!({"cursor":17,"http_calls":616});
    assert!(transient_retry(&mut p));
    assert!(transient_retry(&mut p));
    assert!(!transient_retry(&mut p));
    assert_eq!(p["cursor"], 17);
    assert_eq!(p["http_calls"], 616);
}

#[test]
fn successful_fanout_records_usage_once_per_http_request() {
    let response = json!({"usage":{"input_tokens":321,"output_tokens":8,"raw":"do not retain"}});
    let mut trace = Vec::new();
    for http_call in [1, 2] {
        for offset in 0..3 {
            let mut entry = json!({"httpCallId":http_call,"questionKey":format!("q{offset}")});
            record_provider_usage(&mut entry, &response, offset);
            trace.push(entry);
        }
    }
    for http_call in [1, 2] {
        let receipts: Vec<_> = trace
            .iter()
            .filter(|entry| entry["httpCallId"] == http_call)
            .filter_map(|entry| entry.get("providerUsage"))
            .collect();
        assert_eq!(
            receipts,
            vec![&json!({"input_tokens":321,"output_tokens":8})]
        );
    }
    assert_eq!(
        trace
            .iter()
            .filter_map(|entry| entry["providerUsage"]["input_tokens"].as_u64())
            .sum::<u64>(),
        642
    );
}

#[test]
fn missing_or_invalid_usage_is_not_reported_as_zero() {
    for response in [
        json!({}),
        json!({"usage":null}),
        json!({"usage":{"input_tokens":null,"output_tokens":null}}),
        json!({"usage":{"input_tokens":"secret","output_tokens":-1}}),
    ] {
        let mut entry = json!({"httpCallId":1});
        record_provider_usage(&mut entry, &response, 0);
        assert!(entry.get("providerUsage").is_none());
    }
    let mut entry = json!({"httpCallId":1});
    record_provider_usage(&mut entry, &json!({"usage":{"input_tokens":0}}), 0);
    assert_eq!(entry["providerUsage"], json!({"input_tokens":0}));
}

#[test]
fn overflow_without_usage_keeps_error_diagnostics_without_token_counts() {
    let body = r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#;
    assert!(is_token_overflow(400, body));
    assert_eq!(
        provider_error(400, body, "secret"),
        "Semantic provider HTTP 400: max_tokens_exceeded"
    );
    let mut entry = json!({"httpCallId":1,"tokenOverflow":true});
    record_provider_usage(&mut entry, &serde_json::from_str(body).unwrap(), 0);
    assert!(entry.get("providerUsage").is_none());
    assert_eq!(entry["tokenOverflow"], true);
}
