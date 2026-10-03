// Candidate exploration stays inside the existing imagine/explore/proposals lifecycle.
// These checks are fallible model judgments, never novelty certificates.
use super::super::{
    REASONING_ADMISSION_RESERVE, evidence, field, references_for_endpoints as references,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Shared by imaginative generation and the relation checks that judge it.
pub const WORLD_CHANGE_SEMANTICS: &str = "A world changes what becomes possible or impossible to do or experience, or how the relevant system actually works, with interacting consequences that answer the user's question. Social or economic arrangements matter when they change those capabilities, experiences or causal operation; they are not required. A new tool, process or medium can be a defining mechanism when its capabilities change the whole answer. Renaming a device, adding decorative scenes or changing ownership does not by itself establish a consequential change. Distinct worlds may share tools or institutions while producing materially different capabilities, experiences and causal paths.";

pub fn enabled(program: &Value) -> bool {
    program["endpoint_proposal_contract"] == 2
}
pub fn research_pending(program: &Value) -> bool {
    enabled(program) && program["proposal_pool"]["stage"] == "contrast"
}
pub fn skip_prefreeze_repair(p: &mut Value) -> bool {
    if p["proposal_pool"]["novelty_repair"]["status"] != "pending" {
        return false;
    }
    p["proposal_pool"]["novelty_repair"]["status"] = json!("not_admitted");
    let selected = p["proposal_pool"]["selected_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let candidates = p["proposal_pool"]["candidates"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    freeze_selected(p, &candidates, &selected);
    true
}

fn freeze_selected(p: &mut Value, candidates: &[Value], selected: &[Value]) {
    let accepted: Vec<_> = candidates
        .iter()
        .filter(|e| selected.contains(&e["id"]))
        .cloned()
        .collect();
    p["endpoint_search"] = json!({"status":"imagined","backward_batch_contract":2,"deferred_novelty_contract":1,"endpoints":accepted,"routes":[],"amendments":[],"rounds":[]});
    p["endpoint_proposal_attempt"]["status"] = json!(if selected
        .iter()
        .all(|id| novelty_passed(p, id.as_str().unwrap_or("")))
    {
        "accepted"
    } else {
        "examined"
    });
    p["proposal_pool"]["stage"] = json!("accepted");
    p["stage"] = json!("exploration");
}

pub fn is_task(task: &Value) -> bool {
    matches!(
        field(task, "function"),
        "check_proposal_coverage"
            | "check_proposal_change"
            | "check_proposal_dependence"
            | "check_proposal_pair"
    )
}

// Keep three initial-world route turns plus composition/writing and a small
// evaluation tail. This does not promise every alternative route will fit.
pub fn reserve() -> u64 {
    5 * REASONING_ADMISSION_RESERVE
}
pub fn admits(remaining: u64, reasoning_turns: u64, questions: usize) -> bool {
    remaining
        >= reserve()
            + reasoning_turns * REASONING_ADMISSION_RESERVE
            + 2 * (questions as u64).div_ceil(8)
            + 8
}

pub fn receive(_snapshot: &Value, old: &Value, endpoints: Vec<Value>) -> Result<Value, String> {
    if serde_json::to_vec(&endpoints)
        .map_err(|e| e.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("Compact candidate or enriched endpoint response exceeds 64 KiB".into());
    }
    let mut program = old.clone();
    if old["proposal_pool"]["stage"] == "enrich" {
        if old["proposal_pool"]["development"]["status"] == "completed" {
            return Err("Candidate development runs once before freezing".into());
        }
        let selected = old["proposal_pool"]["selected_ids"]
            .as_array()
            .ok_or("Missing selected candidates")?;
        if endpoints.len() != selected.len() {
            return Err("Develop every selected candidate exactly once".into());
        }
        let mut seen = BTreeSet::new();
        let mut developed = endpoints;
        let mut originals = vec![];
        for endpoint in &mut developed {
            let original = old["proposal_pool"]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == endpoint["id"] && selected.contains(&e["id"]))
                .ok_or("Unknown selected candidate")?;
            if !seen.insert(endpoint["id"].clone().to_string()) {
                return Err("Duplicate developed candidate".into());
            }
            originals.push(original.clone());
            // Development can change the arrangement, so old contrast bindings are
            // not authoritative even when the generator copies them back verbatim.
            endpoint
                .as_object_mut()
                .ok_or("Invalid developed candidate")?
                .remove("contrast");
        }
        program["proposal_pool"]["development"] =
            json!({"status":"completed","originals":originals,"developed":developed});
        program["proposal_pool"]["candidates"] = json!(developed);
        program["proposal_pool"]["stage"] = json!("contrast");
        program["endpoint_proposal_attempt"]["status"] = json!("developed_contrast_required");
        program["tasks"] = json!([]);
        program["cursor"] = json!(0);
        program["stage"] = json!("proposals");
        program
            .as_object_mut()
            .unwrap()
            .remove("response_correction");
        return Ok(program);
    }
    if old["proposal_pool"].is_object() {
        return Err("Candidate pool is already being examined".into());
    }
    program["proposal_pool"] = json!({"stage":"contrast","analogue_challenge_contract":1,"candidates":endpoints,"selected_ids":[],"selection_receipts":[],"research_attempts":0});
    program["tasks"] = json!([]);
    program["cursor"] = json!(0);
    program["stage"] = json!("proposals");
    program
        .as_object_mut()
        .unwrap()
        .remove("response_correction");
    Ok(program)
}

fn text(v: &Value, max: usize) -> Result<(), String> {
    if v.as_str()
        .is_none_or(|s| s.trim().is_empty() || s.chars().count() > max)
    {
        Err("Missing or oversized proposal contrast text".into())
    } else {
        Ok(())
    }
}
fn ids(v: &Value) -> Result<Vec<&str>, String> {
    let values = v
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 8)
        .ok_or("Contrast needs one to eight commitment IDs")?;
    let mut seen = BTreeSet::new();
    values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|id| seen.insert(*id))
                .ok_or("Invalid or duplicate contrast ID".into())
        })
        .collect()
}

// A researcher report is inspectable evidence of their comparison, not proof
// that a query ran or that the closest possible analogue has been found.
fn validate_frontier_challenge(endpoint: &Value, snapshot: &Value) -> Result<(), String> {
    let contrast = &endpoint["contrast"];
    let challenge = &contrast["frontier_challenge"];
    if !challenge.is_object() {
        return Err(
            "Missing strongest-present comparison; report unknown rather than certify novelty"
                .into(),
        );
    }
    let mut bounded = challenge.clone();
    bounded["query_provenance"] = json!("researcher_report_not_verified_against_tool_trace");
    if bounded.to_string().len() > 2400 {
        return Err("Keep each frontier comparison receipt within 2400 bytes; share research and use concise comparisons".into());
    }
    let queries = challenge["reported_queries"]
        .as_array()
        .filter(|v| v.len() <= 3)
        .ok_or("Strongest-present comparison needs bounded reported_queries")?;
    for query in queries {
        text(query, 240)?;
    }
    if !matches!(
        challenge["research_basis"].as_str(),
        Some("live_research" | "frozen_corpus" | "unavailable")
    ) {
        return Err("Invalid strongest-present research basis".into());
    }
    if challenge["research_basis"] != "unavailable" && queries.is_empty() {
        return Err(
            "Report the mechanism-level research queries, shared across candidates when applicable"
                .into(),
        );
    }
    let defining = ids(&contrast["defining_commitment_ids"])?;
    let comparisons = challenge["comparisons"]
        .as_array()
        .filter(|v| v.len() == defining.len())
        .ok_or(
            "Compare every defining commitment against the strongest supplied present evidence",
        )?;
    let mut seen = BTreeSet::new();
    let sources = evidence::active_sources(snapshot);
    for row in comparisons {
        let id = field(row, "commitment_id");
        if !defining.contains(&id) || !seen.insert(id) {
            return Err("Unknown or duplicate frontier comparison commitment".into());
        }
        if !matches!(
            row["result"].as_str(),
            Some("already_present" | "different_arrangement" | "unknown")
        ) {
            return Err("Invalid frontier comparison result".into());
        }
        text(&row["present_match"], 160)?;
        text(&row["remaining_difference"], 160)?;
        let refs = row["evidence_ids"]
            .as_array()
            .filter(|v| v.len() <= 3)
            .ok_or("Invalid frontier evidence IDs")?;
        if row["result"] != "unknown"
            && (refs.is_empty() || challenge["research_basis"] == "unavailable")
        {
            return Err(
                "A resolved present comparison requires source findings; otherwise report unknown"
                    .into(),
            );
        }
        for id in refs {
            let source = sources
                .iter()
                .find(|n| {
                    n["Id"] == *id
                        && !evidence::is_projection(n)
                        && n["evidence_metadata"]["kind"] == "finding"
                })
                .ok_or(
                    "Frontier comparison needs active present findings, not leads or projections",
                )?;
            evidence::validate(&source["evidence_metadata"])?;
            evidence::within_vantage(
                &source["evidence_metadata"],
                field(&snapshot["world"], "last_ingest_date"),
            )?;
        }
    }
    Ok(())
}
fn frontier_status(endpoint: &Value) -> &'static str {
    let challenge = &endpoint["contrast"]["frontier_challenge"];
    if challenge.is_null() {
        return "not_examined";
    }
    let Some(rows) = challenge["comparisons"].as_array() else {
        return "unresolved";
    };
    if challenge["research_basis"] == "unavailable" || rows.iter().any(|r| r["result"] == "unknown")
    {
        "unresolved"
    } else if rows.iter().any(|r| r["result"] == "different_arrangement") {
        "different_arrangement"
    } else {
        "already_present"
    }
}
fn frontier_admissible(endpoint: &Value, required: bool) -> bool {
    let challenge = &endpoint["contrast"]["frontier_challenge"];
    if challenge.is_null() {
        return !required;
    }
    let Some(rows) = challenge["comparisons"].as_array() else {
        return false;
    };
    challenge["research_basis"] != "unavailable"
        && rows.iter().any(|r| r["result"] == "different_arrangement")
}

pub fn validate_contrast(endpoint: &Value, snapshot: &Value) -> Result<(), String> {
    let c = &endpoint["contrast"];
    text(&c["present_analogue"]["statement"], 800)?;
    if !matches!(
        c["present_analogue"]["status"].as_str(),
        Some("supported" | "unknown")
    ) {
        return Err("Analogue status must be supported or unknown".into());
    }
    let sources = c["present_analogue"]["evidence_ids"]
        .as_array()
        .filter(|a| a.len() <= 8)
        .ok_or("Missing analogue evidence IDs")?;
    let active = evidence::active_sources(snapshot);
    if sources.iter().any(|id| {
        !active
            .iter()
            .any(|n| n["Id"] == *id && !evidence::is_projection(n))
    }) {
        return Err("Present analogue requires active present source records".into());
    }
    for id in sources {
        let node = active
            .iter()
            .find(|n| n["Id"] == *id)
            .ok_or("Unknown analogue source")?;
        if node["evidence_metadata"]["kind"] != "finding" {
            return Err(
                "Present analogue needs typed findings, not leads or unverified legacy records"
                    .into(),
            );
        }
        evidence::validate(&node["evidence_metadata"])?;
        evidence::within_vantage(
            &node["evidence_metadata"],
            field(&snapshot["world"], "last_ingest_date"),
        )?;
    }
    if c["present_analogue"]["status"] == "supported" && sources.is_empty() {
        return Err(
            "Supported analogue needs actual source references; missing coverage is not novelty"
                .into(),
        );
    }
    let commitments = endpoint["commitments"]
        .as_array()
        .ok_or("Missing commitments")?;
    let known: BTreeSet<_> = commitments
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    let defining = ids(&c["defining_commitment_ids"])?;
    if defining.iter().any(|id| !known.contains(id)) {
        return Err("Defining implication must be an actual commitment".into());
    }
    let consequences = c["consequences"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 3)
        .ok_or("Contrast needs one to three dependent consequences")?;
    let mut seen = BTreeSet::new();
    let mut dependencies = vec![];
    for relation in consequences {
        let consequence = relation["commitment_id"]
            .as_str()
            .ok_or("Missing consequence commitment")?;
        if !known.contains(consequence) || !seen.insert(consequence) {
            return Err("Dependent consequence must name a distinct actual commitment".into());
        }
        let parents = ids(&relation["depends_on"])?;
        if parents.iter().any(|id| !known.contains(id) || *id == consequence) {
            return Err("Consequence parents must be actual commitments distinct from their consequence".into());
        }
        text(&relation["mechanism"], 600)?;
        dependencies.push((consequence, parents));
    }
    // A defining implication can also be a downstream consequence. Validate
    // the declared graph itself instead of forcing those roles to be disjoint.
    let mut remaining = known;
    while !remaining.is_empty() {
        let next = remaining.iter().copied().find(|id| {
            dependencies.iter().all(|(consequence, parents)| {
                consequence != id || parents.iter().all(|parent| !remaining.contains(parent))
            })
        });
        match next {
            Some(id) => { remaining.remove(id); }
            None => return Err("Dependent consequence graph must not contain a cycle".into()),
        }
    }
    Ok(())
}

