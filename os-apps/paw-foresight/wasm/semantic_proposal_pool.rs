// Candidate exploration stays inside the existing imagine/explore/proposals lifecycle.
// These checks are fallible model judgments, never novelty certificates.
use super::super::{
    REASONING_TRANSITION_RESERVE, evidence, field, references_for_endpoints as references,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn enabled(program: &Value) -> bool {
    program["endpoint_proposal_contract"] == 2
}
pub fn research_pending(program: &Value) -> bool {
    enabled(program) && program["proposal_pool"]["stage"] == "contrast"
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
    5 * REASONING_TRANSITION_RESERVE
}
pub fn admits(remaining: u64, reasoning_turns: u64, questions: usize) -> bool {
    remaining
        >= reserve()
            + reasoning_turns * REASONING_TRANSITION_RESERVE
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
        && rows.iter().all(|r| r["result"] != "unknown")
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
    for relation in consequences {
        let consequence = relation["commitment_id"]
            .as_str()
            .ok_or("Missing consequence commitment")?;
        if !known.contains(consequence)
            || defining.contains(&consequence)
            || !seen.insert(consequence)
        {
            return Err("Dependent consequence must name a distinct actual commitment".into());
        }
        let parents = ids(&relation["depends_on"])?;
        if parents.iter().any(|id| !defining.contains(id)) {
            return Err("Consequence must depend on the declared defining commitments".into());
        }
        text(&relation["mechanism"], 600)?;
    }
    Ok(())
}

pub fn contrasts(snapshot: &Value, old: &Value, generated: &Value) -> Result<Value, String> {
    if generated.to_string().len() > 64 * 1024 {
        return Err("Candidate contrast response exceeds 64 KiB".into());
    }
    let mut generated = generated.clone();
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
    if contrasts.len() != candidates.len() {
        return Err(
            "Every candidate needs an explicit contrast, including unknown analogues".into(),
        );
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
            tasks.push(task(
                number,
                "check_proposal_coverage",
                field(e, "id"),
                "",
                0,
            ));
            tasks.push(task(number, "check_proposal_change", field(e, "id"), "", 0));
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
                .filter(|n| n["evidence_metadata"]["kind"] == "finding")
                .collect();
            (
                json!({"baseline":attempt["baseline"],"analogue":e["contrast"]["present_analogue"],"frontier_challenge":e["contrast"]["frontier_challenge"],"source_evidence":sources,"defining_commitments":e["commitments"].as_array().into_iter().flatten().filter(|c|e["contrast"]["defining_commitment_ids"].as_array().is_some_and(|ids|ids.contains(&c["id"]))).collect::<Vec<_>>(),"consequences":e["contrast"]["consequences"]}),
                json!({"changed_arrangement":"Cited present analogue is supported in its actual scope, and the defining commitments specify a materially different organizing arrangement with dependent consequences. Adoption counts only when it changes the arrangement, not merely availability or prevalence.","present_or_adoption_only":"The defining mechanism already exists in the compared setting or only price, distribution, prevalence, packaging or convenience changes without a different organizing relationship.","unsupported_analogue":"The cited material does not substantiate the claimed present analogue or necessary scope contrast; absence of evidence is not evidence of novelty.","unresolved":"A consequential present-relative change is not established."}),
                "Challenge the chosen analogue against ALL supplied source findings: an uncited stronger present match overrides a weak selected comparison. Check each defining commitment against the strongest relevant existing organizing arrangement, including the present frontier rather than only average adoption. A supported example does not establish that it is closest. A currently published report may project future outcomes: its publication and forecast are evidence only of what the source reports, not that the projected arrangement exists. Preserve claim_type, provenance, source_correction, observation dates and textual qualifications; do not classify a projected outcome as a present counterexample. Reported queries are researcher self-report, not independently verified search or proof of absence. If necessary comparison coverage remains unknown, choose unresolved. Preserve scope/date caveats. Do not award novelty for low probability, future dates, narrative length, or unsupported claims about today's absence. A durable old arrangement under changed conditions may qualify if its new consequential relationship is explicit.",
            )
        }
        "check_proposal_dependence" => (
            json!({"commitments":e["commitments"],"relation":e["contrast"]["consequences"][t["relation_index"].as_u64().unwrap_or(0) as usize]}),
            json!({"dependent":"Removing the named defining change removes or materially alters the named consequence through the stated mechanism.","independent_decoration":"The consequence follows just as well without the defining change; it is decoration or a complementary topic, not a consequence.","unresolved":"The proposed causal dependence is unclear or contradicted."}),
            "Evaluate this small counterfactual dependency, not whether either event is likely. Can the consequence remain essentially unchanged when the defining change is removed?",
        ),
        "check_proposal_pair" => (
            json!({"left":e,"right":endpoint(field(t,"other_endpoint_id"))?}),
            json!({"distinct_arrangements":"The two defining mechanisms organize the answer differently and imply different dependent consequences. Overlap is allowed, but the distinction survives removing devices, topic labels and decorative scenes.","complementary_slices":"These are compatible features/topics of the same organizing arrangement rather than different answers to the whole question.","same_arrangement":"Same organizing mechanism and consequences under different wording.","unresolved":"A consequential distinction is not specified."}),
            "Compare defining relationships and their consequences, not topical coverage. Do not demand logical incompatibility. Ask what whole-answer distinction remains if both named tools exist in the same household or setting.",
        ),
        _ => return Err("Unknown pool relation".into()),
    };
    state["world"] = attempt["world"].clone();
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
pub fn finish(
    snapshot: &Value,
    p: &mut Value,
    allow_retry: bool,
    exhausted: bool,
) -> Result<(), String> {
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
    a["checks"] = json!(checks);
    a["status"] = json!("examined");
    p["endpoint_proposal_history"]
        .as_array_mut()
        .ok_or("Missing pool history")?
        .push(a.clone());
    p["endpoint_proposal_attempt"] = a.clone();
    p["tasks"] = json!([]);
    p["cursor"] = json!(0);
    if exhausted {
        p["endpoint_proposal_attempt"]["status"] = json!("unresolved");
        return Ok(());
    }
    let candidates = a["endpoints"].as_array().unwrap();
    if a["pool_stage"] == "pairs" {
        let remaining =
            super::super::MAX_APP_TRANSITIONS.saturating_sub(super::super::transition_count(p));
        let developed = p["proposal_pool"]["development"]["status"] == "completed";
        let remaining_reasoning_turns = if developed { 2 } else { 4 };
        let max_selected = (remaining.saturating_sub(32) / REASONING_TRANSITION_RESERVE)
            .saturating_sub(remaining_reasoning_turns)
            .min(5) as usize;
        p["proposal_pool"]["selection_budget"] = json!({"remaining_transitions":remaining,"max_selected":max_selected,"reserved_reasoning_turns_per_selected_world":1,"development_research_composition_writing_turns":remaining_reasoning_turns,"evaluation_tail":32});
        let mut selected = vec![];
        // At most twelve candidates: exhaustively select the largest compatible
        // subset, not a greedy first-fit cluster. Ties retain stable input order.
        for bits in 0usize..(1usize << candidates.len()) {
            let ids: Vec<_> = candidates
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, e)| e["id"].clone())
                .collect();
            if ids.len() < 3 || ids.len() > max_selected || ids.len() <= selected.len() {
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
            }
        }
        p["proposal_pool"]["selected_ids"] = json!(selected);
        p["proposal_pool"]["selection_receipts"]=json!(candidates.iter().map(|e|json!({"endpoint_id":e["id"],"selected":selected.contains(&e["id"]),"reason":if selected.contains(&e["id"]){"pairwise_distinct_set"}else{"not_in_bounded_distinct_set"}})).collect::<Vec<_>>());
        if selected.len() >= 3 {
            if developed {
                let accepted: Vec<_> = candidates
                    .iter()
                    .filter(|e| selected.contains(&e["id"]))
                    .cloned()
                    .collect();
                p["endpoint_search"] = json!({"status":"imagined","backward_batch_contract":2,"endpoints":accepted,"routes":[],"amendments":[],"rounds":[]});
                p["endpoint_proposal_attempt"]["status"] = json!("accepted");
                p["proposal_pool"]["stage"] = json!("accepted");
                p["stage"] = json!("exploration");
            } else {
                p["proposal_pool"]["stage"] = json!("enrich");
                p["endpoint_proposal_attempt"]["status"] = json!("development_requested");
            }
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
                        .all(|c| c["passed"] == true)
            })
            .cloned()
            .collect();
        if a["pool_stage"] == "individual" {
            p["proposal_pool"]["candidate_receipts"]=json!(candidates.iter().map(|e|json!({"endpoint_id":e["id"],"admissible":viable.iter().any(|v|v["id"]==e["id"]),"analogue_status":e["contrast"]["present_analogue"]["status"],"frontier_comparison_status":frontier_status(e),"failed_relations":checks.iter().filter(|c|c["task"]["endpoint_id"]==e["id"] && c["passed"]!=true).cloned().collect::<Vec<_>>()})).collect::<Vec<_>>());
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
            let development_turns = if p["proposal_pool"]["development"]["status"] == "completed" {
                0
            } else {
                2
            };
            let reserved_tail = reserve() + development_turns * REASONING_TRANSITION_RESERVE + 32;
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
    fn source_forecasts_keep_their_qualifications_and_compact_eight_claims_fit() {
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
            request["state"]["source_evidence"]
                .as_array()
                .unwrap()
                .contains(&report)
        );
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
        assert_eq!(p["proposal_pool"]["stage"], "contrast");
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
        assert_eq!(p["proposal_pool"]["stage"], "contrast");
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
        assert!(
            validate_contrast(&bad, &s)
                .unwrap_err()
                .contains("actual commitment")
        );
    }
    #[test]
    fn breadth_shortlist_pair_distinction_enrichment_and_exact_cache() {
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
        let mut changed = originals.clone();
        changed[0]["original_statement"] =
            json!("A substantively different arrangement with new interacting consequences");
        changed[0]["commitments"][0]["statement"] = json!("A changed load-bearing future event");
        let developed = receive(&s, &p, changed.clone()).unwrap();
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
        let unchanged = receive(&s, &p, originals.clone()).unwrap();
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
        let mut new_baseline = p.clone();
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
        assert!(2 * batches + 2 * REASONING_TRANSITION_RESERVE + reserve() < 342);
    }
}
