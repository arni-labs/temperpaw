//! adjudicate_nodes — judged resolution for overdue authored nodes
//! (ADR-004 commitment 2).
//!
//! Two triggers land here, split on the triggering action:
//! - World.AdjudicateNodes: sweep the world's EventNodes for overdue
//!   authored Confirmed nodes (resolve_by <= last_adjudication_date — the
//!   caller supplies the as-of date, WASM has no clock) and spawn ONE
//!   adjudicator session per node (capped per dispatch). The adjudicator
//!   researches with web tools and self-reports a structured verdict via
//!   EventNode.AdjudicationVerdict.
//! - EventNode.AdjudicationVerdict: validate the verdict deterministically
//!   and own the transition — Resolve only for a decisive yes/no with
//!   confidence at or above the threshold and 2+ independent evidence refs;
//!   everything else flags the node for a human resolver.
//!
//! The LLM judges; the WASM owns the transition. Hindcast worlds
//! (hindcast_mode = "true") never adjudicate: their vantage is frozen and
//! web evidence is forbidden by design.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const ADJUDICATOR_TOOLS: &str =
    "temper_get,temper_list,temper_action,temper_web_search,temper_web_fetch";
const ADJUDICATOR_MAX_TURNS: &str = "25";

/// At most this many adjudicator sessions per AdjudicateNodes dispatch.
/// Overflow is logged loudly and picked up by the next dispatch.
const MAX_ADJUDICATIONS_PER_DISPATCH: usize = 5;

/// Tunable prior (ADR-004 commitment 1): the minimum self-reported
/// confidence at which the module accepts a judged yes/no. Inclusive.
const CONFIDENCE_THRESHOLD: f64 = 0.8;

/// Minimum count of independent evidence refs behind a judged resolution.
const MIN_EVIDENCE_REFS: usize = 2;

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

// --- Pure rules ---------------------------------------------------------------

/// Hindcast worlds never adjudicate: exact-match contract, mirroring
/// seed_world's tool stripping.
fn is_hindcast(mode: &str) -> bool {
    mode == "true"
}

/// An adjudication candidate is a Confirmed authored node whose resolve_by
/// has passed (ISO date strings compare lexicographically; <= because a node
/// due today is due) and which is not already flagged for a human.
fn is_candidate(
    status: &str,
    provenance: &str,
    resolve_by: &str,
    needs_human_resolution: &str,
    as_of: &str,
) -> bool {
    status == "Confirmed"
        && provenance == "authored"
        && !resolve_by.is_empty()
        && resolve_by <= as_of
        && needs_human_resolution.trim().is_empty()
}

/// Take at most the per-dispatch cap, reporting how many were dropped.
fn split_at_cap<T>(mut candidates: Vec<T>) -> (Vec<T>, usize) {
    let dropped = candidates
        .len()
        .saturating_sub(MAX_ADJUDICATIONS_PER_DISPATCH);
    candidates.truncate(MAX_ADJUDICATIONS_PER_DISPATCH);
    (candidates, dropped)
}

/// Unique non-empty string entries of a JSON-array evidence_refs field.
/// Duplicated URLs are one piece of evidence: "2+ independent" cannot be
/// satisfied by repetition. Malformed input yields no evidence — never a
/// parse error, because thin evidence is a FlagForHuman, not a module crash.
fn parse_evidence_refs(raw: &str) -> Vec<String> {
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    for v in arr {
        if let Some(s) = v.as_str() {
            let s = s.trim();
            if !s.is_empty() && !seen.iter().any(|x| x == s) {
                seen.push(s.to_string());
            }
        }
    }
    seen
}

/// The module's decision after validating an adjudicator verdict.
#[derive(Debug, PartialEq)]
enum Decision {
    Resolve,
    FlagForHuman { reason: String },
}