pub fn contrasts(snapshot: &Value, old: &Value, generated: &Value) -> Result<Value, String> {
    if generated.to_string().len() > 64 * 1024 {
        return Err("Candidate contrast response exceeds 64 KiB".into());
    }
    let mut generated = generated.clone();
    if let Some(delta) = generated.get("proposal_contrasts_delta").cloned() {
        if generated.get("proposal_contrasts").is_some() {
            return Err(
                "Use proposal_contrasts_delta or complete proposal_contrasts, not both".into(),
            );
        }
        let originals = old["proposal_pool"]["candidates"]
            .as_array()
            .ok_or("Missing comparison candidates")?;
        if originals.iter().any(|e| !e["contrast"].is_object()) {
            return Err("Initial comparison requires complete proposal_contrasts".into());
        }
        let mut rows: Vec<Value> = originals
            .iter()
            .map(|e| json!({"endpoint_id":e["id"],"contrast":e["contrast"]}))
            .collect();
        let mut edited = BTreeSet::new();
        for row in delta
            .as_array()
            .ok_or("proposal_contrasts_delta must be an array")?
        {
            let id = field(row, "endpoint_id");
            if !edited.insert(id.to_owned()) {
                return Err("Duplicate comparison delta target".into());
            }
            let target = rows
                .iter_mut()
                .find(|e| field(e, "endpoint_id") == id)
                .ok_or("Unknown comparison delta target")?;
            *target = row.clone();
        }
        for revision in generated["endpoint_revisions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if !edited.contains(field(revision, "endpoint_id")) {
                return Err("A revised endpoint requires a corresponding comparison delta".into());
            }
        }
        generated["proposal_contrasts"] = json!(rows);
        if generated.get("comparison_priority").is_none() {
            generated["comparison_priority"] = json!(
                originals
                    .iter()
                    .map(|e| e["id"].clone())
                    .collect::<Vec<_>>()
            );
        }
    }
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    let local_sources: BTreeSet<_> = generated["research_evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect();
    let round = old["round"].as_u64().unwrap_or(0) + 1;
    fn resolve_local_evidence(value: &mut Value, key: &str, locals: &BTreeSet<String>, round: u64) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    resolve_local_evidence(value, key, locals, round);
                }
            }
            Value::Array(values) => {
                for value in values {
                    resolve_local_evidence(value, key, locals, round);
                }
            }
            Value::String(id) if key == "evidence_ids" && locals.contains(id) => {
                *id = format!("r{round}-{id}")
            }
            _ => (),
        }
    }
    resolve_local_evidence(
        &mut generated["proposal_contrasts"],
        "",
        &local_sources,
        round,
    );
    let contrasts = generated["proposal_contrasts"]
        .as_array()
        .ok_or("Return proposal_contrasts for every supplied candidate")?;
    let mut candidates = old["proposal_pool"]["candidates"]
        .as_array()
        .ok_or("Missing candidate pool")?
        .clone();
    let mut revision_receipts = old["proposal_pool"]["revisions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let prior_revision_count = revision_receipts.len();
    let revisions = generated
        .get("endpoint_revisions")
        .and_then(Value::as_array);
    let mut revised = BTreeSet::new();
    for row in revisions.into_iter().flatten() {
        let id = field(row, "endpoint_id");
        if !revised.insert(id) {
            return Err("Duplicate explicit candidate revision".into());
        }
        text(&row["reason"], 600)?;
        let replacement =
            super::super::endpoints::validate_proposals(&json!([row["replacement"]]), 1, 1)?
                .remove(0);
        if field(&replacement, "id") != id {
            return Err("Candidate revision must retain identity".into());
        }
        let e = candidates
            .iter_mut()
            .find(|e| field(e, "id") == id)
            .ok_or("Unknown revised candidate")?;
        revision_receipts.push(
            json!({"endpoint_id":id,"reason":row["reason"],"original":e,"replacement":replacement}),
        );
        *e = replacement;
    }
    let allowed: BTreeSet<_> = candidates.iter().map(|candidate| field(candidate, "id")).collect();
    let mut supplied = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for row in contrasts {
        let id = field(row, "endpoint_id");
        if !supplied.insert(id) {
            duplicates.insert(id);
        }
    }
    let missing: Vec<_> = allowed.difference(&supplied).copied().collect();
    let extra: Vec<_> = supplied.difference(&allowed).copied().collect();
    if !missing.is_empty() || !extra.is_empty() || !duplicates.is_empty() {
        return Err(format!(
            "Contrast targets must match the current candidate IDs exactly. Allowed: {allowed:?}; missing: {missing:?}; extra: {extra:?}; duplicate: {duplicates:?}. Historical and rejected candidates are not targets. Return proposal_contrasts and comparison_priority for the allowed IDs only, including unknown analogues."
        ));
    }
    let mut seen = BTreeSet::new();
    for row in contrasts {
        let id = field(row, "endpoint_id");
        if !seen.insert(id) {
            return Err("Duplicate candidate contrast".into());
        }
        let e = candidates
            .iter_mut()
            .find(|e| field(e, "id") == id)
            .ok_or("Unknown contrasted candidate")?;
        e["contrast"] = row["contrast"].clone();
        validate_contrast(e, snapshot)?;
        if old["proposal_pool"]["analogue_challenge_contract"] == 1
            || !e["contrast"]["frontier_challenge"].is_null()
        {
            validate_frontier_challenge(e, snapshot)?;
            e["contrast"]["frontier_challenge"]["query_provenance"] =
                json!("researcher_report_not_verified_against_tool_trace");
        }
    }
    // Receipts describe the canonical accepted replacement, not the raw draft's
    // stale embedded contrast. Never rewrite earlier revision history.
    for receipt in revision_receipts.iter_mut().skip(prior_revision_count) {
        receipt["replacement"] = candidates
            .iter()
            .find(|e| e["id"] == receipt["endpoint_id"])
            .ok_or("Missing canonical revision")?
            .clone();
    }
    let priority=generated["comparison_priority"].as_array().ok_or("Return comparison_priority containing every candidate ID once, ordered by strongest distinct organizing arrangements")?;
    let mut order = BTreeSet::new();
    if priority.len() != candidates.len()
        || priority.iter().any(|id| {
            !order.insert(id.as_str().unwrap_or("")) || !candidates.iter().any(|e| e["id"] == *id)
        })
    {
        return Err("Comparison priority must contain every candidate exactly once".into());
    }
    candidates.sort_by_key(|e| priority.iter().position(|id| *id == e["id"]).unwrap());
    let mut program = old.clone();
    let repair_key = if old["proposal_pool"]["creative_repair"]["status"] == "pending" {
        "creative_repair"
    } else {
        "novelty_repair"
    };
    if old["proposal_pool"][repair_key]["status"] == "pending" {
        let repair = &old["proposal_pool"][repair_key];
        let targets = repair["target_ids"]
            .as_array()
            .ok_or("Missing targeted comparison repair IDs")?;
        for candidate in &candidates {
            if !targets.contains(&candidate["id"]) {
                let original = repair["original_candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["id"] == candidate["id"])
                    .ok_or("Unknown repair candidate")?;
                if candidate != original {
                    return Err("Targeted comparison repair must preserve every non-target candidate and its contrast exactly".into());
                }
            }
        }
        let changed = candidates.iter().any(|candidate| {
            repair["original_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == candidate["id"])
                != Some(candidate)
        }) || old["baseline"] != repair["original_baseline"]
            || json!(evidence::active_sources(snapshot)) != repair["original_sources"];
        let revised_commitments = candidates.iter().any(|candidate| {
            let original = repair["original_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == candidate["id"])
                .unwrap();
            candidate["commitments"] != original["commitments"]
                || candidate["original_statement"] != original["original_statement"]
        });
        program["proposal_pool"][repair_key]["status"] = json!("completed");
        program["proposal_pool"][repair_key]["creative_development_used"] =
            json!(revised_commitments);
        if revised_commitments && repair_key == "novelty_repair" {
            program["proposal_pool"]["creative_repair"] = json!({"status":"completed","used_in":"joint_comparison_repair","creative_development_used":true});
        }
        program["proposal_pool"][repair_key]["progress"] = json!(if changed {
            "changed_comparison_or_present_evidence"
        } else {
            "unchanged_no_progress"
        });
    }
    if !local_sources.is_empty() {
        program["round"] = json!(round);
    }
    program["proposal_pool"]["comparison_priority"] = json!(priority);
    program["proposal_pool"]["candidates"] = json!(candidates);
    program["proposal_pool"]["revisions"] = json!(revision_receipts);
    program["proposal_pool"]["research_attempts"] = json!(
        old["proposal_pool"]["research_attempts"]
            .as_u64()
            .unwrap_or(0)
            + 1
    );
    schedule(snapshot, &program, candidates, "individual")
}

fn task(number: u64, function: &str, id: &str, other: &str, index: usize) -> Value {
    json!({"nodeId":format!("pool-{number}-{function}-{id}-{other}-{index}"),"function":function,"proposal_attempt":number,"endpoint_id":id,"other_endpoint_id":other,"relation_index":index,"depth":0})
}
fn schedule(
    snapshot: &Value,
    old: &Value,
    candidates: Vec<Value>,
    stage: &str,
) -> Result<Value, String> {
    let number = old["endpoint_proposal_history"]
        .as_array()
        .map_or(0, Vec::len) as u64
        + 1;
    let mut tasks = vec![];
    if stage == "pairs" {
        for (i, a) in candidates.iter().enumerate() {
            for b in candidates.iter().skip(i + 1) {
                tasks.push(task(
                    number,
                    "check_proposal_pair",
                    field(a, "id"),
                    field(b, "id"),
                    0,
                ));
            }
        }
    } else {
        for e in &candidates {
            if stage != "deferred" { tasks.push(task(
                number,
                "check_proposal_coverage",
                field(e, "id"),
                "",
                0,
            )); }
            tasks.push(task(number, "check_proposal_change", field(e, "id"), "", 0));
            if stage == "deferred" { continue; }
            for (i, _) in e["contrast"]["consequences"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                tasks.push(task(
                    number,
                    "check_proposal_dependence",
                    field(e, "id"),
                    "",
                    i,
                ));
            }
        }
    }
    tasks.sort_by(|a, b| {
        field(a, "function")
            .cmp(field(b, "function"))
            .then_with(|| field(a, "nodeId").cmp(field(b, "nodeId")))
    });
    let mut p = old.clone();
    p["endpoint_proposal_attempt"] = json!({"contract":2,"attempt":number,"status":"checking","pool_stage":stage,"endpoints":candidates,"baseline":old["baseline"],"world":snapshot["world"],"source_evidence":evidence::active_sources(snapshot),"tasks":tasks,"checks":[]});
    if !p["endpoint_proposal_history"].is_array() {
        p["endpoint_proposal_history"] = json!([]);
    }
    let mut pending = vec![];
    for t in &tasks {
        let request = request(&p["endpoint_proposal_attempt"], t)?;
        let reused = old["endpoint_proposal_history"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|a| a["checks"].as_array().into_iter().flatten())
            .find(|c| {
                c["request"] == request
                    && c["evaluation"]["type"] == "choice"
                    && c["result"].is_string()
            });
        if let Some(c) = reused {
            let key = field(t, "nodeId");
            let function = field(t, "function");
            p["results"][key][function] = c["result"].clone();
            p["evaluations"][key][function] = c["evaluation"].clone();
            p["evaluations"][key][function]["reused_from_task"] = c["task"].clone();
        } else {
            pending.push(t.clone());
        }
    }
    p["tasks"] = json!(pending);
    p["cursor"] = json!(0);
    p["stage"] = json!("proposals");
    p["proposal_pool"]["stage"] = json!(stage);
    p.as_object_mut().unwrap().remove("response_correction");
    Ok(p)
}

/// A provisional endpoint is explorable, never an accepted final world.
pub fn novelty_passed(program: &Value, id: &str) -> bool {
    (!program["endpoint_novelty"].is_object()
        && program["endpoint_search"]["deferred_novelty_contract"] != 1)
        || program["endpoint_novelty"][id]["status"] == "passed"
}

/// A status label alone cannot authorize finalization after evidence changes.
pub fn current_novelty_passed(snapshot: &Value, program: &Value, id: &str) -> bool {
    let receipt = &program["endpoint_novelty"][id];
    let prior = if receipt["final_check"].is_object() {
        &receipt["final_check"]
    } else {
        &receipt["initial_check"]
    };
    let current = json!({"endpoints":program["endpoint_search"]["endpoints"],"baseline":program["baseline"],"world":snapshot["world"],"source_evidence":evidence::active_sources(snapshot)});
    receipt["status"] == "passed"
        && prior["passed"] == true
        && request(
            &current,
            &json!({"function":"check_proposal_change","endpoint_id":id}),
        )
        .is_ok_and(|request| request == prior["request"])
}

fn pending_novelty_endpoints(snapshot: &Value, program: &Value) -> Result<Vec<Value>, String> {
    if matches!(field(&program["deferred_novelty_recheck"],"status"),"checking"|"not_admitted") { return Ok(vec![]); }
    let current = json!({"endpoints":program["endpoint_search"]["endpoints"],"baseline":program["baseline"],"world":snapshot["world"],"source_evidence":evidence::active_sources(snapshot)});
    let mut pending = Vec::new();
    for endpoint in current["endpoints"].as_array().into_iter().flatten() {
        let id = field(endpoint,"id");
        let receipt = &program["endpoint_novelty"][id];
        let prior = if receipt["final_check"].is_object() { &receipt["final_check"] } else { &receipt["initial_check"] };
        if receipt["status"] == "provisional" || (program["deferred_novelty_recheck"]["status"] == "interrupted" && receipt["final_check"].is_object() && prior["result"].is_null()) || (receipt["status"] == "passed" && request(&current,&json!({"function":"check_proposal_change","endpoint_id":id}))? != prior["request"]) {
            pending.push(endpoint.clone());
        }
    }
    Ok(pending)
}

/// Same exact-context selection and cache reuse as execution, including changes
/// after a completed recheck. A completed receipt is not perpetual admission.
pub fn pending_novelty_checks(snapshot: &Value, program: &Value) -> Result<usize, String> {
    let pending = pending_novelty_endpoints(snapshot,program)?;
    if pending.is_empty() { return Ok(0); }
    Ok(schedule(snapshot,program,pending,"deferred")?["tasks"].as_array().map_or(0,Vec::len))
}

/// A change-only continuation after backward search. Unchanged passed requests
/// are not reopened. Only current source
/// findings and the original comparison context enter the request, not routes.
pub fn defer_before_composition(
    snapshot: &Value,
    p: &mut Value,
    exhausted: bool,
) -> Result<bool, String> {
    if matches!(
        field(&p["deferred_novelty_recheck"], "status"),
        "checking" | "not_admitted"
    ) {
        return Ok(false);
    }
    let pending = pending_novelty_endpoints(snapshot,p)?;
    if matches!(field(p,"stop_reason"),"provider_error"|"trace_budget") {
        p["deferred_novelty_recheck"] = json!({"status":"not_admitted","error":format!("Current comparisons were not scheduled because {} stopped admitted work",field(p,"stop_reason"))});
        return Ok(false);
    }
    for endpoint in &pending {
        let id = field(endpoint,"id");
        if p["endpoint_novelty"][id]["status"] == "passed" {
            p["endpoint_novelty"][id]["status"] = json!("provisional");
            p["endpoint_novelty"][id]["reason"] = json!("Present comparison context changed after the recorded judgment; a current recheck is required.");
        }
    }
    if pending.is_empty() {
        return Ok(false);
    }
    let mut next = schedule(snapshot, p, pending.clone(), "deferred")?;
    let cost = check_transitions(snapshot, &next)? + 2;
    let remaining =
        super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
    let admitted = !exhausted && remaining >= cost + 2 * REASONING_ADMISSION_RESERVE + 32;
    let admission = json!({"status":if admitted {"checking"} else {"not_admitted"},"required_transitions":cost,"remaining_transitions":remaining,"reserved_finalization":2 * REASONING_ADMISSION_RESERVE + 32});
    if admitted {
        next["deferred_novelty_recheck"] = admission;
        *p = next;
        Ok(true)
    } else {
        p["deferred_novelty_recheck"] = admission;
        for e in pending {
            p["endpoint_novelty"][field(&e, "id")]["status"] = json!("unresolved");
            p["endpoint_novelty"][field(&e, "id")]["reason"] = json!(
                "No admitted comparison recheck before finalization; provisional novelty remains unresolved."
            );
        }
        Ok(false)
    }
}

pub fn check_transitions(snapshot: &Value, program: &Value) -> Result<u64, String> {
    let mut scratch = program.clone();
    let count = scratch["tasks"]
        .as_array()
        .ok_or("Missing comparison tasks")?
        .len();
    let mut cursor = 0;
    let mut transitions = 0;
    while cursor < count {
        scratch["cursor"] = json!(cursor);
        let batch = super::super::batch::prepare(snapshot, &scratch, count - cursor)?;
        if batch.tasks.is_empty() {
            return Err("Cannot pack proposal comparison".into());
        }
        cursor += batch.tasks.len();
        transitions += 2;
    }
    Ok(transitions)
}

pub fn request(attempt: &Value, t: &Value) -> Result<Value, String> {
    let endpoint = |id: &str| {
        attempt["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| field(e, "id") == id)
            .cloned()
            .ok_or("Unknown pool candidate".to_string())
    };
    let e = endpoint(field(t, "endpoint_id"))?;
    let (mut state, criteria, instructions) = match field(t, "function") {
        "check_proposal_coverage" => (
            json!({"statement":e["original_statement"],"narrative":e["original_narrative"],"commitments":e["commitments"],"defining_commitment_ids":e["contrast"]["defining_commitment_ids"]}),
            json!({"represented":"Every distinguishing implication, including the strongest unusual claim in the statement and narrative, is explicitly represented by the defining commitments. Consequences have actual commitments too.","missing_defining_implication":"A strongest or distinguishing implication exists only in narrative/title and would vanish if only commitments were reconstructed.","unresolved":"The mapping is too ambiguous to establish coverage."}),
            "Check exact proposition coverage, not general thematic resemblance. A claim about distribution does not cover a separate change in production or interoperability. Do not invent or weaken the missing commitment.",
        ),
        "check_proposal_change" => {
            let sources: Vec<_> = attempt["source_evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| {
                    n["evidence_metadata"]["kind"] == "finding" && !evidence::is_projection(n)
                })
                .collect();
            (
                json!({"baseline":attempt["baseline"],"analogue":e["contrast"]["present_analogue"],"frontier_challenge":e["contrast"]["frontier_challenge"],"source_evidence":sources,"defining_commitments":e["commitments"].as_array().into_iter().flatten().filter(|c|e["contrast"]["defining_commitment_ids"].as_array().is_some_and(|ids|ids.contains(&c["id"]))).collect::<Vec<_>>(),"consequences":e["contrast"]["consequences"]}),
                json!({"changed_arrangement":"Cited present analogue is supported in its actual scope, and the defining commitments specify materially changed capabilities, experiences or causal operation with dependent consequences. Institutional change is not required. Adoption counts when it enables such a change, not merely greater availability or prevalence.","present_or_adoption_only":"The defining mechanism and its claimed capabilities or experiences already exist in the compared setting, or only price, distribution, prevalence, packaging or convenience changes without a consequential difference in what becomes possible or how it works.","unsupported_analogue":"The cited material does not substantiate the claimed present analogue or necessary scope contrast; absence of evidence is not evidence of novelty.","unresolved":"A consequential present-relative change is not established."}),
                "Challenge the chosen analogue against ALL supplied source findings: an uncited stronger present match overrides a weak selected comparison. Check each defining commitment against the strongest relevant existing capabilities, experiences and causal mechanisms, including the present frontier rather than only average adoption. A supported example does not establish that it is closest. A currently published report may project future outcomes: its publication and forecast are evidence only of what the source reports, not that the projected arrangement exists. Preserve claim_type, provenance, source_correction, observation dates and textual qualifications; do not classify a projected outcome as a present counterexample. Reported queries are researcher self-report, not independently verified search or proof of absence. If necessary comparison coverage remains unknown, choose unresolved. Preserve scope/date caveats. Do not award novelty for low probability, future dates, narrative length, or unsupported claims about today's absence. A familiar mechanism under changed conditions may qualify if its newly enabled capabilities or consequences are explicit and supported by the present comparison.",
            )
        }
        "check_proposal_dependence" => (
            json!({"commitments":e["commitments"],"relation":e["contrast"]["consequences"][t["relation_index"].as_u64().unwrap_or(0) as usize]}),
            json!({"dependent":"Removing the named defining change removes or materially alters the named consequence through the stated mechanism.","independent_decoration":"The consequence follows just as well without the defining change; it is decoration or a complementary topic, not a consequence.","unresolved":"The proposed causal dependence is unclear or contradicted."}),
            "Evaluate this small counterfactual dependency, not whether either event is likely. Can the consequence remain essentially unchanged when the defining change is removed?",
        ),
        "check_proposal_pair" => (
            json!({"left":e,"right":endpoint(field(t,"other_endpoint_id"))?}),
            json!({"distinct_arrangements":"The worlds give materially different whole answers through changed capabilities, experiences or causal operation and their dependent consequences. They may share institutions or tools. The distinction survives removing labels and decorative prose; do not remove the substantive mechanism being compared.","complementary_slices":"These are complementary features or topics without materially different whole answers in capability, experience or causal operation.","same_arrangement":"Same defining capabilities, experiences, causal operation and consequences under different wording.","unresolved":"A consequential distinction is not specified."}),
            "Compare what becomes possible to do or experience, how it works and the interacting consequences. Do not demand different business models, ownership, institutions or logical incompatibility. Sharing a tool or institution does not make two materially different experiences or causal paths the same world.",
        ),
        _ => return Err("Unknown pool relation".into()),
    };
    state["world"] = attempt["world"].clone();
    let instructions = if matches!(field(t, "function"), "check_proposal_change" | "check_proposal_pair") {
        format!("{WORLD_CHANGE_SEMANTICS}\n\n{instructions}")
    } else { instructions.to_owned() };
    Ok(
        json!({"model":super::super::MODEL,"state":state,"questions":{"result":{"type":"choice","instructions":instructions,"criteria":criteria}},"validation":{"selection_policy":"provider_argmax"}}),
    )
}

fn allowed(t: &Value, result: &Value) -> bool {
    result
        == match field(t, "function") {
            "check_proposal_coverage" => "represented",
            "check_proposal_change" => "changed_arrangement",
            "check_proposal_dependence" => "dependent",
            "check_proposal_pair" => "distinct_arrangements",
            _ => "",
        }
}
// Preserve maximum set breadth; among equally broad compatible sets retain
// more currently passed comparisons instead of discarding them by input order.
fn select_distinct_candidates(
    candidates: &[Value],
    checks: &[Value],
    max_selected: usize,
    p: &Value,
) -> Vec<Value> {
    let mut selected = vec![];
    let mut selected_passed = 0;
    for bits in 0usize..(1usize << candidates.len()) {
        let ids: Vec<_> = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, e)| e["id"].clone())
            .collect();
        let passed = ids
            .iter()
            .filter(|id| novelty_passed(p, id.as_str().unwrap_or("")))
            .count();
        if ids.len() < 3
            || ids.len() > max_selected
            || ids.len() < selected.len()
            || (ids.len() == selected.len() && passed <= selected_passed)
        {
            continue;
        }
        if checks
            .iter()
            .filter(|c| {
                ids.contains(&c["task"]["endpoint_id"])
                    && ids.contains(&c["task"]["other_endpoint_id"])
            })
            .all(|c| c["passed"] == true)
        {
            selected = ids;
            selected_passed = passed;
        }
    }
    selected
}

// Estimate current affected work with the same request packer used at runtime.
// Future revisions can add work, so native limits still apply after generation.
fn repair_admission(
    snapshot: &Value,
    p: &Value,
    candidates: &[Value],
    worlds: usize,
) -> Result<Value, String> {
    let mut fresh = p.clone();
    fresh["endpoint_proposal_history"] = json!([]);
    let individual = schedule(snapshot, &fresh, candidates.to_vec(), "individual")?;
    let pairs = schedule(snapshot, &fresh, candidates.to_vec(), "pairs")?;
    let checks = check_transitions(snapshot, &individual)? + check_transitions(snapshot, &pairs)?;
    let required = (worlds as u64 + 3) * REASONING_ADMISSION_RESERVE + checks + 32 + 8;
    let remaining =
        super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
    Ok(
        json!({"admitted":remaining >= required,"remaining_transitions":remaining,"required_transitions":required,"packed_check_transitions":checks,"reserved_route_turns":worlds,"reserved_generation_turns":1,"reserved_composition_writing_turns":2,"guaranteed":false}),
    )
}

pub fn finish(
    snapshot: &Value,
    p: &mut Value,
    allow_retry: bool,
    exhausted: bool,
) -> Result<(), String> {
    // The targeted repair consumes its one generation even when it produces no
    // progress; subsequent failed checks cannot reopen generic revision loops.
    let can_develop = allow_retry;
    let allow_retry = allow_retry && p["proposal_pool"]["novelty_repair"]["status"] != "completed";
    let mut a = p["endpoint_proposal_attempt"].clone();
    let mut checks = vec![];
    for t in a["tasks"].as_array().ok_or("Missing pool tasks")? {
        let result = &p["results"][field(t, "nodeId")][field(t, "function")];
        let eval = &p["evaluations"][field(t, "nodeId")][field(t, "function")];
        let recorded = eval["type"] == "choice"
            && eval["selected"] == *result
            && eval["answer"]["choice"] == *result
            && result.is_string();
        checks.push(json!({"task":t,"result":if recorded {result.clone()} else {Value::Null},"passed":recorded&&allowed(t,result),"evaluation":if recorded{eval.clone()}else{Value::Null},"request":request(&a,t)?}));
    }
    let pending_checks = checks.iter().filter(|check| check["result"].is_null()).count();
    let interrupted = a["pool_stage"] == "deferred" && pending_checks > 0;
    a["checks"] = json!(checks);
    a["status"] = json!(if interrupted {"unresolved"} else {"examined"});
    p["endpoint_proposal_history"]
        .as_array_mut()
        .ok_or("Missing pool history")?
        .push(a.clone());
    p["endpoint_proposal_attempt"] = a.clone();
    p["tasks"] = json!([]);
    p["cursor"] = json!(0);
    if a["pool_stage"] == "deferred" {
        for check in &checks {
            let id = field(&check["task"], "endpoint_id");
            p["endpoint_novelty"][id]["final_check"] = check.clone();
            p["endpoint_novelty"][id]["status"] = json!(if check["passed"] == true {"passed"} else if check["result"].is_null() || check["result"] == "unresolved" {"unresolved"} else {"rejected"});
        }
        p["deferred_novelty_recheck"]["status"] = json!(if interrupted {"interrupted"} else {"completed"});
        p["deferred_novelty_recheck"]["performed_checks"] = json!(checks.len()-pending_checks);
        p["deferred_novelty_recheck"]["pending_checks"] = json!(pending_checks);
        if interrupted {
            let reason = format!("Current comparison checks were interrupted by {}; missing judgments were not performed, not evaluated as uncertain",field(p,"stop_reason"));
            p["deferred_novelty_recheck"]["reason"] = json!(reason);
            for check in &checks {
                if check["result"].is_null() {
                    p["endpoint_novelty"][field(&check["task"],"endpoint_id")]["reason"] = json!(reason);
                }
            }
        }
        p["stage"] = json!("exploration");
        p["proposal_pool"]["stage"] = json!("accepted");
        return Ok(());
    }
    if exhausted {
        p["endpoint_proposal_attempt"]["status"] = json!("unresolved");
        return Ok(());
    }
    let candidates = a["endpoints"].as_array().unwrap();
    if a["pool_stage"] == "pairs" {
        let remaining =
            super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
        let remaining_reasoning_turns = 2;
        let max_selected = (remaining.saturating_sub(32) / REASONING_ADMISSION_RESERVE)
            .saturating_sub(remaining_reasoning_turns)
            .min(5) as usize;
        p["proposal_pool"]["selection_budget"] = json!({"remaining_transitions":remaining,"max_selected":max_selected,"reserved_reasoning_turns_per_selected_world":1,"development_research_composition_writing_turns":remaining_reasoning_turns,"evaluation_tail":32});
        let selected = select_distinct_candidates(candidates, &checks, max_selected, p);
        p["proposal_pool"]["selected_ids"] = json!(selected);
        p["proposal_pool"]["selection_receipts"]=json!(candidates.iter().map(|e|json!({"endpoint_id":e["id"],"selected":selected.contains(&e["id"]),"reason":if selected.contains(&e["id"]){"pairwise_distinct_set"}else{"not_in_bounded_distinct_set"}})).collect::<Vec<_>>());
        if selected.len() >= 3 {
            let passed = selected
                .iter()
                .filter(|id| novelty_passed(p, id.as_str().unwrap_or("")))
                .count();
            if passed < 2
                && allow_retry
                && !p["proposal_pool"]["novelty_repair"].is_object()
                && repair_admission(snapshot, p, candidates, selected.len())?["admitted"] == true
            {
                let targets: Vec<_> = selected
                    .iter()
                    .filter(|id| !novelty_passed(p, id.as_str().unwrap_or("")))
                    .cloned()
                    .collect();
                p["proposal_pool"]["repair_admission"] =
                    repair_admission(snapshot, p, candidates, selected.len())?;
                p["proposal_pool"]["novelty_repair"] = json!({"status":"pending","target_ids":targets,"original_candidates":p["proposal_pool"]["candidates"],"original_baseline":p["baseline"],"original_sources":evidence::active_sources(snapshot),"comparison_receipts":p["endpoint_novelty"],"attempt_limit":1});
                p["proposal_pool"]["stage"] = json!("contrast");
                p["endpoint_proposal_attempt"]["status"] = json!("targeted_comparison_repair");
                return Ok(());
            }
            freeze_selected(p, candidates, &selected);
            return Ok(());
        }
    } else {
        let viable: Vec<_> = candidates
            .iter()
            .filter(|e| {
                e["contrast"]["present_analogue"]["status"] == "supported"
                    && frontier_admissible(
                        e,
                        p["proposal_pool"]["analogue_challenge_contract"] == 1,
                    )
                    && checks
                        .iter()
                        .filter(|c| c["task"]["endpoint_id"] == e["id"])
                        .all(|c| {
                            c["passed"] == true
                                || (c["task"]["function"] == "check_proposal_change"
                                    && c["result"] == "unresolved"
                                    && c["evaluation"]["type"] == "choice")
                        })
            })
            .cloned()
            .collect();
        if a["pool_stage"] == "individual" {
            if !p["endpoint_novelty"].is_object() {
                p["endpoint_novelty"] = json!({});
            }
            for endpoint in candidates {
                if let Some(check) = checks.iter().find(|c| {
                    c["task"]["endpoint_id"] == endpoint["id"]
                        && c["task"]["function"] == "check_proposal_change"
                }) {
                    p["endpoint_novelty"][field(endpoint, "id")] = json!({"status":if check["passed"] == true {"passed"} else if check["result"] == "unresolved" {"provisional"} else if check["result"].is_string() {"rejected"} else {"unresolved"},"initial_check":check});
                }
            }
            p["proposal_pool"]["candidate_receipts"]=json!(candidates.iter().map(|e|json!({"endpoint_id":e["id"],"admissible":viable.iter().any(|v|v["id"]==e["id"]) && novelty_passed(p,field(e,"id")),"explorable":viable.iter().any(|v|v["id"]==e["id"]),"analogue_status":e["contrast"]["present_analogue"]["status"],"frontier_comparison_status":frontier_status(e),"failed_relations":checks.iter().filter(|c|c["task"]["endpoint_id"]==e["id"] && c["passed"]!=true).cloned().collect::<Vec<_>>()})).collect::<Vec<_>>());
        }
        // A completed factual comparison is not a used creative-development
        // attempt. If it defeats the selected set, develop only its uncertain
        // slots through the same explicit-revision receiver, once.
        if a["pool_stage"] == "individual"
            && viable.len() < 3
            && can_develop
            && p["proposal_pool"]["novelty_repair"]["status"] == "completed"
            && p["proposal_pool"]["novelty_repair"]["progress"] != "unchanged_no_progress"
            && !p["proposal_pool"]["creative_repair"].is_object()
        {
            let selected = p["proposal_pool"]["selected_ids"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let targets: Vec<_> = selected
                .iter()
                .filter(|id| {
                    !viable.iter().any(|e| e["id"] == **id)
                        || !novelty_passed(p, id.as_str().unwrap_or(""))
                })
                .cloned()
                .collect();
            let admission = repair_admission(snapshot, p, candidates, selected.len())?;
            p["proposal_pool"]["creative_repair_admission"] = admission.clone();
            if !targets.is_empty() && admission["admitted"] == true {
                p["proposal_pool"]["creative_repair"] = json!({"status":"pending","target_ids":targets,"original_candidates":p["proposal_pool"]["candidates"],"original_baseline":p["baseline"],"original_sources":evidence::active_sources(snapshot),"comparison_receipts":p["endpoint_novelty"],"attempt_limit":1});
                p["proposal_pool"]["stage"] = json!("contrast");
                p["endpoint_proposal_attempt"]["status"] = json!("targeted_creative_repair");
                return Ok(());
            }
        }
        // Failed initial comparisons should trigger creative development, not
        // repeated research over unchanged arrangements. This is provisional:
        // every changed claim still returns through the full admission checks.
        if a["pool_stage"] == "individual"
            && viable.len() < 3
            && allow_retry
            && p["proposal_pool"]["development"]["status"] != "completed"
            && p["proposal_pool"]["research_attempts"]
                .as_u64()
                .unwrap_or(0)
                < 3
        {
            let remaining =
                super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
            let count = (3..=candidates.len().min(5)).rev().find(|n| {
                // One development and one contrast turn, one route turn per
                // provisional world, composition/writing, and conservative scalar
                // cost for the current individual relations plus every pair check.
                let checks = 2 * n
                    + candidates
                        .iter()
                        .take(*n)
                        .map(|e| e["contrast"]["consequences"].as_array().map_or(0, Vec::len))
                        .sum::<usize>()
                    + n * (n - 1) / 2;
                remaining >= (*n as u64 + 4) * REASONING_ADMISSION_RESERVE + 2 * checks as u64 + 32
            });
            p["proposal_pool"]["development_admission"] = json!({
                "remaining_transitions":remaining,"admitted":count.is_some(),
                "provisional_count":count,"research_attempts_remaining":3-p["proposal_pool"]["research_attempts"].as_u64().unwrap_or(0),
                "semantics":"Provisional creative development, not novelty or plausibility acceptance. Estimate reserves fresh contrast, scalar checks for current relations and all pairs, route turns and final writing. Added relations remain subject to native admission; no run budget is reset."
            });
            if let Some(count) = count {
                // Candidates are already ordered by the recorded fallible
                // comparison_priority. Preserve that reason and all failed checks.
                let selected: Vec<_> = candidates
                    .iter()
                    .take(count)
                    .map(|e| e["id"].clone())
                    .collect();
                p["proposal_pool"]["selected_ids"] = json!(selected);
                p["proposal_pool"]["selection_receipts"] = json!(candidates.iter().map(|e| json!({
                    "endpoint_id":e["id"],"selected":selected.contains(&e["id"]),
                    "reason":if selected.contains(&e["id"]){"provisional_development_after_failed_relations"}else{"outside_bounded_provisional_development"}
                })).collect::<Vec<_>>());
                p["proposal_pool"]["stage"] = json!("enrich");
                p["endpoint_proposal_attempt"]["status"] = json!("development_requested");
                return Ok(());
            }
        }
        if a["pool_stage"] == "individual" && viable.len() >= 3 {
            let shortlist = viable;
            p["proposal_pool"]["shortlist_ids"] = json!(
                shortlist
                    .iter()
                    .map(|e| e["id"].clone())
                    .collect::<Vec<_>>()
            );
            let mut paired = schedule(snapshot, p, shortlist, "pairs")?;
            let cost = check_transitions(snapshot, &paired)?;
            let remaining =
                super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
            let reserved_tail = reserve() + 32 + 2 * candidates.len() as u64 + 2;
            paired["proposal_pool"]["pair_admission"] = json!({"remaining_transitions":remaining,"check_transitions":cost,"reserved_tail":reserved_tail,"admitted":remaining>=cost+reserved_tail});
            if remaining < cost + reserved_tail {
                paired["endpoint_proposal_attempt"]["status"] = json!("unresolved");
                paired["proposal_pool"]["stage"] = json!("unresolved");
                paired["proposal_pool"]["selection_receipts"]=json!(paired["endpoint_proposal_attempt"]["endpoints"].as_array().unwrap().iter().map(|e|json!({"endpoint_id":e["id"],"selected":false,"reason":"not_examined_budget"})).collect::<Vec<_>>());
                paired["tasks"] = json!([]);
            }
            *p = paired;
            return Ok(());
        }
    }
    // Re-research/revise contrasts only; originals remain recorded. No permissive
    // fallback admits unknown analogues or silently replaces defining claims.
    let retry = allow_retry
        && p["proposal_pool"]["research_attempts"]
            .as_u64()
            .unwrap_or(0)
            < 3;
    p["endpoint_proposal_attempt"]["status"] = json!(if retry {
        "revision_requested"
    } else {
        "unresolved"
    });
    p["proposal_pool"]["stage"] = json!(if retry { "contrast" } else { "unresolved" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_novelty_recheck_cost_reopens_only_changed_context() {
        let fixture: Value = serde_json::from_str(include_str!("semantic_pass15_order_fixture.json")).unwrap();
        let receipt = &fixture["captured_initial_receipt"];
        let state = &receipt["request"]["state"];
        let snapshot = json!({"world":state["world"],"nodes":state["source_evidence"]});
        let mut p = fixture["program"].clone();
        p["baseline"] = state["baseline"].clone();
        p["endpoint_novelty"] = json!({"games-across-games":{"status":"passed","final_check":receipt}});
        p["deferred_novelty_recheck"] = json!({"status":"completed"});
        p["results"] = json!({}); p["evaluations"] = json!({});
        assert_eq!(pending_novelty_checks(&snapshot,&p).unwrap(),0);
        let mut unchanged = p.clone();
        assert!(!defer_before_composition(&snapshot,&mut unchanged,false).unwrap());
        let mut changed = snapshot.clone(); changed["nodes"][0]["statement"] = json!("New present evidence changes the comparison");
        let cost = super::super::super::endpoints::pending_mandatory_work(&changed,&p).unwrap();
        assert_eq!(cost["novelty_rechecks"],1);
        assert!(defer_before_composition(&changed,&mut p,false).unwrap());
        assert_eq!(p["tasks"].as_array().unwrap().len(),1);
        assert_eq!(p["tasks"][0]["endpoint_id"],"games-across-games");
        assert_eq!(cost["novelty_rechecks"].as_u64().unwrap() as usize,p["tasks"].as_array().unwrap().len());
    }

    #[test]
    fn comparison_delta_retains_untouched_candidates_and_canonical_revision_receipt() {
        let (snapshot, mut p) = pool();
        record(&mut p, pass);
        finish(&snapshot, &mut p, true, false).unwrap();
        let originals = p["proposal_pool"]["candidates"].clone();
        let mut replacement = originals[0].clone();
        replacement["commitments"][0]["statement"] =
            json!("An explicitly revised defining capability");
        replacement["contrast"]["frontier_challenge"]
            .as_object_mut()
            .unwrap()
            .remove("query_provenance");
        let reply = json!({"research_evidence":[],"proposal_contrasts_delta":[{"endpoint_id":replacement["id"],"contrast":replacement["contrast"]}],"endpoint_revisions":[{"endpoint_id":replacement["id"],"reason":"Develop a different mechanism","replacement":replacement}]});
        let next = contrasts(&snapshot, &p, &reply).unwrap();
        let canonical = &next["proposal_pool"]["candidates"][0];
        assert_eq!(
            next["proposal_pool"]["revisions"][0]["replacement"],
            *canonical
        );
        assert_eq!(
            canonical["contrast"]["frontier_challenge"]["query_provenance"],
            "researcher_report_not_verified_against_tool_trace"
        );
        assert_eq!(
            next["proposal_pool"]["revisions"][0]["original"],
            originals[0]
        );
        for i in 1..originals.as_array().unwrap().len() {
            assert_eq!(next["proposal_pool"]["candidates"][i], originals[i]);
        }
        let unchanged = contrasts(
            &snapshot,
            &p,
            &json!({"research_evidence":[],"proposal_contrasts_delta":[]}),
        )
        .unwrap();
        assert_eq!(unchanged["proposal_pool"]["candidates"], originals);
        assert!(unchanged["tasks"].as_array().unwrap().is_empty());
        let mut bad = reply;
        bad["proposal_contrasts_delta"] = json!([]);
        assert!(
            contrasts(&snapshot, &p, &bad)
                .unwrap_err()
                .contains("corresponding comparison delta")
        );
    }

    #[test]
    fn fresh_failed_novelty_replaces_stale_pass_before_creative_targets() {
        let fixture: Value =
            serde_json::from_str(include_str!("semantic_food_repair_fixture.json")).unwrap();
        let mut p = fixture["program"].clone();
        let id = "kitchen-becomes-a-barrier";
        p["endpoint_novelty"][id] =
            json!({"status":"passed","initial_check":{"result":"changed_arrangement"}});
        let history =
            json!({"checks":[{"task":{"endpoint_id":id},"result":"changed_arrangement"}]});
        p["endpoint_proposal_history"] = json!([history.clone()]);
        finish(&fixture["snapshot"], &mut p, true, false).unwrap();
        assert_eq!(p["endpoint_novelty"][id]["status"], "provisional");
        assert!(
            p["proposal_pool"]["creative_repair"]["target_ids"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        assert_eq!(p["endpoint_proposal_history"][0], history);
        assert_eq!(
            p["endpoint_novelty"]["flavor-separates-from-food"]["status"],
            "passed"
        );
        assert!(
            !p["proposal_pool"]["creative_repair"]["target_ids"]
                .as_array()
                .unwrap()
                .contains(&json!("flavor-separates-from-food"))
        );
        let mut failed_frontier = fixture["program"].clone();
        failed_frontier["tasks"] = failed_frontier["endpoint_proposal_attempt"]["tasks"].clone();
        record(&mut failed_frontier, pass);
        finish(&fixture["snapshot"], &mut failed_frontier, true, false).unwrap();
        assert_eq!(failed_frontier["endpoint_novelty"][id]["status"], "passed");
        assert!(
            failed_frontier["proposal_pool"]["creative_repair"]["target_ids"]
                .as_array()
                .unwrap()
                .contains(&json!(id)),
            "A separate failed frontier check still makes this slot repairable"
        );
    }

    #[test]
    fn captured_food_factual_repair_does_not_consume_creative_development() {
        let fixture: Value =
            serde_json::from_str(include_str!("semantic_food_repair_fixture.json")).unwrap();
        let snapshot = &fixture["snapshot"];
        let mut p = fixture["program"].clone();
        assert_eq!(p["transition_count"], 112);
        assert_eq!(p["proposal_pool"]["novelty_repair"]["status"], "completed");
        assert!(
            p["proposal_pool"]["revisions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let originals = p["proposal_pool"]["candidates"].clone();
        finish(snapshot, &mut p, true, false).unwrap();
        assert!(research_pending(&p));
        assert_eq!(
            p["endpoint_proposal_attempt"]["status"],
            "targeted_creative_repair"
        );
        assert_eq!(p["proposal_pool"]["candidates"], originals);
        assert!(
            !p["proposal_pool"]["creative_repair"]["target_ids"]
                .as_array()
                .unwrap()
                .contains(&json!("flavor-separates-from-food"))
        );
        let admission = &p["proposal_pool"]["creative_repair_admission"];
        eprintln!("captured food creative admission: {admission}");
        assert_eq!(admission["remaining_transitions"], 368);
        assert!(admission["required_transitions"].as_u64().unwrap() <= 368);
        let candidates = p["proposal_pool"]["candidates"].as_array().unwrap();
        let reply = json!({"hypotheses":[],"research_evidence":[],"proposal_contrasts":candidates.iter().map(|e|json!({"endpoint_id":e["id"],"contrast":e["contrast"]})).collect::<Vec<_>>(),"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()});
        let mut no_progress = contrasts(snapshot, &p, &reply).unwrap();
        assert_eq!(
            no_progress["proposal_pool"]["creative_repair"]["status"],
            "completed"
        );
        assert_eq!(
            no_progress["proposal_pool"]["creative_repair"]["creative_development_used"],
            false
        );
        finish(snapshot, &mut no_progress, true, false).unwrap();
        assert!(
            !research_pending(&no_progress),
            "Unchanged creative attempt cannot loop"
        );
        let mut refused = fixture["program"].clone();
        refused["transition_count"] = json!(450);
        finish(snapshot, &mut refused, true, false).unwrap();
        assert!(!research_pending(&refused));
        assert_eq!(
            refused["proposal_pool"]["creative_repair_admission"]["admitted"],
            false
        );
    }

    #[test]
    fn captured_food_equal_breadth_retains_currently_passed_candidate() {
        let f: Value =
            serde_json::from_str(include_str!("semantic_food_selection_fixture.json")).unwrap();
        let candidates = f["candidates"].as_array().unwrap();
        let checks = f["checks"].as_array().unwrap();
        let selected = select_distinct_candidates(candidates, checks, 5, &f);
        assert_eq!(selected.len(), 5);
        assert!(selected.contains(&json!("meals-learn-your-body")));
        // A passed candidate cannot bypass a known pairwise incompatibility.
        let mut conflicting = checks.clone();
        for c in &mut conflicting {
            if c["task"]["endpoint_id"] == "meals-learn-your-body"
                || c["task"]["other_endpoint_id"] == "meals-learn-your-body"
            {
                c["passed"] = json!(false);
            }
        }
        let preserved = select_distinct_candidates(candidates, &conflicting, 5, &f);
        assert_eq!(preserved.len(), 5);
        assert!(!preserved.contains(&json!("meals-learn-your-body")));
    }

    #[test]
    fn unresolved_selected_set_repairs_once_and_preserves_successful_candidates() {
        let (snapshot, mut p) = pool();
        let passed = p["proposal_pool"]["candidates"][0]["id"].clone();
        // Fixture alternatives otherwise duplicate exact comparison inputs.
        for key in ["proposal_pool", "endpoint_proposal_attempt"] {
            let array = if key == "proposal_pool" {
                "candidates"
            } else {
                "endpoints"
            };
            p[key][array][0]["contrast"]["present_analogue"]["statement"] =
                json!("A uniquely scoped observed comparison for this candidate");
        }
        record(&mut p, |t| {
            if t["function"] == "check_proposal_change" && t["endpoint_id"] != passed {
                "unresolved".into()
            } else {
                pass(t)
            }
        });
        finish(&snapshot, &mut p, true, false).unwrap();
        record(&mut p, pass);
        finish(&snapshot, &mut p, true, false).unwrap();
        assert!(research_pending(&p));
        assert!(p["endpoint_search"].is_null());
        assert!(
            !p["proposal_pool"]["novelty_repair"]["target_ids"]
                .as_array()
                .unwrap()
                .contains(&passed)
        );
        let candidates = p["proposal_pool"]["candidates"].as_array().unwrap();
        let reply = json!({"hypotheses":[],"research_evidence":[],"proposal_contrasts":candidates.iter().map(|e|json!({"endpoint_id":e["id"],"contrast":e["contrast"]})).collect::<Vec<_>>(),"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()});
        let mut rewritten = reply.clone();
        rewritten["proposal_contrasts"][0]["contrast"]["present_analogue"]["statement"] =
            json!("Changed successful comparison");
        assert!(
            contrasts(&snapshot, &p, &rewritten)
                .unwrap_err()
                .contains("preserve every non-target")
        );
        let target = &p["proposal_pool"]["novelty_repair"]["target_ids"][0];
        let mut replacement = candidates
            .iter()
            .find(|e| e["id"] == *target)
            .unwrap()
            .clone();
        replacement["commitments"][0]["statement"] =
            json!("A changed defining capability with interacting household consequences");
        let mut developed_reply = reply.clone();
        developed_reply["endpoint_revisions"] = json!([{"endpoint_id":target,"reason":"Address the unresolved defining capability","replacement":replacement}]);
        let developed = contrasts(&snapshot, &p, &developed_reply).unwrap();
        assert_eq!(
            developed["proposal_pool"]["creative_repair"]["creative_development_used"],
            true
        );
        assert!(
            !developed["tasks"].as_array().unwrap().is_empty(),
            "Changed commitments require new judgments"
        );
        assert_eq!(
            developed["proposal_pool"]["revisions"][0]["replacement"]["commitments"],
            replacement["commitments"]
        );
        assert_eq!(
            developed["proposal_pool"]["revisions"][0]["original"]["commitments"],
            candidates.iter().find(|e| e["id"] == *target).unwrap()["commitments"]
        );
        let mut repaired = contrasts(&snapshot, &p, &reply).unwrap();
        assert_eq!(
            repaired["proposal_pool"]["novelty_repair"]["progress"],
            "unchanged_no_progress"
        );
        assert!(
            repaired["tasks"].as_array().unwrap().is_empty(),
            "Unchanged requests must reuse exact checks"
        );
        let mut rejected = repaired.clone();
        rejected["tasks"] = rejected["endpoint_proposal_attempt"]["tasks"].clone();
        record(&mut rejected, |task| {
            if task["function"] == "check_proposal_change" {
                "present_or_adoption_only".into()
            } else {
                pass(task)
            }
        });
        finish(&snapshot, &mut rejected, true, false).unwrap();
        assert!(!research_pending(&rejected));
        assert_ne!(rejected["proposal_pool"]["stage"], "enrich");
        finish(&snapshot, &mut repaired, true, false).unwrap();
        assert!(repaired["tasks"].as_array().unwrap().is_empty());
        finish(&snapshot, &mut repaired, true, false).unwrap();
        assert!(!research_pending(&repaired));
        assert!(
            repaired["endpoint_search"]["routes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            repaired["endpoint_novelty"][passed.as_str().unwrap()]["status"],
            "passed"
        );
        assert!(
            repaired["endpoint_search"]["endpoints"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| !novelty_passed(&repaired, field(e, "id")))
        );
        let mut declined = p;
        assert!(skip_prefreeze_repair(&mut declined));
        assert_eq!(
            declined["proposal_pool"]["novelty_repair"]["status"],
            "not_admitted"
        );
        assert!(!research_pending(&declined));
        assert!(!skip_prefreeze_repair(&mut declined));
        let (snapshot, mut successful) = pool();
        record(&mut successful, pass);
        finish(&snapshot, &mut successful, true, false).unwrap();
        record(&mut successful, pass);
        finish(&snapshot, &mut successful, true, false).unwrap();
        assert!(successful["proposal_pool"]["novelty_repair"].is_null());
        assert!(successful["endpoint_search"].is_object());
    }

    #[test]
    fn contrast_targets_report_surplus_history_missing_and_duplicate_ids() {
        let (snapshot, mut program) = pool();
        let current =
            json!([{"id":"current-1"},{"id":"current-2"},{"id":"current-3"},{"id":"current-4"}]);
        program["proposal_pool"]["candidates"] = current.clone();
        let mut rows: Vec<Value> = current
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!({"endpoint_id":e["id"],"contrast":{}}))
            .collect();
        rows.extend(
            (1..=6).map(|i| json!({"endpoint_id":format!("historical-{i}"),"contrast":{}})),
        );
        let error =
            contrasts(&snapshot, &program, &json!({"proposal_contrasts":rows})).unwrap_err();
        assert!(error.contains("missing: []"), "{error}");
        assert!(error.contains("extra: [\"historical-1\""), "{error}");
        assert!(error.contains("current-4"), "{error}");
        assert!(error.contains("Historical and rejected candidates are not targets"));
        let error = contrasts(&snapshot, &program, &json!({"proposal_contrasts":[{"endpoint_id":"current-1"},{"endpoint_id":"current-1"},{"endpoint_id":"current-2"},{"endpoint_id":"current-3"}]})).unwrap_err();
        assert!(error.contains("missing: [\"current-4\"]"), "{error}");
        assert!(error.contains("duplicate: {\"current-1\"}"), "{error}");
        assert_eq!(program["proposal_pool"]["candidates"], current);
    }

    fn fixture() -> (Value, Value, Vec<Value>) {
        let raw: Value =
            serde_json::from_str(include_str!("semantic_food_proposal_fixture.json")).unwrap();
        let sources:Vec<_>=raw["baseline"]["observed"].as_array().unwrap().iter().map(|o|json!({"Id":o["evidence_ids"][0],"kind":"evidence","statement":o["claim"],"edges":"[]","evidence_metadata":{"kind":"finding","publication_date":null,"observation_period":{"start":null,"end":null},"retrieved_at":"2026-10-02"}})).collect();
        let snapshot = json!({"world":{"description":"How will people eat at home in2035?","last_ingest_date":"2026-10-02"},"nodes":sources});
        let p = json!({"world_search_contract":1,"endpoint_proposal_contract":2,"baseline":raw["baseline"],"results":{},"evaluations":{},"rounds":[],"started_at_ms":"12345"});
        (snapshot, p, raw["endpoints"].as_array().unwrap().clone())
    }
    fn contrast(e: &Value, source: &Value) -> Value {
        json!({"present_analogue":{"statement":"Scoped current arrangement from the supplied report","status":"supported","evidence_ids":[source]},"defining_commitment_ids":[e["commitments"][0]["id"]],"frontier_challenge":{"research_basis":"live_research","reported_queries":["current organizing mechanism and existing alternatives"],"comparisons":[{"commitment_id":e["commitments"][0]["id"],"result":"different_arrangement","present_match":"Strongest scoped present finding","remaining_difference":"The proposed organizing relationship differs","evidence_ids":[source]}]},"consequences":[{"commitment_id":e["commitments"][1]["id"],"depends_on":[e["commitments"][0]["id"]],"mechanism":"The defining arrangement changes how this consequence is produced"}]})
    }
    fn pool() -> (Value, Value) {
        let (s, p, mut candidates) = fixture();
        for i in 0..4 {
            let mut c = candidates[i].clone();
            c["id"] = json!(format!("alternative-{i}"));
            candidates.push(c);
        }
        let p = receive(&s, &p, candidates.clone()).unwrap();
        let rows: Vec<_> = candidates
            .iter()
            .map(|e| json!({"endpoint_id":e["id"],"contrast":contrast(e,&s["nodes"][0]["Id"])}))
            .collect();
        (s.clone(),contrasts(&s,&p,&json!({"proposal_contrasts":rows,"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()})).unwrap())
    }
    fn record(p: &mut Value, select: impl Fn(&Value) -> String) {
        for t in p["tasks"].as_array().unwrap().clone() {
            let r = request(&p["endpoint_proposal_attempt"], &t).unwrap();
            let selected = select(&t);
            let probs: serde_json::Map<String, Value> = r["questions"]["result"]["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .map(|k| (k.clone(), json!(if *k == selected { 1.0 } else { 0.0 })))
                .collect();
            let reply = json!({"model":super::super::super::MODEL,"answers":{"result":{"type":"choice","choice":selected,"probabilities":probs}}});
            let result = super::super::super::evaluation::validate(&r, &reply).unwrap();
            let evaluation = super::super::super::evaluation::evaluation_value(&r, &reply).unwrap();
            p["results"][field(&t, "nodeId")][field(&t, "function")] = json!(result);
            p["evaluations"][field(&t, "nodeId")][field(&t, "function")] = evaluation;
        }
    }
    fn pass(t: &Value) -> String {
        match field(t, "function") {
            "check_proposal_coverage" => "represented",
            "check_proposal_change" => "changed_arrangement",
            "check_proposal_dependence" => "dependent",
            _ => "distinct_arrangements",
        }
        .into()
    }

    #[test]
    fn source_forecasts_stay_out_of_present_comparison_and_compact_eight_claims_fit() {
        let (mut snapshot, program) = pool();
        let mut report = snapshot["nodes"][0].clone();
        report["Id"] = json!("forecast-report");
        report["claim_type"] = json!("source_projection");
        report["statement"] = json!(
            "A report published today projects an arrangement for 2040; it is not an observed outcome."
        );
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(report.clone());
        let candidates = program["proposal_pool"]["candidates"]
            .as_array()
            .unwrap()
            .clone();
        let next = schedule(&snapshot, &program, candidates.clone(), "individual").unwrap();
        let task = next["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["function"] == "check_proposal_change")
            .unwrap();
        let request = request(&next["endpoint_proposal_attempt"], task).unwrap();
        assert!(
            !request["state"]["source_evidence"]
                .as_array()
                .unwrap()
                .contains(&report)
        );
        assert!(snapshot["nodes"].as_array().unwrap().contains(&report));
        assert!(
            request["questions"]["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("not that the projected arrangement exists")
        );
        let mut candidate = candidates[0].clone();
        candidate["contrast"]["frontier_challenge"]["comparisons"][0]["evidence_ids"] =
            json!([report["Id"]]);
        assert!(validate_frontier_challenge(&candidate, &snapshot).is_err());
        let ids: Vec<_> = (0..8).map(|i| format!("defining-{i}")).collect();
        candidate["contrast"]["defining_commitment_ids"] = json!(ids);
        candidate["contrast"]["frontier_challenge"]["comparisons"]=json!(ids.iter().map(|id|json!({"commitment_id":id,"result":"different_arrangement","present_match":"Current relationship","remaining_difference":"Changed relationship","evidence_ids":[snapshot["nodes"][0]["Id"]]})).collect::<Vec<_>>());
        validate_frontier_challenge(&candidate, &snapshot).unwrap();
        assert!(
            candidate["contrast"]["frontier_challenge"]
                .to_string()
                .len()
                < 2400
        );
    }

    #[test]
    fn stronger_uncited_present_match_blocks_false_novelty_and_changes_cache_context() {
        let (mut snapshot, mut program) = pool();
        record(&mut program, pass);
        finish(&snapshot, &mut program, false, false).unwrap();
        let candidates = program["proposal_pool"]["candidates"]
            .as_array()
            .unwrap()
            .clone();
        let unchanged = schedule(&snapshot, &program, candidates.clone(), "individual").unwrap();
        assert!(unchanged["tasks"].as_array().unwrap().is_empty());
        let mut strong = snapshot["nodes"][0].clone();
        strong["Id"] = json!("stronger-present-match");
        strong["statement"] = json!(
            "The proposed organizing relationship already operates today, including its reusable components, rights and distribution."
        );
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(strong.clone());
        let changed = schedule(&snapshot, &program, candidates.clone(), "individual").unwrap();
        assert_eq!(changed["tasks"].as_array().unwrap().len(), candidates.len());
        assert!(
            changed["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["function"] == "check_proposal_change")
        );
        let request = request(&changed["endpoint_proposal_attempt"], &changed["tasks"][0]).unwrap();
        assert!(
            request["state"]["source_evidence"]
                .as_array()
                .unwrap()
                .contains(&strong)
        );
        assert!(
            !request["state"]["analogue"]["evidence_ids"]
                .as_array()
                .unwrap()
                .contains(&strong["Id"])
        );
        let mut candidates = candidates;
        let first = candidates[0]["id"].clone();
        candidates[0]["contrast"]["frontier_challenge"]["comparisons"][0]["result"] =
            json!("already_present");
        candidates[0]["contrast"]["frontier_challenge"]["comparisons"][0]["evidence_ids"] =
            json!([strong["Id"]]);
        validate_frontier_challenge(&candidates[0], &snapshot).unwrap();
        let mut checked = schedule(&snapshot, &program, candidates, "individual").unwrap();
        record(&mut checked, pass); // Even an erroneous positive model label cannot erase the known present match.
        finish(&snapshot, &mut checked, false, false).unwrap();
        let receipt = checked["proposal_pool"]["candidate_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["endpoint_id"] == first)
            .unwrap();
        assert_eq!(receipt["admissible"], false);
        assert_eq!(receipt["frontier_comparison_status"], "already_present");
    }

    #[test]
    fn unresolved_change_gets_paths_then_current_evidence_recheck_without_erasing_routes() {
        let (mut snapshot, mut p) = pool();
        p["proposal_pool"]["development"] = json!({"status":"completed","originals":p["proposal_pool"]["candidates"],"developed":p["proposal_pool"]["candidates"]});
        record(&mut p, |t| {
            if t["function"] == "check_proposal_change" {
                "unresolved".into()
            } else {
                pass(t)
            }
        });
        finish(&snapshot, &mut p, false, false).unwrap();
        assert_eq!(p["proposal_pool"]["stage"], "pairs");
        record(&mut p, pass);
        finish(&snapshot, &mut p, false, false).unwrap();
        let endpoints = p["endpoint_search"]["endpoints"].clone();
        assert!(endpoints.as_array().unwrap().len() >= 3);
        assert_eq!(p["endpoint_proposal_attempt"]["status"], "examined");
        let id = field(&endpoints[0], "id").to_owned();
        assert!(!novelty_passed(&p, &id));
        // Stored graph data is not evidence of a present analogue and must not
        // be rewritten by the deferred proposal continuation.
        p["endpoint_search"]["routes"] = json!([{"id":"saved-route","endpoint_id":id,"commitment_id":endpoints[0]["commitments"][0]["id"],"component_ids":["new-hypothetical-step"],"target_component_id":"new-hypothetical-step","grounding_evidence_ids":[],"root_connections":[{"component_id":"new-hypothetical-step","evidence_ids":[],"mechanism":"","unresolved_question":"Which observed capability could support this imagined step?"}],"status":"unresolved","alternative_to":null,"amendment_id":null,"chain":[]}]);
        let routes = p["endpoint_search"]["routes"].clone();
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":"new-hypothetical-step","kind":"scenario","statement":"A proposed path could deliver the imagined capability"}));
        let mut projection = snapshot["nodes"][0].clone();
        projection["Id"] = json!("new-projection");
        projection["claim_type"] = json!("source_projection");
        snapshot["nodes"].as_array_mut().unwrap().push(projection);
        let before = p.clone();
        assert!(defer_before_composition(&snapshot, &mut p, false).unwrap());
        assert!(p["tasks"].as_array().unwrap().is_empty(), "Hypothetical paths alone must reuse unresolved present comparison");
        finish(&snapshot, &mut p, false, false).unwrap();
        assert_eq!(p["endpoint_novelty"][&id]["status"], "unresolved");
        assert_eq!(p["endpoint_search"]["routes"], routes);
        assert!(!defer_before_composition(&snapshot, &mut p, false).unwrap(), "Only one bounded recheck");

        let mut finding = snapshot["nodes"][0].clone();
        finding["Id"] = json!("new-present-comparison");
        finding["statement"] = json!("A retrieved current comparison clarifies which capability is already offered and which consequence remains absent from that inspected arrangement.");
        snapshot["nodes"].as_array_mut().unwrap().push(finding.clone());
        let mut fresh = before.clone();
        assert!(defer_before_composition(&snapshot, &mut fresh, false).unwrap());
        assert_eq!(fresh["tasks"].as_array().unwrap().len(), endpoints.as_array().unwrap().len());
        let req = request(&fresh["endpoint_proposal_attempt"], &fresh["tasks"][0]).unwrap();
        assert!(req["state"]["source_evidence"].as_array().unwrap().contains(&finding));
        assert!(!req["state"]["source_evidence"].to_string().contains("new-hypothetical-step"));
        assert!(super::super::super::transition_limit(&fresh) > super::super::super::transition_limit(&json!({"stage":"proposals"})));
        assert!(super::super::super::transition_limit(&fresh) < super::super::super::MAX_APP_TRANSITIONS);
        let pending = fresh.clone();
        let pending_fixture = pending.clone();
        for completed_count in [0usize, 1] {
            let mut interrupted = pending.clone();
            let initial = interrupted["endpoint_novelty"].clone();
            interrupted["stop_reason"] = json!("provider_error");
            let first = interrupted["tasks"][0].clone();
            if completed_count == 1 {
                record(&mut interrupted, pass);
                for task in interrupted["tasks"].as_array().unwrap().clone().into_iter().skip(1) {
                    interrupted["results"][field(&task,"nodeId")][field(&task,"function")] = Value::Null;
                    interrupted["evaluations"][field(&task,"nodeId")][field(&task,"function")] = Value::Null;
                }
            }
            finish(&snapshot,&mut interrupted,false,true).unwrap();
            assert_eq!(interrupted["deferred_novelty_recheck"]["status"],"interrupted");
            assert_eq!(interrupted["deferred_novelty_recheck"]["performed_checks"],completed_count);
            assert_eq!(interrupted["endpoint_proposal_attempt"]["status"],"unresolved");
            for e in endpoints.as_array().unwrap() {
                let eid=field(e,"id");
                assert_eq!(interrupted["endpoint_novelty"][eid]["initial_check"],initial[eid]["initial_check"]);
            }
            if completed_count==1 { assert_eq!(interrupted["endpoint_novelty"][field(&first,"endpoint_id")]["status"],"passed"); }
            assert_eq!(pending_novelty_endpoints(&snapshot,&interrupted).unwrap().len(),endpoints.as_array().unwrap().len()-completed_count);
            let pending_cost=pending_novelty_checks(&snapshot,&interrupted).unwrap();
            assert!(pending_cost<=endpoints.as_array().unwrap().len()-completed_count);
            let mut resumed=interrupted.clone();
            resumed["stop_reason"]=Value::Null;
            assert!(defer_before_composition(&snapshot,&mut resumed,false).unwrap());
            assert_eq!(resumed["tasks"].as_array().unwrap().len(),pending_cost);

        }
        let mut blocked = before.clone();
        blocked["stop_reason"] = json!("provider_error");
        let original_receipts=blocked["endpoint_novelty"].clone();
        assert!(!defer_before_composition(&snapshot,&mut blocked,false).unwrap());
        assert_eq!(blocked["deferred_novelty_recheck"]["status"],"not_admitted");
        assert_eq!(blocked["endpoint_novelty"],original_receipts);
        record(&mut fresh, pass); // Deterministic Jev boundary; no accuracy claim.
        finish(&snapshot, &mut fresh, false, false).unwrap();
        assert_eq!(fresh["endpoint_novelty"][&id]["status"], "passed");
        assert_eq!(fresh["endpoint_search"]["routes"], routes);
        assert_eq!(fresh["endpoint_search"]["endpoints"], endpoints);
        let mut rejected = pending;
        record(&mut rejected, |_| "present_or_adoption_only".into());
        finish(&snapshot, &mut rejected, false, false).unwrap();
        assert_eq!(rejected["endpoint_novelty"][&id]["status"], "rejected");
        let before_fixture = before.clone();
        let mut no_budget = before;
        assert!(!defer_before_composition(&snapshot, &mut no_budget, true).unwrap());
        assert_eq!(no_budget["endpoint_novelty"][&id]["status"], "unresolved");
        assert_eq!(no_budget["endpoint_search"]["routes"], routes);
        if let Ok(path) = std::env::var("FORESIGHT_DEFERRED_FIXTURE") {
            std::fs::write(path, serde_json::to_vec_pretty(&json!({"snapshot":snapshot,"provisional":before_fixture,"not_admitted":no_budget,"checking":pending_fixture,"passed":fresh,"rejected":rejected,"disclosure":"Native deterministic producer test receipts, not live Jev predictions. Stored route is an immutability sentinel, not a verified causal graph."})).unwrap()).unwrap();
        }
    }

    #[test]
    fn captured_music_mixed_unknowns_retain_positive_checks_and_advance_to_pairs() {
        let fixture: Value =
            serde_json::from_str(include_str!("semantic_music_frontier_fixture.json")).unwrap();
        let attempt = fixture["attempt"].clone();
        assert_eq!(attempt["checks"].as_array().unwrap().len(), 15);
        assert!(
            attempt["checks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["passed"] == true)
        );
        let (mut snapshot, mut program) = pool();
        snapshot["world"] = attempt["world"].clone();
        snapshot["nodes"] = attempt["source_evidence"].clone();
        program["endpoint_proposal_attempt"] = attempt.clone();
        program["endpoint_proposal_history"] = json!([]);
        program["proposal_pool"]["candidates"] = attempt["endpoints"].clone();
        program["proposal_pool"]["development"]["status"] = json!("completed");
        for c in attempt["checks"].as_array().unwrap() {
            program["results"][field(&c["task"], "nodeId")][field(&c["task"], "function")] =
                c["result"].clone();
            program["evaluations"][field(&c["task"], "nodeId")][field(&c["task"], "function")] =
                c["evaluation"].clone();
        }
        for candidate in attempt["endpoints"].as_array().unwrap() {
            assert_eq!(frontier_status(candidate), "unresolved");
            assert!(frontier_admissible(candidate, true));
        }
        let original = program.clone();
        finish(&snapshot, &mut program, false, false).unwrap();
        assert_eq!(program["proposal_pool"]["stage"], "pairs");
        assert_eq!(
            program["endpoint_proposal_attempt"]["endpoints"],
            attempt["endpoints"]
        );
        assert!(program["proposal_pool"]["candidate_receipts"].as_array().unwrap().iter().all(|r| r["admissible"] == true && r["frontier_comparison_status"] == "unresolved"));
        for variant in ["unsupported_analogue", "already_present", "unknown"] {
            let mut rejected = original.clone();
            for candidate in rejected["endpoint_proposal_attempt"]["endpoints"]
                .as_array_mut()
                .unwrap()
            {
                if variant == "unsupported_analogue" {
                    candidate["contrast"]["present_analogue"]["status"] = json!("unknown");
                } else {
                    for row in candidate["contrast"]["frontier_challenge"]["comparisons"]
                        .as_array_mut()
                        .unwrap()
                    {
                        row["result"] = json!(variant);
                    }
                }
            }
            finish(&snapshot, &mut rejected, false, false).unwrap();
            assert_ne!(rejected["proposal_pool"]["stage"], "pairs", "{variant}");
        }
    }

    #[test]
    fn missing_present_comparison_is_unresolved_not_novel_or_impossible() {
        let (snapshot, program) = pool();
        let mut candidates = program["proposal_pool"]["candidates"]
            .as_array()
            .unwrap()
            .clone();
        for candidate in &mut candidates {
            let row = &mut candidate["contrast"]["frontier_challenge"]["comparisons"][0];
            row["result"] = json!("unknown");
            row["evidence_ids"] = json!([]);
            row["remaining_difference"] = json!("Targeted present comparison is still missing.");
            validate_frontier_challenge(candidate, &snapshot).unwrap();
        }
        let mut updated = program.clone();
        updated["proposal_pool"]["candidates"] = json!(candidates);
        let mut p = schedule(&snapshot, &updated, candidates.clone(), "individual").unwrap();
        record(&mut p, pass);
        finish(&snapshot, &mut p, false, false).unwrap();
        assert_eq!(p["proposal_pool"]["stage"], "unresolved");
        assert_eq!(p["proposal_pool"]["candidates"], json!(candidates));
        let mut missing = candidates[0].clone();
        missing["contrast"]
            .as_object_mut()
            .unwrap()
            .remove("frontier_challenge");
        assert!(validate_frontier_challenge(&missing, &snapshot).is_err());
        assert!(!frontier_admissible(&missing, true));
        assert!(frontier_admissible(&missing, false)); // Older saved pool contract remains understood.
    }

    #[test]
    fn captured_food_requires_specific_repairs_not_five_broad_approvals() {
        let (s, mut p) = pool();
        let a = &p["endpoint_proposal_attempt"];
        let food = a["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "gardened-protein")
            .unwrap()
            .clone();
        assert!(field(&food, "original_statement").contains("protein makers"));
        assert!(!food["commitments"].to_string().contains("protein"));
        let task = a["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| {
                t["endpoint_id"] == "gardened-protein" && t["function"] == "check_proposal_coverage"
            })
            .unwrap();
        let exact = request(a, task).unwrap();
        assert_eq!(exact["state"]["statement"], food["original_statement"]);
        assert_eq!(exact["state"]["commitments"], food["commitments"]);
        // Broad captured approvals are not evidence for the new small relations.
        let captured: Value =
            serde_json::from_str(include_str!("semantic_food_proposal_fixture.json")).unwrap();
        for c in captured["captured_checks"].as_array().unwrap() {
            assert!(matches!(
                c["result"].as_str(),
                Some("consequential_change" | "alternative_trajectories")
            ));
        }
        let mut no_relations = p.clone();
        finish(&s, &mut no_relations, true, false).unwrap();
        assert!(no_relations["endpoint_search"].is_null());
        record(&mut p, |t| match field(t, "function") {
            "check_proposal_coverage" if t["endpoint_id"] == "gardened-protein" => {
                "missing_defining_implication".into()
            }
            "check_proposal_change" => "present_or_adoption_only".into(),
            "check_proposal_dependence" => "independent_decoration".into(),
            _ => pass(t),
        });
        finish(&s, &mut p, true, false).unwrap();
        assert_eq!(p["proposal_pool"]["stage"], "enrich");
        assert!(p["proposal_pool"]["candidate_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|receipt| receipt["admissible"] == false));
        assert!(p["endpoint_search"].is_null());
        let failures = p["proposal_pool"]["candidate_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["endpoint_id"] == "gardened-protein")
            .unwrap();
        assert!(
            failures["failed_relations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["result"] == "missing_defining_implication"
                    && c["request"]["state"]["statement"] == food["original_statement"])
        );
        assert_eq!(p["started_at_ms"], "12345");
    }
    #[test]
    fn unknown_analogue_cannot_become_novelty_even_with_all_positive_jev_labels() {
        let (s, mut p) = pool();
        for e in p["endpoint_proposal_attempt"]["endpoints"]
            .as_array_mut()
            .unwrap()
        {
            e["contrast"]["present_analogue"]["status"] = json!("unknown");
            e["contrast"]["present_analogue"]["evidence_ids"] = json!([]);
        }
        record(&mut p, pass);
        finish(&s, &mut p, true, false).unwrap();
        assert!(p["endpoint_search"].is_null());
        assert_eq!(p["proposal_pool"]["stage"], "enrich");
        assert!(p["proposal_pool"]["candidate_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|receipt| receipt["admissible"] == false));
        let mut bad = p["proposal_pool"]["candidates"][0].clone();
        bad["contrast"]["present_analogue"]["status"] = json!("supported");
        bad["contrast"]["present_analogue"]["evidence_ids"] = json!([]);
        assert!(
            validate_contrast(&bad, &s)
                .unwrap_err()
                .contains("actual source")
        );
        bad["contrast"]["present_analogue"]["evidence_ids"] = json!([s["nodes"][0]["Id"]]);
        bad["contrast"]["defining_commitment_ids"] = json!(["unrepresented-protein-production"]);
        assert!(validate_contrast(&bad, &s)
            .unwrap_err()
            .contains("actual commitment"));
    }
    #[test]
    fn capability_and_experience_definition_reaches_change_and_pair_requests() {
        let (_, p) = pool();
        let attempt = &p["endpoint_proposal_attempt"];
        let endpoints = attempt["endpoints"].as_array().unwrap();
        for function in ["check_proposal_change", "check_proposal_pair"] {
            let task = json!({"function":function,"endpoint_id":endpoints[0]["id"],"other_endpoint_id":endpoints[1]["id"]});
            let wire = request(attempt, &task).unwrap();
            let question = &wire["questions"]["result"];
            assert!(question["instructions"].as_str().unwrap().starts_with(WORLD_CHANGE_SEMANTICS));
            let category = if function == "check_proposal_change" {"changed_arrangement"} else {"distinct_arrangements"};
            assert!(question["criteria"][category].as_str().unwrap().contains("capabilities, experiences or causal operation"));
            assert!(!question.to_string().contains("survives removing devices"));
            if function == "check_proposal_change" {
                assert_eq!(wire["state"]["frontier_challenge"], endpoints[0]["contrast"]["frontier_challenge"]);
                assert!(!wire["state"]["source_evidence"].as_array().unwrap().is_empty());
                assert!(question["criteria"].get("unsupported_analogue").is_some());
                assert!(question["criteria"].get("unresolved").is_some());
            } else {
                assert_eq!(wire["state"]["left"], endpoints[0]);
                assert_eq!(wire["state"]["right"], endpoints[1]);
            }
        }
    }

    #[test]
    fn defining_commitments_can_form_acyclic_dependent_consequences() {
        let (snapshot, _, candidates) = fixture();
        let mut endpoint = candidates[0].clone();
        endpoint["commitments"] = json!([
            {"id":"c1","statement":"First defining change"},
            {"id":"c2","statement":"Second defining change"},
            {"id":"c3","statement":"Their interacting defining consequence"},
            {"id":"c4","statement":"A further defining consequence"}
        ]);
        endpoint["contrast"] = contrast(&endpoint, &snapshot["nodes"][0]["Id"]);
        endpoint["contrast"]["defining_commitment_ids"] = json!(["c1","c2","c3","c4"]);
        endpoint["contrast"]["consequences"] = json!([
            {"commitment_id":"c3","depends_on":["c1","c2"],"mechanism":"The first two changes jointly enable the third"},
            {"commitment_id":"c4","depends_on":["c1","c2","c3"],"mechanism":"Their combined effects enable the fourth"}
        ]);
        assert!(validate_contrast(&endpoint, &snapshot).is_ok());
        let task = json!({"function":"check_proposal_dependence","endpoint_id":endpoint["id"],"relation_index":1});
        let state = request(&json!({"endpoints":[endpoint.clone()]}), &task).unwrap();
        assert_eq!(state["state"]["relation"], endpoint["contrast"]["consequences"][1]);
        // Intermediate dependencies need not have the separate defining role.
        let mut intermediate = endpoint.clone();
        intermediate["contrast"]["defining_commitment_ids"] = json!(["c1","c2"]);
        assert!(validate_contrast(&intermediate, &snapshot).is_ok());
        let mut invalid = endpoint.clone();
        invalid["contrast"]["consequences"][0]["depends_on"] = json!(["c3"]);
        assert!(validate_contrast(&invalid, &snapshot).unwrap_err().contains("distinct"));
        invalid["contrast"]["consequences"][0]["depends_on"] = json!(["missing"]);
        assert!(validate_contrast(&invalid, &snapshot).unwrap_err().contains("actual commitments"));
        invalid["contrast"]["consequences"][0]["depends_on"] = json!(["c4"]);
        assert!(validate_contrast(&invalid, &snapshot).unwrap_err().contains("cycle"));
        invalid = endpoint.clone();
        invalid["contrast"]["consequences"].as_array_mut().unwrap().push(json!({"commitment_id":"c1","depends_on":["c4"],"mechanism":"Circular third edge"}));
        assert!(validate_contrast(&invalid, &snapshot).unwrap_err().contains("cycle"));
        invalid = endpoint.clone();
        invalid["contrast"]["consequences"][1] = invalid["contrast"]["consequences"][0].clone();
        assert!(validate_contrast(&invalid, &snapshot).unwrap_err().contains("distinct actual"));
        invalid = endpoint;
        invalid["contrast"]["consequences"][0]["mechanism"] = json!("x".repeat(601));
        assert!(validate_contrast(&invalid, &snapshot).is_err());
    }

    #[test]
    fn one_of_ten_viable_candidates_develops_once_before_fresh_admission() {
        let (s, old, mut candidates) = fixture();
        for i in 0..6 {
            let mut e = candidates[0].clone();
            e["id"] = json!(format!("provisional-{i}"));
            candidates.push(e);
        }
        let response = |items: &[Value]| json!({"proposal_contrasts":items.iter().map(|e|json!({"endpoint_id":e["id"],"contrast":contrast(e,&s["nodes"][0]["Id"])})).collect::<Vec<_>>(),"comparison_priority":items.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()});
        let mut p = contrasts(
            &s,
            &receive(&s, &old, candidates.clone()).unwrap(),
            &response(&candidates),
        )
        .unwrap();
        p["transition_count"] = json!(88);
        let only = candidates[0]["id"].clone();
        record(&mut p, |task| {
            if task["function"] == "check_proposal_change" && task["endpoint_id"] != only {
                "present_or_adoption_only".into()
            } else {
                pass(task)
            }
        });
        let before = p.clone();
        finish(&s, &mut p, true, false).unwrap();
        assert_eq!(p["proposal_pool"]["stage"], "enrich");
        assert_eq!(
            p["proposal_pool"]["candidate_receipts"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["admissible"] == true)
                .count(),
            1
        );
        assert_eq!(
            p["proposal_pool"]["selected_ids"].as_array().unwrap().len(),
            3
        );
        assert!(p["endpoint_search"].is_null());
        if let Ok(path) = std::env::var("FORESIGHT_PROVISIONAL_FIXTURE") {
            std::fs::write(path, serde_json::to_vec_pretty(&json!({"snapshot":s,"program":p,"disclosure":"Native producer state from deterministic Jev fixtures; provisional selection, not live acceptance."})).unwrap()).unwrap();
        }
        let selected = p["proposal_pool"]["selected_ids"].as_array().unwrap();
        let developed: Vec<_> = candidates
            .iter()
            .filter(|e| selected.contains(&e["id"]))
            .cloned()
            .map(|mut e| {
                e["commitments"][0]["statement"] =
                    json!("A genuinely changed defining arrangement requiring a fresh check");
                e
            })
            .collect();
        let next = receive(&s, &p, developed.clone()).unwrap();
        assert_eq!(
            next["proposal_pool"]["research_attempts"],
            before["proposal_pool"]["research_attempts"]
        );
        assert_eq!(next["transition_count"], 88);
        let mut unchecked = contrasts(&s, &next, &response(&developed)).unwrap();
        assert!(!unchecked["tasks"].as_array().unwrap().is_empty());
        finish(&s, &mut unchecked, true, false).unwrap();
        assert!(unchecked["endpoint_search"].is_null());
        assert_ne!(
            unchecked["proposal_pool"]["stage"], "enrich",
            "Development may run only once"
        );
        let mut exhausted = before.clone();
        exhausted["proposal_pool"]["research_attempts"] = json!(3);
        finish(&s, &mut exhausted, true, false).unwrap();
        assert_ne!(exhausted["proposal_pool"]["stage"], "enrich");
        let mut late = before;
        late["transition_count"] = json!(400);
        finish(&s, &mut late, true, false).unwrap();
        assert_ne!(late["proposal_pool"]["stage"], "enrich");
        assert_eq!(
            late["proposal_pool"]["development_admission"]["admitted"],
            false
        );
    }

    #[test]
    fn direct_freeze_keeps_unresolved_novelty_provisional() {
        let (snapshot, mut program) = pool();
        record(&mut program, |t| {
            if t["function"] == "check_proposal_change" {
                "unresolved".into()
            } else {
                pass(t)
            }
        });
        finish(&snapshot, &mut program, true, false).unwrap();
        let receipts = program["endpoint_novelty"].clone();
        record(&mut program, pass);
        finish(&snapshot, &mut program, false, false).unwrap();
        assert_eq!(program["stage"], "exploration");
        assert!(program["proposal_pool"]["development"].is_null());
        assert_eq!(program["endpoint_novelty"], receipts);
        assert_eq!(program["endpoint_proposal_attempt"]["status"], "examined");
        assert!(
            program["endpoint_search"]["endpoints"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| !novelty_passed(&program, field(e, "id")))
        );
    }

    #[test]
    fn breadth_selection_freezes_directly_and_saved_development_keeps_exact_cache() {
        let (s, mut p) = pool();
        record(&mut p, pass);
        finish(&s, &mut p, true, false).unwrap();
        assert_eq!(p["endpoint_proposal_attempt"]["pool_stage"], "pairs");
        assert_eq!(p["tasks"].as_array().unwrap().len(), 28);
        let mut later_distinct = p.clone();
        let early: Vec<_> = p["endpoint_proposal_attempt"]["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .take(6)
            .map(|e| e["id"].clone())
            .collect();
        let late: Vec<_> = p["endpoint_proposal_attempt"]["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .skip(6)
            .map(|e| e["id"].clone())
            .collect();
        record(&mut later_distinct, |t| {
            if early.contains(&t["endpoint_id"]) && early.contains(&t["other_endpoint_id"]) {
                "same_arrangement".into()
            } else {
                "distinct_arrangements".into()
            }
        });
        finish(&s, &mut later_distinct, true, false).unwrap();
        let chosen = later_distinct["proposal_pool"]["selected_ids"]
            .as_array()
            .unwrap();
        assert_eq!(chosen.len(), 3);
        assert!(
            late.iter().all(|id| chosen.contains(id)),
            "viable later candidates must not be pruned behind the first six"
        );
        let mut slices = p.clone();
        record(&mut slices, |_| "complementary_slices".into());
        finish(&s, &mut slices, true, false).unwrap();
        assert!(slices["endpoint_search"].is_null());
        record(&mut p, pass);
        finish(&s, &mut p, true, false).unwrap();
        let selected = p["proposal_pool"]["selected_ids"].as_array().unwrap();
        assert_eq!(selected.len(), 5);
        let originals: Vec<_> = p["proposal_pool"]["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| selected.contains(&e["id"]))
            .cloned()
            .collect();
        assert_eq!(p["stage"], "exploration");
        assert_eq!(p["endpoint_search"]["endpoints"], json!(originals));
        assert!(p["proposal_pool"]["development"].is_null());
        let obligations = super::super::super::backward::batch(&s, &p);
        assert_eq!(obligations["mode"], "complete_original");
        assert_eq!(obligations["commitments"].as_array().unwrap().len(), originals[0]["commitments"].as_array().unwrap().len());
        // A saved pre-change checkpoint may already be waiting for development.
        // Preserve that continuation; new successful selections never enter it.
        let mut queued_development = p.clone();
        queued_development.as_object_mut().unwrap().remove("endpoint_search");
        queued_development["proposal_pool"]["stage"] = json!("enrich");
        let mut changed = originals.clone();
        changed[0]["original_statement"] =
            json!("A substantively different arrangement with new interacting consequences");
        changed[0]["commitments"][0]["statement"] = json!("A changed load-bearing future event");
        let developed = receive(&s, &queued_development, changed.clone()).unwrap();
        assert_eq!(developed["proposal_pool"]["stage"], "contrast");
        assert!(developed["endpoint_search"].is_null());
        assert!(
            developed["proposal_pool"]["candidates"][0]["contrast"].is_null(),
            "Stale contrast from the generator must not transfer"
        );
        assert_eq!(
            developed["proposal_pool"]["development"]["originals"],
            json!(originals)
        );
        let contrast_reply = |candidates: &[Value]| json!({"proposal_contrasts":candidates.iter().map(|e|json!({"endpoint_id":e["id"],"contrast":contrast(e,&s["nodes"][0]["Id"])})).collect::<Vec<_>>(),"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()});
        let changed_context = contrasts(&s, &developed, &contrast_reply(&changed)).unwrap();
        assert!(
            changed_context["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"] == "check_proposal_change"
                    && t["endpoint_id"] == changed[0]["id"]),
            "Changed defining event cannot reuse old approval"
        );
        let mut unapproved = changed_context.clone();
        finish(&s, &mut unapproved, false, false).unwrap();
        assert!(
            unapproved["endpoint_search"].is_null(),
            "Missing new checks cannot freeze changed commitments"
        );
        let mut changed_checked = changed_context;
        record(&mut changed_checked, pass);
        finish(&s, &mut changed_checked, true, false).unwrap();
        assert_eq!(changed_checked["proposal_pool"]["stage"], "pairs");
        assert!(
            changed_checked["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"] == "check_proposal_pair"
                    && (t["endpoint_id"] == changed[0]["id"]
                        || t["other_endpoint_id"] == changed[0]["id"])),
            "Changed worlds need fresh pair distinction"
        );
        record(&mut changed_checked, pass);
        finish(&s, &mut changed_checked, true, false).unwrap();
        assert_eq!(changed_checked["proposal_pool"]["stage"], "accepted");
        assert_eq!(
            changed_checked["endpoint_search"]["endpoints"][0]["commitments"],
            changed[0]["commitments"]
        );
        assert_ne!(changed_checked["proposal_pool"]["stage"], "enrich");
        let mut repeated = developed.clone();
        repeated["proposal_pool"]["stage"] = json!("enrich");
        assert!(
            receive(&s, &repeated, originals.clone())
                .unwrap_err()
                .contains("once")
        );
        let unchanged = receive(&s, &queued_development, originals.clone()).unwrap();
        let mut enriched = contrasts(&s, &unchanged, &contrast_reply(&originals)).unwrap();
        assert_eq!(
            enriched["tasks"],
            json!([]),
            "Unchanged exact requests may reuse recorded checks after explicit contrast reconciliation"
        );
        finish(&s, &mut enriched, true, false).unwrap();
        assert_eq!(enriched["proposal_pool"]["stage"], "pairs");
        assert_eq!(enriched["tasks"], json!([]));
        finish(&s, &mut enriched, true, false).unwrap();
        assert_eq!(
            enriched["endpoint_search"]["endpoints"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        let mut new_baseline = queued_development.clone();
        new_baseline["baseline"]["unknowns"] = json!(["New evidence changes the comparison scope"]);
        let development = receive(&s, &new_baseline, originals.clone()).unwrap();
        let refreshed = contrasts(&s, &development, &contrast_reply(&originals)).unwrap();
        assert!(
            refreshed["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"] == "check_proposal_change"),
            "Changed baseline context invalidates cached present-relative judgments"
        );
        if let Ok(path) = std::env::var("FORESIGHT_POOL_FIXTURE") {
            std::fs::write(path,serde_json::to_string_pretty(&json!({"snapshot":s,"individual":p["endpoint_proposal_history"][0],"pairs":p["endpoint_proposal_history"][1],"program":enriched,"developed_program":changed_checked,"disclosure":"Real producer state from deterministic Jev response fixtures; not live semantic validation."})).unwrap()).unwrap();
        }
    }
    #[test]
    fn same_response_sources_are_bound_and_candidate_revisions_are_explicit() {
        let (mut s, mut p) = pool();
        p["proposal_pool"]["stage"] = json!("contrast");
        let mut source = s["nodes"][0].clone();
        source["Id"] = json!("r1-new-source");
        s["nodes"].as_array_mut().unwrap().push(source);
        let candidates = p["proposal_pool"]["candidates"].as_array().unwrap().clone();
        let rows: Vec<_> = candidates
            .iter()
            .map(|e| json!({"endpoint_id":e["id"],"contrast":contrast(e,&json!("new-source"))}))
            .collect();
        let mut revised = candidates[0].clone();
        revised["commitments"][0]["statement"] =
            json!("A different consequential organizing relationship is explicitly proposed");
        let reply = json!({"research_evidence":[{"id":"new-source"}],"proposal_contrasts":rows,"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>(),"endpoint_revisions":[{"endpoint_id":revised["id"],"reason":"The prior defining commitment only changed adoption","replacement":revised}]});
        let next = contrasts(&s, &p, &reply).unwrap();
        assert_eq!(
            next["proposal_pool"]["candidates"][0]["contrast"]["present_analogue"]["evidence_ids"],
            json!(["r1-new-source"])
        );
        assert_eq!(
            next["proposal_pool"]["revisions"][0]["original"],
            candidates[0]
        );
        assert_eq!(
            next["proposal_pool"]["revisions"][0]["replacement"]["commitments"],
            revised["commitments"]
        );
        let mut unsupported = s.clone();
        unsupported["nodes"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["evidence_metadata"]["kind"] = json!("lead");
        assert!(
            contrasts(&unsupported, &p, &reply)
                .unwrap_err()
                .contains("typed findings")
        );
    }

    #[test]
    fn all_twelve_viable_candidates_are_compared_or_explicitly_budget_limited() {
        let (s, p, base) = fixture();
        let candidates: Vec<_> = (0..12)
            .map(|i| {
                let mut e = base[i % 4].clone();
                e["id"] = json!(format!("candidate-{i}"));
                e
            })
            .collect();
        let draft = receive(&s, &p, candidates.clone()).unwrap();
        let reply = json!({"proposal_contrasts":candidates.iter().map(|e|json!({"endpoint_id":e["id"],"contrast":contrast(e,&s["nodes"][0]["Id"])})).collect::<Vec<_>>(),"comparison_priority":candidates.iter().map(|e|e["id"].clone()).collect::<Vec<_>>()});
        let mut checked = contrasts(&s, &draft, &reply).unwrap();
        record(&mut checked, pass);
        let mut limited = checked.clone();
        limited["transition_count"] = json!(200);
        finish(&s, &mut limited, false, false).unwrap();
        assert_eq!(limited["endpoint_proposal_attempt"]["status"], "unresolved");
        assert_eq!(
            limited["proposal_pool"]["selection_receipts"]
                .as_array()
                .unwrap()
                .len(),
            12
        );
        assert!(
            limited["proposal_pool"]["selection_receipts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["reason"] == "not_examined_budget")
        );
        finish(&s, &mut checked, true, false).unwrap();
        assert_eq!(checked["tasks"].as_array().unwrap().len(), 66);
        let actual_cost = check_transitions(&s, &checked).unwrap();
        assert!(
            actual_cost <= 12,
            "captured full12 pool pair packing uses at most6 HTTP batches, observed{actual_cost} transitions"
        );
        assert_eq!(
            checked["proposal_pool"]["pair_admission"]["check_transitions"],
            actual_cost
        );
    }

    #[test]
    fn representative_pool_packing_and_reserved_budget() {
        let (s, mut p) = pool();
        let mut batches = 0;
        let total = p["tasks"].as_array().unwrap().len();
        while p["cursor"].as_u64().unwrap() < (total as u64) {
            let b = super::super::super::batch::prepare(&s, &p, 5000).unwrap();
            assert!(!b.tasks.is_empty());
            p["cursor"] = json!(p["cursor"].as_u64().unwrap() + b.tasks.len() as u64);
            batches += 1;
        }
        assert_eq!(total, 24);
        assert_eq!(
            batches, 3,
            "same relation types share exact-context provider batches"
        );
        assert_eq!(reserve(), 220);
        assert!(admits(356, 2, 160));
        assert!(!admits(355, 2, 160));
        assert!(2 * batches + 2 * REASONING_ADMISSION_RESERVE + reserve() < 342);
    }
}
