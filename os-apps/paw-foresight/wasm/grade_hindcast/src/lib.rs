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
            &format!(
                "grade_hindcast: embedding endpoint {endpoint} HTTP {}; substring fallback",
                r.status
            ),
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
    resolved_at: String,
    source_refs: Vec<String>,
}

/// One of the world's forecasts, reduced to what grading needs.
#[derive(Debug, Clone, PartialEq)]
struct ForecastRow {
    id: String,
    event_node_id: String,
    question: String,
    probability: String,
    base_probability: String,
    registered_at: String,
    evidence_kind: String,
    status: String,
}

/// Only an exact, dated event match is eligible as historical learning evidence.
fn resolution_params(actual: &Actual, forecast: &ForecastRow, actuals_file_id: &str) -> Value {
    let exact = !actual.event_node_id.is_empty() && actual.event_node_id == forecast.event_node_id;
    let mut refs = actual.source_refs.clone();
    refs.push(format!("file:{actuals_file_id}"));
    json!({"outcome":actual.outcome,"outcome_source_refs":serde_json::to_string(&refs).unwrap_or_default(),
        "resolved_at":actual.resolved_at,"outcome_evidence_kind":if exact {"historical"} else {"unverified_match"}})
}
/// Use the same evidence boundary as fitting before scheduling a batch run.
fn eligible_feedback_time<'a>(
    actual: &'a Actual,
    forecast: &ForecastRow,
    file_id: &str,
) -> Option<&'a str> {
    use foresight_learning_core::{Example, Model, prepare};
    if actual.event_node_id.is_empty()
        || actual.event_node_id != forecast.event_node_id
        || forecast.evidence_kind != "historical"
    {
        return None;
    }
    let mut refs = actual.source_refs.clone();
    refs.push(format!("file:{file_id}"));
    let event = Example {
        event_id: forecast.event_node_id.clone(),
        base_probability: forecast.base_probability.parse().ok()?,
        outcome: match actual.outcome.as_str() {
            "yes" => 1,
            "no" => 0,
            _ => return None,
        },
        registered_at: forecast.registered_at.clone(),
        resolved_at: actual.resolved_at.clone(),
        evidence_kind: "historical".into(),
        source_refs: refs,
    };
    let prepared = prepare(
        vec![event],
        "historical",
        &actual.resolved_at,
        &Model::identity("historical"),
    )
    .ok()?;
    (prepared.validation.len() == 1).then_some(actual.resolved_at.as_str())
}

fn matched_revisions<'a>(
    matched: &ForecastRow,
    forecasts: &'a [ForecastRow],
) -> Vec<&'a ForecastRow> {
    forecasts
        .iter()
        .filter(|f| {
            if matched.event_node_id.is_empty() {
                f.id == matched.id
            } else {
                f.event_node_id == matched.event_node_id
            }
        })
        .collect()
}