/// Deterministic verdict validation: Resolve only when the outcome is a
/// decisive yes/no AND confidence is at or above the threshold AND at least
/// MIN_EVIDENCE_REFS independent evidence refs back it. Everything else —
/// unresolved, malformed, underconfident, thin — goes to a human. Checked
/// most-fundamental-defect first so the flag reason names the real problem.
fn verdict_decision(outcome: &str, confidence: Option<f64>, evidence_count: usize) -> Decision {
    match outcome {
        "yes" | "no" => {}
        "unresolved" => {
            return Decision::FlagForHuman {
                reason: "adjudicator reported the outcome unresolved".to_string(),
            };
        }
        other => {
            return Decision::FlagForHuman {
                reason: format!("invalid adjudicated outcome {other:?} (expected yes|no|unresolved)"),
            };
        }
    }
    let Some(conf) = confidence else {
        return Decision::FlagForHuman {
            reason: "adjudication confidence missing or unparseable".to_string(),
        };
    };
    if !(0.0..=1.0).contains(&conf) {
        // NaN fails the range check too: NaN comparisons are always false.
        return Decision::FlagForHuman {
            reason: format!("adjudication confidence {conf} is not a probability in [0, 1]"),
        };
    }
    if conf < CONFIDENCE_THRESHOLD {
        return Decision::FlagForHuman {
            reason: format!(
                "adjudication confidence {conf} below threshold {CONFIDENCE_THRESHOLD}"
            ),
        };
    }
    if evidence_count < MIN_EVIDENCE_REFS {
        return Decision::FlagForHuman {
            reason: format!(
                "{evidence_count} independent evidence ref(s), {MIN_EVIDENCE_REFS} required"
            ),
        };
    }
    Decision::Resolve
}

// --- Adjudicator prompt (spawn phase) ------------------------------------------

/// The adjudicator's working contract. Single source of truth for the
/// entity set, action, and parameter names it must use — tested below.
fn adjudicator_prompt(node_id: &str, statement: &str, resolve_by: &str) -> String {
    format!(
        "You are the adjudicator for EventNode {node_id}. Its resolve-by date has passed; \
         your job is to judge what actually happened — with cited evidence, never from \
         memory or vibes.\n\n\
         STATEMENT: {statement}\n\
         RESOLVE BY: {resolve_by}\n\n\
         Rubric:\n\
         1. Use temper.web_search / temper.web_fetch to find at least 2 INDEPENDENT pieces \
         of evidence (different publishers or primary sources) bearing on whether the \
         statement came true by {resolve_by}. Cite the URL of every piece you rely on.\n\
         2. Decide \"yes\" or \"no\" ONLY if the evidence is decisive. If the evidence is \
         thin, conflicting, or merely suggestive, report \"unresolved\" — an honest \
         \"unresolved\" beats a guessed \"no\".\n\
         3. Report your confidence as a number between 0 and 1.\n\
         4. Never infer an outcome from the absence of news alone: finding nothing is \
         \"unresolved\", not \"no\".\n\n\
         Then self-report EXACTLY:\n\
         temper.action(\"EventNodes\", \"{node_id}\", \"AdjudicationVerdict\", \
         {{\"adjudicated_outcome\": \"yes|no|unresolved\", \
         \"adjudication_evidence_refs\": \"[\\\"<url>\\\", ...]\", \
         \"adjudication_confidence\": \"<0-1>\"}})\n\
         Then call temper.done(\"complete\")."
    )
}

// --- Session spawning -----------------------------------------------------------

/// Workspace name for a world — the cross-module rendezvous key. Every
/// session-spawning module must resolve the same per-world workspace by
/// this exact name.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

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

