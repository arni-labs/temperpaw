//! register_forecasts — preregister the engine's gradeable word (ADR-002).
//!
//! World serializes requests from accepted paths and explicit replay inputs.
//! Each batch freezes live, probabilistic requirements of Canonical/Tail paths
//! inside the world's frontier. Forecasts are immutable and graded later by
//! evidence_ingest. Changed inputs or an adopted model create linked immutable revisions.
//! Identical requests converge to the same stored identity. Each invocation computes
//! the next registration; declared entity triggers commit it and continue the loop.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use corridor_embed::{build_embed_request, nearest, parse_embeddings};
use foresight_learning_core::Model;
use std::collections::BTreeSet;
use temper_wasm_sdk::prelude::*;

// --- Reconcile / dedup (D2 grounding, ADR-005) -------------------------------
//
// An authored node that merely restates an already-determined fact must not be
// registered as a forecast (it would resolve "yes" for free and poison
// calibration — the G1 "first-party agents grow as rivals" bug). This is a
// CONSERVATIVE, logged backstop: collapse only on a very-near embedding match
// to a determined node, log every decision with its distance, and degrade to
// exact-text matching when no embedder is reachable (never a silent pass). The
// primary G1 fixes are upstream — the web-grounded surveyor capturing the fact
// as determined, and the repairer not minting already-true facts. Embedding
// distance cannot tell "restates a present fact" from "a future change to
// something currently true", so the threshold stays strict to avoid dropping a
// real forecast; it is a tunable prior, calibratable from logged distances.
const RECONCILE_MAX_DISTANCE: f32 = 0.10;

/// Embedding endpoint + model id, from config (secrets) with dev defaults
/// (local Ollama). The model id is informational here but kept for parity with
/// the other consumers, which stamp it on stored vectors.
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

/// Fetch row-aligned embeddings for `texts`, or None if the endpoint is
/// unreachable / returns the wrong count (the caller then degrades to text
/// matching). A wrong-count response is treated as a miss, never silently
/// mapped to the wrong rows.
fn fetch_embeddings(ctx: &Context, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Some(Vec::new());
    }
    let (endpoint, model) = embed_config(ctx);
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let body = build_embed_request(&model, texts);
    let r = ctx.http_call("POST", &endpoint, &headers, &body).ok()?;
    if !(200..300).contains(&r.status) {
        ctx.log(
            "warn",
            &format!(
                "register_forecasts: embedding endpoint {endpoint} returned HTTP {}; \
                 reconcile falls back to exact-text matching",
                r.status
            ),
        );
        return None;
    }
    let vecs = parse_embeddings(&r.body);
    if vecs.len() != texts.len() {
        ctx.log(
            "warn",
            &format!(
                "register_forecasts: embedding count {} != {} requested; reconcile falls back \
                 to exact-text matching",
                vecs.len(),
                texts.len()
            ),
        );
        return None;
    }
    Some(vecs)
}

/// Normalize a statement for exact-text comparison: lowercase, collapse
/// whitespace, drop trailing punctuation. The no-embedder fallback only
/// collapses verbatim restatements — anything looser risks dropping a real
/// forecast.
fn normalize(s: &str) -> String {
    let lowered = s.to_lowercase();
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_end_matches(['.', '!', '?', ' ']).to_string()
}

/// Per candidate, the nearest determined node (index, distance) when it is
/// within `threshold` — i.e. the candidate is covered by that determined fact.
/// Pure over the vectors; the embedding judgment came from the model.
fn covered_by_embedding(
    cand_vecs: &[Vec<f32>],
    det_vecs: &[Vec<f32>],
    threshold: f32,
) -> Vec<Option<(usize, f32)>> {
    cand_vecs
        .iter()
        .map(|c| match nearest(c, det_vecs) {
            Some((i, d)) if d <= threshold => Some((i, d)),
            _ => None,
        })
        .collect()
}

