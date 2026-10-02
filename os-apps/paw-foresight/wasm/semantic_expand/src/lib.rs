#[allow(dead_code)]
mod scope {
    include!("../../semantic_scope.rs");
}
use temper_wasm_sdk::prelude::*;
// Each phase includes the shared evaluator contract but uses only its own subset.
#[allow(dead_code, unused_imports)]
mod core {
    include!("../../semantic_core.rs");
}
mod outlook {
    include!("../../semantic_outlook.rs");
}
// The producer projects aliases; the consumer resolves them. Both share one mapping.
#[allow(dead_code)]
mod references {
    include!("../../semantic_references.rs");
}
fn identifier(v: &Value) -> Result<&str, String> {
    let id = v
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 100)
        .ok_or("Missing generated identity")?;
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("Invalid generated identity".into());
    }
    Ok(id)
}
fn generated_node(
    v: &Value,
    kind: &str,
    known: &std::collections::BTreeSet<String>,
) -> Result<Value, String> {
    let id = identifier(&v["id"])?;
    let statement = v["statement"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() < 2000)
        .ok_or("Invalid hypothetical statement")?;
    let deps = v["requires"].as_array().ok_or("Missing prerequisites")?;
    if deps.len() > 12 {
        return Err("Too many generated prerequisites".into());
    }
    let mut edges = vec![];
    for dep in deps {
        let target = identifier(dep)?;
        if !known.contains(target) {
            return Err(format!(
                "Generated prerequisite {target} is not in frozen evidence"
            ));
        }
        edges.push(json!({"kind":"requires","to_id":target}));
    }
    Ok(
        json!({"Id":id,"statement":statement,"kind":kind,"Status":"Hypothesis","provenance":"generated_hypothesis","edges":serde_json::to_string(&edges).unwrap(),"source_refs":"[]","evidence_note":v["evidence_note"],"signal":v["signal"],"falsifier":v["falsifier"],"scene":v["scene"],"parent":v["parent"],"research_question":v["research_question"]}),
    )
}
fn resolve_challenge(snapshot: &Value, generated: &mut Value) -> Result<(), String> {
    references::References::new(snapshot)?.resolve_generated(generated);
    let premises = generated["premises_challenged"]
        .as_array()
        .ok_or("Missing challenged premises")?;
    if premises.len() > 32 {
        return Err("Too many challenged premises".into());
    }
    for (index, premise) in premises.iter().enumerate() {
        bounded_text(
            &premise["assumption"],
            600,
            &format!("premises_challenged[{index}].assumption"),
        )?;
        bounded_text(
            &premise["alternative"],
            1200,
            &format!("premises_challenged[{index}].alternative"),
        )?;
    }
    let evidence_ids: std::collections::BTreeSet<_> = generated["research_evidence"]
        .as_array()
        .ok_or("Missing challenge research evidence")?
        .iter()
        .filter_map(|report| report["id"].as_str())
        .collect();
    let hypotheses = generated["hypotheses"]
        .as_array()
        .ok_or("Missing challenge hypotheses")?;
    let new_ids: std::collections::BTreeSet<_> =
        hypotheses.iter().filter_map(|n| n["id"].as_str()).collect();
    let existing_ids: std::collections::BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| {
            matches!(
                core::field(n, "kind"),
                "evidence" | "research_evidence" | "scenario" | "revision"
            )
        })
        .filter_map(|n| n["Id"].as_str())
        .collect();
    let prior_ids: std::collections::BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| matches!(core::field(n, "kind"), "scenario" | "revision"))
        .filter_map(|n| n["Id"].as_str())
        .collect();
    for premise in premises {
        for (key, allowed) in [
            ("prior_hypothesis_ids", &prior_ids),
            ("alternative_hypothesis_ids", &new_ids),
        ] {
            let ids = premise[key]
                .as_array()
                .filter(|ids| !ids.is_empty() && ids.len() <= 128)
                .ok_or_else(|| {
                    format!("Challenge {key} must name a nonempty bounded hypothesis group")
                })?;
            let mut seen = std::collections::BTreeSet::new();
            for id in ids {
                let id = id
                    .as_str()
                    .ok_or("Invalid challenged hypothesis reference")?;
                if !allowed.contains(id) || !seen.insert(id) {
                    return Err(format!(
                        "Invalid or duplicate challenge {key} reference: {id}"
                    ));
                }
            }
        }
    }
    let linked_ids: std::collections::BTreeSet<_> = premises
        .iter()
        .flat_map(|p| p["alternative_hypothesis_ids"].as_array().unwrap())
        .filter_map(Value::as_str)
        .collect();
    if linked_ids != new_ids {
        return Err(
            "Every challenge hypothesis must belong to a challenged premise alternative group"
                .into(),
        );
    }
    for hypothesis in hypotheses {
        if let Some(parent) = hypothesis["parent"].as_str().filter(|p| !p.is_empty())
            && !new_ids.contains(parent)
        {
            return Err("Challenge parent must belong to its own batch".into());
        }
        for reference in hypothesis["requires"]
            .as_array()
            .ok_or("Missing challenge prerequisites")?
        {
            let reference = reference.as_str().ok_or("Invalid challenge reference")?;
            if !new_ids.contains(reference)
                && !existing_ids.contains(reference)
                && !evidence_ids.contains(reference)
            {
                return Err(format!(
                    "Challenge reference {reference} is outside its visible catalog and new hypotheses or evidence"
                ));
            }
        }
    }
    Ok(())
}