/// Parse the actuals file: a JSON array of objects with an optional
/// "event_node_id", an optional "question_contains", and a required
/// "outcome" of "yes" or "no". Entries without a valid outcome are dropped.
fn parse_actuals(body: &str) -> Result<Vec<Actual>, String> {
    let parsed: Value =
        serde_json::from_str(body).map_err(|e| format!("actuals file is not valid JSON: {e}"))?;
    let arr = parsed
        .as_array()
        .ok_or("actuals file must be a JSON array")?;
    if arr.len() > 512 || body.len() > 1_000_000 {
        return Err("Actuals exceed the bounded 512-entry / 1MB input".into());
    }
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
                resolved_at: str_of("resolved_at"),
                source_refs: entry
                    .get("source_refs")
                    .and_then(Value::as_array)
                    .map(|refs| {
                        refs.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
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
                base_probability: row_str(f, "BaseProbability").to_string(),
                registered_at: row_str(f, "RegisteredAt").to_string(),
                evidence_kind: row_str(f, "EvidenceKind").to_string(),
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
            out[i] = forecasts
                .iter()
                .position(|f| f.event_node_id == a.event_node_id);
        } else if !a.question_contains.is_empty() {
            need.push(i);
        }
    }
    if need.is_empty() || forecasts.is_empty() {
        return out;
    }

    let questions: Vec<String> = forecasts.iter().map(|f| f.question.clone()).collect();
    let needles: Vec<String> = need
        .iter()
        .map(|&i| actuals[i].question_contains.clone())
        .collect();
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
    let mean_brier = mean(briers).map(|m| format!("{m:.4}")).unwrap_or_default();
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
    let forecasts_url = format!("{api}/tdata/Forecasts?$filter=world_id eq '{world_id}'&$top=513");
    let fresp = ctx.http_call("GET", &forecasts_url, &headers, "")?;
    if fresp.status < 200 || fresp.status >= 300 {
        return Err(format!("failed to list Forecasts (HTTP {})", fresp.status));
    }
    let fbody: Value = serde_json::from_str(&fresp.body).unwrap_or(json!({}));
    let forecasts = parse_forecast_rows(&fbody);
    if forecasts.len() > 512 || fbody.get("@odata.nextLink").is_some() {
        return Err("Forecast history exceeds 512 rows; refusing partial grading".into());
    }

    // 3. Match (D4: id-exact -> nearest-embedding -> substring), resolve, score.
    let matches = resolve_matches(ctx, &actuals, &forecasts);
    let mut briers: Vec<f64> = Vec::new();
    let mut graded_ids: Vec<String> = Vec::new();
    let mut learning_as_of = String::new();
    for (ai, actual) in actuals.iter().enumerate() {
        let Some(matched) = matches[ai].map(|idx| &forecasts[idx]) else {
            ctx.log(
                "info",
                &format!(
                    "grade_hindcast: actual (node={:?}, contains={:?}) matched no forecast",
                    actual.event_node_id, actual.question_contains
                ),
            );
            continue;
        };
        for forecast in matched_revisions(matched, &forecasts) {
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
            let resolve_body = resolution_params(actual, forecast, &actuals_file_id);
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
                        &format!(
                            "grade_hindcast: Forecast.Resolve on {} failed: {e}",
                            forecast.id
                        ),
                    );
                    continue;
                }
            }

            let score = brier(probability, actual.outcome == "yes");
            let score_url = format!(
                "{api}/tdata/Forecasts('{}')/TemperPaw.ScoreBatch",
                forecast.id
            );
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
                    if let Some(resolved) =
                        eligible_feedback_time(actual, forecast, &actuals_file_id)
                        && resolved > learning_as_of.as_str()
                    {
                        learning_as_of = resolved.to_string();
                    }
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
                    &format!(
                        "grade_hindcast: Forecast.Score on {} failed: {e}",
                        forecast.id
                    ),
                ),
            }
        }
    }

    // 4. Report the score, honest about coverage.
    let mut params = score_complete_params(&briers, forecasts.len(), &actuals_file_id);
    params["learning_as_of"] = json!(learning_as_of);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn forecast(id: &str, node: &str, question: &str, p: &str, status: &str) -> ForecastRow {
        ForecastRow {
            id: id.to_string(),
            event_node_id: node.to_string(),
            question: question.to_string(),
            probability: p.to_string(),
            base_probability: String::new(),
            registered_at: String::new(),
            evidence_kind: String::new(),
            status: status.to_string(),
        }
    }

    fn actual(node: &str, contains: &str, outcome: &str) -> Actual {
        Actual {
            event_node_id: node.to_string(),
            question_contains: contains.to_string(),
            outcome: outcome.to_string(),
            resolved_at: String::new(),
            source_refs: Vec::new(),
        }
    }

    #[test]
    fn learning_metadata_preserves_dated_exact_actuals_and_rejects_approximate_matches() {
        let mut actual = actual("event", "", "yes");
        actual.resolved_at = "2025-03-01T12:00:00Z".into();
        actual.source_refs = vec!["https://example.org/outcome".into()];
        let forecast = forecast("f1", "event", "question", "0.3", "Preregistered");
        let params = resolution_params(&actual, &forecast, "actual-file");
        assert_eq!(params["resolved_at"], actual.resolved_at);
        assert_eq!(params["outcome_evidence_kind"], "historical");
        assert!(
            params["outcome_source_refs"]
                .as_str()
                .unwrap()
                .contains("https://example.org/outcome")
        );
        actual.event_node_id.clear();
        assert_eq!(
            resolution_params(&actual, &forecast, "actual-file")["outcome_evidence_kind"],
            "unverified_match"
        );
    }
    #[test]
    fn every_revision_of_matched_event_is_graded() {
        let forecasts = vec![
            forecast("f1", "event", "q", "0.3", "Preregistered"),
            forecast("f2", "event", "q", "0.6", "Preregistered"),
            forecast("f3", "other", "q", "0.3", "Preregistered"),
        ];
        assert_eq!(
            matched_revisions(&forecasts[0], &forecasts)
                .iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>(),
            vec!["f1", "f2"]
        );
    }

    #[test]
    fn matching_prefers_the_event_node_id() {
        let forecasts = vec![
            forecast("f-1", "n-1", "Will rates fall?", "0.6", "Preregistered"),
            forecast("f-2", "n-2", "Will rates rise?", "0.4", "Preregistered"),
        ];
        let m = &forecasts[match_index_substring(&actual("n-2", "", "yes"), &forecasts).unwrap()];
        assert_eq!(m.id, "f-2");
        // An id match is exact even when a substring would hit another row.
        let m = &forecasts
            [match_index_substring(&actual("n-1", "rates rise", "yes"), &forecasts).unwrap()];
        assert_eq!(m.id, "f-1");
    }

    #[test]
    fn matching_falls_back_to_case_insensitive_substring() {
        let forecasts = vec![
            forecast(
                "f-1",
                "n-1",
                "Will GPT-6 ship before July?",
                "0.7",
                "Preregistered",
            ),
            forecast("f-2", "n-2", "Will rates rise?", "0.4", "Preregistered"),
        ];
        let m = &forecasts
            [match_index_substring(&actual("", "gpt-6 SHIP", "yes"), &forecasts).unwrap()];
        assert_eq!(m.id, "f-1");
    }

    #[test]
    fn unmatched_actuals_match_nothing() {
        let forecasts = vec![forecast(
            "f-1",
            "n-1",
            "Will rates fall?",
            "0.6",
            "Preregistered",
        )];
        assert!(match_index_substring(&actual("n-9", "", "yes"), &forecasts).is_none());
        assert!(match_index_substring(&actual("", "quantum", "no"), &forecasts).is_none());
        // Neither key given: nothing to match on.
        assert!(match_index_substring(&actual("", "", "yes"), &forecasts).is_none());
    }

    #[test]
    fn brier_and_mean_compute_squared_error() {
        assert_eq!(brier(0.7, true), (0.7f64 - 1.0).powi(2));
        assert_eq!(brier(0.7, false), 0.7f64.powi(2));
        let m = mean(&[0.09, 0.49]).unwrap();
        assert!((m - 0.29).abs() < 1e-12);
        assert_eq!(mean(&[]), None);
    }

    #[test]
    fn empty_actuals_still_report_honestly() {
        assert_eq!(parse_actuals("[]").unwrap(), Vec::<Actual>::new());
        let params = score_complete_params(&[], 7, "file-9");
        assert_eq!(params["graded_count"], "0");
        assert_eq!(params["mean_brier"], "");
        assert_eq!(params["actuals_file_id"], "file-9");
        let note = params["coverage_note"].as_str().unwrap();
        assert!(note.contains("graded 0/7 forecasts"));
        assert!(note.contains("anachronism check not yet implemented"));
        assert!(note.contains("model-prior contamination"));
    }

    #[test]
    fn actuals_parsing_is_strict_about_shape_and_outcomes() {
        assert!(parse_actuals("not json").is_err());
        assert!(parse_actuals("{\"outcome\": \"yes\"}").is_err()); // not an array
        // Invalid outcomes are dropped, valid ones kept.
        let parsed = parse_actuals(
            r#"[
                {"event_node_id": "n-1", "outcome": "yes"},
                {"question_contains": "rates", "outcome": "maybe"},
                {"question_contains": "rates", "outcome": "no"}
            ]"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], actual("n-1", "", "yes"));
        assert_eq!(parsed[1], actual("", "rates", "no"));
    }

    #[test]
    fn score_complete_params_carry_the_four_decimal_mean() {
        let params = score_complete_params(&[0.25, 0.0625], 4, "file-1");
        assert_eq!(params["mean_brier"], "0.1562"); // (0.25+0.0625)/2 = 0.15625
        assert_eq!(params["graded_count"], "2");
        assert!(
            params["coverage_note"]
                .as_str()
                .unwrap()
                .contains("graded 2/4 forecasts")
        );
    }
    #[test]
    fn forecast_rows_parse_from_the_live_envelope_shape() {
        // The shape a live /tdata list actually returns — fields nested,
        // snake_case. The 0/18 grading run is pinned here: a parser that
        // reads top-level PascalCase produces empty questions and every
        // substring match fails silently.
        let body = json!({"value": [{
            "entity_id": "fc-1",
            "status": "Registered",
            "fields": {
                "event_node_id": "n-7",
                "question": "By 2025-09-30, Cursor converts its spring momentum",
                "probability": "0.64"
            }
        }]});
        let rows = parse_forecast_rows(&body);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].question,
            "By 2025-09-30, Cursor converts its spring momentum"
        );
        assert_eq!(rows[0].event_node_id, "n-7");
        assert_eq!(rows[0].probability, "0.64");
        assert_eq!(rows[0].status, "Registered");
        let m = match_index_substring(&actual("", "cursor", "yes"), &rows);
        assert!(m.is_some(), "envelope-shaped rows must be matchable");
    }

    #[test]
    fn row_readers_handle_both_odata_shapes() {
        let nested = json!({"entity_id": "e-1", "status": "Scored",
            "fields": {"repair_cost": "17.50", "world_id": "w-1"}});
        assert_eq!(row_id(&nested), "e-1");
        assert_eq!(row_status(&nested), "Scored");
        assert_eq!(row_str(&nested, "RepairCost"), "17.50");
        assert_eq!(row_str(&nested, "Status"), "Scored"); // lowercase top-level
        let pascal = json!({"Id": "e-2", "Status": "Tail", "RepairCost": "50.00"});
        assert_eq!(row_id(&pascal), "e-2");
        assert_eq!(row_status(&pascal), "Tail");
        assert_eq!(row_str(&pascal, "RepairCost"), "50.00");
    }
}