/// The exact-text fallback: a candidate is covered only if its normalized
/// statement equals a determined node's. Returns the matched determined index.
fn covered_by_text(candidates: &[String], determined: &[String]) -> Vec<Option<usize>> {
    let norm_det: Vec<String> = determined.iter().map(|s| normalize(s)).collect();
    candidates
        .iter()
        .map(|c| {
            let nc = normalize(c);
            norm_det.iter().position(|d| !d.is_empty() && *d == nc)
        })
        .collect()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

/// The set of registrable authored node ids that merely restate a determined
/// fact (and so must not become forecasts). Embeds the determined reference and
/// the candidates in one batch and matches with the strict distance threshold;
/// degrades to exact-text matching when no embedder is reachable. Every
/// collapse is logged with its distance and the matched fact.
fn compute_covered(
    ctx: &Context,
    nodes: &[Value],
    frontier: &str,
) -> std::collections::HashSet<String> {
    let mut covered = std::collections::HashSet::new();

    let mut det_stmts: Vec<String> = Vec::new();
    for n in nodes {
        if row_str(n, "Provenance") == "determined" {
            let s = row_str(n, "Statement");
            if !s.is_empty() {
                det_stmts.push(s.to_string());
            }
        }
    }
    if det_stmts.is_empty() {
        return covered; // nothing to reconcile against
    }

    let mut cand_ids: Vec<String> = Vec::new();
    let mut cand_stmts: Vec<String> = Vec::new();
    for n in nodes {
        let id = row_id(n).to_string();
        if id.is_empty() || row_str(n, "Provenance") != "authored" {
            continue;
        }
        if !should_register(
            row_str(n, "Status"),
            row_str(n, "Probability"),
            row_str(n, "Provenance"),
            row_str(n, "ResolveBy"),
            frontier,
        ) {
            continue;
        }
        let s = row_str(n, "Statement");
        if s.is_empty() {
            continue;
        }
        cand_ids.push(id);
        cand_stmts.push(s.to_string());
    }
    if cand_stmts.is_empty() {
        return covered;
    }

    // One batch embed of [determined ++ candidates], split back by length.
    let mut all = det_stmts.clone();
    all.extend(cand_stmts.clone());
    let matched: Vec<Option<(String, f32)>> = match fetch_embeddings(ctx, &all) {
        Some(vecs) => {
            let det_vecs = vecs[..det_stmts.len()].to_vec();
            let cand_vecs = vecs[det_stmts.len()..].to_vec();
            covered_by_embedding(&cand_vecs, &det_vecs, RECONCILE_MAX_DISTANCE)
                .into_iter()
                .map(|m| m.map(|(di, d)| (det_stmts[di].clone(), d)))
                .collect()
        }
        None => covered_by_text(&cand_stmts, &det_stmts)
            .into_iter()
            .map(|m| m.map(|di| (det_stmts[di].clone(), 0.0)))
            .collect(),
    };

    for (ci, m) in matched.into_iter().enumerate() {
        if let Some((det, dist)) = m {
            covered.insert(cand_ids[ci].clone());
            ctx.log(
                "info",
                &format!(
                    "register_forecasts: reconcile — node {} restates a determined fact \
                     (dist {:.3}): \"{}\" ~= \"{}\"; not registering it as a forecast",
                    cand_ids[ci],
                    dist,
                    clip(&cand_stmts[ci], 60),
                    clip(&det, 60)
                ),
            );
        }
    }
    covered
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

/// Engine version stamped on every registration.
const ENGINE_VERSION: &str = "0.3.0";
/// Stable event/input/model identity makes retries converge at the storage boundary.
fn registration_id(key: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("forecast-{hash:016x}")
}
fn registration_time(raw: &str) -> Result<String, String> {
    let value = if raw.len() == 10 {
        format!("{raw}T00:00:00Z")
    } else {
        raw.to_string()
    };
    if foresight_learning_core::valid_time(&value) {
        Ok(value)
    } else {
        Err("Registration requires an as-of UTC date or timestamp".into())
    }
}

/// Date-only deadlines include that UTC day; timestamp deadlines are exact.
/// Use the committed batch clock, never a caller-supplied observed clock.
fn resolves_after_registration(resolve_by: &str, registered_at: &str) -> Result<bool, String> {
    let deadline = if resolve_by.len() == 10 {
        format!("{resolve_by}T23:59:59Z")
    } else {
        resolve_by.to_string()
    };
    if !foresight_learning_core::valid_time(&deadline) {
        return Err("Forecast resolution requires a valid UTC date or timestamp".into());
    }
    Ok(deadline.as_str() > registered_at)
}

/// Host EntityEvent timestamps are UTC; normalize subsecond precision for the learner.
fn host_registration_time(state: &Value) -> Result<String, String> {
    let recorded = state
        .get("events")
        .and_then(Value::as_array)
        .and_then(|events| events.last())
        .and_then(|event| event.get("timestamp"))
        .and_then(Value::as_str)
        .ok_or("Observed registration requires its host-recorded event timestamp")?;
    if !(recorded.ends_with('Z') || recorded.ends_with("+00:00")) {
        return Err("Host registration timestamp must be UTC".into());
    }
    let seconds = recorded
        .get(..19)
        .ok_or("Invalid host registration timestamp")?;
    registration_time(&format!("{seconds}Z"))
}

/// Pure selection rule: is this node a registrable forecast?
///
/// Registrable means: still live (Proposed or Confirmed), genuinely
/// probabilistic (probability parses into (0, 1) exclusive — 1.0 is a
/// determined fact, not a forecast), not skeleton provenance, and resolving
/// on or before the world's frontier date (ISO dates compare
/// lexicographically, so a plain string compare is correct).
fn should_register(
    status: &str,
    probability: &str,
    provenance: &str,
    resolve_by: &str,
    frontier: &str,
) -> bool {
    if status != "Proposed" && status != "Confirmed" {
        return false;
    }
    if provenance == "determined" {
        return false;
    }
    let p = match probability.trim().parse::<f64>() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if !p.is_finite() || p <= 0.0 || p >= 1.0 {
        return false;
    }
    !resolve_by.is_empty() && !frontier.is_empty() && resolve_by <= frontier
}

/// First entry of a source_refs JSON array — the market_ref for market nodes.
fn first_source_ref(source_refs: &str) -> String {
    serde_json::from_str::<Value>(source_refs)
        .ok()
        .and_then(|v| {
            v.as_array()
                .and_then(|arr| arr.first().and_then(|x| x.as_str().map(str::to_string)))
        })
        .unwrap_or_default()
}

/// Only deterministic, completed evaluations release requirements for registration.
fn evaluated_required_nodes(paths: &[Value], world_id: &str) -> Result<BTreeSet<String>, String> {
    let mut nodes = BTreeSet::new();
    for path in paths {
        if row_str(path, "WorldId") != world_id || !matches!(row_status(path), "Canonical" | "Tail")
        {
            continue;
        }
        let required: Vec<String> = serde_json::from_str(row_str(path, "RequiredNodeIds"))
            .map_err(|e| format!("Invalid evaluated path requirements: {e}"))?;
        if required.len() > 512 || required.iter().any(String::is_empty) {
            return Err(
                "Evaluated path requires nonempty event IDs within the 512-event bound".into(),
            );
        }
        nodes.extend(required);
        if nodes.len() > 512 {
            return Err("Evaluated path requirements exceed 512 events".into());
        }
    }
    Ok(nodes)
}

fn list_rows(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    collection: &str,
    world_id: &str,
) -> Result<Vec<Value>, String> {
    let response = ctx.http_call(
        "GET",
        &format!("{api}/tdata/{collection}?$filter=world_id eq '{world_id}'&$top=513"),
        headers,
        "",
    )?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Failed to list {collection}: HTTP {}",
            response.status
        ));
    }
    let body: Value = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
    let rows = body
        .get("value")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{collection} response missing value array"))?;
    if rows.len() > 512 || body.get("@odata.nextLink").is_some() {
        return Err(format!(
            "{collection} exceeded 512 rows; refusing partial forecast eligibility"
        ));
    }
    Ok(rows.clone())
}