fn record_challenge(
    snapshot: &Value,
    before: usize,
    generated: &Value,
    old: &Value,
    program: &mut Value,
) -> Result<(), String> {
    let mut prior = snapshot.clone();
    prior["nodes"]
        .as_array_mut()
        .ok_or("Missing nodes")?
        .truncate(before);
    let mut generated = generated.clone();
    resolve_challenge(&prior, &mut generated)?;
    let round = program["round"].as_u64().ok_or("Missing challenge round")?;
    for premise in generated["premises_challenged"].as_array_mut().unwrap() {
        for id in premise["alternative_hypothesis_ids"]
            .as_array_mut()
            .unwrap()
        {
            *id = json!(format!("r{round}-{}", id.as_str().unwrap()));
        }
    }
    let generated_branch_ids: std::collections::BTreeSet<_> = generated["branches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|branch| format!("branch-r{round}-{}", core::field(branch, "id")))
        .collect();
    let actual_branches: Vec<_> = snapshot["branches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|branch| generated_branch_ids.contains(core::field(branch, "id")))
        .cloned()
        .collect();
    program["independent_challenge"] = json!({
        "status":"completed","trigger":old["independent_challenge"]["trigger"].as_str().unwrap_or("candidate_generation_reported_saturation"),
        "round":program["round"],"premises_challenged":generated["premises_challenged"],
        "added_hypothesis_ids":snapshot["nodes"].as_array().unwrap().iter().skip(before).filter(|n| matches!(core::field(n, "kind"), "scenario" | "revision")).map(|n|n["Id"].clone()).collect::<Vec<_>>(),
        "added_evidence_ids":snapshot["nodes"].as_array().unwrap().iter().skip(before).filter(|n| core::field(n, "kind") == "research_evidence").map(|n|n["Id"].clone()).collect::<Vec<_>>(),
        "note":generated["exploration_note"],"accuracy_verified":false,
        "branches":actual_branches
    });
    Ok(())
}

fn expand(
    snapshot: &mut Value,
    generated: &Value,
    phase: &str,
    program: &Value,
) -> Result<(), String> {
    if !matches!(phase, "seed" | "explore" | "challenge" | "backward") {
        return Err("Unknown exploration phase".into());
    }
    let mut generated = generated.clone();
    if phase == "challenge" {
        resolve_challenge(snapshot, &mut generated)?;
    }
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    if scope_pending(program)
        && (generated["hypotheses"]
            .as_array()
            .is_none_or(|v| !v.is_empty())
            || generated["branches"]
                .as_array()
                .is_some_and(|v| !v.is_empty()))
    {
        return Err("Scope repair is research-only: hypotheses and branches must be empty".into());
    }
    let hypotheses = generated["hypotheses"]
        .as_array()
        .ok_or("Missing hypotheses")?;
    let reports = generated["research_evidence"]
        .as_array()
        .ok_or("Missing research evidence")?;
    if hypotheses.len() + reports.len() > 128 {
        return Err("Exploration batch exceeds memory budget".into());
    }
    generated["continue_exploring"]
        .as_bool()
        .ok_or("Missing continuation decision")?;
    generated["exploration_note"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4000)
        .ok_or("Missing exploration rationale")?;
    if core::field(&snapshot["world"], "hindcast_mode") != "false" && !reports.is_empty() {
        return Err("Frozen world cannot add fresh research".into());
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    if nodes.len() + hypotheses.len() + reports.len() > core::MAX_NODES - 6 {
        return Err("Node budget exceeded".into());
    }
    let mut known: std::collections::BTreeSet<String> = nodes
        .iter()
        .map(|n| core::field(n, "Id").to_owned())
        .collect();
    let mut mapped = std::collections::BTreeMap::new();
    let round = program["round"].as_u64().unwrap_or(0) + 1;
    for v in reports.iter().chain(hypotheses) {
        let local = identifier(&v["id"])?;
        if local.starts_with(references::PREFIX) {
            return Err("Generated identity uses reserved reference namespace".into());
        }
        let id = if scope_pending(program) {
            format!("scope-{local}")
        } else {
            format!("r{round}-{local}")
        };
        if known.contains(local)
            || mapped.insert(local.to_owned(), id.clone()).is_some()
            || !known.insert(id)
        {
            return Err("Duplicate generated identity".into());
        }
    }
    let resolve = |id: &str| mapped.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let mut hypothesis_ids: std::collections::BTreeSet<String> = nodes
        .iter()
        .filter(|n| matches!(core::field(n, "kind"), "scenario" | "revision"))
        .map(|n| core::field(n, "Id").to_owned())
        .collect();
    for hypothesis in hypotheses {
        hypothesis_ids.insert(resolve(identifier(&hypothesis["id"])?));
    }
    let mut lineage: std::collections::BTreeMap<String, String> = nodes
        .iter()
        .filter_map(|n| {
            n["parent"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(|parent| (core::field(n, "Id").to_owned(), parent.to_owned()))
        })
        .collect();
    for hypothesis in hypotheses {
        if let Some(parent) = hypothesis["parent"].as_str().filter(|p| !p.is_empty()) {
            let parent = resolve(identifier(&json!(parent))?);
            if !hypothesis_ids.contains(&parent) {
                let kind = nodes
                    .iter()
                    .find(|n| core::field(n, "Id") == parent)
                    .map(|n| core::field(n, "kind"))
                    .unwrap_or("unknown");
                return Err(format!(
                    "Hypothesis {} has invalid parent {parent} (kind: {kind}). Parent must name an existing or same-batch scenario/revision for lineage; source evidence belongs in requires/support context, not parent.",
                    core::field(hypothesis, "id")
                ));
            }
            lineage.insert(resolve(identifier(&hypothesis["id"])?), parent);
        }
    }
    // Parent is lineage, not an implicit causal dependency. It may name a new
    // hypothesis in any batch order, but it must never create a lineage cycle.
    for hypothesis in hypotheses {
        let mut current = resolve(identifier(&hypothesis["id"])?);
        let mut visited = std::collections::BTreeSet::new();
        while let Some(parent) = lineage.get(&current) {
            if !visited.insert(current.clone()) {
                return Err("Cyclic hypothesis parent lineage".into());
            }
            current = parent.clone();
        }
    }
    let mut added = vec![];
    for report in reports {
        let statement = report["statement"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() < 2000)
            .ok_or("Invalid research report")?;
        let url = report["url"]
            .as_str()
            .filter(|s| {
                s.starts_with("https://") && s.len() <= 2000 && !s.chars().any(char::is_whitespace)
            })
            .ok_or("Research needs exact HTTPS source")?;
        let quote = report["quote"]
            .as_str()
            .filter(|s| {
                !s.trim().is_empty()
                    && s.chars().count() <= 200
                    && s.split_whitespace().count() <= 25
            })
            .ok_or("Research excerpt exceeds quotation limit")?;
        let metadata = if report["evidence_metadata"].is_null()
            && snapshot["world"]["evidence_contract"] != "v1"
        {
            core::evidence::legacy()
        } else {
            core::evidence::validate(&report["evidence_metadata"])?;
            report["evidence_metadata"].clone()
        };
        added.push(json!({"evidence_metadata":metadata,"Id":resolve(identifier(&report["id"])?),"statement":statement,"kind":"research_evidence","Status":"Reported","provenance":"session_research_report","claim_type":report["provenance"],"edges":"[]","source_refs":json!([url]).to_string(),"quote":quote,"observed_at":report["observed_at"],"evidence_note":"Retrieved report, not proof of a future event."}));
    }
    for hypothesis in hypotheses {
        let mut v = hypothesis.clone();
        v["id"] = json!(resolve(identifier(&v["id"])?));
        let requires = hypothesis["requires"]
            .as_array()
            .ok_or("Missing prerequisites")?
            .iter()
            .map(|id| identifier(id).map(|id| json!(resolve(id))))
            .collect::<Result<Vec<_>, _>>()?;
        v["requires"] = json!(requires);
        let parent = hypothesis["parent"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(resolve);
        v["parent"] = json!(parent);
        let mut node = generated_node(
            &v,
            if parent.is_some() {
                "revision"
            } else {
                "scenario"
            },
            &known,
        )?;
        // A source supports an event; the source's existence is not a future prerequisite.
        let evidence_ids: std::collections::BTreeSet<_> = nodes
            .iter()
            .chain(added.iter())
            .filter(|n| matches!(core::field(n, "kind"), "evidence" | "research_evidence"))
            .map(|n| core::field(n, "Id").to_owned())
            .collect();
        let mut edges = core::parse(core::field(&node, "edges"))?;
        for edge in edges.as_array_mut().ok_or("Invalid generated edges")? {
            if evidence_ids.contains(core::field(edge, "to_id")) {
                edge["kind"] = json!("supports");
            }
        }
        node["edges"] = json!(edges.to_string());
        if !hypothesis["branch_id"].is_null() && !hypothesis["branch_id"].is_string() {
            return Err("branch_id must be a string or null".into());
        }
        if let Some(branch_id) = hypothesis["branch_id"].as_str().filter(|id| !id.is_empty()) {
            let is_new = generated["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|b| core::field(b, "id") == branch_id);
            node["branch_id"] = json!(if is_new {
                format!("branch-r{}-{branch_id}", round)
            } else {
                branch_id.to_owned()
            });
        }
        node["title"] = v["title"].clone();
        node["mechanism"] = v["mechanism"].clone();
        if let Some(parent) = parent {
            node["before_gap"] = program["results"][&parent]["classify_gap"].clone();
        }
        node["research_status"] = json!(if reports.is_empty() {
            "no_new_sources"
        } else {
            "source_reports_available"
        });
        added.push(node);
    }
    // Commit nodes and branch records together only after complete validation.
    let mut updated = snapshot.clone();
    updated["nodes"]
        .as_array_mut()
        .ok_or("Missing nodes")?
        .extend(added);
    if let Some(branches) = generated.get("branches") {
        let branches = branches.as_array().ok_or("branches must be an array")?;
        let locals: std::collections::BTreeSet<_> = branches
            .iter()
            .map(|b| identifier(&b["id"]))
            .collect::<Result<_, _>>()?;
        if locals.len() != branches.len() {
            return Err("Duplicate branch identity".into());
        }
        let branch_id = |id: &str| {
            if locals.contains(id) {
                format!("branch-r{round}-{id}")
            } else {
                id.to_owned()
            }
        };
        if updated["branches"].is_null() {
            updated["branches"] = json!([]);
        }
        for branch in branches {
            if !branch["parent_branch_id"].is_null() && !branch["parent_branch_id"].is_string() {
                return Err("parent_branch_id must be a string or null".into());
            }
            let mut record = branch.clone();
            record["id"] = json!(branch_id(identifier(&branch["id"])?));
            record["parent_branch_id"] = match branch["parent_branch_id"]
                .as_str()
                .filter(|id| !id.is_empty())
            {
                Some(id) => json!(branch_id(id)),
                None => Value::Null,
            };
            record["condition"]["event_ids"] = json!(
                branch["condition"]["event_ids"]
                    .as_array()
                    .ok_or("Missing branch premise IDs")?
                    .iter()
                    .map(|id| identifier(id).map(&resolve))
                    .collect::<Result<Vec<_>, _>>()?
            );
            updated["branches"]
                .as_array_mut()
                .ok_or("Invalid persisted branches")?
                .push(record);
        }
    }
    core::branches::validate(&updated)?;
    let states: Vec<_> = updated["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            if core::field(n, "branch_id").is_empty() {
                Ok(Value::Null)
            } else {
                core::branches::state(
                    &updated,
                    core::field(n, "branch_id"),
                    Some(core::field(n, "Id")),
                )
            }
        })
        .collect::<Result<_, String>>()?;
    for (node, state) in updated["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(states)
    {
        if !state.is_null() {
            node["branch_state"] = state;
        }
    }
    *snapshot = updated;
    Ok(())
}

fn validate_scope(review: &Value, snapshot: &Value) -> Result<(), String> {
    if review["requested_question"] != snapshot["world"]["description"] {
        return Err("Scope requested_question must equal the original question exactly".into());
    }
    bounded_text(
        &review["evidence_scope"],
        scope::SCOPE_TEXT_MAX,
        "scope_review.evidence_scope",
    )?;
    bounded_texts(
        &review["limitations"],
        0,
        scope::LIMITATIONS_MAX,
        scope::LIMITATION_TEXT_MAX,
        "scope_review.limitations",
    )?;
    scope::validate_review(review)?;
    if review["status"] != "aligned" && review["limitations"].as_array().unwrap().is_empty() {
        return Err("Limited or uncertain scope needs explicit limitations".into());
    }
    Ok(())
}

fn retain_scope_limits(baseline: &Value, review: &Value) -> Result<(), String> {
    let unknowns = baseline["unknowns"]
        .as_array()
        .ok_or("Missing baseline unknowns")?;
    if review["status"] != "aligned"
        && review["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|limit| !unknowns.contains(limit))
    {
        return Err("Copy each unresolved scope limitation exactly into baseline.unknowns".into());
    }
    Ok(())
}

fn scope_pending(program: &Value) -> bool {
    program["scope_repair"]["status"] == "pending"
}

fn failed_scope_repair(old: &Value, error: &str) -> Value {
    let mut program = old.clone();
    program["scope_repair"] = json!({"status":"failed","attempted":true,"coverage_certified":false,"disposition":"limited","original_baseline":old["baseline"],"original_review":old["scope_review"],"report":format!("Scope repair response not applied: {error}")});
    program["response_correction"] = Value::Null;
    program["continue_exploring"] = json!(true);
    program
}

fn finish_scope_repair(snapshot: &Value, generated: &Value, old: &Value) -> Result<Value, String> {
    let mut generated = generated.clone();
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    let locals: std::collections::BTreeSet<String> = generated["research_evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect();
    for claim in generated["baseline"]["observed"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for id in claim["evidence_ids"].as_array_mut().into_iter().flatten() {
            if let Some(local) = id.as_str().filter(|v| locals.contains(*v)) {
                *id = json!(format!("scope-{local}"));
            }
        }
    }
    for id in generated["scope_disposition"]["evidence_ids"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if let Some(local) = id.as_str().filter(|v| locals.contains(*v)) {
            *id = json!(format!("scope-{local}"));
        }
    }
    if let Some(dispositions) = generated
        .get_mut("baseline_dispositions")
        .and_then(Value::as_array_mut)
    {
        for disposition in dispositions {
            for id in disposition["evidence_ids"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                if let Some(local) = id.as_str().filter(|v| locals.contains(*v)) {
                    *id = json!(format!("scope-{local}"));
                }
            }
        }
    }
    validate_scope(&generated["scope_review"], snapshot)?;
    outlook::validate_new_baseline(&generated["baseline"], snapshot)?;
    validate_baseline_dispositions(old, &generated, snapshot)?;
    retain_scope_limits(&generated["baseline"], &generated["scope_review"])?;
    let report = &generated["scope_disposition"];
    bounded_text(
        &report["report"],
        scope::REPORT_MAX,
        "scope_disposition.report",
    )?;
    scope::validate_disposition(report)?;
    let refs = report["evidence_ids"]
        .as_array()
        .ok_or("Missing scope disposition evidence_ids")?;
    if refs.len() > scope::REPORT_REFS_MAX
        || refs.iter().any(|id| {
            !snapshot["nodes"].as_array().unwrap().iter().any(|n| {
                n["Id"] == *id && matches!(core::field(n, "kind"), "evidence" | "research_evidence")
            })
        })
    {
        return Err("Scope disposition references unknown evidence".into());
    }
    if report["status"] == "addressed"
        && !refs.iter().any(|id| {
            snapshot["nodes"].as_array().unwrap().iter().any(|n| {
                n["Id"] == *id
                    && n["evidence_metadata"]["kind"] == "finding"
                    && core::evidence::validate(&n["evidence_metadata"]).is_ok()
                    && core::evidence::within_vantage(
                        &n["evidence_metadata"],
                        core::field(&snapshot["world"], "last_ingest_date"),
                    )
                    .is_ok()
            })
        })
    {
        return Err("Addressed scope needs a current typed finding; lead-only or legacy sources remain limited".into());
    }
    Ok(
        json!({"status":"completed","attempted":true,"disposition":report["status"],"report":report["report"],"evidence_ids":refs,"coverage_certified":false,"original_baseline":old["baseline"],"original_review":old["scope_review"],"baseline":generated["baseline"],"review":generated["scope_review"],"dispositions":generated["baseline_dispositions"],"dispositions_verified":false}),
    )
}

// Reconcile newly read findings in the same atomic proposal, not a later model phase.
// A replacement may consolidate or retract claims, but never silently forget them.
fn validate_baseline_dispositions(
    old: &Value,
    reply: &Value,
    snapshot: &Value,
) -> Result<(), String> {
    let prior = old["baseline"]["observed"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let current = reply["baseline"]["observed"]
        .as_array()
        .ok_or("Missing baseline observations")?;
    let dispositions = match reply.get("baseline_dispositions") {
        None => vec![],
        Some(value) => value
            .as_array()
            .ok_or("baseline_dispositions must be an array")?
            .clone(),
    };
    if dispositions.len() > prior.len() {
        return Err("baseline_dispositions exceeds prior observation count".into());
    }
    let mut accounted = std::collections::BTreeSet::new();
    for disposition in &dispositions {
        let index = disposition["prior_observation_index"]
            .as_u64()
            .ok_or("Invalid prior_observation_index")?;
        let index = usize::try_from(index).map_err(|_| "Invalid prior_observation_index")?;
        if index >= prior.len() || !accounted.insert(index) {
            return Err("Unknown or duplicate prior_observation_index".into());
        }
        bounded_text(&disposition["reason"], 400, "baseline_dispositions.reason")?;
        let replacements = disposition["replacement_observation_indices"]
            .as_array()
            .ok_or("Missing replacement_observation_indices")?;
        if replacements.len() > 16
            || replacements
                .iter()
                .any(|v| v.as_u64().is_none_or(|i| i >= current.len() as u64))
        {
            return Err("Invalid replacement_observation_indices".into());
        }
        // Reuse source-kind, chronology and reference validation. This validates
        // provenance, not the truth of the model's revision/retraction judgment.
        let citation = json!({"as_of":reply["baseline"]["as_of"],"observed":[{"claim":disposition["reason"],"evidence_ids":disposition["evidence_ids"]}],"assumptions":[],"unknowns":[]});
        outlook::validate_new_baseline(&citation, snapshot)?;
    }
    for (index, observation) in prior.iter().enumerate() {
        // Adding citations to an unchanged claim is not a retraction.
        if !current.iter().any(|v| v["claim"] == observation["claim"])
            && !accounted.contains(&index)
        {
            return Err(format!(
                "Refreshed baseline omitted prior observation {index}; retain its claim or provide baseline_dispositions with explicit replacement indices (empty for retraction), reason and finding evidence_ids"
            ));
        }
    }
    Ok(())
}

fn refresh_researched_baseline(
    before: &Value,
    after: &Value,
    generated: &Value,
    old: &Value,
) -> Result<Option<Value>, String> {
    let reports = generated["research_evidence"]
        .as_array()
        .ok_or("Missing research evidence")?;
    if !reports
        .iter()
        .any(|r| r["evidence_metadata"]["kind"] == "finding")
    {
        return Ok(None);
    }
    if !generated["baseline"].is_object() || !generated["scope_review"].is_object() {
        return Err("New research findings require reconciled baseline and current scope_review using the supplied contracts".into());
    }
    let mut reply = generated.clone();
    references::References::new(before)?.resolve_generated(&mut reply);
    let round = old["round"].as_u64().unwrap_or(0) + 1;
    let locals: std::collections::BTreeSet<_> =
        reports.iter().filter_map(|r| r["id"].as_str()).collect();
    for claim in reply["baseline"]["observed"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for id in claim["evidence_ids"].as_array_mut().into_iter().flatten() {
            if let Some(local) = id.as_str().filter(|id| locals.contains(id)) {
                *id = json!(format!("r{round}-{local}"));
            }
        }
    }
    if let Some(dispositions) = reply
        .get_mut("baseline_dispositions")
        .and_then(Value::as_array_mut)
    {
        for disposition in dispositions {
            for id in disposition["evidence_ids"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                if let Some(local) = id.as_str().filter(|id| locals.contains(id)) {
                    *id = json!(format!("r{round}-{local}"));
                }
            }
        }
    }
    outlook::validate_new_baseline(&reply["baseline"], after)?;
    validate_baseline_dispositions(old, &reply, after)?;
    validate_scope(&reply["scope_review"], after)?;
    retain_scope_limits(&reply["baseline"], &reply["scope_review"])?;
    Ok(Some(
        json!({"round":round,"prior_baseline":old["baseline"],"prior_scope_review":old["scope_review"],"baseline":reply["baseline"],"scope_review":reply["scope_review"],"added_finding_ids":reports.iter().filter(|r|r["evidence_metadata"]["kind"]=="finding").map(|r|json!(format!("r{round}-{}",r["id"].as_str().unwrap()))).collect::<Vec<_>>(),"coverage_certified":false,"dispositions":reply["baseline_dispositions"],"dispositions_verified":false}),
    ))
}

fn establish_baseline(snapshot: &Value, generated: &Value, old: &Value) -> Result<Value, String> {
    let mut generated = generated.clone();
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    outlook::validate_new_baseline(&generated["baseline"], snapshot)?;
    let mut program = old.clone();
    program["baseline"] = generated["baseline"].clone();
    if !generated["scope_review"].is_null() || snapshot["world"]["evidence_contract"] == "v1" {
        validate_scope(&generated["scope_review"], snapshot)?;
        retain_scope_limits(&generated["baseline"], &generated["scope_review"])?;
        program["scope_review"] = generated["scope_review"].clone();
        let pending = generated["scope_review"]["narrowing_basis"] == "evidence_availability"
            && generated["scope_review"]["status"] != "aligned";
        program["scope_repair"] = json!({"status":if pending {"pending"} else {"not_requested"},"attempted":false,"coverage_certified":false});
    }

    program["baseline_status"] = json!("established");
    program["baseline_correction"] = Value::Null;
    program["continue_exploring"] = json!(true);
    Ok(program)
}
fn baseline_correction(old: &Value, error: &str) -> Result<Value, String> {
    let attempt = old["baseline_correction"]["attempt"].as_u64().unwrap_or(0) + 1;
    if attempt > 2 {
        return Err(format!(
            "Baseline rejected after two corrective attempts: {error}"
        ));
    }
    let mut program = old.clone();
    program["baseline_correction"] = json!({"attempt":attempt,"validation_error":error});
    Ok(program)
}

fn compose(snapshot: &mut Value, generated: &Value, old: &Value) -> Result<Value, String> {
    let mut generated = generated.clone();
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    core::endpoints::validate_composition(old, &generated)?;
    let baseline = if old["baseline"].is_object() {
        &old["baseline"]
    } else {
        &generated["baseline"]
    };
    if old["baseline"].is_object() {
        outlook::validate_baseline(baseline, snapshot)?;
    } else {
        outlook::validate_new_baseline(baseline, snapshot)?;
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let by_id: std::collections::BTreeMap<_, _> =
        nodes.iter().map(|n| (core::field(n, "Id"), n)).collect();
    bounded_text(&generated["shared_question"], 800, "shared_question")
        .map_err(|error| format!("Invalid shared_question: {error}"))?;
    let worlds = generated["worlds"]
        .as_array()
        .filter(|w| (2..=6).contains(&w.len()))
        .ok_or("Compose two to six worlds")?;
    if nodes.len() + worlds.len() > core::MAX_NODES {
        return Err("World composition exceeds node budget".into());
    }
    let revision = old["world_revision"].as_u64().unwrap_or(0).max(
        nodes
            .iter()
            .filter(|node| node["kind"] == "world")
            .filter_map(|node| node["revision"].as_u64())
            .max()
            .unwrap_or(0),
    ) + 1;
    let mut added = vec![];
    let mut identities = std::collections::BTreeSet::new();
    for (index, world) in worlds.iter().enumerate() {
        let local = identifier(&world["id"])?;
        if local.starts_with(references::PREFIX) {
            return Err("Reserved world identity".into());
        }
        let id = format!("world-r{revision}-{local}");
        if by_id.contains_key(id.as_str()) || !identities.insert(id.clone()) {
            return Err("Duplicate world identity".into());
        }
        for (key, max) in [
            ("title", 100),
            ("statement", 1000),
            ("mechanism", 1200),
            ("trajectory_answer", 1000),
            ("scene", 600),
            ("narrative", 1200),
        ] {
            bounded_text(&world[key], max, &format!("worlds[{index}].{key}"))
                .map_err(|error| format!("World {local}: invalid {key}: {error}"))?;
        }
        for key in ["signals", "falsifiers"] {
            bounded_texts(&world[key], 1, 8, 240, &format!("worlds[{index}].{key}"))?;
        }
        bounded_texts(
            &world["what_you_can_do"],
            0,
            4,
            240,
            &format!("worlds[{index}].what_you_can_do"),
        )?;
        let mut components = std::collections::BTreeSet::new();
        for reference in world["component_ids"]
            .as_array()
            .ok_or("Missing world components")?
        {
            let reference = reference.as_str().ok_or("Invalid world component")?;
            if by_id
                .get(reference)
                .is_none_or(|n| !matches!(core::field(n, "kind"), "scenario" | "revision"))
            {
                return Err(format!(
                    "World component {reference} is not an existing scenario or revision"
                ));
            }
            if !core::branches::future_eligible(snapshot, old, reference) {
                let classification = core::forecast_exclusion_reason(old, reference);
                return Err(format!(
                    "World component {reference} is ineligible for forecasting: {classification}. Select only composition_candidates.component_ids; keep this node as context."
                ));
            }
            if !components.insert(reference) {
                return Err(format!("Duplicate world component {reference}"));
            }
        }
        let component_limit = if core::endpoints::enabled(old) {
            32
        } else {
            12
        };
        if components.len() < 3 || components.len() > component_limit {
            return Err(format!(
                "World needs three to {component_limit} defining components"
            ));
        }
        let counters = world["counter_ids"]
            .as_array()
            .ok_or("Missing world counter hypotheses")?;
        if counters.len() > 12 {
            return Err(format!("World {local}: counter_ids exceeds twelve entries"));
        }
        let mut counter_ids = std::collections::BTreeSet::new();
        for value in counters {
            let reference = value.as_str().ok_or("Counter reference must be a string")?;
            let reason = if components.contains(reference) {
                Some("is also a defining component")
            } else if !counter_ids.insert(reference) {
                Some("is repeated")
            } else {
                match by_id.get(reference) {
                    None => Some("does not exist in the catalog"),
                    Some(node) if !matches!(core::field(node, "kind"), "scenario" | "revision") => {
                        Some(
                            "is evidence, not a future hypothesis; evidence belongs in the baseline or source context",
                        )
                    }
                    Some(_) if !core::claim_role_allows_forecast(old, reference) => Some(
                        "is not an admitted event proposition; retain research commentary in context",
                    ),
                    _ => None,
                }
            };
            if let Some(reason) = reason {
                let alias = references::References::new(snapshot)?
                    .project(&json!({"counter_ids":[reference]}));
                let alias = alias["counter_ids"][0].as_str().unwrap_or(reference);
                return Err(format!("World {local}: counter reference {alias} {reason}"));
            }
        }
        let mut node = world.clone();
        node.as_object_mut().ok_or("Invalid world")?.remove("id");
        node["Id"] = json!(id);
        node["kind"] = json!("world");
        node["shared_question"] = generated["shared_question"].clone();
        core::comparison::validate_proposal(
            &generated["comparison_frame"],
            &node["trajectory_binding"],
        )?;
        node["comparison_contract"] = json!("v1");
        node["comparison_frame"] =
            core::comparison::frame(snapshot, &generated["comparison_frame"]);
        if let Some(counterpart) = node["trajectory_binding"]["counterpart_world_id"].as_str()
            && worlds.iter().any(|w| w["id"] == counterpart)
        {
            node["trajectory_binding"]["counterpart_world_id"] =
                json!(format!("world-r{revision}-{counterpart}"));
        }
        node["branch_conditions"] =
            core::branches::world_conditions(snapshot, &node["component_ids"])?;
        for clause in node["branch_conditions"].as_array().unwrap() {
            for event in clause["events"].as_array().unwrap() {
                if !core::forecast_allows(old, core::field(event, "id")) {
                    return Err(format!(
                        "Branch premise {} is not a currently evaluated future event; separate observed conditions from hypothetical future changes",
                        core::field(event, "id")
                    ));
                }
            }
        }
        node["revision"] = json!(revision);
        node["archived"] = json!(false);
        node["Status"] = json!("Hypothesis");
        node["provenance"] = json!("composed_world_hypothesis");
        node["edges"] = json!(
            components
                .iter()
                .map(|id| json!({"kind":"requires","to_id":id}))
                .collect::<Vec<_>>()
                .pipe_json()
        );
        added.push(node);
    }
    let mut updated = snapshot.clone();
    for node in updated["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|n| n["kind"] == "world")
    {
        node["archived"] = json!(true);
    }
    updated["nodes"].as_array_mut().unwrap().extend(added);
    let active: Vec<_> = updated["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| identities.contains(core::field(n, "Id")))
        .collect();
    let errors: Vec<_> = active
        .iter()
        .filter_map(|node| core::search::validate_world(node, &updated).err())
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let mut tasks: Vec<_> = active
        .iter()
        .flat_map(|n| core::search::world_tasks(n))
        .collect();
    tasks.splice(
        0..0,
        core::search::world_set_tasks(&identities.iter().map(|s| json!(s)).collect::<Vec<_>>()),
    );
    let mut program = core::plan(updated["nodes"].as_array().unwrap())?;
    // Composition evaluates whole worlds, not candidate admission. Preserve
    // whether the source exploration actually ran the claim-role contract.
    if let Some(contract) = old
        .get("claim_role_contract")
        .filter(|value| !value.is_null())
    {
        program["claim_role_contract"] = contract.clone();
    } else {
        program
            .as_object_mut()
            .unwrap()
            .remove("claim_role_contract");
    }
    for key in [
        "endpoint_proposal_contract",
        "endpoint_proposal_attempt",
        "endpoint_proposal_history",
        "world_search_contract",
        "endpoint_search",
        "candidate_basis",
        "novelty_basis",
        "route_basis",
        "results",
        "evaluations",
        "round",
        "rounds",
        "baseline_status",
        "baseline_history",
        "scope_review",
        "scope_repair",
        "temporal_decomposition_requested",
        "last_error",
        "combination_search",
        "world_audits",
        "world_set_audits",
        "world_set_audit",
        "world_refinement",
        "independent_challenge",
        "historical_search_guidance",
        "exploration_admission",
        "batch_byte_cap",
        "http_calls",
        "transition_count",
        "evidence_ids",
    ] {
        if !old[key].is_null() {
            program[key] = old[key].clone();
        }
    }
    program["comparison_bindings"] = json!({});
    let active_values: Vec<_> = active.iter().map(|w| (*w).clone()).collect();
    for world in &active_values {
        program["comparison_bindings"][core::field(world, "Id")] =
            core::comparison::audit(&updated, world, &active_values);
    }
    program["tasks"] = json!(tasks);
    program["active_world_ids"] = json!(identities);
    if core::endpoints::enabled(old) {
        program["unreconstructed_endpoints"] = generated["unreconstructed_endpoints"]
            .as_array()
            .cloned()
            .map_or(json!([]), |v| json!(v));
    }
    program["world_revision"] = json!(revision);
    program["world_set_audit"] = Value::Null;
    program["world_pass"] = json!(1);
    program["evidence_ids"] = json!(evidence_ids(&updated));
    program["stage"] = json!("worlds");
    program["baseline"] = baseline.clone();
    program["continue_exploring"] = json!(false);
    program["exploration_stop_reason"] = old["stop_reason"].clone();
    // A provider/trace failure cannot be repaired by asking again within this run.
    if matches!(
        old["stop_reason"].as_str(),
        Some("provider_error" | "trace_budget")
    ) {
        program["stop_reason"] = old["stop_reason"].clone();
    }
    let mut first_pass = program.clone();
    // The shared estimator plans the next pass; zero requests the exact initial
    // pass context rather than adding prior-world feedback that does not exist.
    first_pass["world_pass"] = json!(0);
    let admission = core::search::refinement_admission(
        &updated,
        &first_pass,
        program["tasks"].as_array().unwrap(),
    );
    if admission["admitted"] != true {
        return Err(format!(
            "Proposed world set cannot fit its complete first audit pass: {}. Return a complete 2–6 world composition whose full audit plan fits the remaining capacity; preserve defining claims and do not omit required audits. A corrective reasoning phase may consume up to {} additional transitions, and the corrected proposal will be measured again against the then-current budget.",
            admission,
            core::REASONING_TRANSITION_RESERVE,
        ));
    }
    program["first_world_pass_admission"] = admission;
    *snapshot = updated;
    Ok(program)
}
trait JsonEncode {
    fn pipe_json(self) -> String;
}
impl JsonEncode for Vec<Value> {
    fn pipe_json(self) -> String {
        serde_json::to_string(&self).unwrap()
    }
}
fn bounded_text(value: &Value, max: usize, path: &str) -> Result<(), String> {
    let text = value.as_str().ok_or_else(|| {
        format!(
            "{path} must be a string containing 1–{max} characters; received a non-string value"
        )
    })?;
    let actual = text.chars().count();
    if text.trim().is_empty() || actual > max {
        return Err(format!(
            "{path} must contain nonblank text of 1–{max} characters; received {actual} characters (maximum {max})"
        ));
    }
    Ok(())
}
fn bounded_texts(
    value: &Value,
    min: usize,
    max: usize,
    chars: usize,
    path: &str,
) -> Result<(), String> {
    let values = value.as_array().ok_or_else(|| {
        format!("{path} must be an array containing {min}–{max} items; received a non-array value")
    })?;
    if !(min..=max).contains(&values.len()) {
        return Err(format!(
            "{path} must contain {min}–{max} items; received {} items (maximum {max})",
            values.len()
        ));
    }
    for (index, value) in values.iter().enumerate() {
        bounded_text(value, chars, &format!("{path}[{index}]"))?;
    }
    Ok(())
}
fn attach_world_probabilities(
    answer: &mut Value,
    program: &Value,
    snapshot: &Value,
) -> Result<(), String> {
    if answer["schema"] != "foresight-worlds-v3" {
        return Err("World runs require world outlook v3".into());
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let mut evaluated = 0;
    let mut outstanding_audits = 0;
    let outcomes = answer["outcomes"].as_array_mut().ok_or("Missing worlds")?;
    let count = outcomes.len();
    for outcome in outcomes {
        let id = outcome["world_id"]
            .as_str()
            .ok_or("Missing world identity")?
            .to_owned();
        let node = nodes
            .iter()
            .find(|n| core::field(n, "Id") == id && n["kind"] == "world")
            .ok_or("Outcome must reference a composed world")?;
        outcome["component_temporal"] = core::component_temporal(snapshot, program, node)?;
        let raw = &program["results"][&id]["estimate_likelihood"];
        let probability = if raw.is_null() {
            None
        } else {
            Some(
                raw.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                    .ok_or("Invalid world likelihood")?,
            )
        };
        if probability.is_some() {
            evaluated += 1;
        }
        outcome["probability"] = json!(probability);
        if let Some(active) = program["active_world_ids"].as_array()
            && !active.iter().any(|v| v.as_str() == Some(id.as_str()))
        {
            return Err("Outcome references inactive world".into());
        }
        for key in [
            "component_ids",
            "counter_ids",
            "branch_conditions",
            "facets",
            "chain",
            "assumptions",
        ] {
            outcome[key] = node[key].clone();
        }
        for key in [
            "comparison_contract",
            "comparison_frame",
            "trajectory_binding",
        ] {
            if let Some(value) = node.get(key) {
                outcome[key] = value.clone();
            }
        }
        if let Some(binding_audit) = program["comparison_bindings"].get(&id) {
            outcome["comparison_binding_audit"] = binding_audit.clone();
        }
        if core::endpoints::enabled(program) {
            for key in ["endpoint_id", "selected_route_ids", "commitment_bindings"] {
                outcome[key] = node[key].clone();
            }
            outcome["original_endpoint"] = program["endpoint_search"]["endpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|e| e["id"] == node["endpoint_id"])
                .cloned()
                .unwrap_or(Value::Null);
        }
        // Context references are persisted world inputs, not writer-generated identities.
        let mut context_ids = std::collections::BTreeSet::new();
        let refinement = &program["world_refinement"][&id];
        let evidence = refinement["rounds"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|round| round["evidence_ids"].as_array().into_iter().flatten());
        for reference in node["component_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(node["counter_ids"].as_array().into_iter().flatten())
            .chain(evidence)
        {
            let reference = reference.as_str().ok_or("Invalid world context identity")?;
            if !nodes.iter().any(|n| core::field(n, "Id") == reference) {
                return Err("Unknown canonical world context reference".into());
            }
            context_ids.insert(reference);
        }
        outcome["scenario_ids"] = json!(context_ids);
        outcome["definition"] = node["statement"].clone();
        for key in ["shared_question", "trajectory_answer"] {
            if !node[key].is_null() {
                outcome[key] = node[key].clone();
            } else if let Some(object) = outcome.as_object_mut() {
                object.remove(key);
            }
        }
        outcome["audit"] = core::search::audit_world(node, program);
        if !program["world_refinement"][&id].is_null() {
            outcome["refinement"] = program["world_refinement"][&id].clone();
        } else if let Some(object) = outcome.as_object_mut() {
            object.remove("refinement");
        }
        if outcome["audit"]["status"] != "no_conflict_found" {
            outstanding_audits += 1;
        }
    }
    if core::endpoints::enabled(program) {
        let mut omitted = vec![];
        for original in program["endpoint_search"]["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if answer["outcomes"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|outcome| outcome["endpoint_id"] == original["id"])
            {
                continue;
            }
            let receipt = program["unreconstructed_endpoints"].as_array().into_iter().flatten()
                .find(|receipt|receipt["endpoint_id"]==original["id"])
                .ok_or("Final answer omitted an original endpoint without a recorded reconstruction limit")?;
            omitted.push(json!({"endpoint_id":original["id"],"reason":receipt["reason"],"original_endpoint":original}));
        }
        answer["unreconstructed_endpoints"] = json!(omitted);
    }
    let probability_warnings: Vec<String> = answer["outcomes"].as_array().into_iter().flatten()
        .flat_map(|o| o["audit"]["probability_coherence"]["findings"].as_array().into_iter().flatten())
        .map(|f| format!("Independent estimates conflict: whole world {:.1}% exceeds a required event at {:.1}%; raw estimates are unchanged, not calibrated.", f["joint_probability"].as_f64().unwrap()*100.0, f["component_probability"].as_f64().unwrap()*100.0)).collect();
    answer["world_set_audit"] = if program["world_set_audit"].is_object() {
        program["world_set_audit"].clone()
    } else {
        json!({"task_id":core::search::world_set_task(program["active_world_ids"].as_array().unwrap_or(&vec![]))["nodeId"],"revision":program["world_revision"],"world_ids":program["active_world_ids"],"verdict":"uncertain","evaluation":null,"correction_status":"unavailable"})
    };
    answer["baseline"] = program["baseline"].clone();
    answer["scope_review"] = program["scope_review"].clone();
    answer["scope_repair"] = program["scope_repair"].clone();
    answer["evaluation_status"] = json!(if evaluated == count && count > 0 {
        "evaluated"
    } else if evaluated > 0 {
        "partial"
    } else {
        "unavailable"
    });
    answer["probability_basis"] = json!("model_implied_world_estimate");
    answer["probability_model"] = json!("overlapping_worlds");
    answer["calibrated"] = json!(false);
    answer["evaluation_note"] = json!(if evaluated == count && count > 0 {
        format!("All {count} worlds were evaluated separately by Jev.")
    } else {
        let reason = match core::field(program, "stop_reason") {
            "provider_error" => format!(
                "Jev could not finish: {}",
                core::field(program, "last_error")
            ),
            "time_budget" => "The available evaluation time ended.".to_owned(),
            "transition_budget" => {
                "The native work budget was reserved for completing this answer.".to_owned()
            }
            "call_budget" => "The available evaluation calls were used.".to_owned(),
            "trace_budget" => "The evaluation record reached its size limit.".to_owned(),
            _ => "The remaining worlds have no whole-world probability estimate.".to_owned(),
        };
        format!("{evaluated} of {count} worlds were evaluated separately by Jev. {reason}")
    });
    let set_note = match answer["world_set_audit"]["verdict"].as_str() {
        Some("complementary_slices") if answer["world_set_audit"]["mode"] == "per_world" => {
            " At least one world was judged a complementary slice or duplicate rather than an alternative trajectory for the same situation; consult the individual findings."
        }
        Some("complementary_slices") => {
            " These are complementary views of a shared direction; distinct alternative answers remain unresolved."
        }
        Some("alternative_answers") => {
            " Jev judged these meaningfully different answers; that judgment does not establish their truth."
        }
        _ => {
            " Whether this set offers meaningfully different answers remains uncertain or unverified."
        }
    };
    answer["evaluation_note"] = json!(format!(
        "{}{set_note}",
        answer["evaluation_note"].as_str().unwrap_or("")
    ));
    if outstanding_audits > 0 {
        let note = answer["evaluation_note"].as_str().unwrap_or("");
        answer["evaluation_note"] = json!(format!(
            "{note} {outstanding_audits} world audits still have conflicts, uncertainty or unfinished checks; likelihood estimates do not establish consistency."
        ));
    }
    if !probability_warnings.is_empty() {
        let note = answer["evaluation_note"].as_str().unwrap_or("");
        answer["evaluation_note"] = json!(format!("{note} {}", probability_warnings[0]));
        let warning = "Some whole-world estimates exceed required-event estimates under identical recorded context. These independent judgments are inconsistent; raw odds are retained, not calibrated.";
        let limits = answer["evidence_limits"]
            .as_array_mut()
            .ok_or("Missing evidence limits")?;
        if limits.len() < 32 {
            limits.push(json!(warning));
        } // At capacity the mandatory evaluation_note still exposes the warning.
    }
    Ok(())
}

fn attach_probabilities(answer: &mut Value, program: &Value) -> Result<(), String> {
    if answer["schema"] != "foresight-outlook-v2" {
        return Err("New runs require open outlook v2".into());
    }
    for outcome in answer["outcomes"]
        .as_array_mut()
        .ok_or("Missing outcomes")?
    {
        let id = outcome["hypothesis_id"]
            .as_str()
            .ok_or("Missing evaluated hypothesis")?;
        let probability = program["results"][&id]["estimate_likelihood"]
            .as_str()
            .ok_or("Hypothesis has no Jev event estimate")?
            .parse::<f64>()
            .map_err(|_| "Invalid event estimate")?;
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err("Invalid event probability".into());
        }
        outcome["probability"] = json!(probability);
    }
    Ok(())
}

fn evidence_ids(snapshot: &Value) -> std::collections::BTreeSet<String> {
    snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| matches!(core::field(n, "kind"), "evidence" | "research_evidence"))
        .map(|n| core::field(n, "Id").to_owned())
        .collect()
}

fn replan(snapshot: &Value, old: &Value, generated: &Value, added: usize) -> Result<Value, String> {
    let mut program = core::plan(snapshot["nodes"].as_array().ok_or("Missing nodes")?)?;
    for key in [
        "endpoint_proposal_contract",
        "endpoint_proposal_attempt",
        "endpoint_proposal_history",
        "world_search_contract",
        "endpoint_search",
        "candidate_basis",
        "novelty_basis",
        "route_basis",
        "results",
        "evaluations",
        "baseline",
        "baseline_status",
        "baseline_history",
        "scope_review",
        "scope_repair",
        "temporal_decomposition_requested",
        "rounds",
        "http_calls",
        "transition_count",
        "independent_challenge",
        "historical_search_guidance",
        "exploration_admission",
        "batch_byte_cap",
        "world_revision",
        "world_refinement",
        "world_audits",
        "world_set_audits",
        "world_set_audit",
        "active_world_ids",
        "resume_mode",
    ] {
        if !old[key].is_null() {
            program[key] = old[key].clone();
        }
    }
    // Older recovered programs dropped this counter; immutable nodes retain
    // the authoritative revision, so subsequent composition cannot reuse IDs.
    let stored_revision = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "world")
        .filter_map(|node| node["revision"].as_u64())
        .max()
        .unwrap_or(0);
    program["world_revision"] = json!(
        old["world_revision"]
            .as_u64()
            .unwrap_or(0)
            .max(stored_revision)
    );
    program["round"] = json!(old["round"].as_u64().unwrap_or(0) + 1);
    program["continue_exploring"] =
        json!(generated["continue_exploring"].as_bool().unwrap_or(false));
    program["exploration_note"] = generated["exploration_note"].clone();
    let receipt = json!({"round":program["round"],"added_nodes":added,"note":generated["exploration_note"],"continue_exploring":program["continue_exploring"]});
    program["rounds"]
        .as_array_mut()
        .ok_or("Invalid round history")?
        .push(receipt);
    if !core::endpoints::enabled(old) {
        core::defer_recorded_rankings(&mut program, old);
    }
    let current_evidence = evidence_ids(snapshot);
    let previous_evidence: std::collections::BTreeSet<_> = old["evidence_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if !current_evidence.is_subset(&previous_evidence) {
        // Receipts remain in the immutable trace. Current estimates are invalid
        // until the same claim is assessed against the new evidence context.
        for node in snapshot["nodes"]
            .as_array()
            .ok_or("Missing nodes")?
            .iter()
            .filter(|n| matches!(core::field(n, "kind"), "scenario" | "revision"))
        {
            for collection in ["results", "evaluations"] {
                if let Some(values) = program[collection][core::field(node, "Id")].as_object_mut() {
                    values.remove("classify_claim_role");
                    values.remove("classify_temporal");
                    values.remove("evaluate_novelty");
                    values.remove("decision_value");
                    values.remove("classify_gap");
                    values.remove("estimate_likelihood");
                    values.remove("estimate_conditional");
                }
            }
        }
    }
    program["evidence_ids"] = json!(current_evidence);
    core::endpoints::invalidate_changed_candidates(snapshot, &mut program, old);
    core::clear_ineligible_forecasts(&mut program);
    let results = program["results"].clone();
    program["tasks"]
        .as_array_mut()
        .ok_or("Missing tasks")?
        .retain(|t| results[core::field(t, "nodeId")][core::field(t, "function")].is_null());
    Ok(program)
}
// Invalid model output is correctable without discarding evaluated work. The
// rejected draft remains untrusted and never enters the snapshot or task queue.
// Optional set-quality revision must not discard a structurally valid prior answer.
// Initial composition and unrelated failures have no such fallback.
fn optional_composition_fallback(
    snapshot: &Value,
    old: &Value,
    error: &str,
    raw: &str,
) -> Option<Value> {
    let audit = &old["world_set_audit"];
    let active = old["active_world_ids"].as_array()?;
    if old["stage"] != "worlds"
        || !(2..=6).contains(&active.len())
        || audit["correction_status"] != "revision_requested"
        || !matches!(
            audit["verdict"].as_str(),
            Some("complementary_slices" | "uncertain")
        )
        || audit["world_ids"] != old["active_world_ids"]
        || audit["task_id"] != core::search::world_set_task(active)["nodeId"]
    {
        return None;
    }
    let nodes = snapshot["nodes"].as_array()?;
    for id in active {
        let world = nodes
            .iter()
            .find(|n| n["Id"] == *id && n["kind"] == "world" && n["archived"] != true)?;
        core::search::validate_world(world, snapshot).ok()?;
    }
    let exhausted = old["composition_correction"]["attempt"]
        .as_u64()
        .unwrap_or(0)
        .max(old["response_correction"]["attempt"].as_u64().unwrap_or(0))
        >= 2;
    if !exhausted && raw.len() <= 256 * 1024 {
        return None;
    }
    let mut program = old.clone();
    program["world_set_audit"]["correction_status"] = json!(if exhausted {
        "correction_exhausted"
    } else {
        "correction_context_limit"
    });
    program["world_set_audit"]["correction_error"] = json!(error);
    program["stop_reason"] = json!("round_evaluated");
    Some(program)
}

fn composition_correction(old: &Value, raw: &str, error: &str) -> Result<Value, String> {
    let attempt = old["composition_correction"]["attempt"]
        .as_u64()
        .unwrap_or(0)
        + 1;
    if attempt > 2 {
        return Err(format!(
            "Composition rejected after two corrective attempts: {error}"
        ));
    }
    if raw.len() > 256 * 1024 {
        return Err(format!(
            "Rejected composition exceeds correction context limit: {error}"
        ));
    }
    let mut program = old.clone();
    program["composition_correction"] = json!({
        "attempt":attempt, "validation_error":error, "rejected_draft":raw,
        "instruction":"Correct the rejected composition against the unchanged catalog. Return the complete composition JSON. Do not invent references, turn observations into future hypotheses, or claim any new evaluation ran."
    });
    Ok(program)
}

fn exploration_correction(
    phase: &str,
    old: &Value,
    error: &str,
    raw: &str,
) -> Result<Value, String> {
    let attempt = old["response_correction"]["attempt"].as_u64().unwrap_or(0) + 1;
    if attempt > 2 {
        return Err(format!(
            "{phase} rejected after two corrective attempts: {error}"
        ));
    }
    let mut program = old.clone();
    let instruction = if scope_pending(old) {
        "Scope research repair was not applied. Return the complete research-only repair JSON: no hypotheses or branches; preserve original question and source qualifications, validate refreshed baseline, and report remaining limits honestly. Empty research_evidence is valid when no new findings were obtained."
    } else if phase == "imagine" {
        "The endpoint proposal was not applied. Return the complete endpoint-only JSON against the same question and sourced present; do not generate components, routes or estimates yet."
    } else if phase == "backward" {
        "The backward search response was not applied. Return complete corrected hypotheses, evidence and route JSON. Preserve immutable endpoint commitments and earlier routes, ground each proposed root or mark its unresolved question, and retain supplied source qualifications."
    } else if phase == "challenge" {
        "The challenge response was not applied. Return the complete corrected challenge JSON against the unchanged visible catalog. Every premise must link existing prior hypotheses to new alternative hypotheses; every new hypothesis must be linked. Do not invent references, evidence, or evaluations. You may return empty premises and hypotheses with an honest explanation."
    } else {
        "The exploration response was not applied. Return the complete corrected exploration JSON against the unchanged visible catalog. Parent denotes hypothesis lineage and must reference a scenario/revision or a new hypothesis in this batch; source evidence is support, not a parent. Preserve the distinction between source observations and future hypotheses. Do not invent references or evaluations, and cite only research actually retrieved."
    };
    program["response_correction"] = json!({"attempt":attempt,"validation_error":error,"instruction":instruction,"rejected_draft":bounded_rejected_draft(raw)?});
    Ok(program)
}

fn bounded_rejected_draft(raw: &str) -> Result<&str, String> {
    if raw.len() > 256 * 1024 {
        return Err(
            "Rejected response exceeds 256 KiB correction context limit; no draft was applied"
                .into(),
        );
    }
    Ok(raw)
}

fn generated_response(raw: &str, old: &Value) -> Result<Result<Value, Value>, String> {
    let raw = raw.trim();
    let json_text = if raw.starts_with("```") {
        raw.split_once('\n')
            .and_then(|(_, body)| body.rsplit_once("```").map(|(text, _)| text))
    } else {
        Some(raw)
    };
    match json_text.and_then(|text| serde_json::from_str::<Value>(text).ok()) {
        Some(value) if value.is_object() => Ok(Ok(value)),
        _ => {
            let attempt = old["response_correction"]["attempt"].as_u64().unwrap_or(0) + 1;
            if attempt > 2 {
                return Err("Reasoning returned invalid JSON after two correction attempts; saved evidence and judgments are preserved".into());
            }
            let mut program = old.clone();
            program["response_correction"] = json!({"attempt":attempt,"rejected_draft":bounded_rejected_draft(raw)?,
                "instruction":"The previous response was not a JSON object and was not applied. Return the complete JSON object required by this phase. Tool-call prose is not an executed tool call or a final result. Use actual tools if research is needed, then return the required JSON. Do not claim new evidence or judgments unless they were obtained."});
            Ok(Err(program))
        }
    }
}

fn expand_with_baseline(
    snapshot: &mut Value,
    generated: &Value,
    phase: &str,
    old: &Value,
) -> Result<Option<Value>, String> {
    if phase == "backward" {
        core::backward::validate(old, generated)?;
    }
    let mut candidate = snapshot.clone();
    expand(&mut candidate, generated, phase, old)?;
    let refresh = if matches!(phase, "explore" | "challenge" | "backward") {
        refresh_researched_baseline(snapshot, &candidate, generated, old)?
    } else {
        None
    };
    if phase == "backward" {
        core::endpoints::add_routes(snapshot, &mut candidate, old, generated)?;
    }
    *snapshot = candidate;
    Ok(refresh)
}

fn run_inner(ctx: &Context) -> Result<(), String> {
    let phase = core::field(&ctx.entity_state, "phase");
    let mut old = core::parse(core::field(&ctx.entity_state, "program_json"))?;
    // Reasoning and corrective attempts consume transitions after the planner's
    // saved checkpoint. Admission must use the live count, never that stale copy.
    if phase == "compose" {
        old["transition_count"] =
            json!(core::transition_count(&old).max(core::transition_count(&ctx.entity_state)));
    }
    let mut snapshot = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
    let response = core::field(&ctx.entity_state, "reasoning_result");
    let generated = match generated_response(response, &old) {
        Ok(Ok(value)) => value,
        Err(error) => {
            if scope_pending(&old) {
                let program = failed_scope_repair(&old, &error);
                set_success_result(
                    "Expanded",
                    &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
                );
                return Ok(());
            }
            if phase == "compose"
                && let Some(program) =
                    optional_composition_fallback(&snapshot, &old, &error, response)
            {
                set_success_result(
                    "Expanded",
                    &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
                );
                return Ok(());
            }
            return Err(error);
        }
        Ok(Err(program)) => {
            set_success_result(
                "CompositionRejected",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
    };
    let raw_owned = generated.to_string();
    let raw = raw_owned.as_str();
    if phase == "synthesize" {
        let mut answer = core::parse(raw)?;
        references::References::new(&snapshot)?.resolve_generated(&mut answer);
        let program = core::parse(core::field(&ctx.entity_state, "program_json"))?;
        if program["stage"] == "worlds" || answer["schema"] == "foresight-worlds-v3" {
            attach_world_probabilities(&mut answer, &program, &snapshot)?;
        } else {
            if program["stage"] == "exploration" {
                return Err("Compose whole worlds before synthesis".into());
            }
            attach_probabilities(&mut answer, &program)?;
        }
        outlook::validate(&answer, &snapshot)?;
        set_success_result(
            "Complete",
            &json!({"answer":answer.to_string(),"finished_at_ms":Context::get_time_millis().to_string()}),
        );
        return Ok(());
    }

    if phase == "imagine" {
        match core::endpoints::imagine(&snapshot, &old, &generated) {
            Ok(prepared) => set_success_result(
                "Expanded",
                &json!({"snapshot_json":snapshot.to_string(),"program_json":prepared.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
            ),
            Err(error) => {
                let corrected = exploration_correction(raw, &old, phase, &error)?;
                set_success_result(
                    "CompositionRejected",
                    &json!({"program_json":corrected.to_string()}),
                );
            }
        }
        return Ok(());
    }
    if phase == "seed" {
        match core::parse(raw).and_then(|generated| establish_baseline(&snapshot, &generated, &old))
        {
            Ok(program) => set_success_result(
                "Expanded",
                &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
            ),
            Err(error) => {
                let program = baseline_correction(&old, &error)?;
                set_success_result(
                    "CompositionRejected",
                    &json!({"program_json":program.to_string()}),
                );
            }
        }
        return Ok(());
    }
    let before = snapshot["nodes"].as_array().ok_or("Missing nodes")?.len();
    let mut baseline_refresh = None;
    let composed = if phase == "compose" {
        match core::parse(raw).and_then(|generated| compose(&mut snapshot, &generated, &old)) {
            Ok(program) => Some(program),
            Err(error) => {
                let program = match composition_correction(&old, raw, &error) {
                    Ok(program) => program,
                    Err(correction_error) => {
                        if let Some(program) =
                            optional_composition_fallback(&snapshot, &old, &error, raw)
                        {
                            set_success_result(
                                "Expanded",
                                &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
                            );
                            return Ok(());
                        }
                        return Err(correction_error);
                    }
                };
                set_success_result(
                    "CompositionRejected",
                    &json!({"program_json":program.to_string()}),
                );
                return Ok(());
            }
        }
    } else {
        let generated = core::parse(raw)?;
        let expansion = if scope_pending(&old) {
            let mut candidate = snapshot.clone();
            expand(&mut candidate, &generated, phase, &old).and_then(|_| {
                finish_scope_repair(&candidate, &generated, &old)?;
                snapshot = candidate;
                Ok(())
            })
        } else {
            expand_with_baseline(&mut snapshot, &generated, phase, &old).map(|refresh| {
                baseline_refresh = refresh;
            })
        };
        if let Err(error) = expansion {
            if !matches!(phase, "challenge" | "explore" | "backward") {
                return Err(error);
            }
            let program = match exploration_correction(phase, &old, &error, raw) {
                Ok(program) => program,
                Err(correction_error) if scope_pending(&old) => {
                    let program = failed_scope_repair(&old, &correction_error);
                    set_success_result(
                        "Expanded",
                        &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
                    );
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            set_success_result(
                "CompositionRejected",
                &json!({"program_json":program.to_string()}),
            );
            return Ok(());
        }
        None
    };
    let nodes = snapshot["nodes"].as_array_mut().ok_or("Missing nodes")?;
    for node in nodes.iter_mut().skip(before) {
        node["source_session_id"] = json!(core::field(&ctx.entity_state, "reasoning_session_id"));
    }
    let added = nodes.len() - before;
    let mut program = if let Some(program) = composed {
        program
    } else {
        let mut planning_old = old.clone();
        if scope_pending(&old) {
            let repaired = finish_scope_repair(&snapshot, &core::parse(raw)?, &old)?;
            if repaired["baseline"] != old["baseline"] {
                planning_old["evidence_ids"] = json!([]);
            }
        }
        if let Some(ref receipt) = baseline_refresh {
            planning_old["baseline"] = receipt["baseline"].clone();
            planning_old["scope_review"] = receipt["scope_review"].clone();
        }
        replan(&snapshot, &planning_old, &core::parse(raw)?, added)?
    };
    if scope_pending(&old) {
        let receipt = finish_scope_repair(&snapshot, &core::parse(raw)?, &old)?;
        program["baseline"] = receipt["baseline"].clone();
        program["scope_review"] = receipt["review"].clone();
        program["scope_repair"] = receipt;
        program["round"] = old["round"].clone();
        program["rounds"] = old["rounds"].clone();
        program["continue_exploring"] = json!(true);
    }
    if let Some(mut receipt) = baseline_refresh {
        receipt["source_session_id"] =
            json!(core::field(&ctx.entity_state, "reasoning_session_id"));
        program["baseline"] = receipt["baseline"].clone();
        program["scope_review"] = receipt["scope_review"].clone();
        if !program["baseline_history"].is_array() {
            program["baseline_history"] = json!([]);
        }
        program["baseline_history"]
            .as_array_mut()
            .unwrap()
            .push(receipt);
    }
    if phase == "backward" {
        let original = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
        let mut candidate = snapshot.clone();
        // Route nodes were validated atomically in expansion; reconstruct the
        // append-only receipt against the same ordinary candidate graph.
        candidate["nodes"].as_array_mut().unwrap().retain(|n| {
            n["route_only"] != true
                || original["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|prior| prior["Id"] == n["Id"])
        });
        program["endpoint_search"] =
            core::endpoints::add_routes(&original, &mut candidate, &old, &generated)?;
    }
    if phase == "challenge" {
        record_challenge(&snapshot, before, &core::parse(raw)?, &old, &mut program)?;
    }
    set_success_result(
        "Expanded",
        &json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"started_at_ms":core::field(&ctx.entity_state,"started_at_ms")}),
    );
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host().and_then(|ctx| run_inner(&ctx)) {
        Ok(()) => (),
        Err(e) => set_success_result("Fail", &json!({"error_message":e})),
    };
    0
}
#[cfg(test)]
mod tests {
    #[test]
    fn text_diagnostics_name_field_and_unicode_counts_without_changing_bounds() {
        let mut review = json!({"requested_question":"Question","evidence_scope":"é".repeat(838),"status":"narrowed","narrowing_basis":"evidence_availability","limitations":["Unknown"]});
        let snapshot = json!({"world":{"description":"Question"}});
        let error = validate_scope(&review, &snapshot).unwrap_err();
        for detail in ["scope_review.evidence_scope", "838", "800"] {
            assert!(error.contains(detail), "{error}");
        }
        review["evidence_scope"] = json!("é".repeat(800));
        assert!(validate_scope(&review, &snapshot).is_ok());
        review["limitations"] = json!(["é".repeat(241)]);
        let error = validate_scope(&review, &snapshot).unwrap_err();
        for detail in ["scope_review.limitations[0]", "241", "240"] {
            assert!(error.contains(detail), "{error}");
        }
        review["limitations"] = json!(vec!["Unknown"; 17]);
        let error = validate_scope(&review, &snapshot).unwrap_err();
        for detail in ["scope_review.limitations", "17", "16"] {
            assert!(error.contains(detail), "{error}");
        }
        assert!(
            bounded_text(&Value::Null, 800, "scope_review.evidence_scope")
                .unwrap_err()
                .contains("non-string")
        );
        assert!(
            bounded_text(&json!("   "), 800, "scope_review.evidence_scope")
                .unwrap_err()
                .contains("3 characters")
        );
        assert!(
            bounded_texts(&Value::Null, 0, 4, 240, "worlds[0].what_you_can_do")
                .unwrap_err()
                .contains("non-array")
        );
    }

    #[test]
    fn baseline_replacement_accounts_for_all_prior_claims() {
        let snapshot = json!({"world":{"last_ingest_date":"2026-10-01","evidence_contract":"v1"},"nodes":[{"Id":"e","kind":"research_evidence","evidence_metadata":{"kind":"finding","publication_date":"2026","observation_period":{"start":null,"end":null},"retrieved_at":null}}]});
        let old = json!({"baseline":{"observed":(0..16).map(|i|json!({"claim":format!("Prior {i}"),"evidence_ids":["e"]})).collect::<Vec<_>>()}});
        let mut reply = json!({"baseline":{"as_of":"2026-10-01","observed":[{"claim":"Reconciled finding","evidence_ids":["e"]}]}});
        assert!(
            validate_baseline_dispositions(&old, &reply, &snapshot)
                .unwrap_err()
                .contains("omitted prior observation 0")
        );
        reply["baseline_dispositions"] = json!((0..16).map(|i|json!({"prior_observation_index":i,"replacement_observation_indices":if i==15 {vec![]} else {vec![0]},"reason":"Source supports consolidation or withdrawal","evidence_ids":["e"]})).collect::<Vec<_>>());
        validate_baseline_dispositions(&old, &reply, &snapshot).unwrap();
        let mut bad = reply.clone();
        bad["baseline_dispositions"][15]["prior_observation_index"] = json!(16);
        assert!(validate_baseline_dispositions(&old, &bad, &snapshot).is_err());
        bad = reply.clone();
        bad["baseline_dispositions"][0]["evidence_ids"] = json!(["foreign"]);
        assert!(validate_baseline_dispositions(&old, &bad, &snapshot).is_err());
        for (field, value) in [
            ("prior_observation_index", json!(0)),
            ("prior_observation_index", json!(4294967296u64)),
            ("replacement_observation_indices", json!([1])),
            ("replacement_observation_indices", json!([4294967296u64])),
        ] {
            bad = reply.clone();
            bad["baseline_dispositions"][15][field] = value;
            assert!(validate_baseline_dispositions(&old, &bad, &snapshot).is_err());
        }
        for (field, value) in [("kind", json!("lead")), ("publication_date", json!("2027"))] {
            let mut invalid_source = snapshot.clone();
            invalid_source["nodes"][0]["evidence_metadata"][field] = value;
            assert!(validate_baseline_dispositions(&old, &reply, &invalid_source).is_err());
        }
        let enriched =
            json!({"baseline":{"observed":[{"claim":"Prior 0","evidence_ids":["e","other"]}]}});
        validate_baseline_dispositions(
            &json!({"baseline":{"observed":[old["baseline"]["observed"][0]]}}),
            &enriched,
            &snapshot,
        )
        .unwrap();
    }

    #[test]
    #[ignore = "private captured clothing fixture; set CLOTHING_BASELINE_CAPTURE"]
    fn captured_clothing_baseline_delta_is_rejected() {
        let row: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("CLOTHING_BASELINE_CAPTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let program: Value =
            serde_json::from_str(row["fields"]["program_json"].as_str().unwrap()).unwrap();
        let snapshot: Value =
            serde_json::from_str(row["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
        let repair = &program["scope_repair"];
        let repaired=finish_scope_repair(&snapshot,&json!({"baseline":repair["baseline"],"scope_review":repair["review"],"research_evidence":[],"scope_disposition":{"status":repair["disposition"],"report":repair["report"],"evidence_ids":repair["evidence_ids"]}}),&json!({"baseline":repair["original_baseline"],"scope_review":repair["original_review"]})).unwrap();
        assert_eq!(repaired["baseline"], repair["baseline"]);
        let receipt = &program["baseline_history"][1];
        assert_eq!(
            receipt["prior_baseline"]["observed"]
                .as_array()
                .unwrap()
                .len(),
            16
        );
        assert_eq!(receipt["baseline"]["observed"].as_array().unwrap().len(), 1);
        let old = json!({"round":2,"baseline":receipt["prior_baseline"],"scope_review":receipt["prior_scope_review"]});
        let reply = json!({"baseline":receipt["baseline"],"scope_review":receipt["scope_review"],"research_evidence":[{"id":"e-bnpl-fca-2026","evidence_metadata":{"kind":"finding"}}]});
        outlook::validate_new_baseline(&reply["baseline"], &snapshot).unwrap(); // old acceptance boundary
        assert!(
            refresh_researched_baseline(&snapshot, &snapshot, &reply, &old)
                .unwrap_err()
                .contains("omitted prior observation 0")
        );
    }

    #[test]
    fn researched_baseline_maps_new_sources_and_rejects_unaccepted_summaries() {
        let before = json!({"world":{"description":"Question","last_ingest_date":"2026-10-01","evidence_contract":"v1"},"nodes":[]});
        let source = json!({"Id":"r4-local","kind":"research_evidence","evidence_metadata":{"kind":"finding","publication_date":"2025","observation_period":{"start":null,"end":null},"retrieved_at":"2026-10-01"}});
        let mut after = before.clone();
        after["nodes"] = json!([source]);
        let old = json!({"round":3,"baseline":{"unknowns":["No finding"]},"scope_review":{"status":"narrowed"}});
        let reply = json!({"research_evidence":[{"id":"local","evidence_metadata":{"kind":"finding"}}],"baseline":{"as_of":"2026-10-01","observed":[{"claim":"Known limited observation","evidence_ids":["local"]}],"assumptions":[],"unknowns":["Future remains unknown"]},"scope_review":{"requested_question":"Question","evidence_scope":"Limited observation","status":"narrowed","narrowing_basis":"evidence_availability","limitations":["Future remains unknown"]}});
        let receipt = refresh_researched_baseline(&before, &after, &reply, &old)
            .unwrap()
            .unwrap();
        assert_eq!(receipt["prior_baseline"], old["baseline"]);
        assert_eq!(
            receipt["baseline"]["observed"][0]["evidence_ids"][0],
            "r4-local"
        );
        let mut replacement = reply.clone();
        replacement["baseline_dispositions"] = json!([{"prior_observation_index":0,"replacement_observation_indices":[0],"reason":"The new finding corrects the earlier account","evidence_ids":["local"]}]);
        let mut earlier = old.clone();
        earlier["baseline"]["observed"] =
            json!([{"claim":"Earlier account","evidence_ids":["r4-local"]}]);
        let revision = refresh_researched_baseline(&before, &after, &replacement, &earlier)
            .unwrap()
            .unwrap();
        assert_eq!(
            revision["dispositions"][0]["evidence_ids"],
            json!(["r4-local"])
        );
        assert_eq!(revision["prior_baseline"], earlier["baseline"]);
        assert_eq!(revision["dispositions_verified"], false);
        let mut bad = reply.clone();
        bad.as_object_mut().unwrap().remove("baseline");
        assert!(
            refresh_researched_baseline(&before, &after, &bad, &old)
                .unwrap_err()
                .contains("require reconciled")
        );
        bad = reply.clone();
        bad["baseline"]["observed"][0]["evidence_ids"] = json!(["invented"]);
        assert!(refresh_researched_baseline(&before, &after, &bad, &old).is_err());
        bad = reply.clone();
        bad["scope_review"]["requested_question"] = json!("Other");
        assert!(refresh_researched_baseline(&before, &after, &bad, &old).is_err());
        let mut future = after.clone();
        future["nodes"][0]["evidence_metadata"]["publication_date"] = json!("2027");
        assert!(refresh_researched_baseline(&before, &future, &reply, &old).is_err());
        bad = reply.clone();
        bad["research_evidence"][0]["evidence_metadata"]["kind"] = json!("lead");
        assert!(
            refresh_researched_baseline(&before, &after, &bad, &old)
                .unwrap()
                .is_none()
        );
    }
    use super::*;
    fn world_fixture() -> (Value, Value, Value) {
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-19","target_date":"2027-09-19"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed baseline","edges":"[]"},{"Id":"a","kind":"scenario","statement":"Component A","edges":"[]"},{"Id":"b","kind":"revision","statement":"Component B","edges":"[]"},{"Id":"c","kind":"scenario","statement":"Component C","edges":"[]"},{"Id":"d","kind":"scenario","statement":"Counter D","edges":"[]"}]});
        let world = json!({"id":"one","trajectory_answer":"One organization of the entire system","title":"A whole world","statement":"A and B and C occur jointly","mechanism":"A enables B enables C","component_ids":["ref_0002","ref_0003","ref_0004"],"counter_ids":["ref_0005"],"scene":"An imagined day","narrative":"A causes B and C but D may prevent it","what_you_can_do":[],"signals":["Observe A"],"falsifiers":["Observe D"],"facets":[{"id":"f1","title":"First change","description":"A changes daily life","component_ids":["ref_0002"]},{"id":"f2","title":"Second change","description":"B changes software","component_ids":["ref_0003"]},{"id":"f3","title":"Third change","description":"C changes economic choices","component_ids":["ref_0004"]}],"chain":[{"id":"l1","from_ids":["ref_0002"],"to_id":"ref_0003","mechanism":"A makes B possible","by":"2027-03-01"},{"id":"l2","from_ids":["ref_0003"],"to_id":"ref_0004","mechanism":"B enables C","by":"2027-09-01"}],"assumptions":["The mechanism persists"]});
        let mut second = world.clone();
        second["id"] = json!("two");
        second["trajectory_answer"] =
            json!("A different organization with different downstream consequences");
        let generated = json!({"shared_question":"How do the interacting constraints change the system?","baseline":{"as_of":"2026-09-19","observed":[{"claim":"Observed baseline","evidence_ids":["ref_0001"]}],"assumptions":[],"unknowns":[]},"worlds":[world,second]});
        let program = json!({"claim_role_contract":1,"results":{"a":{"classify_claim_role":"event","estimate_likelihood":"0.9"},"b":{"classify_claim_role":"event","estimate_likelihood":"0.8"},"c":{"classify_claim_role":"event"},"d":{"classify_claim_role":"event"}},"evaluations":{},"rounds":[],"round":6,"stop_reason":"exploration_converged"});
        (snapshot, generated, program)
    }
    #[test]
    fn endpoint_routes_compose_without_link_collisions_and_keep_original_lineage() {
        let (mut snapshot, mut generated, mut old) = world_fixture();
        snapshot["world"]["description"] = json!("How could these interacting systems change?");
        references::References::new(&snapshot)
            .unwrap()
            .resolve_generated(&mut generated);
        old["baseline"] = generated["baseline"].clone();
        old["world_search_contract"] = json!(1);
        old["endpoint_search"] =
            json!({"status":"searching","endpoints":[],"routes":[],"amendments":[],"rounds":[]});
        let mut routes = vec![];
        for e in 0..2 {
            let endpoint_id = format!("endpoint{e}");
            old["endpoint_search"]["endpoints"].as_array_mut().unwrap().push(json!({"id":endpoint_id,"title":"Original endpoint","original_statement":generated["worlds"][e]["statement"],"original_narrative":"Frozen imagined world","commitments":[{"id":"ca","statement":"Component A"},{"id":"cb","statement":"Component B"},{"id":"cc","statement":"Component C"}],"signals":["A changes"],"falsifiers":["A fails"]}));
            let mut selected = vec![];
            let mut bindings = vec![];
            for (from, to, claim) in [("d", "a", "ca"), ("a", "b", "cb"), ("b", "c", "cc")] {
                let id = format!("route{e}-{claim}");
                selected.push(id.clone());
                bindings.push(json!({"commitment_id":claim,"component_id":to,"amendment_id":null}));
                routes.push(json!({"id":id,"endpoint_id":endpoint_id,"commitment_id":claim,"target_component_id":to,"component_ids":[from,to],"chain":[{"id":"same-local-label","from_ids":[from],"to_id":to,"by":"2027-03-01","mechanism":format!("{from} enables {to}")}],"root_connections":[{"component_id":from,"evidence_ids":["e"],"mechanism":"A proposed bridge from observed conditions"}],"grounding_evidence_ids":["e"],"alternative_to":null,"amendment_id":null}));
            }
            generated["worlds"][e]["endpoint_id"] = json!(endpoint_id);
            generated["worlds"][e]["selected_route_ids"] = json!(selected);
            generated["worlds"][e]["commitment_bindings"] = json!(bindings);
            generated["worlds"][e]["component_ids"] = json!(["a", "b", "c", "d"]);
            generated["worlds"][e]["counter_ids"] = json!([]);
            generated["worlds"][e]["facets"][0]["component_ids"] = json!(["a", "d"]);
        }
        let before = snapshot.clone();
        old["endpoint_search"]=core::endpoints::add_routes(&before,&mut snapshot,&old,&json!({"routes":routes,"amendments":[],"hypotheses":[],"research_evidence":[],"exploration_note":"Two endpoint routes"})).unwrap();
        for world in generated["worlds"].as_array_mut().unwrap() {
            world["chain"] = json!(
                old["endpoint_search"]["routes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["endpoint_id"] == world["endpoint_id"])
                    .flat_map(|r| r["chain"].as_array().unwrap().iter().cloned())
                    .collect::<Vec<_>>()
            );
        }
        let mut omitted = old["endpoint_search"]["endpoints"][0].clone();
        omitted["id"] = json!("unreached-original");
        omitted["status"] = json!("unresolved");
        old["endpoint_search"]["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(omitted);
        generated["unreconstructed_endpoints"] = json!([{"endpoint_id":"unreached-original","reason":"No evaluated path reached this original commitment set."}]);
        let route_node = old["endpoint_search"]["routes"][0]["world_node_id"]
            .as_str()
            .unwrap()
            .to_owned();
        old["results"][route_node]["check_route_grounding"] = json!("conflict");
        core::endpoints::finish_routes(&snapshot, &mut old);
        let program = compose(&mut snapshot, &generated, &old).unwrap();
        assert_eq!(program["active_world_ids"].as_array().unwrap().len(), 2);
        for id in program["active_world_ids"].as_array().unwrap() {
            let world = snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["Id"] == *id)
                .unwrap();
            core::search::validate_chain(world, &snapshot).unwrap();
            assert_eq!(world["chain"].as_array().unwrap().len(), 3);
            assert_ne!(world["route_only"], true);
        }
        let mut answer = json!({"schema":"foresight-worlds-v3","headline":"Different imagined ways the system could change","horizon":"2027-09-19","probability_basis":"model_implied_world_estimate","probability_model":"overlapping_worlds","calibrated":false,"summary":"Compare two imagined endpoint worlds and their causal routes.","evidence_limits":["Synthetic producer-contract fixture; no live estimates were made."],"research_questions":[],"outcomes":program["active_world_ids"].as_array().unwrap().iter().map(|id|{let node=snapshot["nodes"].as_array().unwrap().iter().find(|n|n["Id"]==*id).unwrap();json!({"id":id,"world_id":id,"title":node["title"],"definition":node["statement"],"scene":node["scene"],"narrative":node["narrative"],"what_you_can_do":node["what_you_can_do"],"signals":node["signals"],"falsifiers":node["falsifiers"]})}).collect::<Vec<_>>()});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(
            answer["outcomes"][0]["original_endpoint"],
            old["endpoint_search"]["endpoints"][0]
        );
        assert_eq!(
            answer["unreconstructed_endpoints"][0]["original_endpoint"],
            old["endpoint_search"]["endpoints"][2]
        );
        assert_eq!(
            answer["outcomes"][0]["audit"]["selected_routes"]["status"],
            "blocked"
        );
        if let Ok(path) = std::env::var("FORESIGHT_COMPOSE_FIXTURE") {
            std::fs::write(
                path,
                serde_json::to_string_pretty(
                    &json!({"program":program,"snapshot":snapshot,"answer":answer}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn claim_role_admission_rejects_context_components_without_altering_worlds() {
        let (snapshot, generated, mut program) = world_fixture();
        program["claim_role_contract"] = json!(1);
        program["baseline_status"] = json!("established");
        for id in ["a", "b", "c", "d"] {
            program["results"][id]["classify_claim_role"] = json!("event");
            program["results"][id]["classify_temporal"] = json!("uncertain");
        }
        assert!(compose(&mut snapshot.clone(), &generated, &program).is_ok());
        for role in ["context", "unresolved"] {
            program["results"]["a"]["classify_claim_role"] = json!(role);
            let mut untouched = snapshot.clone();
            assert!(compose(&mut untouched, &generated, &program).is_err());
            assert_eq!(untouched, snapshot);
        }
        program["results"]["a"]["classify_claim_role"] = json!("event");
        program["results"]["d"]["classify_claim_role"] = json!("context");
        assert!(
            compose(&mut snapshot.clone(), &generated, &program)
                .unwrap_err()
                .contains("not an admitted event proposition")
        );
        program["results"]["d"]["classify_claim_role"] = json!("event");
        program["results"]["a"]
            .as_object_mut()
            .unwrap()
            .remove("classify_claim_role");
        assert!(compose(&mut snapshot.clone(), &generated, &program).is_err());
    }

    #[test]
    fn legacy_composition_does_not_claim_new_admission_checks_ran() {
        let (mut snapshot, generated, mut old) = world_fixture();
        old.as_object_mut().unwrap().remove("claim_role_contract");
        for fields in old["results"].as_object_mut().unwrap().values_mut() {
            fields
                .as_object_mut()
                .unwrap()
                .remove("classify_claim_role");
        }
        let program = compose(&mut snapshot, &generated, &old).unwrap();
        assert!(program.get("claim_role_contract").is_none());
        assert_eq!(program["results"]["a"]["estimate_likelihood"], "0.9");
    }

    #[test]
    fn legacy_replan_requeues_probability_after_new_role_admission() {
        let snapshot = json!({"nodes":[{"Id":"h","kind":"scenario","edges":"[]"}]});
        let old = json!({"results":{"h":{"classify_temporal":"uncertain","estimate_likelihood":"0.61"}},"rounds":[]});
        let mut program = replan(&snapshot, &old, &json!({"continue_exploring":true}), 0).unwrap();
        assert!(program["results"]["h"]["estimate_likelihood"].is_null());
        assert!(
            program["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["function"] == "estimate_likelihood")
        );
        program["results"]["h"]["classify_claim_role"] = json!("event");
        assert!(core::forecast_allows(&program, "h"));
        assert_eq!(old["results"]["h"]["estimate_likelihood"], "0.61");
    }

    #[test]
    fn revisions_get_new_role_checks_and_new_evidence_invalidates_roles() {
        let mut snapshot = json!({"nodes":[{"Id":"e","kind":"evidence","edges":"[]"},{"Id":"h","kind":"scenario","statement":"Research commentary","edges":"[]"}]});
        let mut old = core::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        old["evidence_ids"] = json!(["e"]);
        old["results"]["h"]["classify_claim_role"] = json!("context");
        old["evaluations"]["h"]["classify_claim_role"] = json!({"selected":"context"});
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":"revision","kind":"revision","parent":"h","statement":"An observable event happens by 2030","edges":"[]"}));
        let generated = json!({"continue_exploring":true});
        let revised = replan(&snapshot, &old, &generated, 1).unwrap();
        assert_eq!(revised["results"]["h"]["classify_claim_role"], "context");
        assert!(revised["results"]["revision"]["classify_claim_role"].is_null());
        assert!(
            revised["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "revision" && t["function"] == "classify_claim_role")
        );
        assert!(
            !revised["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "classify_claim_role")
        );
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"new-source","kind":"research_evidence","edges":"[]"}));
        let refreshed = replan(&snapshot, &revised, &generated, 1).unwrap();
        assert!(refreshed["results"]["h"]["classify_claim_role"].is_null());
        assert!(refreshed["evaluations"]["h"]["classify_claim_role"].is_null());
        assert!(
            refreshed["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "classify_claim_role")
        );
    }

    #[test]
    fn rejected_draft_is_exact_or_explicitly_too_large() {
        let raw = "{\"source\":\"retrieved text\"}";
        assert_eq!(bounded_rejected_draft(raw).unwrap(), raw);
        assert!(bounded_rejected_draft(&"x".repeat(256 * 1024)).is_ok());
        assert!(
            bounded_rejected_draft(&"x".repeat(256 * 1024 + 1))
                .unwrap_err()
                .contains("exceeds 256 KiB")
        );
    }

    #[test]
    fn malformed_scope_repair_exhaustion_keeps_work_and_continues() {
        let old = json!({"scope_repair":{"status":"pending"},"baseline":{"unknowns":["narrow"]},"results":{"h":{"classify_gap":"none"}},"http_calls":9,"round":0,"response_correction":{"attempt":2}});
        assert!(generated_response("invalid", &old).is_err());
        let next = failed_scope_repair(&old, "invalid JSON");
        assert!(!scope_pending(&next));
        assert_eq!(next["results"], old["results"]);
        assert_eq!(next["baseline"], old["baseline"]);
        assert_eq!(next["http_calls"], 9);
        assert_eq!(next["continue_exploring"], true);
    }

    #[test]
    fn scope_repair_distinguishes_user_scope_and_preserves_sources() {
        let snapshot = json!({"world":{"description":"How might teenagers learn?","last_ingest_date":"2026-10-01","hindcast_mode":"false"},"branches":[],"nodes":[{"Id":"e","kind":"evidence","statement":"Observed US AI use","edges":"[]"}]});
        let baseline = json!({"as_of":"2026-10-01","observed":[{"claim":"US AI use observed","evidence_ids":["e"]}],"assumptions":[],"unknowns":["Evidence only covers US school AI use."]});
        let review = json!({"requested_question":"How might teenagers learn?","evidence_scope":"US school AI use","narrowing_basis":"evidence_availability","status":"narrowed","limitations":["Evidence only covers US school AI use."]});
        let old = core::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        let pending = establish_baseline(
            &snapshot,
            &json!({"baseline":baseline,"scope_review":review}),
            &old,
        )
        .unwrap();
        assert!(scope_pending(&pending));
        let mut explicit = review.clone();
        explicit["narrowing_basis"] = json!("user_explicit");
        assert!(!scope_pending(
            &establish_baseline(
                &snapshot,
                &json!({"baseline":baseline,"scope_review":explicit}),
                &old
            )
            .unwrap()
        ));
        let mut wrong = review.clone();
        wrong["requested_question"] = json!("US AI policy");
        assert!(validate_scope(&wrong, &snapshot).is_err());
        let generated = json!({"hypotheses":[],"branches":[],"research_evidence":[],"continue_exploring":true,"exploration_note":"No additional supported evidence found","baseline":baseline,"scope_review":review,"scope_disposition":{"status":"limited","report":"Sources remain narrow; no new supported findings.","evidence_ids":["e"]}});
        let mut repaired = snapshot.clone();
        expand(&mut repaired, &generated, "explore", &pending).unwrap();
        assert_eq!(repaired, snapshot);
        let receipt = finish_scope_repair(&repaired, &generated, &pending).unwrap();
        assert_eq!(receipt["coverage_certified"], false);
        let mut done = pending.clone();
        done["scope_repair"] = receipt;
        assert!(!scope_pending(&done));
        let mut bad = generated.clone();
        bad["hypotheses"] = json!([{"id":"future"}]);
        assert!(
            expand(&mut repaired, &bad, "explore", &pending)
                .unwrap_err()
                .contains("research-only")
        );
        assert_eq!(repaired, snapshot);
        bad = generated.clone();
        bad["baseline"]["observed"] = json!([]);
        assert!(
            finish_scope_repair(&snapshot, &bad, &pending)
                .unwrap_err()
                .contains("omitted prior observation 0")
        );
        bad = generated.clone();
        bad["scope_disposition"]["evidence_ids"] = json!(["invented"]);
        assert!(finish_scope_repair(&snapshot, &bad, &pending).is_err());
    }

    #[test]
    fn scope_repair_accounts_for_prior_fourteen_and_normalizes_dispositions() {
        let snapshot = json!({"world":{"description":"Question","last_ingest_date":"2026-10-01","evidence_contract":"v1"},"nodes":[{"Id":"scope-new","kind":"research_evidence","evidence_metadata":{"kind":"finding","publication_date":"2026","observation_period":{"start":null,"end":null},"retrieved_at":null}}]});
        let baseline = json!({"as_of":"2026-10-01","observed":(0..14).map(|i|json!({"claim":format!("Prior {i}"),"evidence_ids":["scope-new"]})).collect::<Vec<_>>(),"assumptions":[],"unknowns":[]});
        let old = json!({"baseline":baseline,"started_at_ms":12345});
        let mut reply = json!({"baseline":baseline,"research_evidence":[{"id":"new"}],"scope_review":{"requested_question":"Question","evidence_scope":"Current evidence","narrowing_basis":"none","status":"aligned","limitations":[]},"scope_disposition":{"status":"addressed","report":"Retained prior findings","evidence_ids":["new"]}});
        let receipt = finish_scope_repair(&snapshot, &reply, &old).unwrap();
        assert_eq!(receipt["baseline"], baseline);
        reply["baseline"]["observed"].as_array_mut().unwrap().pop();
        assert!(
            finish_scope_repair(&snapshot, &reply, &old)
                .unwrap_err()
                .contains("omitted prior observation 13")
        );
        reply["baseline_dispositions"] = json!([{"prior_observation_index":13,"replacement_observation_indices":[],"reason":"New source supports retracting this claim","evidence_ids":["new"]}]);
        let receipt = finish_scope_repair(&snapshot, &reply, &old).unwrap();
        assert_eq!(
            receipt["dispositions"][0]["evidence_ids"],
            json!(["scope-new"])
        );
        assert_eq!(receipt["original_baseline"], baseline);
        assert_eq!(receipt["dispositions_verified"], false);
        assert_eq!(old["started_at_ms"], 12345);
    }

    #[test]
    fn optional_fallback_requires_valid_prior_set_and_exhausted_invalid_correction() {
        let (mut snapshot, generated, old) = world_fixture();
        let mut program = compose(&mut snapshot, &generated, &old).unwrap();
        let active = program["active_world_ids"].clone();
        program["world_set_audit"] = json!({"task_id":core::search::world_set_task(active.as_array().unwrap())["nodeId"],"world_ids":active,"verdict":"complementary_slices","correction_status":"revision_requested"});
        assert!(
            optional_composition_fallback(&snapshot, &program, "invalid draft", "{}").is_none()
        );
        program["composition_correction"] = json!({"attempt":2});
        let fallback =
            optional_composition_fallback(&snapshot, &program, "specific link error", "{}")
                .unwrap();
        assert_eq!(
            fallback["world_set_audit"]["correction_error"],
            "specific link error"
        );
        assert_eq!(
            fallback["world_set_audit"]["correction_status"],
            "correction_exhausted"
        );
        assert_eq!(fallback["tasks"], program["tasks"]);
        assert!(optional_composition_fallback(&snapshot, &old, "invalid initial", "{}").is_none());
        let id = active[0].as_str().unwrap();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["Id"] == id)
            .unwrap()["chain"][1]["by"] = json!("2026-01-01");
        assert!(
            optional_composition_fallback(&snapshot, &program, "invalid saved world", "{}")
                .is_none()
        );
    }
    #[test]
    fn research_admission_receipt_survives_composition_and_replanning() {
        let (mut snapshot, generated, mut old) = world_fixture();
        old["exploration_admission"] =
            json!({"admitted":false,"required_transitions":106,"remaining_transitions":22});
        let composed = compose(&mut snapshot, &generated, &old).unwrap();
        assert_eq!(
            composed["exploration_admission"],
            old["exploration_admission"]
        );
        let replanned = replan(&snapshot, &old, &json!({"continue_exploring":false}), 0).unwrap();
        assert_eq!(
            replanned["exploration_admission"],
            old["exploration_admission"]
        );
    }

    #[test]
    #[ignore = "Requires captured rejected composition"]
    fn actual_composition_rejection_identifies_unclassified_component_not_bad_alias() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_COMPOSITION_FIXTURE").unwrap())
                .unwrap(),
        )
        .unwrap();
        let record = &raw["fields"];
        let mut snapshot = core::parse(core::field(record, "snapshot_json")).unwrap();
        let program = core::parse(core::field(record, "program_json")).unwrap();
        let generated = core::parse(core::field(
            &program["composition_correction"],
            "rejected_draft",
        ))
        .unwrap();
        let before = snapshot.clone();
        let error = compose(&mut snapshot, &generated, &program).unwrap_err();
        assert!(error.contains("r8-hyp_inbox_triage_01"));
        assert!(error.contains("not evaluated in current evidence context"));
        assert_eq!(snapshot, before);
    }
    #[test]
    fn malformed_provider_reply_requests_bounded_correction_without_losing_state() {
        let old = json!({"baseline":{"observed":["saved"]},"results":{"h":{"estimate_likelihood":0.4}},"http_calls":57});
        let reply = "Tool call call_X: execute({\"code\":\"temper.web_fetch(url)\"})";
        let first = generated_response(reply, &old).unwrap().unwrap_err();
        assert_eq!(first["baseline"], old["baseline"]);
        assert_eq!(first["results"], old["results"]);
        assert_eq!(first["http_calls"], 57);
        assert_eq!(first["response_correction"]["attempt"], 1);
        let second = generated_response(reply, &first).unwrap().unwrap_err();
        assert!(
            generated_response(reply, &second)
                .unwrap_err()
                .contains("two correction")
        );
        assert_eq!(
            generated_response("```json\n{\"hypotheses\":[]}\n```", &second)
                .unwrap()
                .unwrap(),
            json!({"hypotheses":[]})
        );
        assert!(generated_response("[]", &old).unwrap().is_err());
        assert!(generated_response("```json", &old).unwrap().is_err());
    }
    #[test]
    fn baseline_precedes_exploration_is_sourced_and_survives_replanning() {
        let (snapshot, generated, old) = world_fixture();
        let seeded = establish_baseline(&snapshot, &generated, &old).unwrap();
        assert_eq!(
            seeded["baseline"]["observed"][0]["evidence_ids"],
            json!(["e"])
        );
        assert_eq!(seeded["baseline_status"], "established");
        let replanned = replan(&snapshot, &seeded, &json!({"continue_exploring":true}), 0).unwrap();
        assert_eq!(replanned["baseline"], seeded["baseline"]);
        let mut invalid = generated.clone();
        invalid["baseline"]["observed"][0]["evidence_ids"] = json!(["ref_0002"]);
        assert!(establish_baseline(&snapshot, &invalid, &old).is_err());
        let correction = baseline_correction(&old, "Hypothesis cannot source baseline").unwrap();
        assert!(correction["baseline"].is_null());
        let correction = baseline_correction(&correction, "Still invalid").unwrap();
        assert!(baseline_correction(&correction, "Still invalid").is_err());
        let mut observed = seeded.clone();
        assert!(compose(&mut snapshot.clone(), &generated, &observed).is_err());
        for id in ["a", "b", "c"] {
            observed["results"][id]["classify_temporal"] = json!("future_change");
        }

        observed["results"]["a"]["classify_temporal"] = json!("already_observed");
        assert!(compose(&mut snapshot.clone(), &generated, &observed).is_err());
        observed["results"]["a"]["classify_temporal"] = json!("mixed");
        assert!(compose(&mut snapshot.clone(), &generated, &observed).is_err());
        observed["results"]["a"]["classify_temporal"] = json!("uncertain");
        assert!(compose(&mut snapshot.clone(), &generated, &observed).is_ok());
    }
    #[test]
    #[ignore = "Requires captured final writer output"]
    fn captured_writer_context_is_attached_from_canonical_nodes() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_WRITER_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let record = &raw["fields"];
        let snapshot = core::parse(core::field(record, "snapshot_json")).unwrap();
        let program = core::parse(core::field(record, "program_json")).unwrap();
        let mut answer = core::parse(core::field(record, "reasoning_result")).unwrap();
        references::References::new(&snapshot)
            .unwrap()
            .resolve_generated(&mut answer);
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        outlook::validate(&answer, &snapshot).unwrap();
        for outcome in answer["outcomes"].as_array().unwrap() {
            for id in outcome["scenario_ids"].as_array().unwrap() {
                assert!(
                    snapshot["nodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|n| n["Id"] == *id)
                );
            }
        }
        answer["outcomes"][0]["world_id"] = json!("invented-world");
        assert!(attach_world_probabilities(&mut answer, &program, &snapshot).is_err());
    }

    #[test]
    fn composition_cannot_reuse_immutable_world_ids_when_old_counter_was_lost() {
        let (mut snapshot, generated, old) = world_fixture();
        let mut program = compose(&mut snapshot, &generated, &old).unwrap();
        program.as_object_mut().unwrap().remove("world_revision");
        let revised = compose(&mut snapshot, &generated, &program).unwrap();
        assert_eq!(revised["world_revision"], 2);
        assert!(
            revised["active_world_ids"]
                .as_array()
                .unwrap()
                .iter()
                .all(|id| id.as_str().unwrap().starts_with("world-r2-"))
        );
    }

    #[test]
    fn recovered_exploration_replans_only_events_and_preserves_world_history() {
        let mut snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","kind":"evidence","edges":"[]"},{"Id":"h","kind":"scenario","statement":"An explored event","edges":"[]"},{"Id":"world-r3-old","kind":"world","revision":3,"archived":true,"edges":"[]"}]});
        let old = json!({"stage":"exploration","world_revision":3,"world_refinement":{"world-r3-old":{"rounds":[{"round":1,"complete":false}]}},"world_audits":{"world-r3-old":{"status":"not_tested"}},"active_world_ids":[],"resume_mode":"unfinished_exploration","results":{},"evaluations":{},"rounds":[]});
        let generated = batch("new-alternative");
        expand(&mut snapshot, &generated, "explore", &old).unwrap();
        let replanned = replan(&snapshot, &old, &generated, 1).unwrap();
        assert!(
            replanned["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["nodeId"] != "world-r3-old")
        );
        for key in [
            "world_revision",
            "world_refinement",
            "world_audits",
            "active_world_ids",
            "resume_mode",
        ] {
            assert_eq!(replanned[key], old[key]);
        }
        let mut lost = old.clone();
        lost.as_object_mut().unwrap().remove("world_revision");
        assert_eq!(
            replan(&snapshot, &lost, &generated, 1).unwrap()["world_revision"],
            3
        );
        let mut active = snapshot.clone();
        active["nodes"][2]["archived"] = json!(false);
        assert!(
            core::plan(active["nodes"].as_array().unwrap()).unwrap()["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["nodeId"] != "world-r3-old")
        );
    }

    #[test]
    fn evidence_as_exploration_parent_is_rejected_atomically_and_repairable() {
        let snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"evidence-id","kind":"evidence","statement":"Support experiment","edges":"[]"}]});
        let mut generated = batch("h_ai_makes_customer_service_more_scripted");
        generated["hypotheses"][0]["parent"] = json!("ref_0001");
        let old = json!({"round":0,"http_calls":6,"transition_count":45,"started_at_ms":"123","results":{"evidence-id":{"classify_gap":"none"}}});
        let mut actual = snapshot.clone();
        let error = expand(&mut actual, &generated, "explore", &old).unwrap_err();
        assert_eq!(actual, snapshot);
        assert!(error.contains("h_ai_makes_customer_service_more_scripted"));
        assert!(error.contains("evidence-id (kind: evidence)"));
        let first = exploration_correction("explore", &old, &error, "{}").unwrap();
        assert!(
            !first["response_correction"]["instruction"]
                .as_str()
                .unwrap()
                .contains("premise")
        );
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(&first[key], value);
        }
        let second = exploration_correction("explore", &first, &error, "{}").unwrap();
        assert!(
            exploration_correction("explore", &second, &error, "{}")
                .unwrap_err()
                .contains("after two corrective attempts")
        );
        generated["hypotheses"][0]["parent"] = Value::Null;
        generated["hypotheses"][0]["requires"] = json!(["ref_0001"]);
        expand(&mut actual, &generated, "explore", &first).unwrap();
        assert_eq!(actual["nodes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn challenge_correction_preserves_work_and_exhausts_without_resetting_budget() {
        let old = json!({"round":3,"http_calls":42,"transition_count":102,"results":{"h":{"estimate_likelihood":"0.3"}},"independent_challenge":{"status":"pending"}});
        let first =
            exploration_correction("challenge", &old, "Unknown prior hypothesis: missing", "{}")
                .unwrap();
        assert_eq!(first["response_correction"]["attempt"], 1);
        assert_eq!(
            first["response_correction"]["validation_error"],
            "Unknown prior hypothesis: missing"
        );
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(&first[key], value);
        }
        let second =
            exploration_correction("challenge", &first, "Missing alternative group", "{}").unwrap();
        assert_eq!(second["response_correction"]["attempt"], 2);
        assert!(
            exploration_correction("challenge", &second, "Still invalid", "{}")
                .unwrap_err()
                .contains("after two corrective attempts")
        );
    }

    #[test]
    fn new_baseline_rejects_invented_limits_but_preserves_historical_reads() {
        let mut snapshot = json!({"world":{"description":"How will people get around cities in 2030?","last_ingest_date":"2026-10-01"},"nodes":[]});
        let restriction = "2030 means what a city resident notices in ordinary travel by 2030-12-31, not a complete replacement of today’s systems.";
        let baseline =
            json!({"as_of":"2026-10-01","observed":[],"assumptions":[restriction],"unknowns":[]});
        outlook::validate_baseline(&baseline, &snapshot).unwrap();
        assert!(
            outlook::validate_new_baseline(&baseline, &snapshot)
                .unwrap_err()
                .contains("verbatim")
        );
        snapshot["world"]["description"] = json!(format!(
            "How will people get around cities in 2030? {restriction}"
        ));
        outlook::validate_new_baseline(&baseline, &snapshot).unwrap();
        let mut reply = json!({"baseline":baseline,"scope_review":{"requested_question":snapshot["world"]["description"],"evidence_scope":"No current observations","narrowing_basis":"none","status":"aligned","limitations":[]}});
        establish_baseline(&snapshot, &reply, &json!({})).unwrap();
        snapshot["world"]["description"] = json!("How will people get around cities in 2030?");
        reply["scope_review"]["requested_question"] = snapshot["world"]["description"].clone();
        assert!(
            establish_baseline(&snapshot, &reply, &json!({}))
                .unwrap_err()
                .contains("verbatim")
        );
    }

    #[test]
    fn challenge_can_introduce_and_evaluate_a_new_paired_premise() {
        let mut snapshot = json!({"world":{"hindcast_mode":"false","last_ingest_date":"2026-10-01","target_date":"2030-12-31"},"nodes":[{"Id":"old","kind":"scenario","statement":"The current arrangement grows","edges":"[]"}]});
        let old = json!({"baseline_status":"established","results":{"old":{"classify_temporal":"future_change","classify_gap":"evidence","decision_value":"3"}}});
        let generated = json!({
            "hypotheses":[
                {"id":"premise","statement":"A different mechanism becomes available","requires":[]},
                {"id":"on","statement":"People reorganize the activity","requires":[],"branch_id":"new-on"},
                {"id":"off","statement":"People develop another workaround","requires":[],"branch_id":"new-off"}
            ],
            "branches":[
                {"id":"new-on","parent_branch_id":null,"condition":{"kind":"all_occurring","event_ids":["premise"]},"by":"2030-12-31"},
                {"id":"new-off","parent_branch_id":null,"condition":{"kind":"not_all_occurring","event_ids":["premise"]},"by":"2030-12-31"}
            ],
            "premises_challenged":[{"assumption":"The old arrangement remains necessary","alternative":"Another mechanism changes the activity","prior_hypothesis_ids":["old"],"alternative_hypothesis_ids":["premise","on","off"]}],
            "research_evidence":[],"continue_exploring":true,"exploration_note":"Investigate a premise outside the ranked event"
        });
        expand(&mut snapshot, &generated, "challenge", &old).unwrap();
        let mut program = replan(&snapshot, &old, &generated, 3).unwrap();
        for id in ["r1-premise", "r1-on", "r1-off"] {
            let node = snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["Id"] == id)
                .unwrap();
            assert_eq!(node["provenance"], "generated_hypothesis");
            assert_eq!(node["source_refs"], "[]");
            assert!(
                program["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["nodeId"] == id && t["function"] == "classify_temporal")
            );
        }
        for (id, sign) in [("r1-on", "all_occurring"), ("r1-off", "not_all_occurring")] {
            let node = snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["Id"] == id)
                .unwrap();
            assert_eq!(node["branch_state"]["conditions"][0]["kind"], sign);
            assert_eq!(
                node["branch_state"]["conditions"][0]["events"][0]["id"],
                "r1-premise"
            );
            assert!(
                program["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["nodeId"] == id && t["function"] == "estimate_conditional")
            );
            program["results"][id]["classify_claim_role"] = json!("event");
            program["results"][id]["classify_temporal"] = json!("future_change");
            assert!(!core::branches::future_eligible(&snapshot, &program, id));
        }
        program["results"]["r1-premise"]["classify_claim_role"] = json!("event");
        program["results"]["r1-premise"]["classify_temporal"] = json!("future_change");
        assert!(core::branches::future_eligible(
            &snapshot, &program, "r1-on"
        ));
        assert!(core::branches::future_eligible(
            &snapshot, &program, "r1-off"
        ));
        record_challenge(&snapshot, 1, &generated, &old, &mut program).unwrap();
        let receipt = &program["independent_challenge"];
        assert!(receipt.get("recommended_causal_rollout").is_none());
        assert!(receipt.get("recommended_rollout_adopted").is_none());
        assert!(receipt.get("causal_rollout").is_none());
        assert_eq!(receipt["branches"], snapshot["branches"]);
        assert_eq!(
            receipt["branches"][0]["condition"]["event_ids"],
            json!(["r1-premise"])
        );
    }

    #[test]
    fn challenge_research_updates_sources_without_turning_findings_into_hypotheses() {
        let before = json!({"world":{"hindcast_mode":"false","evidence_contract":"v1","description":"Question","last_ingest_date":"2026-10-01"},"nodes":[{"Id":"old","kind":"scenario","statement":"Old framing","edges":"[]"}]});
        let mut reply = batch("alternative");
        reply["premises_challenged"] = json!([{"assumption":"The service is new","alternative":"Existing adoption changes its downstream consequences","prior_hypothesis_ids":["ref_0001"],"alternative_hypothesis_ids":["alternative"]}]);
        reply["hypotheses"][0]["requires"] = json!(["adoption"]);
        reply["research_evidence"] = json!([{"id":"adoption","statement":"A measured portion already uses the service","url":"https://example.org/adoption","quote":"Measured adoption","evidence_metadata":{"kind":"finding","publication_date":"2025","observation_period":{"start":null,"end":null},"retrieved_at":"2026-10-01"},"provenance":"observed"}]);
        reply["baseline"] = json!({"as_of":"2026-10-01","observed":[{"claim":"A measured portion already uses the service","evidence_ids":["adoption"]}],"assumptions":[],"unknowns":["Future adoption remains unknown"]});
        reply["scope_review"] = json!({"requested_question":"Question","evidence_scope":"Limited adoption evidence","status":"narrowed","narrowing_basis":"evidence_availability","limitations":["Future adoption remains unknown"]});
        let old = json!({"round":0,"baseline":{"observed":[],"unknowns":["No finding"]}});
        let mut after = before.clone();
        let refresh = expand_with_baseline(&mut after, &reply, "challenge", &old)
            .unwrap()
            .unwrap();
        let edges = core::parse(after["nodes"][2]["edges"].as_str().unwrap()).unwrap();
        assert_eq!(edges[0]["to_id"], "r1-adoption");
        assert_eq!(edges[0]["kind"], "supports");
        assert_eq!(
            refresh["baseline"]["observed"][0]["evidence_ids"],
            json!(["r1-adoption"])
        );
        let mut program = replan(&after, &old, &reply, 2).unwrap();
        record_challenge(&after, 1, &reply, &old, &mut program).unwrap();
        assert_eq!(
            program["independent_challenge"]["added_hypothesis_ids"],
            json!(["r1-alternative"])
        );
        assert_eq!(
            program["independent_challenge"]["added_evidence_ids"],
            json!(["r1-adoption"])
        );
        for bad_url in ["http://example.org/adoption", ""] {
            let mut bad = reply.clone();
            bad["research_evidence"][0]["url"] = json!(bad_url);
            let mut unchanged = before.clone();
            assert!(expand(&mut unchanged, &bad, "challenge", &old).is_err());
            assert_eq!(unchanged, before);
        }
        let mut missing_baseline = reply.clone();
        missing_baseline.as_object_mut().unwrap().remove("baseline");
        let mut unchanged = before.clone();
        assert!(
            expand_with_baseline(&mut unchanged, &missing_baseline, "challenge", &old).is_err()
        );
        assert_eq!(unchanged, before);
        let mut frozen = before.clone();
        frozen["world"]["hindcast_mode"] = json!("true");
        let unchanged = frozen.clone();
        assert!(expand(&mut frozen, &reply, "challenge", &old).is_err());
        assert_eq!(frozen, unchanged);
    }

    #[test]
    fn contrastive_challenge_preserves_exact_prior_and_new_alternative_relationships() {
        let snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"old","kind":"scenario","statement":"Old framing","edges":"[]"},{"Id":"e","kind":"evidence","statement":"Observed fact","edges":"[]"}]});
        let mut generated = batch("alternative");
        generated["premises_challenged"] = json!([{"assumption":"The current workflow remains necessary","alternative":"A different mechanism performs its purpose","prior_hypothesis_ids":["ref_0001"],"alternative_hypothesis_ids":["alternative"]}]);
        generated["hypotheses"][0]["requires"] = json!(["ref_0002"]);
        let mut updated = snapshot.clone();
        expand(&mut updated, &generated, "challenge", &json!({})).unwrap();
        assert_eq!(updated["nodes"][0], snapshot["nodes"][0]);
        let added = &updated["nodes"][2];
        assert_eq!(
            core::parse(added["edges"].as_str().unwrap()).unwrap()[0]["to_id"],
            "e"
        );
        let mut program = replan(&updated, &json!({}), &generated, 1).unwrap();
        record_challenge(&updated, 2, &generated, &json!({}), &mut program).unwrap();
        assert_eq!(
            program["independent_challenge"]["trigger"],
            "candidate_generation_reported_saturation"
        );
        for trigger in [
            "reserved_before_next_exploration",
            "reserved_transition_window",
        ] {
            let old = json!({"independent_challenge":{"status":"pending","trigger":trigger}});
            record_challenge(&updated, 2, &generated, &old, &mut program).unwrap();
            assert_eq!(program["independent_challenge"]["trigger"], trigger);
        }

        assert_eq!(
            program["independent_challenge"]["premises_challenged"][0]["prior_hypothesis_ids"],
            json!(["old"])
        );
        assert_eq!(
            program["independent_challenge"]["premises_challenged"][0]["alternative_hypothesis_ids"],
            json!(["r1-alternative"])
        );
        let id = added["Id"].as_str().unwrap();
        assert!(
            program["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["nodeId"] == id && task["function"] == "estimate_likelihood")
        );
        assert!(program["results"][id]["estimate_likelihood"].is_null());
        assert_eq!(
            program["independent_challenge"]["added_hypothesis_ids"],
            json!([id])
        );
        let next = replan(&updated, &program, &batch("later"), 0).unwrap();
        assert_eq!(
            next["independent_challenge"],
            program["independent_challenge"]
        );
        for invalid in ["ref_9999", "invented"] {
            let mut bad = generated.clone();
            bad["hypotheses"][0]["requires"] = json!([invalid]);
            assert!(expand(&mut snapshot.clone(), &bad, "challenge", &json!({})).is_err());
        }
        for (field, invalid) in [
            ("prior_hypothesis_ids", json!(["ref_0002"])),
            ("prior_hypothesis_ids", json!(["missing"])),
            ("prior_hypothesis_ids", json!([])),
            ("alternative_hypothesis_ids", json!(["old"])),
            ("alternative_hypothesis_ids", json!(["missing"])),
            (
                "alternative_hypothesis_ids",
                json!(["alternative", "alternative"]),
            ),
        ] {
            let mut bad = generated.clone();
            bad["premises_challenged"][0][field] = invalid;
            let mut unchanged = snapshot.clone();
            assert!(expand(&mut unchanged, &bad, "challenge", &json!({})).is_err());
            assert_eq!(
                unchanged, snapshot,
                "Invalid relationship must fail atomically"
            );
        }
        let mut linked = generated.clone();
        linked["hypotheses"].as_array_mut().unwrap().push(json!({"id":"consequence","statement":"A subsequent future consequence","requires":["alternative"],"parent":"alternative"}));
        let mut unchanged = snapshot.clone();
        assert!(expand(&mut unchanged, &linked, "challenge", &json!({})).is_err());
        assert_eq!(unchanged, snapshot);
        linked["premises_challenged"][0]["alternative_hypothesis_ids"] =
            json!(["alternative", "consequence"]);
        assert!(expand(&mut snapshot.clone(), &linked, "challenge", &json!({})).is_ok());
        let mut hidden = snapshot.clone();
        hidden["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"hidden-world","kind":"world"}));
        let mut bad = generated.clone();
        bad["hypotheses"][0]["requires"] = json!(["ref_0003"]);
        let original = hidden.clone();
        assert!(expand(&mut hidden, &bad, "challenge", &json!({})).is_err());
        assert_eq!(hidden, original);
        let mut empty = generated.clone();
        empty["hypotheses"] = json!([]);
        empty["premises_challenged"] = json!([]);
        assert!(expand(&mut snapshot.clone(), &empty, "challenge", &json!({})).is_ok());
    }

    #[test]
    fn world_context_uses_only_canonical_components_counters_and_evidence() {
        let (mut snapshot, generated, old) = world_fixture();
        let mut program = compose(&mut snapshot, &generated, &old).unwrap();
        program["world_refinement"]["world-r1-one"] = json!({"rounds":[
            {"evidence_ids":["e","e"]}, {"evidence_ids":["e"]}
        ]});
        let mut answer = json!({"schema":"foresight-worlds-v3","outcomes":[{
            "world_id":"world-r1-one","scenario_ids":["invented-structural-check"]
        }]});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(
            answer["outcomes"][0]["scenario_ids"],
            json!(["a", "b", "c", "d", "e"])
        );
        program["world_refinement"]["world-r1-one"]["rounds"][0]["evidence_ids"] =
            json!(["unknown-source"]);
        assert_eq!(
            attach_world_probabilities(&mut answer, &program, &snapshot).unwrap_err(),
            "Unknown canonical world context reference"
        );
        answer["outcomes"][0]["world_id"] = json!("unknown-world");
        assert_eq!(
            attach_world_probabilities(&mut answer, &program, &snapshot).unwrap_err(),
            "Outcome must reference a composed world"
        );
    }

    #[test]
    fn final_answer_preserves_omitted_originals_and_selected_route_failures() {
        let (mut snapshot, generated, old) = world_fixture();
        let mut program = compose(&mut snapshot, &generated, &old).unwrap();
        program["world_search_contract"] = json!(1);
        let original = json!({"id":"omitted","title":"Unreached world","original_statement":"A bold original outcome","original_narrative":"Its exact imagined everyday life","commitments":[{"id":"c","statement":"An unchanged commitment"}],"signals":["Signal"],"falsifiers":["Failure"],"status":"unresolved"});
        program["endpoint_search"] = json!({"endpoints":[{"id":"kept"},original],"routes":[{"id":"route-a","endpoint_id":"kept","commitment_id":"c","status":"blocked","root_connections":[{"component_id":"a","evidence_ids":["e"],"mechanism":"An unsupported proposed bridge"}],"audit":{"status":"conflicts_found","checks":[]}}]});
        program["unreconstructed_endpoints"] =
            json!([{"endpoint_id":"omitted","reason":"No connected path was found"}]);
        let world = snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["Id"] == "world-r1-one")
            .unwrap();
        world["endpoint_id"] = json!("kept");
        world["selected_route_ids"] = json!(["route-a"]);
        for task in core::search::world_tasks(world) {
            program["results"][core::field(&task, "nodeId")][core::field(&task, "function")] =
                json!(if task["function"] == "estimate_likelihood" {
                    "0.23"
                } else {
                    "supported"
                });
        }
        let mut answer = json!({"schema":"foresight-worlds-v3","outcomes":[{"world_id":"world-r1-one"}],"unreconstructed_endpoints":[{"reason":"writer invented reason"}]});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(
            answer["unreconstructed_endpoints"][0]["original_endpoint"],
            original
        );
        assert_eq!(
            answer["unreconstructed_endpoints"][0]["reason"],
            "No connected path was found"
        );
        assert_eq!(answer["outcomes"][0]["audit"]["status"], "conflicts_found");
        assert_eq!(
            answer["outcomes"][0]["audit"]["selected_routes"]["status"],
            "blocked"
        );
        assert_eq!(answer["outcomes"][0]["probability"], 0.23);
        program["endpoint_search"]["routes"][0]["status"] = json!("unresolved");
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(answer["outcomes"][0]["audit"]["status"], "uncertain");
        assert_eq!(
            answer["outcomes"][0]["audit"]["selected_routes"]["status"],
            "unresolved"
        );
        program["endpoint_search"]["routes"] = json!([]);
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        let missing = &answer["outcomes"][0]["audit"]["selected_routes"]["routes"][0];
        assert_eq!(missing["route_id"], "route-a");
        assert_eq!(missing["status"], "unresolved");
        assert!(missing["commitment_id"].is_null());
        program["unreconstructed_endpoints"] = json!([]);
        assert!(
            attach_world_probabilities(&mut answer, &program, &snapshot)
                .unwrap_err()
                .contains("without a recorded reconstruction limit")
        );
    }

    #[test]
    fn legacy_synthesis_preserves_outcomes_without_fabricating_comparison_fields() {
        let (mut snapshot, generated, old) = world_fixture();
        let program = compose(&mut snapshot, &generated, &old).unwrap();
        for node in snapshot["nodes"].as_array_mut().unwrap() {
            node.as_object_mut().unwrap().remove("shared_question");
            node.as_object_mut().unwrap().remove("trajectory_answer");
        }
        let mut answer = json!({"schema":"foresight-worlds-v3","outcomes":[
            {"world_id":"world-r1-one","narrative":"Original first narrative"},
            {"world_id":"world-r1-two","narrative":"Original second narrative"}
        ]});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        let original = answer.clone();
        answer["outcomes"][0]["shared_question"] = json!("Invented comparison");
        answer["outcomes"][0]["trajectory_answer"] = json!("Invented trajectory");
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(answer, original);
        assert!(answer["outcomes"][0].get("shared_question").is_none());
        assert!(answer["outcomes"][0].get("trajectory_answer").is_none());
    }

    #[test]
    fn worlds_are_evaluated_fresh_and_never_inherit_component_probabilities() {
        let (mut snapshot, generated, old) = world_fixture();
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":"recent","kind":"research_evidence","statement":"Recent extracted claim","quote":"Actual source excerpt","edges":"[]"}));
        let mut program = compose(&mut snapshot, &generated, &old).unwrap();
        assert!(program["tasks"].as_array().unwrap().len() > 4);
        assert!(
            program["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["function"] == "check_world_set"
                    || core::field(t, "world_id").starts_with("world-r1-")
                    || core::field(t, "nodeId").starts_with("world-r1-"))
        );
        assert!(program["results"]["world-r1-one"].is_null());
        program["cursor"] = json!(
            program["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| t["function"] == "estimate_likelihood")
                .unwrap()
        );
        let request = core::request(&snapshot, &program).unwrap();
        assert_eq!(request["state"]["counter_hypotheses"][0]["node"]["Id"], "d");
        assert_eq!(request["state"]["source_evidence"][0]["Id"], "e");
        assert_eq!(request["state"]["source_evidence"][1]["Id"], "recent");
        assert_eq!(
            request["state"]["source_evidence"][1]["quote"],
            "Actual source excerpt"
        );
        let request = core::request(&snapshot, &program).unwrap();
        assert!(
            request["questions"]["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("never average, multiply, inherit")
        );
        let mut answer = json!({"schema":"foresight-worlds-v3","outcomes":[{"world_id":"world-r1-one","probability":0.99},{"world_id":"world-r1-two","probability":0.88}]});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert!(answer["outcomes"][0]["probability"].is_null());
        assert_eq!(answer["evaluation_status"], "unavailable");
        program["results"]["world-r1-one"] = json!({"estimate_likelihood":"0.23"});
        attach_world_probabilities(&mut answer, &program, &snapshot).unwrap();
        assert_eq!(answer["outcomes"][0]["probability"], 0.23);
        assert_eq!(answer["evaluation_status"], "partial");
        assert!(
            answer["evaluation_note"]
                .as_str()
                .unwrap()
                .contains("unfinished checks")
        );
        assert_eq!(
            answer["outcomes"][0]["definition"],
            "A and B and C occur jointly"
        );
        assert_eq!(
            answer["outcomes"][0]["component_ids"],
            json!(["a", "b", "c"])
        );
        assert_eq!(
            answer["baseline"]["observed"][0]["evidence_ids"],
            json!(["e"])
        );
        answer["outcomes"][0]["world_id"] = json!("a");
        assert!(attach_world_probabilities(&mut answer, &program, &snapshot).is_err());
    }
    #[test]
    fn revised_worlds_archive_old_nodes_without_reusing_their_estimates() {
        let (mut snapshot, generated, old) = world_fixture();
        let mut first = compose(&mut snapshot, &generated, &old).unwrap();
        first["results"]["world-r1-one"] = json!({"estimate_likelihood":"0.23"});
        first["http_calls"] = json!(23);
        first["world_audits"] = json!({"world-r1-one":{"status":"challenged"}});
        first["combination_search"] = json!({"candidate_combinations":[["a","b","c"]]});
        let second = compose(&mut snapshot, &generated, &first).unwrap();
        assert_eq!(second["world_revision"], 2);
        assert_eq!(second["http_calls"], 23);
        assert_eq!(second["evidence_ids"], json!(["e"]));
        assert_eq!(
            second["active_world_ids"],
            json!(["world-r2-one", "world-r2-two"])
        );
        assert_eq!(
            second["results"]["world-r1-one"]["estimate_likelihood"],
            "0.23"
        );
        assert!(second["results"]["world-r2-one"].is_null());
        assert_eq!(second["world_audits"], first["world_audits"]);
        assert_eq!(second["combination_search"], first["combination_search"]);
        let nodes = snapshot["nodes"].as_array().unwrap();
        assert!(
            nodes
                .iter()
                .filter(|n| n["kind"] == "world" && n["revision"] == 1)
                .all(|n| n["archived"] == true)
        );
        let node = nodes.iter().find(|n| n["Id"] == "world-r2-one").unwrap();
        assert_eq!(node["facets"][0]["component_ids"], json!(["a"]));
        assert_eq!(node["chain"][0]["from_ids"], json!(["a"]));
        assert_eq!(node["chain"][0]["to_id"], "b");
        let mut answer =
            json!({"schema":"foresight-worlds-v3","outcomes":[{"world_id":"world-r1-one"}]});
        assert!(attach_world_probabilities(&mut answer, &second, &snapshot).is_err());
        answer["outcomes"][0]["world_id"] = json!("world-r2-one");
        attach_world_probabilities(&mut answer, &second, &snapshot).unwrap();
        assert!(answer["outcomes"][0]["probability"].is_null());
        assert_eq!(answer["outcomes"][0]["facets"], node["facets"]);
        assert_eq!(answer["outcomes"][0]["chain"], node["chain"]);
        assert_eq!(answer["outcomes"][0]["assumptions"], node["assumptions"]);
    }

    #[test]
    fn new_evidence_rechecks_same_claim_without_reusing_old_estimates() {
        let mut snapshot = json!({"nodes":[{"Id":"e","kind":"evidence","edges":"[]"},{"Id":"h","kind":"scenario","statement":"The same event","branch_id":"condition-a","edges":"[]"}]});
        let old = json!({"http_calls":17,"rounds":[],"evidence_ids":["e"],"results":{"h":{"classify_claim_role":"event","classify_gap":"evidence","estimate_likelihood":"0.4","estimate_conditional":"0.7","evaluate_novelty":"2"}},"evaluations":{"h":{"classify_gap":{"selected":"evidence"},"estimate_likelihood":{"probability":0.4},"estimate_conditional":{"probability":0.7}}}});
        let generated = json!({"continue_exploring":true,"exploration_note":"Investigate"});
        let unchanged = replan(&snapshot, &old, &generated, 0).unwrap();
        assert_eq!(unchanged["http_calls"], 17);
        assert_eq!(unchanged["results"]["h"]["estimate_likelihood"], "0.4");
        assert_eq!(unchanged["results"]["h"]["estimate_conditional"], "0.7");
        assert!(
            !unchanged["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "estimate_conditional")
        );
        assert!(
            !unchanged["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "estimate_likelihood")
        );
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"new-source","kind":"research_evidence","edges":"[]"}));
        let refreshed = replan(&snapshot, &old, &generated, 1).unwrap();
        assert_eq!(refreshed["http_calls"], 17);
        for function in [
            "classify_gap",
            "estimate_likelihood",
            "estimate_conditional",
        ] {
            assert!(refreshed["results"]["h"][function].is_null());
            assert!(refreshed["evaluations"]["h"][function].is_null());
            assert!(
                refreshed["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["nodeId"] == "h" && t["function"] == function)
            );
        }
        assert!(refreshed["results"]["h"]["evaluate_novelty"].is_null());
        assert_eq!(
            refreshed["historical_search_guidance"]["h"]["evaluate_novelty"]["result"],
            "2"
        );
        assert_eq!(
            refreshed["historical_search_guidance"]["h"]["evaluate_novelty"]["current"],
            false
        );
        assert!(
            !refreshed["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == "h" && t["function"] == "evaluate_novelty")
        );

        assert_eq!(refreshed["evidence_ids"], json!(["e", "new-source"]));
        assert_eq!(old["results"]["h"]["estimate_likelihood"], "0.4");
    }

    #[test]
    fn source_support_is_not_a_future_prerequisite() {
        let mut snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed","edges":"[]"},{"Id":"h","kind":"scenario","statement":"Future premise","edges":"[]"}]});
        let mut generated = batch("next");
        generated["hypotheses"][0]["requires"] = json!(["e", "h"]);
        expand(&mut snapshot, &generated, "explore", &json!({})).unwrap();
        let edges = core::parse(snapshot["nodes"][2]["edges"].as_str().unwrap()).unwrap();
        assert_eq!(
            edges,
            json!([{"kind":"supports","to_id":"e"},{"kind":"requires","to_id":"h"}])
        );
    }

    #[test]
    fn evidence_counter_failure_is_correctable_without_losing_evaluated_work() {
        let (snapshot, valid, mut old) = world_fixture();
        old["http_calls"] = json!(408);
        old["combination_search"] = json!({"tested_pairs":820});
        let mut rejected = valid.clone();
        // The live failure used a genuine evidence alias as a counter hypothesis.
        rejected["worlds"][0]["counter_ids"] = json!(["ref_0001"]);
        let mut candidate = snapshot.clone();
        let error = compose(&mut candidate, &rejected, &old).unwrap_err();
        assert!(error.contains("is evidence, not a future hypothesis"));
        assert_eq!(candidate, snapshot);
        let corrected = composition_correction(&old, &rejected.to_string(), &error).unwrap();
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(&corrected[key], value);
        }
        assert_eq!(
            corrected["composition_correction"]["validation_error"],
            error
        );
        let twice = composition_correction(&corrected, &rejected.to_string(), &error).unwrap();
        assert!(composition_correction(&twice, &rejected.to_string(), &error).is_err());
        let accepted = compose(&mut candidate, &valid, &twice).unwrap();
        assert!(accepted["composition_correction"].is_null());
        assert_eq!(accepted["http_calls"], old["http_calls"]);
        assert_eq!(accepted["combination_search"], old["combination_search"]);
    }

    #[test]
    fn shared_comparison_contract_is_required_and_reaches_set_audit() {
        let (snapshot, generated, old) = world_fixture();
        for location in ["shared_question", "trajectory_answer"] {
            let mut bad = generated.clone();
            if location == "shared_question" {
                bad[location] = Value::Null;
            } else {
                bad["worlds"][0][location] = json!("");
            }
            let mut unchanged = snapshot.clone();
            assert!(
                compose(&mut unchanged, &bad, &old)
                    .unwrap_err()
                    .contains(location)
            );
            assert_eq!(unchanged, snapshot);
        }
        let mut updated = snapshot.clone();
        let program = compose(&mut updated, &generated, &old).unwrap();
        let request = core::search::request(&updated, &program, &program["tasks"][0]).unwrap();
        assert_eq!(
            request["state"]["shared_question"],
            generated["shared_question"]
        );
        for (i, world) in request["state"]["proposed_worlds"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(world["shared_question"], generated["shared_question"]);
            assert_eq!(
                world["trajectory_answer"],
                generated["worlds"][i]["trajectory_answer"]
            );
        }
    }

    #[test]
    fn composition_rejects_hypothetical_observations_and_unknown_components_atomically() {
        let (snapshot, generated, old) = world_fixture();
        for (key, value) in [
            ("component_ids", json!(["ref_0002", "invented"])),
            ("component_ids", json!(["ref_0002", "ref_0002"])),
            ("counter_ids", json!(["ref_0001"])),
            ("counter_ids", json!(["ref_0004", "ref_0004"])),
            ("counter_ids", json!(["ref_0002"])),
        ] {
            let mut bad = generated.clone();
            bad["worlds"][0][key] = value;
            let mut candidate = snapshot.clone();
            assert!(compose(&mut candidate, &bad, &old).is_err());
            assert_eq!(candidate, snapshot);
        }
        let mut bad = generated.clone();
        bad["baseline"]["observed"][0]["evidence_ids"] = json!(["ref_0002"]);
        assert!(compose(&mut snapshot.clone(), &bad, &old).is_err());
        let mut stopped = old.clone();
        stopped["stop_reason"] = json!("provider_error");
        assert_eq!(
            compose(&mut snapshot.clone(), &generated, &stopped).unwrap()["stop_reason"],
            "provider_error"
        );
    }
    fn batch(id: &str) -> Value {
        json!({"hypotheses":[{"id":id,"statement":"A distinct hypothetical event","requires":["e"]}],"research_evidence":[],"continue_exploring":true,"exploration_note":"Explore another mechanism"})
    }
    #[test]
    fn invalid_parent_lineage_rejects_the_whole_batch() {
        let original = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","kind":"evidence","edges":"[]"}]});
        for parent in ["unknown", "ref_0001", "h"] {
            let mut snapshot = original.clone();
            let mut generated = batch("h");
            generated["hypotheses"][0]["parent"] = json!(parent);
            assert!(
                expand(&mut snapshot, &generated, "explore", &json!({})).is_err(),
                "parent {parent}"
            );
            assert_eq!(snapshot, original);
        }
        let mut snapshot = original.clone();
        let mut generated = batch("h");
        generated["hypotheses"][0]["parent"] = json!("other");
        generated["hypotheses"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"other","statement":"Another future","requires":[],"parent":"h"}));
        assert!(
            expand(&mut snapshot, &generated, "explore", &json!({}))
                .unwrap_err()
                .contains("Cyclic")
        );
        assert_eq!(snapshot, original);
    }

    #[test]
    fn same_batch_parent_resolves_exactly_even_when_child_appears_first() {
        let mut snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"existing-hypothesis","kind":"scenario","statement":"Original","edges":"[]"}]});
        let generated = json!({"hypotheses":[
            {"id":"hyp_ai_feature_geofencing_0148","statement":"AI feature providers restrict regions","requires":["hyp_eu_ai_scope_0147"],"parent":"hyp_eu_ai_scope_0147"},
            {"id":"hyp_eu_ai_scope_0147","statement":"AI feature providers face product regulation","requires":["ref_0001"],"parent":"ref_0001"}
        ],"research_evidence":[],"continue_exploring":true,"exploration_note":"A new implication extends a new mechanism"});
        expand(&mut snapshot, &generated, "explore", &json!({"round":14})).unwrap();
        assert_eq!(snapshot["nodes"][1]["parent"], "r15-hyp_eu_ai_scope_0147");
        assert_eq!(snapshot["nodes"][1]["kind"], "revision");
        assert_eq!(snapshot["nodes"][2]["parent"], "existing-hypothesis");
        let plan = core::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        assert_eq!(plan["issues"], json!([]));
        let deep_order: Vec<_> = plan["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["function"] == "classify_gap")
            .map(|t| core::field(t, "nodeId"))
            .collect();
        assert_eq!(
            deep_order,
            vec![
                "existing-hypothesis",
                "r15-hyp_eu_ai_scope_0147",
                "r15-hyp_ai_feature_geofencing_0148"
            ]
        );
    }

    #[test]
    fn short_references_resolve_exactly_and_mixed_uuid_still_fails() {
        let a = "en-01a0ba3d-11c5-79f1-b578-741b76950dee";
        let b = "en-01a0ba3d-13e3-7bf1-b2ad-8751edb87e9c";
        let original = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":a,"edges":"[]"},{"Id":b,"edges":"[]"}]});
        let mut s = original.clone();
        let mut g = batch("h");
        g["hypotheses"][0]["requires"] = json!(["ref_0001", "ref_0002"]);
        expand(&mut s, &g, "seed", &json!({})).unwrap();
        let edges: Value = serde_json::from_str(s["nodes"][2]["edges"].as_str().unwrap()).unwrap();
        assert_eq!(edges[0]["to_id"], a);
        assert_eq!(edges[1]["to_id"], b);
        let mut invalid = original.clone();
        g["hypotheses"][0]["requires"] = json!(["en-01a0ba3d-13e3-7bf1-b578-741b76950dee"]);
        assert!(expand(&mut invalid, &g, "seed", &json!({})).is_err());
        assert_eq!(invalid, original);
        g["hypotheses"][0]["requires"] = json!(["ref_9999"]);
        assert!(expand(&mut invalid, &g, "seed", &json!({})).is_err());
        g["hypotheses"][0]["requires"] = json!(["ref_0001"]);
        g["hypotheses"][0]["id"] = json!("ref_0002");
        assert!(expand(&mut invalid, &g, "seed", &json!({})).is_err());
    }
    #[test]
    fn parent_alias_preserves_exact_lineage_and_parent_assessment() {
        let mut snapshot = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","edges":"[]"},{"Id":"existing-hypothesis","kind":"scenario","edges":"[]"}]});
        let mut generated = batch("new-path");
        generated["hypotheses"][0]["requires"] = json!(["ref_0001"]);
        generated["hypotheses"][0]["parent"] = json!("ref_0002");
        expand(&mut snapshot,&generated,"explore",&json!({"round":1,"results":{"existing-hypothesis":{"classify_gap":"evidence","choose_next_operation":"challenge"}}})).unwrap();
        let revised = &snapshot["nodes"][2];
        assert_eq!(revised["parent"], "existing-hypothesis");
        assert_eq!(revised["kind"], "revision");
        assert_eq!(revised["before_gap"], "evidence");
        assert!(revised["operation"].is_null());
    }

    #[test]
    fn synthesis_alias_resolves_before_probability_attachment() {
        let snapshot = json!({"nodes":[{"Id":"actual-hypothesis"}]});
        let mut answer = json!({"schema":"foresight-outlook-v2","outcomes":[{"hypothesis_id":"ref_0001","scenario_ids":["ref_0001"]}]});
        references::References::new(&snapshot)
            .unwrap()
            .resolve_generated(&mut answer);
        attach_probabilities(
            &mut answer,
            &json!({"results":{"actual-hypothesis":{"estimate_likelihood":"0.37"}}}),
        )
        .unwrap();
        assert_eq!(answer["outcomes"][0]["hypothesis_id"], "actual-hypothesis");
        assert_eq!(
            answer["outcomes"][0]["scenario_ids"][0],
            "actual-hypothesis"
        );
        assert_eq!(answer["outcomes"][0]["probability"], 0.37);
    }

    #[test]
    fn repeated_rounds_preserve_history_and_evaluate_new_hypotheses() {
        let mut s = json!({"world":{"hindcast_mode":"false"},"nodes":[{"Id":"e","edges":"[]"}]});
        let original = s["nodes"][0].clone();
        let g = batch("h");
        expand(&mut s, &g, "seed", &json!({})).unwrap();
        let mut p = replan(&s, &json!({}), &g, 1).unwrap();
        p["evidence_ids"] = json!(["e"]);
        p["results"] = json!({"e":{"classify_gap":"none","choose_next_operation":"monitor"},"r1-h":{"classify_claim_role":"event","classify_temporal":"future_change","classify_gap":"evidence","estimate_likelihood":"0.37","evaluate_novelty":"0.8","decision_value":"0.7","choose_next_operation":"connect"}});
        expand(&mut s, &g, "explore", &p).unwrap();
        let next = replan(&s, &p, &g, 1).unwrap();
        assert_eq!(next["round"], 2);
        assert_eq!(next["tasks"].as_array().unwrap().len(), 6);
        assert_eq!(s["nodes"][0], original);
        let mut expected = p["results"].clone();
        for function in ["evaluate_novelty", "decision_value"] {
            expected["r1-h"].as_object_mut().unwrap().remove(function);
            assert_eq!(
                next["historical_search_guidance"]["r1-h"][function]["result"],
                p["results"]["r1-h"][function]
            );
        }
        assert_eq!(next["results"], expected);
    }
    #[test]
    fn empty_research_round_can_continue_without_false_convergence() {
        let s = json!({"nodes":[{"Id":"e","edges":"[]"}]});
        let p = replan(&s, &json!({}), &batch("h"), 0).unwrap();
        assert_eq!(p["continue_exploring"], true);
    }
    #[test]
    fn invented_reference_rejected_atomically() {
        let mut s = json!({"nodes":[{"Id":"e"}]});
        let original = s.clone();
        let mut g = batch("h");
        g["hypotheses"][0]["requires"] = json!(["invented"]);
        assert!(expand(&mut s, &g, "seed", &json!({})).is_err());
        assert_eq!(s, original);
    }
    #[test]
    fn probability_is_actual_jev_event_estimate() {
        let mut a = json!({"schema":"foresight-outlook-v2","outcomes":[{"hypothesis_id":"h","probability":0.99}]});
        attach_probabilities(
            &mut a,
            &json!({"results":{"h":{"estimate_likelihood":"0.37"}}}),
        )
        .unwrap();
        assert_eq!(a["outcomes"][0]["probability"], 0.37);
        assert!(attach_probabilities(&mut a, &json!({})).is_err());
    }
}
