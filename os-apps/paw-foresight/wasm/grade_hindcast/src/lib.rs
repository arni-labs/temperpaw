//! grade_hindcast — score a retrodiction run against recorded reality
//! (ADR-002).
//!
//! On Hindcast.Grade: read the recorded actuals from paw-fs, match each
//! actual to one of the hindcast world's preregistered Forecasts (by
//! event_node_id when given, else by case-insensitive substring on the
//! question), resolve and Brier-score the matches, and dispatch
//! Hindcast.ScoreComplete with the mean Brier and an honest coverage note.
//!
//! Honest limitations, recorded in the coverage note on every run: the
//! anachronism check is not yet implemented, and residual model-prior
//! contamination applies whenever the vantage predates the model's training
//! cutoff.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

// --- Embedding-matched grading (D4, ADR-006) ---------------------------------
//
// An actual is matched to its forecast by event_node_id when given, else by
// nearest embedding of the actual's descriptor to the forecast questions —
// retiring the brittle, ordering-sensitive case-insensitive substring `.find`
// (which silently took the FIRST of several matches). Degrades to substring
// matching when no embedder is reachable, so grading never depends on it.
const MATCH_MAX_DISTANCE: f32 = 0.55;

fn embed_config(ctx: &Context) -> (String, String) {
    let nonempty = |k: &str| {
        ctx.config
            .get(k)
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
    };
    (
        nonempty("embedding_endpoint")
            .unwrap_or_else(|| "http://127.0.0.1:11434/api/embed".to_string()),
        nonempty("embedding_model").unwrap_or_else(|| "mxbai-embed-large".to_string()),
    )
}

fn fetch_embeddings(ctx: &Context, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Some(Vec::new());
    }
    let (endpoint, model) = embed_config(ctx);
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let body = corridor_embed::build_embed_request(&model, texts);
    let r = ctx.http_call("POST", &endpoint, &headers, &body).ok()?;
    if !(200..300).contains(&r.status) {
        ctx.log(
            "warn",
            &format!("grade_hindcast: embedding endpoint {endpoint} HTTP {}; substring fallback", r.status),
        );
        return None;
    }
    let vecs = corridor_embed::parse_embeddings(&r.body);
    if vecs.len() != texts.len() {
        return None;
    }
    Some(vecs)
}

/// Read a string field from an OData row. List/GET rows nest snake_case
/// values under "fields" with lowercase status/entity_id at the top level;
/// some surfaces serve PascalCase top-level properties. Check both.
fn row_str<'a>(row: &'a Value, pascal: &str) -> &'a str {
    fn snake(p: &str) -> String {
        let mut s = String::new();
        for (i, ch) in p.chars().enumerate() {
            if ch.is_uppercase() {
                if i > 0 {
                    s.push('_');
                }
                s.extend(ch.to_lowercase());
            } else {
                s.push(ch);
            }
        }
        s
    }
    let s = snake(pascal);
    if let Some(v) = row
        .get("fields")
        .and_then(|f| f.get(s.as_str()))
        .and_then(|v| v.as_str())
    {
        return v;
    }
    if let Some(v) = row.get(pascal).and_then(|v| v.as_str()) {
        return v;
    }
    // List rows also carry lowercase top-level keys (status, entity_id).
    row.get(s.as_str()).and_then(|v| v.as_str()).unwrap_or("")
}