/// Capture only the reviewed fields used to identify and price a forecast.
fn registration_candidates(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
    frontier: &str,
    mode: &str,
) -> Result<Vec<Value>, String> {
    let endpoints = list_rows(ctx, api, headers, "Endpoints", world_id)?;
    let eligible = if endpoints.is_empty() && matches!(mode, "historical" | "simulated") {
        None // Explicit replay examples have no corridor requirements.
    } else if endpoints.is_empty() {
        Some(BTreeSet::new()) // An empty projection must not bypass observed review.
    } else {
        let mut paths = list_rows(ctx, api, headers, "Paths", world_id)?;
        // Collection projections can lag. A path must pass its current durable
        // evaluation state, even if the list still says Challenged (or Scored).
        for path in &mut paths {
            let id = row_id(path);
            if id.is_empty() {
                return Err("Path query returned a row without identity".into());
            }
            let response =
                ctx.http_call("GET", &format!("{api}/tdata/Paths('{id}')"), headers, "")?;
            if response.status != 200 {
                return Err(format!(
                    "Failed to verify path {id}: HTTP {}",
                    response.status
                ));
            }
            *path = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
        }
        Some(evaluated_required_nodes(&paths, world_id)?)
    };
    let mut nodes = list_rows(ctx, api, headers, "EventNodes", world_id)?;
    if let Some(ids) = &eligible {
        nodes.retain(|node| {
            row_str(node, "Provenance") == "determined"
                || manual_question(node)
                || ids.contains(row_id(node))
        });
    }
    let covered = compute_covered(ctx, &nodes, frontier);
    let mut candidates = Vec::new();
    for node in &nodes {
        let id = row_id(node);
        if id.is_empty()
            || covered.contains(id)
            || eligible
                .as_ref()
                .is_some_and(|ids| !ids.contains(id) && !manual_question(node))
            || !should_register(
                row_status(node),
                row_str(node, "Probability"),
                row_str(node, "Provenance"),
                row_str(node, "ResolveBy"),
                frontier,
            )
        {
            continue;
        }
        candidates.push(json!({"Id":id,"Statement":row_str(node,"Statement"),"Probability":row_str(node,"Probability"),"Provenance":row_str(node,"Provenance"),"ResolveBy":row_str(node,"ResolveBy"),"SourceRefs":row_str(node,"SourceRefs")}));
    }
    Ok(candidates)
}

