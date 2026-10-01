//! aggregate_costs — deterministic costing, search policy, and
//! classification for corridor routes (ADR-002, ADR-004).
//!
//! Sessions flag costs; this module computes them — and in 0.3 it also owns
//! the route search: pruning before adversary spend, revision rounds that
//! answer expensive objections, alternate routes when a claim's best is not
//! acceptable, and the claim → endpoint → world settlement cascade. All of
//! it is arithmetic over recorded flags; no model judgment enters here.
//!
//! Phases, keyed on the triggering action:
//! - Path.RepairComplete  → prune gate: cut routes that already cost more
//!   than the claim's best allows, before any adversary session is spent.
//! - Path.ChallengeComplete → cost + revision decision: price the union of
//!   flags; an expensive objection within the round budget sends the route
//!   back to its repairer, otherwise the route is scored.
//! - Path.Score / Path.Prune → relay the settled route to its claim.
//! - Claim.RouteSettled → claim decision: wait for in-flight routes, spawn
//!   an alternate route, settle on the cheapest, or declare the claim
//!   unreachable; then, when every claim in the world is terminal, derive
//!   endpoint weights and dispatch World.PathsScored.
//!
//! Legacy (0.2) paths carry no claim_id; their Score still runs the
//! endpoint-level classify pass unchanged (in-place evolution, ADR-003).
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

// --- Search policy constants -------------------------------------------------
//
// Tunable priors (ADR-004 commitment 1): the ORDERING and the mechanism are
// the design; the values await calibration against the hindcast library.

/// Revision rounds allowed per route (a route is repaired at most 1 + this
/// many times).
const MAX_ROUNDS: usize = 2;

/// Routes (Paths) allowed per claim.
const MAX_ROUTES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SearchBudget {
    revision_rounds: usize,
    routes: usize,
}

const FIRST_PASS_BUDGET: SearchBudget = SearchBudget {
    revision_rounds: 0,
    routes: 1,
};
const DEEP_BUDGET: SearchBudget = SearchBudget {
    revision_rounds: MAX_ROUNDS,
    routes: MAX_ROUTES,
};

fn search_budget(world_fields: &Value) -> SearchBudget {
    if world_fields
        .get("exploration_phase")
        .and_then(Value::as_str)
        == Some("first_pass")
    {
        FIRST_PASS_BUDGET
    } else {
        // Worlds created before progressive exploration retain their search policy.
        DEEP_BUDGET
    }
}

/// A single flag at or above this many points triggers a revision round —
/// miracle/medium (40) and above. Calibrated against run 1 (2026-06-12):
/// at 20, nearly every route revised and session spend tripled for little
/// cost movement.
const REVISION_FLAG_THRESHOLD: f64 = 40.0;

/// A claim whose best route costs more than this is not settled — search
/// continues, or the claim is honestly Unreachable when the budget is spent.
/// Calibrated against run 1's live distribution (15 scored routes,
/// 90–312.5, median ~245): the original 60 marked everything unreachable.
/// Still a prior; the hindcast calibrator owns the next revision.
const ACCEPTABLE_CLAIM_COST: f64 = 240.0;

/// Settled claims at or under this are "reachable"; above it (but within
/// the acceptability bound) they are "strained".
const REACHABLE_MAX: f64 = 120.0;

/// An unreachable claim's contribution to its endpoint's cost: at least the
/// acceptability bound, since no route under it exists.
const UNREACHABLE_PENALTY: f64 = 240.0;

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

fn row_bool(row: &Value, snake: &str, pascal: &str) -> bool {
    row.get("booleans")
        .and_then(|b| b.get(snake))
        .or_else(|| row.get("fields").and_then(|f| f.get(snake)))
        .or_else(|| row.get(pascal))
        .or_else(|| row.get(snake))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

/// Base cost per flag kind. A miracle (unexplained discontinuity) costs the
/// most; a compressed lag the least. Deformation — amending the claim to
/// make it bridgeable — prices like a contradiction, so drifting back to
/// consensus is never free (ADR-004 commitment 4). Unknown kinds cost like
/// contradictions so malformed flags are never free.
fn kind_weight(kind: &str) -> f64 {
    match kind {
        "miracle" => 40.0,
        "contradiction" => 25.0,
        "deformation" => 25.0,
        "incentive" => 15.0,
        "lag" => 10.0,
        _ => 25.0,
    }
}

fn severity_multiplier(severity: &str) -> f64 {
    match severity {
        "low" => 0.5,
        "high" => 2.0,
        _ => 1.0,
    }
}

fn parse_flags(raw: &Value) -> Vec<Value> {
    if let Some(arr) = raw.as_array() {
        return arr.clone();
    }
    if let Some(s) = raw.as_str() {
        if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(s) {
            return arr;
        }
    }
    Vec::new()
}

fn flag_points(flag: &Value) -> f64 {
    let kind = flag.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let severity = flag
        .get("severity")
        .and_then(|v| v.as_str())
        .unwrap_or("medium");
    kind_weight(kind) * severity_multiplier(severity)
}

/// Repair cost = sum over the union of both sides' flags of
/// kind_weight x severity_multiplier. Pure and order-independent.
fn repair_cost(cost_flags: &Value, challenge_flags: &Value) -> f64 {
    parse_flags(cost_flags)
        .iter()
        .chain(parse_flags(challenge_flags).iter())
        .map(flag_points)
        .sum()
}

/// The most expensive single flag in a set — the revision trigger signal.
fn max_flag_points(flags: &Value) -> f64 {
    parse_flags(flags)
        .iter()
        .map(flag_points)
        .fold(0.0, f64::max)
}

/// Human-readable brief of the expensive objections a revision (or an
/// alternate route) must beat. Most expensive first, capped so the brief
/// stays a brief.
fn brief_from_flags(flags: &Value) -> String {
    let mut entries: Vec<(f64, String)> = parse_flags(flags)
        .iter()
        .map(|f| {
            let kind = f.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
            let severity = f
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");
            let note = f.get("note").and_then(|v| v.as_str()).unwrap_or("");
            (
                flag_points(f),
                format!("- {kind}/{severity} ({} pts): {note}", flag_points(f)),
            )
        })
        .collect();
    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut brief = String::new();
    for (_, line) in entries.into_iter().take(6) {
        brief.push_str(&line);
        brief.push('\n');
    }
    if brief.len() > 2000 {
        let mut end = 2000;
        while !brief.is_char_boundary(end) {
            end -= 1;
        }
        brief.truncate(end);
    }
    brief.trim_end().to_string()
}

// --- Pure search policy ------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum RouteDecision {
    Score,
    Revise,
}