fn row_status(row: &Value) -> &str {
    row.get("status")
        .or_else(|| row.get("Status"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn row_id(row: &Value) -> &str {
    row.get("entity_id")
        .or_else(|| row.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

// --- Pure grading logic -----------------------------------------------------

/// One recorded ground-truth outcome from the actuals file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Actual {
    event_node_id: String,
    question_contains: String,
    outcome: String, // "yes" | "no"
}

/// One of the world's forecasts, reduced to what grading needs.
#[derive(Debug, Clone, PartialEq)]
struct ForecastRow {
    id: String,
    event_node_id: String,
    question: String,
    probability: String,
    status: String,
}

/// Parse the actuals file: a JSON array of objects with an optional
/// "event_node_id", an optional "question_contains", and a required
/// "outcome" of "yes" or "no". Entries without a valid outcome are dropped.
fn parse_actuals(body: &str) -> Result<Vec<Actual>, String> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| format!("actuals file is not valid JSON: {e}"))?;
    let arr = parsed
        .as_array()
        .ok_or("actuals file must be a JSON array")?;
    Ok(arr
        .iter()
        .filter_map(|entry| {
            let str_of = |k: &str| {
                entry
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let outcome = str_of("outcome");
            if outcome != "yes" && outcome != "no" {
                return None;
            }
            Some(Actual {
                event_node_id: str_of("event_node_id"),
                question_contains: str_of("question_contains"),
                outcome,
            })
        })
        .collect())
}

/// Parse a Forecasts list response into rows. Field values live in the
/// envelope (`fields.snake_case`) on live servers — the first hindcast
/// grading run matched 0/18 because this read PascalCase top-level only,
/// so every question was empty. row_str checks all observed shapes.
fn parse_forecast_rows(fbody: &Value) -> Vec<ForecastRow> {
    fbody
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            let id = row_id(f).to_string();
            if id.is_empty() {
                return None;
            }
            Some(ForecastRow {
                id,
                event_node_id: row_str(f, "EventNodeId").to_string(),
                question: row_str(f, "Question").to_string(),
                probability: row_str(f, "Probability").to_string(),
                status: row_status(f).to_string(),
            })
        })
        .collect()
}

/// Substring fallback (no embedder): exact event_node_id, else the first
/// case-insensitive substring match. Returns the forecast INDEX.
fn match_index_substring(actual: &Actual, forecasts: &[ForecastRow]) -> Option<usize> {
    if !actual.event_node_id.is_empty() {
        return forecasts
            .iter()
            .position(|f| f.event_node_id == actual.event_node_id);
    }
    if actual.question_contains.is_empty() {
        return None;
    }
    let needle = actual.question_contains.to_lowercase();
    forecasts
        .iter()
        .position(|f| f.question.to_lowercase().contains(&needle))
}

/// Resolve every actual to a forecast index (D4): event_node_id exact first,
/// then nearest-embedding of the actual's descriptor to the forecast questions
/// (deterministic — retires the order-sensitive substring `.find`), then the
/// substring fallback when the embedder is unreachable. Logs each embedding
/// match with its distance for calibration.
fn resolve_matches(
    ctx: &Context,
    actuals: &[Actual],
    forecasts: &[ForecastRow],
) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; actuals.len()];
    let mut need: Vec<usize> = Vec::new();
    for (i, a) in actuals.iter().enumerate() {
        if !a.event_node_id.is_empty() {
            out[i] = forecasts.iter().position(|f| f.event_node_id == a.event_node_id);
        } else if !a.question_contains.is_empty() {
            need.push(i);
        }
    }
    if need.is_empty() || forecasts.is_empty() {
        return out;
    }

    let questions: Vec<String> = forecasts.iter().map(|f| f.question.clone()).collect();
    let needles: Vec<String> = need.iter().map(|&i| actuals[i].question_contains.clone()).collect();
    let mut all = questions.clone();
    all.extend(needles);
    match fetch_embeddings(ctx, &all) {
        Some(vecs) => {
            let qvecs = vecs[..questions.len()].to_vec();
            for (k, &ai) in need.iter().enumerate() {
                let nv = &vecs[questions.len() + k];
                match corridor_embed::nearest(nv, &qvecs) {
                    Some((idx, d)) if d <= MATCH_MAX_DISTANCE => {
                        ctx.log(
                            "info",
                            &format!(
                                "grade_hindcast: matched actual {:?} -> forecast {} (dist {:.3})",
                                actuals[ai].question_contains, forecasts[idx].id, d
                            ),
                        );
                        out[ai] = Some(idx);
                    }
                    // Nearest too far: fall back to substring (may still hit).
                    _ => out[ai] = match_index_substring(&actuals[ai], forecasts),
                }
            }
        }
        None => {
            for &ai in &need {
                out[ai] = match_index_substring(&actuals[ai], forecasts);
            }
        }
    }
    out
}

/// Brier score for a registered probability against a yes/no outcome
/// (yes = 1.0, no = 0.0).
fn brier(probability: f64, outcome_yes: bool) -> f64 {
    let outcome = if outcome_yes { 1.0 } else { 0.0 };
    (probability - outcome).powi(2)
}

fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

/// The ScoreComplete params, honest about coverage. graded 0/n is a valid
/// score report, not an error — silence would hide the gap.
fn score_complete_params(briers: &[f64], total_forecasts: usize, actuals_file_id: &str) -> Value {
    let mean_brier = mean(briers)
        .map(|m| format!("{m:.4}"))
        .unwrap_or_default();
    json!({
        "mean_brier": mean_brier,
        "graded_count": briers.len().to_string(),
        "actuals_file_id": actuals_file_id,
        "coverage_note": format!(
            "graded {}/{} forecasts; anachronism check not yet implemented; residual \
             model-prior contamination applies (vantage vs training cutoff)",
            briers.len(),
            total_forecasts
        ),
    })
}