fn manual_question(node: &Value) -> bool {
    row_str(node, "AuthorAgentId") == "dashboard" && row_str(node, "Provenance") == "authored"
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let mut registration_attempt = 0;
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        registration_attempt = ctx
            .entity_state
            .get("counters")
            .and_then(|c| c.get("registration_attempt"))
            .and_then(Value::as_u64)
            .filter(|attempt| *attempt > 0)
            .ok_or("Missing World registration attempt")?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let get = |k: &str| -> String {
            fields
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let world_id = ctx.entity_id.clone();
        let frontier = get("frontier_date");
        let mut mode = match get("learning_mode").as_str() {
            "" => "observed".to_string(),
            mode => mode.to_string(),
        };
        // The existing hindcast flag predates learning_mode. Persist its historical
        // meaning in the trusted preparation callback before registering a forecast.
        if get("hindcast_mode") == "true" {
            if mode == "simulated" {
                return Err("A hindcast cannot use simulated learning provenance".into());
            }
            mode = "historical".into();
        }
        let continuing = matches!(
            ctx.trigger_action.as_str(),
            "CommitForecastSnapshot" | "ForecastRegistered"
        );
        let registered_at = if ctx.trigger_action == "ForecastRegistered"
            || (ctx.trigger_action == "CommitForecastSnapshot" && mode != "observed")
        {
            registration_time(&get("forecast_registered_at"))?
        } else if mode == "observed" {
            host_registration_time(&ctx.entity_state)?
        } else {
            registration_time(&get("last_ingest_date"))?
        };
        let raw_model = get(if continuing {
            "registration_model_json"
        } else {
            "model_json"
        });
        if continuing && raw_model.is_empty() {
            return Err("Registration continuation has no frozen model snapshot".into());
        }
        let learning_run_id = get(if continuing {
            "forecast_learning_run_id"
        } else {
            "adopted_learning_run_id"
        });
        let model: Model = if raw_model.is_empty() {
            Model::identity(&mode)
        } else {
            serde_json::from_str(&raw_model).map_err(|e| e.to_string())?
        };
        model.validate()?;
        if model.mode != mode
            || (!model.evaluated_through.is_empty() && model.evaluated_through >= registered_at)
        {
            return Err(
                "Model provenance or evaluation time is incompatible with this registration".into(),
            );
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
            ("x-temper-principal-id".to_string(), world_id.clone()),
            ("x-temper-agent-type".to_string(), "system".to_string()),
        ];

        // Freeze evaluated inputs with the model and clock. A newly completed
        // path belongs to the queued next batch, never an earlier preregistration.
        let nodes = if continuing {
            let rows: Vec<Value> = serde_json::from_str(&get("registration_nodes_json"))
                .map_err(|e| format!("Invalid registration input snapshot: {e}"))?;
            if rows.len() > 512 {
                return Err("Registration snapshot exceeds 512 events".into());
            }
            rows
        } else {
            registration_candidates(&ctx, &api, &headers, &world_id, &frontier, &mode)?
        };
        let registration_nodes_json = serde_json::to_string(&nodes).map_err(|e| e.to_string())?;
        if !continuing {
            // Persist the candidate snapshot before assigning observed time.
            // The following declared action supplies a host timestamp after
            // every selected path/node was read, including concurrent arrivals.
            set_success_result(
                "RegistrationSnapshotPrepared",
                &json!({
                    "registration_nodes_json":registration_nodes_json,
                    "registration_model_json":serde_json::to_string(&model).map_err(|e|e.to_string())?,
                    "forecast_learning_run_id":learning_run_id,
                    "forecast_registered_at":registered_at,
                    "learning_mode":mode,
                    "expected_registration_attempt":registration_attempt
                }),
            );
            return Ok(());
        }

        for node in &nodes {
            let str_of = |k: &str| row_str(node, k);
            let node_id = row_id(node).to_string();
            // Historical prerequisites remain in the path, but are not new
            // forward predictions. Recheck after the snapshot clock is frozen.
            if !resolves_after_registration(str_of("ResolveBy"), &registered_at)? {
                continue;
            }

            // Each input/model pair has one immutable identity, including concurrent retries.
            let existing_url =
                format!("{api}/tdata/Forecasts?$filter=event_node_id eq '{node_id}'&$top=513");
            let existing = ctx.http_call("GET", &existing_url, &headers, "")?;
            if !(200..300).contains(&existing.status) {
                return Err(format!(
                    "Forecast lookup failed for {node_id}: HTTP {}",
                    existing.status
                ));
            }
            let existing_body: Value =
                serde_json::from_str(&existing.body).map_err(|e| e.to_string())?;
            let revisions = existing_body
                .get("value")
                .and_then(Value::as_array)
                .ok_or("Forecast response missing value")?;
            if revisions.len() > 512 || existing_body.get("@odata.nextLink").is_some() {
                return Err(
                    "Forecast revision query exceeded 512 rows; refusing incomplete history".into(),
                );
            }
            if revisions
                .iter()
                .any(|r| matches!(row_str(r, "Outcome"), "yes" | "no"))
            {
                continue; // Resolution is known: no new prediction can be preregistered.
            }
            let base_probability: f64 = str_of("Probability")
                .parse()
                .map_err(|_| "invalid base probability")?;
            let probability = model.predict(base_probability);
            let registration_key = json!([
                world_id,
                node_id,
                base_probability.to_string(),
                model.version,
                str_of("Statement"),
                str_of("ResolveBy")
            ])
            .to_string();
            if revisions
                .iter()
                .any(|r| row_str(r, "RegistrationKey") == registration_key)
            {
                continue;
            }
            let previous = revisions
                .iter()
                .filter(|r| {
                    let id = row_id(r);
                    !revisions
                        .iter()
                        .any(|other| row_str(other, "PreviousForecastId") == id)
                })
                .max_by_key(|r| (row_str(r, "RegisteredAt"), row_id(r)));
            let previous_id = previous.map(row_id).unwrap_or("");
            let forecast_id = registration_id(&registration_key);

            let market_ref = if str_of("Provenance") == "market" {
                first_source_ref(str_of("SourceRefs"))
            } else {
                String::new()
            };
            let existing_identity = ctx.http_call(
                "GET",
                &format!("{api}/tdata/Forecasts('{forecast_id}')"),
                &headers,
                "",
            )?;
            if existing_identity.status == 200 {
                let row: Value =
                    serde_json::from_str(&existing_identity.body).map_err(|e| e.to_string())?;
                if row_status(&row) != "Created" {
                    if row_str(&row, "RegistrationKey") != registration_key {
                        return Err(
                            "Forecast identity collision or conflicting registration".into()
                        );
                    }
                    // The point read is authoritative even when the list projection lags.
                    continue;
                }
            } else if existing_identity.status != 404 {
                return Err(format!(
                    "Forecast identity lookup failed: HTTP {}",
                    existing_identity.status
                ));
            }
            // A strict Forecast starts with identity only. The host executes the
            // declared Register transition after this read/compute callback.
            set_success_result(
                "ForecastPrepared",
                &json!({
                    "expected_registration_attempt":registration_attempt,
                    "registration_nodes_json":registration_nodes_json,
                    "forecast_id":forecast_id, "forecast_registration_key":registration_key,
                    "forecast_world_id": world_id, "forecast_event_node_id": node_id,
                    "forecast_question": str_of("Statement"), "forecast_probability": probability.to_string(),
                    "forecast_base_probability":base_probability.to_string(),
                    "forecast_model_version":model.version,"forecast_learning_run_id":learning_run_id,
                    "registration_model_json":serde_json::to_string(&model).map_err(|e|e.to_string())?,
                    "learning_mode":mode,
                    "forecast_previous_forecast_id":previous_id,"forecast_evidence_kind":mode,
                    "forecast_resolve_by": str_of("ResolveBy"), "forecast_market_ref": market_ref,
                    "forecast_engine_version": ENGINE_VERSION, "forecast_registered_at": registered_at,
                }),
            );
            return Ok(());
        }

        ctx.log(
            "info",
            &format!(
                "register_forecasts: world {world_id} — registration complete, \
                 from {} frozen candidate node(s)",
                nodes.len()
            ),
        );
        // A successful run with nothing to dispatch must still set a
        // result: the host treats an empty result as failure.
        set_success_result(
            "ForecastRegistrationComplete",
            &json!({"error_message":"","learning_mode":mode,"expected_registration_attempt":registration_attempt}),
        );
        Ok(())
    })();

    if let Err(error) = result {
        set_success_result(
            "ForecastRegistrationFailed",
            &json!({"error_message":error,"expected_registration_attempt":registration_attempt}),
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRONTIER: &str = "2026-09-30";

    #[test]
    fn resolution_deadlines_use_valid_dates_and_the_frozen_clock() {
        let clock = "2026-09-16T04:00:01Z";
        for date in ["", "2026-02-30", "not-a-date", "2026-09-17T04:00:00+01:00"] {
            assert!(resolves_after_registration(date, clock).is_err(), "{date}");
        }
        assert!(!resolves_after_registration("2026-09-15", clock).unwrap());
        assert!(!resolves_after_registration(clock, clock).unwrap());
        assert!(resolves_after_registration("2026-09-16", clock).unwrap());
        assert!(resolves_after_registration("2026-09-16T04:00:02Z", clock).unwrap());
    }

    #[test]
    fn only_evaluated_paths_release_their_own_required_events() {
        let paths = json!([
            {"WorldId":"world","Status":"Solving","RequiredNodeIds":"[\"unfinished\"]"},
            {"WorldId":"world","Status":"Repaired","RequiredNodeIds":"[\"unchallenged\"]"},
            {"WorldId":"world","Status":"Challenged","RequiredNodeIds":"[\"unscored\"]"},
            {"WorldId":"world","Status":"Scored","RequiredNodeIds":"[\"unclassified\"]"},
            {"WorldId":"world","Status":"Rejected","RequiredNodeIds":"[\"rejected\"]"},
            {"WorldId":"world","Status":"Tail","RequiredNodeIds":"[\"ready\",\"shared\"]"},
            {"WorldId":"world","Status":"Canonical","RequiredNodeIds":"[\"canonical\",\"shared\"]"},
            {"WorldId":"other","Status":"Scored","RequiredNodeIds":"[\"foreign\"]"}
        ]);
        let eligible = evaluated_required_nodes(paths.as_array().unwrap(), "world").unwrap();
        assert_eq!(
            eligible,
            ["canonical", "ready", "shared"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn malformed_evaluated_requirements_fail_closed() {
        for required in ["not json", "[1]", "[\"\"]"] {
            let paths =
                json!([{"WorldId":"world","Status":"Canonical","RequiredNodeIds":required}]);
            assert!(evaluated_required_nodes(paths.as_array().unwrap(), "world").is_err());
        }
    }

    #[test]
    fn determined_provenance_is_never_registered() {
        assert!(!should_register(
            "Confirmed",
            "0.50",
            "determined",
            "2026-06-30",
            FRONTIER
        ));
    }

    #[test]
    fn probability_one_is_a_fact_not_a_forecast() {
        assert!(!should_register(
            "Confirmed",
            "1.0",
            "authored",
            "2026-06-30",
            FRONTIER
        ));
        // ...and the other degenerate end is no forecast either.
        assert!(!should_register(
            "Confirmed",
            "0.0",
            "authored",
            "2026-06-30",
            FRONTIER
        ));
    }

    #[test]
    fn past_frontier_claims_are_structured_speculation_not_forecasts() {
        assert!(!should_register(
            "Confirmed",
            "0.50",
            "authored",
            "2026-12-31",
            FRONTIER
        ));
        // No resolve-by date at all means nothing to grade against.
        assert!(!should_register(
            "Confirmed",
            "0.50",
            "authored",
            "",
            FRONTIER
        ));
    }

    #[test]
    fn resolved_nodes_are_excluded() {
        assert!(!should_register(
            "Resolved",
            "0.50",
            "authored",
            "2026-06-30",
            FRONTIER
        ));
        assert!(!should_register(
            "Retired",
            "0.50",
            "market",
            "2026-06-30",
            FRONTIER
        ));
    }

    #[test]
    fn live_probabilistic_inside_frontier_claims_register() {
        assert!(should_register(
            "Confirmed",
            "0.65",
            "market",
            "2026-06-30",
            FRONTIER
        ));
        // Resolving exactly on the frontier still counts.
        assert!(should_register(
            "Proposed", "0.10", "authored", FRONTIER, FRONTIER
        ));
        // Unparseable probabilities never register.
        assert!(!should_register(
            "Confirmed",
            "likely",
            "authored",
            "2026-06-30",
            FRONTIER
        ));
    }

    #[test]
    fn market_ref_is_the_first_source_ref() {
        assert_eq!(
            first_source_ref(r#"["https://polymarket.com/event/x", "file:abc"]"#),
            "https://polymarket.com/event/x"
        );
        assert_eq!(first_source_ref("[]"), "");
        assert_eq!(first_source_ref("not json"), "");
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

    #[test]
    fn registration_is_stable_and_model_specific() {
        assert_eq!(
            registration_id("event|0.55|model1"),
            registration_id("event|0.55|model1")
        );
        assert_ne!(
            registration_id("event|0.55|model1"),
            registration_id("event|0.55|model2")
        );
        assert_ne!(
            registration_id("event|0.55|model1"),
            registration_id("event|0.56|model1")
        );
    }
    #[test]
    fn registration_rejects_non_finite_and_invalid_time() {
        assert!(!should_register(
            "Proposed",
            "NaN",
            "authored",
            "2026-01-01",
            "2027-01-01"
        ));
        assert!(registration_time("2025-02-30").is_err());
        assert_eq!(
            registration_time("2025-03-01").unwrap(),
            "2025-03-01T00:00:00Z"
        );
    }
    // --- Reconcile / dedup (D2) ---

    fn v(seed: &[f32]) -> Vec<f32> {
        seed.to_vec()
    }

    #[test]
    fn covered_by_embedding_collapses_only_within_the_strict_threshold() {
        let det = vec![v(&[1.0, 0.0, 0.0]), v(&[0.0, 1.0, 0.0])];
        // c0 ~= det[0] (distance ~0) -> covered; c1 orthogonal -> not covered.
        let cands = vec![v(&[1.0, 0.0, 0.0]), v(&[0.0, 0.0, 1.0])];
        let out = covered_by_embedding(&cands, &det, RECONCILE_MAX_DISTANCE);
        assert!(matches!(out[0], Some((0, d)) if d < 1e-6));
        assert!(out[1].is_none(), "an orthogonal claim is a real forecast");
        // A near-but-not-identical claim above the strict threshold is NOT
        // collapsed — we must never drop a genuine forecast.
        let near = vec![v(&[0.9, 0.44, 0.0])]; // distance ~0.10+ from det[0]
        let near_out = covered_by_embedding(&near, &det, 0.05);
        assert!(near_out[0].is_none());
    }

    #[test]
    fn covered_by_text_collapses_only_verbatim_restatements() {
        let det = vec!["EU AI Act applies from 2026-08-02.".to_string()];
        let cands = vec![
            "eu ai act applies from 2026-08-02".to_string(), // normalized-equal
            "The EU AI Act will reshape enterprise procurement.".to_string(), // different claim
        ];
        let out = covered_by_text(&cands, &det);
        assert_eq!(out[0], Some(0));
        assert!(out[1].is_none(), "a distinct claim is not a restatement");
    }

    #[test]
    fn normalize_is_case_whitespace_and_trailing_punct_insensitive() {
        assert_eq!(normalize("  Foo   Bar.  "), normalize("foo bar"));
        assert_eq!(normalize("Done!"), "done");
    }
}