/// After a challenge: revise when an objection is expensive and the round
/// budget allows it. Revision answers the ADVERSARY's flags — the repairer
/// already had its say.
fn route_decision(
    round_count: usize,
    max_challenge_flag: f64,
    budget: SearchBudget,
) -> RouteDecision {
    if round_count < budget.revision_rounds && max_challenge_flag >= REVISION_FLAG_THRESHOLD {
        RouteDecision::Revise
    } else {
        RouteDecision::Score
    }
}

/// Prune gate: a fresh repair that already costs more than the claim's best
/// scored route allows can never win — cut it before adversary spend. The
/// first route of a claim never prunes (there is no best yet).
fn should_prune(repairer_phase_cost: f64, best_scored: Option<f64>) -> bool {
    match best_scored {
        Some(best) => repairer_phase_cost > best * 2.0 + 20.0,
        None => false,
    }
}

#[derive(Debug, PartialEq, Clone)]
enum ClaimDecision {
    Wait,
    Alternate,
    Settle { route_id: String, cost: f64 },
    Unreachable { reason: String },
}

/// The claim-level decision once a route settles. `scored` carries every
/// Scored route (id, cost); `in_flight` is true while any route is still
/// being repaired or challenged.
fn claim_decision(
    route_count: usize,
    in_flight: bool,
    scored: &[(String, f64)],
    budget: SearchBudget,
) -> ClaimDecision {
    let best = scored
        .iter()
        .min_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        })
        .cloned();

    // A still-churning route normally means "wait for it before deciding."
    // The exception is the wedged straggler: once the route budget is spent
    // (no fresh alternate can open) AND an acceptable route already exists,
    // the claim's answer is determined. A stuck or flapping route can only
    // ever match or slightly beat a route we'd already accept, so it must not
    // block settlement forever. This is what bounds the ADR-007 self-heal —
    // the search budget caps it, exactly as the ADR promises. With budget
    // still left, or with no acceptable route yet, the in-flight route can
    // still change the outcome, so we wait (Path self-heal re-drives a dead
    // session in that case).
    if in_flight {
        let budget_spent = route_count >= budget.routes;
        let settleable = matches!(&best, Some((_, cost)) if *cost <= ACCEPTABLE_CLAIM_COST);
        if !(budget_spent && settleable) {
            return ClaimDecision::Wait;
        }
    }

    match best {
        None => {
            if route_count < budget.routes {
                ClaimDecision::Alternate
            } else {
                ClaimDecision::Unreachable {
                    reason: format!("no route survived costing after {route_count} routes"),
                }
            }
        }
        Some((route_id, cost)) => {
            if cost > ACCEPTABLE_CLAIM_COST {
                if route_count < budget.routes {
                    ClaimDecision::Alternate
                } else {
                    ClaimDecision::Unreachable {
                        reason: format!(
                            "best route costs {} (> acceptability bound {}) after {route_count} routes",
                            fmt_cost(cost),
                            fmt_cost(ACCEPTABLE_CLAIM_COST)
                        ),
                    }
                }
            } else {
                ClaimDecision::Settle { route_id, cost }
            }
        }
    }
}

/// Deterministic classification of a settled claim's best cost.
fn claim_classification(cost: f64) -> &'static str {
    if cost <= REACHABLE_MAX {
        "reachable"
    } else {
        "strained"
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Classification {
    Canonical,
    Tail,
    Reject,
}

/// Classify scored routes. Exactly one Canonical: the cheapest (ties broken
/// by lexicographic id so reruns agree). A route is Tail when its cost is
/// within 2x of the canonical cost plus a 20-point grace; beyond that it is
/// rejected.
fn classify(paths: &[(String, f64)]) -> Vec<(String, Classification)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<(String, f64)> = paths.to_vec();
    sorted.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let canonical_id = sorted[0].0.clone();
    let min_cost = sorted[0].1;
    let tail_ceiling = min_cost * 2.0 + 20.0;
    sorted
        .into_iter()
        .map(|(id, cost)| {
            let class = if id == canonical_id {
                Classification::Canonical
            } else if cost <= tail_ceiling {
                Classification::Tail
            } else {
                Classification::Reject
            };
            (id, class)
        })
        .collect()
}

/// Endpoint cost from its claims' verdicts: settled claims contribute their
/// best route cost, unreachable claims contribute the penalty (no route
/// under the bound exists), averaged over all decided claims.
fn endpoint_cost(settled_costs: &[f64], unreachable_count: usize) -> f64 {
    let n = settled_costs.len() + unreachable_count;
    if n == 0 {
        return f64::MAX;
    }
    let sum: f64 =
        settled_costs.iter().sum::<f64>() + UNREACHABLE_PENALTY * unreachable_count as f64;
    sum / n as f64
}

/// Endpoint weight from its cost: exp(-cost / 25), normalized over all
/// endpoints, rounded to 4 decimals. Deterministic for equal inputs.
fn endpoint_weights(best_costs: &[(String, f64)]) -> Vec<(String, f64)> {
    if best_costs.is_empty() {
        return Vec::new();
    }
    let raw: Vec<(String, f64)> = best_costs
        .iter()
        .map(|(id, cost)| (id.clone(), (-cost / 25.0).exp()))
        .collect();
    let total: f64 = raw.iter().map(|(_, w)| w).sum();
    let mut weights: Vec<(String, f64)> = raw
        .into_iter()
        .map(|(id, w)| (id, ((w / total) * 10000.0).round() / 10000.0))
        .collect();
    weights.sort_by(|a, b| a.0.cmp(&b.0));
    weights
}

fn fmt_cost(cost: f64) -> String {
    format!("{:.2}", cost)
}

// --- HTTP plumbing -----------------------------------------------------------

fn api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn system_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn fetch(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
) -> Option<Value> {
    let r = ctx
        .http_call("GET", &format!("{api}/tdata/{set}('{id}')"), headers, "")
        .ok()?;
    if !(200..300).contains(&r.status) {
        return None;
    }
    serde_json::from_str(&r.body).ok()
}