// --- Entry point -------------------------------------------------------------

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        grade(&ctx)
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn grade(ctx: &Context) -> Result<(), String> {
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let get = |k: &str| -> String {
        fields
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let world_id = get("world_id");
    if world_id.is_empty() {
        return Err("Hindcast.world_id is required for grading".to_string());
    }
    let actuals_file_id = get("actuals_file_id");
    if actuals_file_id.is_empty() {
        return Err("Hindcast.actuals_file_id is required: nothing to grade against".to_string());
    }

    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ];

    // 1. Read the recorded actuals.
    let actuals_url = format!("{api}/tdata/Files('{actuals_file_id}')/$value");
    let actuals_resp = ctx.http_call("GET", &actuals_url, &headers, "")?;
    if actuals_resp.status < 200 || actuals_resp.status >= 300 {
        return Err(format!(
            "failed to read actuals file {actuals_file_id} (HTTP {})",
            actuals_resp.status
        ));
    }
    let actuals = parse_actuals(&actuals_resp.body)?;

    // 2. The hindcast world's forecasts.
    let forecasts_url = format!("{api}/tdata/Forecasts?$filter=world_id eq '{world_id}'");
    let fresp = ctx.http_call("GET", &forecasts_url, &headers, "")?;
    if fresp.status < 200 || fresp.status >= 300 {
        return Err(format!("failed to list Forecasts (HTTP {})", fresp.status));
    }
    let fbody: Value = serde_json::from_str(&fresp.body).unwrap_or(json!({}));
    let forecasts = parse_forecast_rows(&fbody);

    // 3. Match (D4: id-exact -> nearest-embedding -> substring), resolve, score.
    let matches = resolve_matches(ctx, &actuals, &forecasts);
    let outcome_refs = json!([format!("file:{actuals_file_id}")]).to_string();
    let mut briers: Vec<f64> = Vec::new();
    let mut graded_ids: Vec<String> = Vec::new();
    for (ai, actual) in actuals.iter().enumerate() {
        let Some(forecast) = matches[ai].map(|idx| &forecasts[idx]) else {
            ctx.log(
                "info",
                &format!(
                    "grade_hindcast: actual (node={:?}, contains={:?}) matched no forecast",
                    actual.event_node_id, actual.question_contains
                ),
            );
            continue;
        };
        if graded_ids.contains(&forecast.id) {
            ctx.log(
                "info",
                &format!(
                    "grade_hindcast: forecast {} already graded this run; skipping duplicate actual",
                    forecast.id
                ),
            );
            continue;
        }
        if forecast.status != "Preregistered" {
            ctx.log(
                "info",
                &format!(
                    "grade_hindcast: forecast {} is {} (not Preregistered); skipping",
                    forecast.id, forecast.status
                ),
            );
            continue;
        }
        let Ok(probability) = forecast.probability.trim().parse::<f64>() else {
            ctx.log(
                "warn",
                &format!(
                    "grade_hindcast: forecast {} has unparseable probability {:?}; cannot grade",
                    forecast.id, forecast.probability
                ),
            );
            continue;
        };

        let resolve_url = format!("{api}/tdata/Forecasts('{}')/TemperPaw.Resolve", forecast.id);
        let resolve_body = json!({
            "outcome": actual.outcome,
            "outcome_source_refs": outcome_refs,
        });
        match ctx.http_call("POST", &resolve_url, &headers, &resolve_body.to_string()) {
            Ok(r) if (200..300).contains(&r.status) => {}
            Ok(r) => {
                ctx.log(
                    "warn",
                    &format!(
                        "grade_hindcast: Forecast.Resolve on {} failed (HTTP {})",
                        forecast.id, r.status
                    ),
                );
                continue;
            }
            Err(e) => {
                ctx.log(
                    "warn",
                    &format!("grade_hindcast: Forecast.Resolve on {} failed: {e}", forecast.id),
                );
                continue;
            }
        }

        let score = brier(probability, actual.outcome == "yes");
        let score_url = format!("{api}/tdata/Forecasts('{}')/TemperPaw.Score", forecast.id);
        match ctx.http_call(
            "POST",
            &score_url,
            &headers,
            &json!({ "brier": format!("{score:.4}") }).to_string(),
        ) {
            Ok(r) if (200..300).contains(&r.status) => {
                ctx.log(
                    "info",
                    &format!(
                        "grade_hindcast: forecast {} graded — outcome {}, brier {score:.4}",
                        forecast.id, actual.outcome
                    ),
                );
                briers.push(score);
                graded_ids.push(forecast.id.clone());
            }
            Ok(r) => ctx.log(
                "warn",
                &format!(
                    "grade_hindcast: Forecast.Score on {} failed (HTTP {})",
                    forecast.id, r.status
                ),
            ),
            Err(e) => ctx.log(
                "warn",
                &format!("grade_hindcast: Forecast.Score on {} failed: {e}", forecast.id),
            ),
        }
    }

    // 4. Report the score, honest about coverage.
    let params = score_complete_params(&briers, forecasts.len(), &actuals_file_id);
    ctx.log(
        "info",
        &format!(
            "grade_hindcast: hindcast {} — graded {}/{} forecasts from {} actual(s)",
            ctx.entity_id,
            briers.len(),
            forecasts.len(),
            actuals.len()
        ),
    );
    set_success_result("ScoreComplete", &params);
    Ok(())
}