/// Resolve (or create) the per-world PawFS workspace and return its id.
/// Every corridor session is Configured with the world workspace uniformly —
/// without one, any PawFS File create is denied (resource.workspaceId must
/// match the principal's workspace).
fn ensure_world_workspace(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<String, String> {
    let name = workspace_name(world_id);
    // Workspace rows are not readable by agent principals (paw-fs Cedar has
    // no read/list permit on Workspace), so idempotent lookup is impossible:
    // create one per spawn batch. Correctness needs only that each session's
    // Configure workspace matches the files it writes; file READS are
    // unrestricted, so cross-session reads work across workspaces.
    let create_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Workspaces"),
        headers,
        &json!({ "name": name, "quota_limit": "104857600" }).to_string(),
    )?;
    if create_resp.status < 200 || create_resp.status >= 300 {
        return Err(format!(
            "create Workspace {name} failed (HTTP {})",
            create_resp.status
        ));
    }
    serde_json::from_str::<Value>(&create_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| format!("Workspace {name} create returned no entity_id"))
}

#[allow(clippy::too_many_arguments)]
fn spawn_session(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    name: &str,
    role: &str,
    model: &str,
    provider: &str,
    tools: &str,
    max_turns: &str,
    user_message: &str,
    workspace_id: &str,
) -> Result<String, String> {
    let agent_body = json!({ "Name": name, "Role": role });
    let agent_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Agents"),
        headers,
        &agent_body.to_string(),
    )?;
    if agent_resp.status < 200 || agent_resp.status >= 300 {
        return Err(format!(
            "create Agent {name} failed (HTTP {})",
            agent_resp.status
        ));
    }
    let agent_id = serde_json::from_str::<Value>(&agent_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Agent create returned no entity_id")?;

    let session_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions"),
        headers,
        &json!({ "agent_id": agent_id }).to_string(),
    )?;
    if session_resp.status < 200 || session_resp.status >= 300 {
        return Err(format!(
            "create Session for {name} failed (HTTP {})",
            session_resp.status
        ));
    }
    let session_id = serde_json::from_str::<Value>(&session_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Session create returned no entity_id")?;

    // The prompt needs real ids the module only knows now; substitute them.
    let message = user_message
        .replace("{AGENT_ID}", &agent_id)
        .replace("{SESSION_ID}", &session_id);
    let configure_body = json!({
        "model": model,
        "provider": provider,
        "agent_name": role,
        "tools_enabled": tools,
        "tool_choice": "required",
        "max_turns": max_turns,
        "user_message": message,
        "sandbox_url": "none",
        "workspace_id": workspace_id,
        "temper_api_url": api
    });
    let configure_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions('{session_id}')/TemperPaw.Configure"),
        headers,
        &configure_body.to_string(),
    )?;
    if configure_resp.status < 200 || configure_resp.status >= 300 {
        return Err(format!(
            "Configure for {name} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("adjudicate_nodes: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(session_id)
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        match ctx.trigger_action.as_str() {
            "AdjudicateNodes" => spawn_phase(&ctx, &fields),
            "AdjudicationVerdict" => validate_phase(&ctx, &fields),
            other => Err(format!(
                "adjudicate_nodes: unexpected trigger action {other:?}; this module handles \
                 World.AdjudicateNodes and EventNode.AdjudicationVerdict only"
            )),
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

/// World.AdjudicateNodes: sweep for overdue authored nodes and spawn one
/// adjudicator session per candidate, capped per dispatch.
fn spawn_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let world_id = ctx.entity_id.clone();

    // Hindcast worlds never adjudicate: their vantage is frozen and web
    // evidence is forbidden by design — a judged "what actually happened"
    // would leak post-vantage knowledge into the run.
    if is_hindcast(&get_str(fields, "hindcast_mode")) {
        ctx.log(
            "info",
            &format!(
                "adjudicate_nodes: world {world_id} is a hindcast world — adjudication is \
                 disabled by design (frozen vantage, web evidence forbidden); nothing to do"
            ),
        );
        set_success_result("", &json!({}));
        return Ok(());
    }

    let as_of = get_str(fields, "last_adjudication_date");
    if as_of.trim().is_empty() {
        return Err(
            "World.last_adjudication_date is required: the caller of AdjudicateNodes supplies \
             the as-of date (WASM has no clock)"
                .to_string(),
        );
    }
    let model = get_str(fields, "agent_model");
    let provider = get_str(fields, "agent_provider");
    if model.trim().is_empty() || provider.trim().is_empty() {
        return Err("World.agent_model and World.agent_provider are required".to_string());
    }

    let api = api_url(ctx);
    let headers = system_headers(ctx);

    // 1. Every EventNode in this world.
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

    // 2. Candidates: Confirmed, authored, overdue, unflagged. Sorted by
    // (resolve_by, id) so the most overdue go first and the cap is
    // deterministic across reruns.
    let mut candidates: Vec<(String, String, String)> = Vec::new(); // (resolve_by, id, statement)
    for node in &nodes {
        let id = row_id(node);
        if id.is_empty() {
            continue;
        }
        if is_candidate(
            row_status(node),
            row_str(node, "Provenance"),
            row_str(node, "ResolveBy"),
            row_str(node, "NeedsHumanResolution"),
            &as_of,
        ) {
            candidates.push((
                row_str(node, "ResolveBy").to_string(),
                id.to_string(),
                row_str(node, "Statement").to_string(),
            ));
        }
    }
    candidates.sort();
    let total = candidates.len();
    let (candidates, dropped) = split_at_cap(candidates);
    if dropped > 0 {
        ctx.log(
            "warn",
            &format!(
                "adjudicate_nodes: world {world_id} has {total} overdue candidate(s) but the \
                 per-dispatch cap is {MAX_ADJUDICATIONS_PER_DISPATCH} — DROPPING {dropped} this \
                 dispatch; they stay Confirmed and will be picked up by the next AdjudicateNodes"
            ),
        );
    }
    if candidates.is_empty() {
        ctx.log(
            "info",
            &format!(
                "adjudicate_nodes: world {world_id} has no overdue authored nodes to adjudicate \
                 as of {as_of}"
            ),
        );
        set_success_result("", &json!({}));
        return Ok(());
    }

    // 3. One adjudicator session per candidate node.
    let workspace_id = ensure_world_workspace(ctx, &api, &headers, &world_id)?;
    let mut spawned = 0usize;
    for (resolve_by, node_id, statement) in &candidates {
        let prompt = adjudicator_prompt(node_id, statement, resolve_by);
        match spawn_session(
            ctx,
            &api,
            &headers,
            &format!("Adjudicator-{node_id}"),
            "adjudicator",
            &model,
            &provider,
            ADJUDICATOR_TOOLS,
            ADJUDICATOR_MAX_TURNS,
            &prompt,
            &workspace_id,
        ) {
            Ok(_) => spawned += 1,
            Err(e) => ctx.log(
                "warn",
                &format!("adjudicate_nodes: adjudicator spawn for node {node_id} failed: {e}"),
            ),
        }
    }
    if spawned == 0 {
        return Err(format!(
            "adjudicate_nodes: every adjudicator spawn failed ({} candidate(s))",
            candidates.len()
        ));
    }
    ctx.log(
        "info",
        &format!(
            "adjudicate_nodes: world {world_id} — spawned {spawned}/{} adjudicator session(s) \
             as of {as_of}",
            candidates.len()
        ),
    );
    // No result action: each adjudicator self-reports AdjudicationVerdict,
    // which re-enters this module in the validate phase. A successful run
    // with nothing to dispatch must still set a result: the host treats an
    // empty result as failure.
    set_success_result("", &json!({}));
    Ok(())
}

/// EventNode.AdjudicationVerdict: deterministic validation; the module owns
/// the Resolve / FlagForHuman transition.
fn validate_phase(ctx: &Context, fields: &Value) -> Result<(), String> {
    let node_id = ctx.entity_id.clone();
    let outcome = get_str(fields, "adjudicated_outcome");
    let confidence_raw = get_str(fields, "adjudication_confidence");
    let confidence = confidence_raw.trim().parse::<f64>().ok();
    let refs = parse_evidence_refs(&get_str(fields, "adjudication_evidence_refs"));

    match verdict_decision(&outcome, confidence, refs.len()) {
        Decision::Resolve => {
            // resolved_at is the node's resolve_by: WASM has no clock, and
            // the adjudication answers "had this happened by resolve_by".
            let resolve_by = get_str(fields, "resolve_by");
            if resolve_by.trim().is_empty() {
                ctx.log(
                    "warn",
                    &format!(
                        "adjudicate_nodes: node {node_id} verdict was decisive but the node has \
                         no resolve_by to date the resolution; flagging for human"
                    ),
                );
                set_success_result(
                    "FlagForHuman",
                    &json!({
                        "needs_human_resolution":
                            "adjudication inconclusive: verdict was decisive but the node has no \
                             resolve_by to date the resolution"
                    }),
                );
                return Ok(());
            }
            let source_refs =
                serde_json::to_string(&refs).unwrap_or_else(|_| "[]".to_string());
            ctx.log(
                "info",
                &format!(
                    "adjudicate_nodes: node {node_id} adjudicated {outcome} (confidence \
                     {confidence_raw}, {} independent evidence refs); resolving",
                    refs.len()
                ),
            );
            set_success_result(
                "Resolve",
                &json!({
                    "resolution": outcome,
                    "resolved_at": resolve_by,
                    "source_refs": source_refs,
                }),
            );
        }
        Decision::FlagForHuman { reason } => {
            ctx.log(
                "info",
                &format!(
                    "adjudicate_nodes: node {node_id} verdict did not clear the bar ({reason}); \
                     flagging for human"
                ),
            );
            set_success_result(
                "FlagForHuman",
                &json!({
                    "needs_human_resolution": format!("adjudication inconclusive: {reason}")
                }),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Deterministic verdict decision matrix (exhaustive) -----------------

    #[test]
    fn decisive_outcomes_with_confidence_and_evidence_resolve() {
        for outcome in ["yes", "no"] {
            for conf in [0.8, 0.9, 1.0] {
                for evidence in [2usize, 3, 7] {
                    assert_eq!(
                        verdict_decision(outcome, Some(conf), evidence),
                        Decision::Resolve,
                        "{outcome}/{conf}/{evidence} must resolve"
                    );
                }
            }
        }
    }

    #[test]
    fn confidence_threshold_is_exactly_the_tunable_prior() {
        // The tunable prior (ADR-004 commitment 1): 0.8, inclusive.
        assert_eq!(CONFIDENCE_THRESHOLD, 0.8);
        assert_eq!(verdict_decision("yes", Some(0.8), 2), Decision::Resolve);
        assert!(matches!(
            verdict_decision("yes", Some(0.79), 2),
            Decision::FlagForHuman { .. }
        ));
        assert!(matches!(
            verdict_decision("no", Some(0.7999), 5),
            Decision::FlagForHuman { .. }
        ));
    }

    #[test]
    fn unresolved_always_flags_even_with_perfect_confidence_and_evidence() {
        match verdict_decision("unresolved", Some(1.0), 5) {
            Decision::FlagForHuman { reason } => assert!(
                reason.contains("unresolved"),
                "reason must say why: {reason}"
            ),
            Decision::Resolve => panic!("unresolved must never resolve"),
        }
    }

    #[test]
    fn invalid_outcomes_flag_with_the_offending_value_in_the_reason() {
        for outcome in ["", "maybe", "YES", "No", "true", "0.9"] {
            match verdict_decision(outcome, Some(0.95), 3) {
                Decision::FlagForHuman { reason } => assert!(
                    reason.contains("outcome"),
                    "reason for {outcome:?} must mention the outcome: {reason}"
                ),
                Decision::Resolve => panic!("invalid outcome {outcome:?} must never resolve"),
            }
        }
    }

    #[test]
    fn missing_or_unparseable_confidence_flags() {
        match verdict_decision("yes", None, 3) {
            Decision::FlagForHuman { reason } => {
                assert!(reason.contains("confidence"), "got: {reason}")
            }
            Decision::Resolve => panic!("missing confidence must never resolve"),
        }
    }

    #[test]
    fn out_of_range_confidence_flags() {
        for conf in [-0.1, 1.01, 100.0, f64::NAN] {
            assert!(
                matches!(
                    verdict_decision("yes", Some(conf), 3),
                    Decision::FlagForHuman { .. }
                ),
                "confidence {conf} is not a probability"
            );
        }
    }

    #[test]
    fn thin_evidence_flags_even_at_full_confidence() {
        assert_eq!(MIN_EVIDENCE_REFS, 2);
        for evidence in [0usize, 1] {
            match verdict_decision("yes", Some(1.0), evidence) {
                Decision::FlagForHuman { reason } => {
                    assert!(reason.contains("evidence"), "got: {reason}")
                }
                Decision::Resolve => panic!("{evidence} evidence ref(s) must never resolve"),
            }
        }
    }

    #[test]
    fn outcome_validity_is_checked_before_confidence_and_evidence() {
        // An invalid outcome with garbage everything else still names the
        // outcome as the problem — the most fundamental defect first.
        match verdict_decision("maybe", None, 0) {
            Decision::FlagForHuman { reason } => assert!(reason.contains("outcome")),
            Decision::Resolve => panic!("must flag"),
        }
    }

    // --- Evidence refs parsing ----------------------------------------------

    #[test]
    fn evidence_refs_parse_counts_unique_nonempty_strings() {
        assert_eq!(
            parse_evidence_refs(r#"["https://a.example/x", "https://b.example/y"]"#),
            vec![
                "https://a.example/x".to_string(),
                "https://b.example/y".to_string()
            ]
        );
        // Duplicates are one piece of evidence, not two: "2+ independent"
        // cannot be satisfied by repeating a URL.
        assert_eq!(
            parse_evidence_refs(r#"["https://a.example/x", "https://a.example/x"]"#).len(),
            1
        );
        // Empty strings and non-strings are not evidence.
        assert_eq!(
            parse_evidence_refs(r#"["", 42, null, "https://a.example/x"]"#).len(),
            1
        );
    }

    #[test]
    fn malformed_evidence_refs_parse_to_nothing() {
        for raw in ["not json", "{\"a\": 1}", "\"just a string\"", "", "42"] {
            assert!(
                parse_evidence_refs(raw).is_empty(),
                "{raw:?} must yield no evidence"
            );
        }
    }

    // --- Candidate selection -------------------------------------------------

    #[test]
    fn candidates_are_confirmed_authored_overdue_and_unflagged() {
        let as_of = "2026-06-01";
        assert!(is_candidate("Confirmed", "authored", "2026-05-01", "", as_of));
        // resolve_by == as_of is due (<=, ISO strings compare lexicographically).
        assert!(is_candidate("Confirmed", "authored", "2026-06-01", "", as_of));
    }

    #[test]
    fn non_candidates_are_excluded_for_every_reason() {
        let as_of = "2026-06-01";
        // Wrong status.
        for status in ["Proposed", "Resolved", "Retired", ""] {
            assert!(!is_candidate(status, "authored", "2026-05-01", "", as_of));
        }
        // Wrong provenance: judged resolution is for authored nodes only.
        for provenance in ["market", "determined", "ensemble", "base_rate", ""] {
            assert!(!is_candidate(
                "Confirmed",
                provenance,
                "2026-05-01",
                "",
                as_of
            ));
        }
        // Not yet due, or no due date at all.
        assert!(!is_candidate("Confirmed", "authored", "2026-06-02", "", as_of));
        assert!(!is_candidate("Confirmed", "authored", "", "", as_of));
        // Already flagged for a human: never re-adjudicate over their head.
        assert!(!is_candidate(
            "Confirmed",
            "authored",
            "2026-05-01",
            "adjudication inconclusive: thin evidence",
            as_of
        ));
    }

    #[test]
    fn cap_takes_the_first_five_and_reports_the_dropped_count() {
        assert_eq!(MAX_ADJUDICATIONS_PER_DISPATCH, 5);
        let (taken, dropped) = split_at_cap((0..8).collect::<Vec<_>>());
        assert_eq!(taken, vec![0, 1, 2, 3, 4]);
        assert_eq!(dropped, 3);
        let (taken, dropped) = split_at_cap(vec![1, 2]);
        assert_eq!(taken, vec![1, 2]);
        assert_eq!(dropped, 0);
        let (taken, dropped) = split_at_cap(Vec::<i32>::new());
        assert!(taken.is_empty());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn hindcast_worlds_never_adjudicate() {
        // Exact-match contract, same as seed_world's tool stripping.
        assert!(is_hindcast("true"));
        assert!(!is_hindcast("false"));
        assert!(!is_hindcast(""));
        assert!(!is_hindcast("TRUE"));
    }

    // --- Prompt contract -------------------------------------------------------

    #[test]
    fn adjudicator_prompt_carries_the_verdict_contract() {
        let p = adjudicator_prompt(
            "n-1",
            "Acme ships a flying car before year end.",
            "2026-05-31",
        );
        for needle in [
            "Acme ships a flying car before year end.",
            "2026-05-31",
            "2",
            "INDEPENDENT",
            "temper.web_search",
            "URL",
            "\"unresolved\"",
            "confidence",
            "0 and 1",
            "absence of news",
            "temper.action(\"EventNodes\", \"n-1\", \"AdjudicationVerdict\"",
            "adjudicated_outcome",
            "yes|no|unresolved",
            "adjudication_evidence_refs",
            "adjudication_confidence",
            "temper.done",
        ] {
            assert!(p.contains(needle), "adjudicator prompt missing: {needle}");
        }
        // The adjudicator judges; the module owns the transition. The session
        // must never be told to Resolve or FlagForHuman itself.
        assert!(!p.contains("\"Resolve\""));
        assert!(!p.contains("FlagForHuman"));
    }

    #[test]
    fn adjudicator_tools_and_turns_match_the_contract() {
        assert_eq!(
            ADJUDICATOR_TOOLS,
            "temper_get,temper_list,temper_action,temper_web_search,temper_web_fetch"
        );
        assert_eq!(ADJUDICATOR_MAX_TURNS, "25");
    }

    // --- Workspace rendezvous --------------------------------------------------

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        // All session-spawning modules must derive the exact same workspace
        // name from a world id, or their sessions land in different
        // workspaces.
        assert_eq!(workspace_name("w-1"), "world-w-1");
        assert_eq!(workspace_name("0197abc"), "world-0197abc");
    }

    // --- Row readers -------------------------------------------------------------

    #[test]
    fn row_readers_handle_both_odata_shapes() {
        let nested = json!({"entity_id": "e-1", "status": "Confirmed",
            "fields": {"resolve_by": "2026-05-01", "provenance": "authored"}});
        assert_eq!(row_id(&nested), "e-1");
        assert_eq!(row_status(&nested), "Confirmed");
        assert_eq!(row_str(&nested, "ResolveBy"), "2026-05-01");
        assert_eq!(row_str(&nested, "Provenance"), "authored");
        let pascal = json!({"Id": "e-2", "Status": "Resolved", "ResolveBy": "2026-01-01"});
        assert_eq!(row_id(&pascal), "e-2");
        assert_eq!(row_status(&pascal), "Resolved");
        assert_eq!(row_str(&pascal, "ResolveBy"), "2026-01-01");
    }
}