fn list(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    filter: &str,
) -> Result<Vec<Value>, String> {
    let url = format!("{api}/tdata/{set}?$filter={filter}");
    let r = ctx.http_call("GET", &url, headers, "")?;
    if !(200..300).contains(&r.status) {
        return Err(format!("list {set} failed (HTTP {})", r.status));
    }
    let body: Value = serde_json::from_str(&r.body).unwrap_or(json!({}));
    Ok(body
        .get("value")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

fn dispatch(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    set: &str,
    id: &str,
    action: &str,
    body: &Value,
) -> Result<(), String> {
    let r = ctx.http_call(
        "POST",
        &format!("{api}/tdata/{set}('{id}')/TemperPaw.{action}"),
        headers,
        &body.to_string(),
    )?;
    if !(200..300).contains(&r.status) {
        return Err(format!(
            "{set}.{action} on {id} failed (HTTP {}): {}",
            r.status,
            &r.body[..r.body.len().min(200)]
        ));
    }
    Ok(())
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        match ctx.trigger_action.as_str() {
            "RepairComplete" | "EvaluateRepair" => prune_gate(&ctx, &fields),
            "ChallengeComplete" | "EvaluateChallenge" | "ResumeCosting" => {
                cost_phase(&ctx, &fields)
            }
            "Score" | "RelayScoredRoute" => route_settled_relay(&ctx, &fields, false),
            "Prune" | "RelayPrunedRoute" => route_settled_relay(&ctx, &fields, true),
            "RouteSettled" | "ResumeBridge" | "EvaluateRoutes" => claim_phase(&ctx, &fields),
            "ResumeWorldCascade" | "EvaluateWorldCascade" => world_cascade_self_heal(&ctx, &fields),
            "MaybeDeepen" => prepare_deepening(&ctx, &fields),
            "ContinueDeepening" => continue_deepening(&fields),
            "ResumeEndpointScoring" | "EvaluateEndpointScoring" => {
                endpoint_scoring_self_heal(&ctx, &fields)
            }
            other => {
                ctx.log(
                    "warn",
                    &format!("aggregate_costs: unexpected trigger action {other}, nothing to do"),
                );
                // A successful run with nothing to dispatch must still set a
                // result: the host treats an empty result as failure.
                set_success_result("", &json!({}));
                Ok(())
            }
        }
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn get_str(fields: &Value, key: &str) -> String {
    fields
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn world_search_budget(ctx: &Context, world_id: &str) -> Result<SearchBudget, String> {
    if world_id.is_empty() {
        return Ok(DEEP_BUDGET);
    }
    let world = fetch(ctx, &api_url(ctx), &system_headers(ctx), "Worlds", world_id)
        .ok_or_else(|| format!("cannot read search policy for world {world_id}"))?;
    Ok(search_budget(world.get("fields").unwrap_or(&world)))
}

/// Path.RepairComplete: prune the route before adversary spend, or request
/// the challenge.
fn prune_gate(ctx: &Context, fields: &Value) -> Result<(), String> {
    let path_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let claim_id = get_str(fields, "claim_id");
    let cost_flags = fields.get("cost_flags").cloned().unwrap_or(json!([]));
    let repairer_cost = repair_cost(&cost_flags, &json!([]));

    // Legacy 0.2 path (no claim): no prune gate existed; go straight to the
    // challenge.
    if claim_id.is_empty() {
        set_success_result("RequestChallenge", &json!({}));
        return Ok(());
    }

    // The claim's best scored sibling route, if any (first routes never
    // prune: there is no best yet).
    let siblings = list(
        ctx,
        &api,
        &headers,
        "Paths",
        &format!("claim_id eq '{claim_id}'"),
    )?;
    let mut best: Option<f64> = None;
    for s in &siblings {
        if row_id(s) == path_id {
            continue;
        }
        if matches!(row_status(s), "Scored" | "Canonical" | "Tail") {
            let c = row_str(s, "RepairCost").parse::<f64>().unwrap_or(f64::MAX);
            best = Some(best.map_or(c, |b: f64| b.min(c)));
        }
    }

    if should_prune(repairer_cost, best) {
        ctx.log(
            "info",
            &format!(
                "aggregate_costs: pruning route {path_id} (repairer-phase cost {} vs best {})",
                fmt_cost(repairer_cost),
                fmt_cost(best.unwrap_or(f64::MAX))
            ),
        );
        set_success_result(
            "Prune",
            &json!({
                "classification_note": format!(
                    "repairer-phase cost {} already exceeds the claim's best route ceiling",
                    fmt_cost(repairer_cost)
                )
            }),
        );
    } else {
        set_success_result("RequestChallenge", &json!({}));
    }
    Ok(())
}

/// Path.ChallengeComplete: price the union of flags; revise or score.
fn cost_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let cost_flags = fields.get("cost_flags").cloned().unwrap_or(json!([]));
    let challenge_flags = fields.get("challenge_flags").cloned().unwrap_or(json!([]));
    let cost = repair_cost(&cost_flags, &challenge_flags);
    let round_count = get_str(fields, "round_count").parse::<usize>().unwrap_or(0);
    let claim_id = get_str(fields, "claim_id");

    // Revision is a search step — claim-era routes only (legacy 0.2 paths
    // keep the single-round behavior).
    let budget = world_search_budget(ctx, &get_str(fields, "world_id"))?;
    let max_objection = max_flag_points(&challenge_flags);
    if !claim_id.is_empty()
        && route_decision(round_count, max_objection, budget) == RouteDecision::Revise
    {
        ctx.log(
            "info",
            &format!(
                "aggregate_costs: route {} revision round {} (max objection {} pts)",
                ctx.entity_id,
                round_count + 1,
                fmt_cost(max_objection)
            ),
        );
        set_success_result(
            "RevisionRequested",
            &json!({
                "revision_brief": brief_from_flags(&challenge_flags),
                "round_count": (round_count + 1).to_string(),
            }),
        );
        return Ok(());
    }

    ctx.log(
        "info",
        &format!(
            "aggregate_costs: path {} cost={} (repairer + adversary flags)",
            ctx.entity_id,
            fmt_cost(cost)
        ),
    );
    set_success_result("Score", &json!({ "repair_cost": fmt_cost(cost) }));
    Ok(())
}

/// Path.Score / Path.Prune: hand the settled route to its claim. Legacy
/// paths (no claim) run the 0.2 endpoint-level classify pass instead.
fn route_settled_relay(ctx: &Context, fields: &Value, pruned: bool) -> Result<(), String> {
    let claim_id = get_str(fields, "claim_id");
    if claim_id.is_empty() {
        if pruned {
            // Legacy paths are never pruned (gate skips them); defensive.
            set_success_result("", &json!({}));
            return Ok(());
        }
        return legacy_classify_phase(ctx, fields);
    }
    let api = api_url(ctx);
    let headers = system_headers(ctx);
    let cost = if pruned {
        // A pruned route's recorded cost is its repairer-phase lower bound.
        let cost_flags = fields.get("cost_flags").cloned().unwrap_or(json!([]));
        repair_cost(&cost_flags, &json!([]))
    } else {
        get_str(fields, "repair_cost")
            .parse::<f64>()
            .unwrap_or(f64::MAX)
    };
    dispatch(
        ctx,
        &api,
        &headers,
        "Claims",
        &claim_id,
        "RouteSettled",
        &json!({
            "path_id": ctx.entity_id.clone(),
            "route_cost": fmt_cost(cost),
        }),
    )?;
    set_success_result("", &json!({}));
    Ok(())
}

/// Claim.RouteSettled: the claim decision, then the world cascade.
fn claim_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let claim_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let world_id = get_str(fields, "world_id");
    let route_count = get_str(fields, "route_count").parse::<usize>().unwrap_or(0);
    let budget = world_search_budget(ctx, &world_id)?;

    // Single-writer guard: if this claim already settled, another relay got
    // here first.
    if !get_str(fields, "settled_route_id").is_empty() {
        set_success_result("", &json!({}));
        return Ok(());
    }

    // 1. This claim's routes, authoritatively re-read when in flight (the
    // list projection can lag the event store — v1 lesson, pinned).
    let rows = list(
        ctx,
        &api,
        &headers,
        "Paths",
        &format!("claim_id eq '{claim_id}'"),
    )?;
    let mut in_flight = false;
    let mut in_flight_ids: Vec<String> = Vec::new();
    let mut scored: Vec<(String, f64)> = Vec::new();
    let mut last_flags: Value = json!([]);
    for row in &rows {
        let id = row_id(row).to_string();
        let mut status = row_status(row).to_string();
        let mut cost_src = row_str(row, "RepairCost").to_string();
        if matches!(status.as_str(), "Solving" | "Repaired" | "Challenged") {
            if let Some(fresh) = fetch(ctx, &api, &headers, "Paths", &id) {
                status = row_status(&fresh).to_string();
                cost_src = row_str(&fresh, "RepairCost").to_string();
            }
        }
        match status.as_str() {
            "Solving" | "Repaired" | "Challenged" => {
                in_flight = true;
                in_flight_ids.push(id.clone());
            }
            "Scored" | "Canonical" | "Tail" => {
                let cost = cost_src.parse::<f64>().unwrap_or(f64::MAX);
                scored.push((id.clone(), cost));
                // Remember the flags of a scored route for the
                // alternate-route brief.
                if let Some(fresh) = fetch(ctx, &api, &headers, "Paths", &id) {
                    let cf = fresh
                        .get("fields")
                        .and_then(|f| f.get("cost_flags"))
                        .cloned()
                        .unwrap_or(json!("[]"));
                    let ch = fresh
                        .get("fields")
                        .and_then(|f| f.get("challenge_flags"))
                        .cloned()
                        .unwrap_or(json!("[]"));
                    let mut both = parse_flags(&cf);
                    both.extend(parse_flags(&ch));
                    last_flags = Value::Array(both);
                }
            }
            _ => {} // Pruned/Rejected/Failed: spent, not candidates
        }
    }

    match claim_decision(route_count, in_flight, &scored, budget) {
        ClaimDecision::Wait => {
            ctx.log(
                "info",
                &format!("aggregate_costs: claim {claim_id} has routes in flight; waiting"),
            );
            set_success_result("", &json!({}));
            Ok(())
        }
        ClaimDecision::Alternate => {
            // Brief the next repairer on the standing objections, then open
            // the next route.
            let brief = brief_from_flags(&last_flags);
            ctx.log(
                "info",
                &format!(
                    "aggregate_costs: claim {claim_id} opening alternate route (route_count {route_count})"
                ),
            );
            set_success_result("SubmitForBridge", &json!({"revision_brief": brief}));
            Ok(())
        }
        ClaimDecision::Unreachable { reason } => {
            ctx.log(
                "info",
                &format!("aggregate_costs: claim {claim_id} unreachable: {reason}"),
            );
            set_success_result("MarkUnreachable", &json!({ "unreachable_reason": reason }));
            Ok(())
        }
        ClaimDecision::Settle { route_id, cost } => {
            // Classify this claim's routes deterministically.
            for (id, class) in classify(&scored) {
                let (action, note) = match class {
                    Classification::Canonical => ("ClassifyCanonical", "cheapest route for claim"),
                    Classification::Tail => ("ClassifyTail", "coherent but expensive: tail route"),
                    Classification::Reject => ("Reject", "route cost beyond claim tolerance"),
                };
                if let Err(e) = dispatch(
                    ctx,
                    &api,
                    &headers,
                    "Paths",
                    &id,
                    action,
                    &json!({ "classification_note": note }),
                ) {
                    ctx.log("warn", &format!("aggregate_costs: {e}"));
                }
            }
            let classification = claim_classification(cost);
            dispatch(
                ctx,
                &api,
                &headers,
                "Claims",
                &claim_id,
                "Settle",
                &json!({
                    "settled_route_id": route_id,
                    "best_route_cost": fmt_cost(cost),
                    "classification": classification,
                }),
            )?;
            ctx.log(
                "info",
                &format!(
                    "aggregate_costs: claim {claim_id} settled at {} ({classification})",
                    fmt_cost(cost)
                ),
            );
            // Terminalize any in-flight straggler routes: the claim has its
            // answer and its route budget is spent (claim_decision only
            // settles past an in-flight route when route_count >= MAX_ROUTES),
            // so a route still churning is no longer needed. Failing it stops
            // its self-heal timeout (ADR-007) from re-spawning a route nobody
            // will use — closing the only unbounded self-heal left after the
            // settle. A live route normally would have been in `scored`; this
            // fires only for the wedged-straggler case the liveness fix added.
            for straggler in &in_flight_ids {
                if let Err(e) = dispatch(
                    ctx,
                    &api,
                    &headers,
                    "Paths",
                    straggler,
                    "Fail",
                    &json!({
                        "error_message": format!(
                            "superseded: claim {claim_id} settled on its cheapest acceptable \
                             route while this route was still in flight; route budget spent, \
                             no longer needed"
                        )
                    }),
                ) {
                    ctx.log(
                        "warn",
                        &format!("aggregate_costs: failing straggler {straggler}: {e}"),
                    );
                }
            }
            set_success_result("", &json!({}));
            Ok(())
        }
    }
}

/// World.ResumeWorldCascade (Active state_timeout): all claims terminal but
/// canonical_path_id still empty — re-run the world aggregate. No-ops when
/// canonical is already set or any claim is still in flight.
fn world_cascade_self_heal(ctx: &Context, fields: &Value) -> Result<(), String> {
    if !get_str(fields, "canonical_path_id").is_empty()
        && get_str(fields, "exploration_phase") != "deepening"
    {
        set_success_result("", &json!({}));
        return Ok(());
    }
    let world_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);
    world_cascade(ctx, &api, &headers, &world_id)?;
    // No-op paths inside world_cascade (claims still Bridging, canonical already
    // set, etc.) must still report success: the host treats an empty WASM result
    // as an integration failure.
    set_success_result("", &json!({}));
    Ok(())
}

/// Prepare one bounded background pass after the first forecast batch is usable.
fn prepare_deepening(ctx: &Context, fields: &Value) -> Result<(), String> {
    if get_str(fields, "exploration_phase") != "first_pass" {
        set_success_result("", &json!({}));
        return Ok(());
    }
    let api = api_url(ctx);
    let headers = system_headers(ctx);
    let rows = list(
        ctx,
        &api,
        &headers,
        "Claims",
        &format!("world_id eq '{}'", ctx.entity_id),
    )?;
    let mut ids = Vec::new();
    for row in rows {
        let id = row_id(&row);
        let fresh = fetch(ctx, &api, &headers, "Claims", id)
            .ok_or("cannot read claim before background exploration")?;
        if matches!(row_status(&fresh), "Proposed" | "Bridging") {
            return Err("first pass contains an unfinished claim".into());
        }
        if matches!(row_status(&fresh), "Settled" | "Unreachable") {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    if ids.len() > 9 {
        return Err("first pass exceeded nine claims".into());
    }
    set_success_result(
        "BeginDeepening",
        &json!({
            "deepening_claim_ids": serde_json::to_string(&ids).map_err(|e| e.to_string())?,
            "deepening_cursor": "0", "exploration_phase": "deepening"
        }),
    );
    Ok(())
}

fn deepening_step(fields: &Value) -> Result<Option<Value>, String> {
    let ids: Vec<String> = serde_json::from_str(&get_str(fields, "deepening_claim_ids"))
        .map_err(|e| format!("invalid background claim list: {e}"))?;
    if ids.len() > 9 {
        return Err("background exploration exceeded nine claims".into());
    }
    let cursor = get_str(fields, "deepening_cursor")
        .parse::<usize>()
        .map_err(|e| format!("invalid background cursor: {e}"))?;
    Ok(ids.get(cursor).map(|id| {
        json!({
            "deepening_claim_id": id,
            "expected_deepening_cursor": cursor.to_string(),
            "deepening_cursor": (cursor + 1).to_string()
        })
    }))
}

fn continue_deepening(fields: &Value) -> Result<(), String> {
    match deepening_step(fields)? {
        Some(params) => set_success_result("DeepeningClaimPrepared", &params),
        None => set_success_result("", &json!({})),
    }
    Ok(())
}

/// Endpoint.ResumeEndpointScoring (UnderRepair state_timeout): every claim on
/// this endpoint is terminal but ScoreComplete never landed — re-derive weight.
fn endpoint_scoring_self_heal(ctx: &Context, _fields: &Value) -> Result<(), String> {
    let endpoint_id = ctx.entity_id.clone();
    let api = api_url(ctx);
    let headers = system_headers(ctx);

    let claims = list(
        ctx,
        &api,
        &headers,
        "Claims",
        &format!("endpoint_id eq '{endpoint_id}'"),
    )?;
    let mut settled_costs: Vec<f64> = Vec::new();
    let mut unreachable_count = 0usize;
    for c in &claims {
        let id = row_id(c).to_string();
        let mut status = row_status(c).to_string();
        if matches!(status.as_str(), "Proposed" | "Bridging") {
            if let Some(fresh) = fetch(ctx, &api, &headers, "Claims", &id) {
                status = row_status(&fresh).to_string();
            }
        }
        match status.as_str() {
            "Proposed" | "Bridging" => {
                ctx.log(
                    "info",
                    &format!(
                        "aggregate_costs: endpoint {endpoint_id} claim {id} still {status}; scoring deferred"
                    ),
                );
                set_success_result("", &json!({}));
                return Ok(());
            }
            "Settled" => {
                let cost = row_str(c, "BestRouteCost")
                    .parse::<f64>()
                    .unwrap_or(f64::MAX);
                settled_costs.push(cost);
            }
            "Unreachable" => unreachable_count += 1,
            _ => {}
        }
    }

    if settled_costs.is_empty() && unreachable_count == 0 {
        set_success_result("", &json!({}));
        return Ok(());
    }

    let weight = endpoint_cost(&settled_costs, unreachable_count);
    dispatch(
        ctx,
        &api,
        &headers,
        "Endpoints",
        &endpoint_id,
        "ScoreComplete",
        &json!({ "weight": format!("{:.4}", weight) }),
    )?;
    let _ = dispatch(
        ctx,
        &api,
        &headers,
        "Endpoints",
        &endpoint_id,
        "MarkWeighted",
        &json!({}),
    );
    ctx.log(
        "info",
        &format!("aggregate_costs: endpoint {endpoint_id} self-heal scored at weight {weight:.4}"),
    );
    set_success_result("", &json!({}));
    Ok(())
}

/// When every claim in the world is terminal, derive endpoint weights and
/// report the settled pass — once (the world's canonical_path_id is the
/// single-writer guard).
fn world_cascade(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<(), String> {
    if world_id.is_empty() {
        return Ok(());
    }

    // Every claim must be durably terminal; post-commit entity triggers request this pass.
    let claims = list(
        ctx,
        api,
        headers,
        "Claims",
        &format!("world_id eq '{world_id}'"),
    )?;
    #[allow(clippy::type_complexity)]
    let mut per_endpoint: std::collections::BTreeMap<String, (Vec<f64>, usize)> =
        std::collections::BTreeMap::new();
    // (endpoint_id -> (settled costs, unreachable count)); also remember the
    // cheapest settled route per endpoint for the world receipt.
    let mut cheapest_route: std::collections::BTreeMap<String, (f64, String)> =
        std::collections::BTreeMap::new();

    for c in &claims {
        let id = row_id(c).to_string();
        let mut status = row_status(c).to_string();
        let mut best_cost = row_str(c, "BestRouteCost").to_string();
        let mut route_id = row_str(c, "SettledRouteId").to_string();
        if matches!(status.as_str(), "Proposed" | "Bridging") {
            if let Some(fresh) = fetch(ctx, api, headers, "Claims", &id) {
                status = row_status(&fresh).to_string();
                best_cost = row_str(&fresh, "BestRouteCost").to_string();
                route_id = row_str(&fresh, "SettledRouteId").to_string();
            }
        }
        let endpoint_id = row_str(c, "EndpointId").to_string();
        match status.as_str() {
            "Proposed" | "Bridging" => {
                ctx.log(
                    "info",
                    &format!("aggregate_costs: claim {id} still {status}; world not settled"),
                );
                return Ok(());
            }
            "Settled" => {
                let cost = best_cost.parse::<f64>().unwrap_or(f64::MAX);
                let entry = per_endpoint.entry(endpoint_id.clone()).or_default();
                entry.0.push(cost);
                let best = cheapest_route
                    .entry(endpoint_id)
                    .or_insert((f64::MAX, String::new()));
                if cost < best.0 && !route_id.is_empty() {
                    *best = (cost, route_id);
                }
            }
            "Unreachable" => {
                per_endpoint.entry(endpoint_id).or_default().1 += 1;
            }
            _ => {} // Failed claims: the endpoint failed or will; excluded
        }
    }

    if per_endpoint.is_empty() {
        return Ok(());
    }

    // 2. Endpoint costs over decided claims.
    let costs: Vec<(String, f64)> = per_endpoint
        .iter()
        .map(|(eid, (settled, unreachable))| (eid.clone(), endpoint_cost(settled, *unreachable)))
        .collect();

    // Each pass reports once. Do not finish deepening while claims are still being opened.
    let world = fetch(ctx, api, headers, "Worlds", world_id)
        .ok_or("cannot read world before reporting settled paths")?;
    let phase = row_str(&world, "ExplorationPhase");
    let complete = row_bool(&world, "first_pass_complete", "FirstPassComplete");
    if phase == "complete"
        || (phase == "first_pass" && complete)
        || (phase.is_empty() && !row_str(&world, "CanonicalPathId").is_empty())
    {
        return Ok(());
    }
    if phase == "first_pass" {
        // Fast claims may settle before another endpoint finishes decomposition.
        // Only a complete attached claim set can close the pass. Individual
        // accepted paths publish their predictions independently of this barrier.
        let endpoints = list(
            ctx,
            api,
            headers,
            "Endpoints",
            &format!("world_id eq '{world_id}'"),
        )?;
        let expected_endpoints = row_str(&world, "EndpointBudget")
            .parse::<usize>()
            .unwrap_or(3)
            .clamp(1, 3);
        if endpoints.len() < expected_endpoints {
            return Ok(());
        }
        let listed_claims: std::collections::BTreeSet<&str> = claims.iter().map(row_id).collect();
        for row in endpoints {
            let endpoint = fetch(ctx, api, headers, "Endpoints", row_id(&row))
                .ok_or("cannot read endpoint decomposition progress")?;
            if matches!(row_status(&endpoint), "Failed" | "Discarded") {
                continue;
            }
            let attached: Vec<String> = serde_json::from_str(row_str(&endpoint, "ClaimIds"))
                .map_err(|e| format!("invalid endpoint claim list: {e}"))?;
            if attached.is_empty()
                || attached
                    .iter()
                    .any(|id| !listed_claims.contains(id.as_str()))
            {
                return Ok(());
            }
        }
    }
    if phase == "deepening" {
        let ids: Vec<String> = serde_json::from_str(row_str(&world, "DeepeningClaimIds"))
            .map_err(|e| format!("invalid deepening claim list: {e}"))?;
        let cursor = row_str(&world, "DeepeningCursor")
            .parse::<usize>()
            .unwrap_or(0);
        if cursor < ids.len() {
            return Ok(());
        }
        for id in ids {
            let claim = fetch(ctx, api, headers, "Claims", &id)
                .ok_or("cannot read background claim progress")?;
            if !row_bool(&claim, "deepening_started", "DeepeningStarted")
                || matches!(row_status(&claim), "Proposed" | "Bridging")
            {
                return Ok(());
            }
        }
    }
    let reported_phase = if phase == "first_pass" {
        "first_pass"
    } else {
        "complete"
    };

    for (endpoint_id, weight) in endpoint_weights(&costs) {
        let endpoint = fetch(ctx, api, headers, "Endpoints", &endpoint_id)
            .ok_or("cannot read endpoint for weighting")?;
        let weight_action = if row_status(&endpoint) == "Weighted" {
            "Reweigh"
        } else {
            "ScoreComplete"
        };
        if let Err(e) = dispatch(
            ctx,
            api,
            headers,
            "Endpoints",
            &endpoint_id,
            weight_action,
            &json!({ "weight": format!("{:.4}", weight) }),
        ) {
            ctx.log("warn", &format!("aggregate_costs: {e}"));
            continue;
        }
        let _ = dispatch(
            ctx,
            api,
            headers,
            "Endpoints",
            &endpoint_id,
            "MarkWeighted",
            &json!({}),
        );
    }

    // 4. The world receipt: the canonical endpoint is the cheapest; its
    // cheapest settled route stands as the representative canonical path.
    let canonical_endpoint = costs
        .iter()
        .min_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|(id, _)| id.clone())
        .unwrap_or_default();
    let canonical_path_id = cheapest_route
        .get(&canonical_endpoint)
        .map(|(_, rid)| rid.clone())
        .unwrap_or_default();

    dispatch(
        ctx,
        api,
        headers,
        "Worlds",
        world_id,
        "PathsScored",
        &json!({ "canonical_path_id": canonical_path_id, "exploration_phase": reported_phase, "expected_exploration_phase": phase }),
    )?;
    ctx.log(
        "info",
        &format!(
            "aggregate_costs: pass settled for world {world_id}; canonical endpoint \
             {canonical_endpoint}, representative route {canonical_path_id}"
        ),
    );
    Ok(())
}

/// The 0.2 endpoint-level classify pass, kept for legacy paths (claim_id
/// empty). In-place evolution: old worlds keep working (ADR-003).
fn legacy_classify_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let world_id = fields
        .get("world_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("Path.world_id is required for classification")?
        .to_string();

    let temper_api_url = api_url(ctx);
    let headers = system_headers(ctx);

    // 1. Load every path for this world.
    let paths = list(
        ctx,
        &temper_api_url,
        &headers,
        "Paths",
        &format!("world_id eq '{world_id}'"),
    )?;

    // 2. The pass is settled when no path is still in flight.
    let mut scored: Vec<(String, f64, String)> = Vec::new();
    for p in &paths {
        let id = row_id(p).to_string();
        let mut status = row_status(p).to_string();
        let mut cost_src = row_str(p, "RepairCost").to_string();
        let mut endpoint_src = row_str(p, "EndpointId").to_string();
        if matches!(status.as_str(), "Solving" | "Repaired" | "Challenged") {
            if id == ctx.entity_id {
                status = "Scored".to_string();
                let own = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
                if let Some(v) = own.get("repair_cost").and_then(|v| v.as_str()) {
                    cost_src = v.to_string();
                }
                if let Some(v) = own.get("endpoint_id").and_then(|v| v.as_str()) {
                    endpoint_src = v.to_string();
                }
            } else if let Some(fresh) = fetch(ctx, &temper_api_url, &headers, "Paths", &id) {
                status = row_status(&fresh).to_string();
                cost_src = row_str(&fresh, "RepairCost").to_string();
                endpoint_src = row_str(&fresh, "EndpointId").to_string();
            }
        }
        match status.as_str() {
            "Solving" | "Repaired" | "Challenged" => {
                ctx.log(
                    "info",
                    &format!("aggregate_costs: path {id} still {status}; pass not settled"),
                );
                set_success_result("", &json!({}));
                return Ok(());
            }
            "Scored" => {
                let cost = cost_src.parse::<f64>().unwrap_or(f64::MAX);
                scored.push((id, cost, endpoint_src));
            }
            _ => {} // Canonical/Tail/Rejected/Pruned/Failed from earlier passes
        }
    }

    if scored.is_empty() {
        ctx.log(
            "info",
            "aggregate_costs: no freshly scored paths to classify",
        );
        set_success_result("", &json!({}));
        return Ok(());
    }

    // 3. Classify deterministically.
    let pairs: Vec<(String, f64)> = scored.iter().map(|(id, c, _)| (id.clone(), *c)).collect();
    let classified = classify(&pairs);
    let mut canonical_path_id = String::new();
    for (id, class) in &classified {
        let (action, note) = match class {
            Classification::Canonical => {
                canonical_path_id = id.clone();
                ("ClassifyCanonical", "lowest repair cost in pass")
            }
            Classification::Tail => ("ClassifyTail", "coherent but expensive: tail sample"),
            Classification::Reject => ("Reject", "repair cost beyond world tolerance"),
        };
        if let Err(e) = dispatch(
            ctx,
            &temper_api_url,
            &headers,
            "Paths",
            id,
            action,
            &json!({ "classification_note": note }),
        ) {
            ctx.log("warn", &format!("aggregate_costs: {e}"));
        }
    }

    // 4. Endpoint weights from each endpoint's best path cost.
    let mut best: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for (_, cost, endpoint_id) in &scored {
        if endpoint_id.is_empty() {
            continue;
        }
        let entry = best.entry(endpoint_id.clone()).or_insert(f64::MAX);
        if *cost < *entry {
            *entry = *cost;
        }
    }
    let best_costs: Vec<(String, f64)> = best.into_iter().collect();
    for (endpoint_id, weight) in endpoint_weights(&best_costs) {
        if dispatch(
            ctx,
            &temper_api_url,
            &headers,
            "Endpoints",
            &endpoint_id,
            "ScoreComplete",
            &json!({ "weight": format!("{:.4}", weight) }),
        )
        .is_ok()
        {
            let _ = dispatch(
                ctx,
                &temper_api_url,
                &headers,
                "Endpoints",
                &endpoint_id,
                "MarkWeighted",
                &json!({}),
            );
        }
    }

    // 5. Report the settled pass to the world — once.
    let mut expected_phase = String::new();
    if let Some(world) = fetch(ctx, &temper_api_url, &headers, "Worlds", &world_id) {
        expected_phase = row_str(&world, "ExplorationPhase").to_owned();
        if !row_str(&world, "CanonicalPathId").is_empty() {
            ctx.log(
                "info",
                "aggregate_costs: pass already reported (canonical set); skipping duplicate PathsScored",
            );
            set_success_result("", &json!({}));
            return Ok(());
        }
    }
    dispatch(
        ctx,
        &temper_api_url,
        &headers,
        "Worlds",
        &world_id,
        "PathsScored",
        &json!({ "canonical_path_id": canonical_path_id, "exploration_phase": "complete", "expected_exploration_phase": expected_phase }),
    )?;
    ctx.log(
        "info",
        &format!(
            "aggregate_costs: pass settled for world {world_id}; canonical={canonical_path_id}"
        ),
    );
    set_success_result("", &json!({}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(kind: &str, severity: &str) -> Value {
        json!({"kind": kind, "severity": severity, "note": "t"})
    }

    #[test]
    fn first_pass_publishes_after_one_challenged_route_without_extra_search() {
        let budget = search_budget(&json!({"exploration_phase":"first_pass"}));
        assert_eq!(route_decision(0, 80.0, budget), RouteDecision::Score);
        assert_eq!(
            claim_decision(1, false, &[("p".into(), 202.5)], budget),
            ClaimDecision::Settle {
                route_id: "p".into(),
                cost: 202.5
            }
        );
        assert!(matches!(
            claim_decision(1, false, &[("p".into(), 300.0)], budget),
            ClaimDecision::Unreachable { .. }
        ));
        assert_eq!(claim_decision(1, true, &[], budget), ClaimDecision::Wait);
        assert_eq!(search_budget(&json!({})), DEEP_BUDGET);
        assert_eq!(
            search_budget(&json!({"exploration_phase":"deepening"})),
            DEEP_BUDGET
        );
    }

    #[test]
    fn background_cursor_visits_each_claim_once_and_carries_its_fence() {
        for count in 0..=9 {
            let ids: Vec<String> = (0..count).map(|i| format!("c-{i}")).collect();
            let mut fields = json!({"deepening_claim_ids":serde_json::to_string(&ids).unwrap(),
                "deepening_cursor":"0"});
            let mut visited = Vec::new();
            while let Some(step) = deepening_step(&fields).unwrap() {
                assert_eq!(
                    step["expected_deepening_cursor"],
                    fields["deepening_cursor"]
                );
                visited.push(step["deepening_claim_id"].as_str().unwrap().to_owned());
                fields["deepening_cursor"] = step["deepening_cursor"].clone();
            }
            assert_eq!(visited, ids);
        }
        assert!(
            deepening_step(&json!({"deepening_claim_ids":"[]", "deepening_cursor":"bad"})).is_err()
        );
    }

    #[test]
    fn cost_sums_kind_weights_times_severity() {
        let cost_flags = json!([flag("contradiction", "high"), flag("lag", "low")]);
        let challenge_flags = json!([flag("incentive", "medium")]);
        // 25*2 + 10*0.5 + 15*1 = 70
        assert_eq!(repair_cost(&cost_flags, &challenge_flags), 70.0);
    }

    #[test]
    fn cost_accepts_json_strings_and_is_order_independent() {
        let as_string =
            json!(r#"[{"kind":"miracle","severity":"medium"},{"kind":"lag","severity":"high"}]"#);
        // 40 + 20 = 60
        assert_eq!(repair_cost(&as_string, &json!([])), 60.0);
        assert_eq!(repair_cost(&json!([]), &as_string), 60.0);
    }

    #[test]
    fn empty_and_malformed_flags_cost_nothing_but_unknown_kinds_are_not_free() {
        assert_eq!(repair_cost(&json!([]), &json!([])), 0.0);
        assert_eq!(repair_cost(&json!("not json"), &json!(null)), 0.0);
        // Unknown kind prices like a contradiction.
        assert_eq!(
            repair_cost(&json!([flag("mystery", "medium")]), &json!([])),
            25.0
        );
    }

    #[test]
    fn deformation_prices_like_contradiction_so_drift_is_never_free() {
        // ADR-004 commitment 4: amending a claim toward consensus costs.
        assert_eq!(kind_weight("deformation"), 25.0);
        assert_eq!(
            repair_cost(&json!([flag("deformation", "high")]), &json!([])),
            50.0
        );
    }

    #[test]
    fn classification_is_deterministic_with_lexicographic_tiebreak() {
        let paths = vec![
            ("p-b".to_string(), 10.0),
            ("p-a".to_string(), 10.0),
            ("p-c".to_string(), 35.0),
            ("p-d".to_string(), 41.0),
        ];
        let classes = classify(&paths);
        // p-a wins the tie lexicographically; ceiling = 40.
        assert_eq!(classes[0], ("p-a".to_string(), Classification::Canonical));
        assert_eq!(classes[1], ("p-b".to_string(), Classification::Tail));
        assert_eq!(classes[2], ("p-c".to_string(), Classification::Tail));
        assert_eq!(classes[3], ("p-d".to_string(), Classification::Reject));
        // Same inputs, same ranking.
        assert_eq!(classify(&paths), classify(&paths));
    }

    #[test]
    fn endpoint_weights_normalize_and_favor_cheap_endpoints() {
        let weights = endpoint_weights(&[("e-1".to_string(), 17.5), ("e-2".to_string(), 50.0)]);
        let total: f64 = weights.iter().map(|(_, w)| w).sum();
        assert!((total - 1.0).abs() < 0.001);
        assert!(weights[0].1 > weights[1].1);
    }

    // --- Search policy (ADR-004) ---

    #[test]
    fn routes_revise_only_on_expensive_objections_within_round_budget() {
        // 20-point objection (miracle/low) at round 0: revise.
        assert_eq!(route_decision(0, 40.0, DEEP_BUDGET), RouteDecision::Revise);
        // Cheap objections never trigger revision.
        assert_eq!(route_decision(0, 30.0, DEEP_BUDGET), RouteDecision::Score);
        // Round budget spent: score regardless.
        assert_eq!(
            route_decision(MAX_ROUNDS, 80.0, DEEP_BUDGET),
            RouteDecision::Score
        );
        assert_eq!(route_decision(1, 50.0, DEEP_BUDGET), RouteDecision::Revise);
    }

    #[test]
    fn first_routes_never_prune_and_hopeless_routes_always_do() {
        assert!(!should_prune(500.0, None));
        // best 30 -> ceiling 80
        assert!(!should_prune(80.0, Some(30.0)));
        assert!(should_prune(80.1, Some(30.0)));
    }

    #[test]
    fn claim_decisions_wait_search_settle_and_give_up_honestly() {
        let scored = |v: &[(&str, f64)]| -> Vec<(String, f64)> {
            v.iter().map(|(s, c)| (s.to_string(), *c)).collect()
        };
        // In-flight routes always wait.
        assert_eq!(
            claim_decision(1, true, &scored(&[("p-1", 10.0)]), DEEP_BUDGET),
            ClaimDecision::Wait
        );
        // Nothing scored, budget left: alternate.
        assert_eq!(
            claim_decision(1, false, &[], DEEP_BUDGET),
            ClaimDecision::Alternate
        );
        // Nothing scored, budget spent: unreachable.
        assert!(matches!(
            claim_decision(MAX_ROUTES, false, &[], DEEP_BUDGET),
            ClaimDecision::Unreachable { .. }
        ));
        // Best above the bound, budget left: alternate.
        assert_eq!(
            claim_decision(1, false, &scored(&[("p-1", 241.0)]), DEEP_BUDGET),
            ClaimDecision::Alternate
        );
        // Best above the bound, budget spent: unreachable, honestly.
        assert!(matches!(
            claim_decision(MAX_ROUTES, false, &scored(&[("p-1", 241.0)]), DEEP_BUDGET),
            ClaimDecision::Unreachable { .. }
        ));
        // Acceptable best settles on the cheapest with id tiebreak.
        assert_eq!(
            claim_decision(
                2,
                false,
                &scored(&[("p-b", 25.0), ("p-a", 25.0)]),
                DEEP_BUDGET
            ),
            ClaimDecision::Settle {
                route_id: "p-a".to_string(),
                cost: 25.0
            }
        );
        // Liveness: a wedged straggler must NOT block a claim that already has
        // an acceptable route once the route budget is spent. The search has
        // its answer; a dead/flapping route can only match or slightly beat a
        // route we'd already accept, so settle on the cheapest acceptable
        // instead of waiting forever (this is what bounds the ADR-007
        // self-heal — the budget caps it).
        assert_eq!(
            claim_decision(
                MAX_ROUTES,
                true,
                &scored(&[("p-2", 142.5), ("p-1", 300.0)]),
                DEEP_BUDGET
            ),
            ClaimDecision::Settle {
                route_id: "p-2".to_string(),
                cost: 142.5
            }
        );
        // Budget left: an in-flight route still blocks. Path self-heal owns
        // the stuck-route case here; we don't settle early while a fresh
        // alternate could still open.
        assert_eq!(
            claim_decision(1, true, &scored(&[("p-1", 142.5)]), DEEP_BUDGET),
            ClaimDecision::Wait
        );
        // Budget spent and in flight, but no acceptable route yet: keep
        // waiting on the only route that could still make the claim reachable.
        assert_eq!(
            claim_decision(MAX_ROUTES, true, &scored(&[("p-1", 300.0)]), DEEP_BUDGET),
            ClaimDecision::Wait
        );
    }

    #[test]
    fn claim_classification_splits_reachable_from_strained() {
        assert_eq!(claim_classification(120.0), "reachable");
        assert_eq!(claim_classification(120.1), "strained");
        assert_eq!(claim_classification(0.0), "reachable");
    }

    #[test]
    fn endpoint_cost_averages_settled_and_penalizes_unreachable() {
        // Two settled at 100 and 200, one unreachable at the 240 penalty:
        // (100+200+240)/3 = 180.
        assert_eq!(endpoint_cost(&[100.0, 200.0], 1), 180.0);
        assert_eq!(endpoint_cost(&[], 2), UNREACHABLE_PENALTY);
        assert_eq!(endpoint_cost(&[], 0), f64::MAX);
    }

    #[test]
    fn briefs_rank_expensive_objections_first_and_stay_bounded() {
        let flags = json!([
            {"kind": "lag", "severity": "low", "note": "cheap one"},
            {"kind": "miracle", "severity": "high", "note": "the big one"},
        ]);
        let brief = brief_from_flags(&flags);
        let big = brief.find("the big one").unwrap();
        let cheap = brief.find("cheap one").unwrap();
        assert!(big < cheap, "expensive objections come first");
        assert!(brief.len() <= 2000);
        assert_eq!(brief_from_flags(&json!([])), "");
    }

    #[test]
    fn max_flag_points_reads_the_single_most_expensive_flag() {
        let flags = json!([
            flag("lag", "low"),
            flag("miracle", "low"),
            flag("incentive", "high")
        ]);
        // 5, 20, 30 -> 30
        assert_eq!(max_flag_points(&flags), 30.0);
        assert_eq!(max_flag_points(&json!([])), 0.0);
    }

    #[test]
    fn search_budget_worst_case_is_bounded() {
        // The plan's hard bound: routes per claim and rounds per route are
        // structural constants, not suggestions.
        assert!(
            MAX_ROUTES * (1 + MAX_ROUNDS) <= 9,
            "search budget per claim"
        );
    }
}
