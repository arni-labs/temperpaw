//! register_forecasts — preregister the engine's gradeable word (ADR-002).
//!
//! On World.PathsScored: every EventNode that is a live, genuinely
//! probabilistic, non-determined claim resolving inside the world's frontier
//! becomes a Forecast — preregistered, immutable, graded later by
//! evidence_ingest. Nodes already carrying a Forecast are skipped: one
//! registration per node, ever. No follow-up action is dispatched —
//! registration is a side effect of scoring, not a state transition.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use corridor_embed::{build_embed_request, nearest, parse_embeddings};
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
    collapsed
        .trim_end_matches(['.', '!', '?', ' '])
        .to_string()
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

fn row_id(row: &Value) -> &str {
    row.get("entity_id")
        .or_else(|| row.get("Id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Engine version stamped on every registration.
const ENGINE_VERSION: &str = "0.2.0";

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
    if p <= 0.0 || p >= 1.0 {
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

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
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
        // The as-of date of the most recent evidence ingest, if any: WASM has
        // no clock, so registered_at can only be as fresh as the last ingest.
        let registered_at = get("last_ingest_date");

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

        // 1. Load every EventNode in this world.
        let nodes_url = format!("{api}/tdata/EventNodes?$filter=world_id eq '{world_id}'");
        let resp = ctx.http_call("GET", &nodes_url, &headers, "")?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!("failed to list EventNodes (HTTP {})", resp.status));
        }
        let body: Value = serde_json::from_str(&resp.body).unwrap_or(json!({}));
        let nodes = body
            .get("value")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Reconcile (D2): authored nodes that merely restate a determined fact
        // are collapsed here, before they can become free-resolving forecasts.
        let covered = compute_covered(&ctx, &nodes, &frontier);

        let mut registered = 0usize;
        let mut reconciled = 0usize;
        for node in &nodes {
            let str_of = |k: &str| row_str(node, k);
            let node_id = node
                .get("Id")
                .or_else(|| node.get("entity_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if node_id.is_empty() {
                continue;
            }
            if !should_register(
                str_of("Status"),
                str_of("Probability"),
                str_of("Provenance"),
                str_of("ResolveBy"),
                &frontier,
            ) {
                continue;
            }
            // Already-true fact restated as a forecast: skip (logged in
            // compute_covered with the matched fact + distance).
            if covered.contains(&node_id) {
                reconciled += 1;
                continue;
            }

            // 2. One Forecast per node, ever: skip nodes already registered.
            let existing_url =
                format!("{api}/tdata/Forecasts?$filter=event_node_id eq '{node_id}'");
            let existing = ctx.http_call("GET", &existing_url, &headers, "")?;
            if existing.status < 200 || existing.status >= 300 {
                ctx.log(
                    "warn",
                    &format!(
                        "register_forecasts: Forecast lookup for node {node_id} failed (HTTP {}); skipping",
                        existing.status
                    ),
                );
                continue;
            }
            let has_forecast = serde_json::from_str::<Value>(&existing.body)
                .ok()
                .and_then(|v| {
                    v.get("value")
                        .and_then(|x| x.as_array())
                        .map(|a| !a.is_empty())
                })
                .unwrap_or(false);
            if has_forecast {
                continue;
            }

            // 3. Preregister.
            let market_ref = if str_of("Provenance") == "market" {
                first_source_ref(str_of("SourceRefs"))
            } else {
                String::new()
            };
            let create_body = json!({
                "world_id": world_id,
                "event_node_id": node_id,
                "question": str_of("Statement"),
                "probability": str_of("Probability"),
                "resolve_by": str_of("ResolveBy"),
                "market_ref": market_ref,
                "engine_version": ENGINE_VERSION,
                "registered_at": registered_at,
            });
            let created = ctx.http_call(
                "POST",
                &format!("{api}/tdata/Forecasts"),
                &headers,
                &create_body.to_string(),
            )?;
            if created.status < 200 || created.status >= 300 {
                ctx.log(
                    "warn",
                    &format!(
                        "register_forecasts: Forecast create for node {node_id} failed (HTTP {})",
                        created.status
                    ),
                );
                continue;
            }
            registered += 1;
            ctx.log(
                "info",
                &format!(
                    "register_forecasts: registered forecast for node {node_id} (p={}, resolve_by={})",
                    str_of("Probability"),
                    str_of("ResolveBy")
                ),
            );
        }

        ctx.log(
            "info",
            &format!(
                "register_forecasts: world {world_id} — {registered} forecast(s) registered, \
                 {reconciled} collapsed as determined-restating, from {} node(s)",
                nodes.len()
            ),
        );
        // A successful run with nothing to dispatch must still set a
        // result: the host treats an empty result as failure.
        set_success_result("", &json!({}));
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}
