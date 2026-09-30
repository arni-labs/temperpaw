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
    for premise in premises {
        bounded_text(&premise["assumption"], 600)?;
        bounded_text(&premise["alternative"], 1200)?;
    }
    if generated["research_evidence"]
        .as_array()
        .is_none_or(|reports| !reports.is_empty())
    {
        return Err("Independent challenge cannot add unresearched evidence".into());
    }
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
            if !new_ids.contains(reference) && !existing_ids.contains(reference) {
                return Err(format!(
                    "Challenge reference {reference} is outside its visible catalog and new hypotheses"
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
    program["independent_challenge"] = json!({
        "status":"completed","trigger":"candidate_generation_reported_saturation",
        "round":program["round"],"premises_challenged":generated["premises_challenged"],
        "added_hypothesis_ids":snapshot["nodes"].as_array().unwrap().iter().skip(before).map(|n|n["Id"].clone()).collect::<Vec<_>>(),
        "note":generated["exploration_note"],"accuracy_verified":false
    });
    Ok(())
}

fn expand(
    snapshot: &mut Value,
    generated: &Value,
    phase: &str,
    program: &Value,
) -> Result<(), String> {
    if !matches!(phase, "seed" | "explore" | "challenge") {
        return Err("Unknown exploration phase".into());
    }
    let mut generated = generated.clone();
    if phase == "challenge" {
        resolve_challenge(snapshot, &mut generated)?;
    }
    references::References::new(snapshot)?.resolve_generated(&mut generated);
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
        let id = format!("r{round}-{local}");
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

fn establish_baseline(snapshot: &Value, generated: &Value, old: &Value) -> Result<Value, String> {
    let mut generated = generated.clone();
    references::References::new(snapshot)?.resolve_generated(&mut generated);
    outlook::validate_baseline(&generated["baseline"], snapshot)?;
    let mut program = old.clone();
    program["baseline"] = generated["baseline"].clone();
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
    let baseline = if old["baseline"].is_object() {
        &old["baseline"]
    } else {
        &generated["baseline"]
    };
    outlook::validate_baseline(baseline, snapshot)?;
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let by_id: std::collections::BTreeMap<_, _> =
        nodes.iter().map(|n| (core::field(n, "Id"), n)).collect();
    bounded_text(&generated["shared_question"], 800)
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
    for world in worlds {
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
            bounded_text(&world[key], max)
                .map_err(|error| format!("World {local}: invalid {key}: {error}"))?;
        }
        for key in ["signals", "falsifiers"] {
            bounded_texts(&world[key], 1, 8, 240)?;
        }
        bounded_texts(&world["what_you_can_do"], 0, 4, 240)?;
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
                let classification = old["results"][reference]["classify_temporal"]
                    .as_str()
                    .unwrap_or("not evaluated in current evidence context");
                return Err(format!(
                    "World component {reference} is temporally ineligible: {classification}. Select only composition_candidates.component_ids; keep this node as context."
                ));
            }
            if !components.insert(reference) {
                return Err(format!("Duplicate world component {reference}"));
            }
        }
        if components.len() < 3 || components.len() > 12 {
            return Err("World needs three to twelve defining components".into());
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
        node["branch_conditions"] =
            core::branches::world_conditions(snapshot, &node["component_ids"])?;
        for clause in node["branch_conditions"].as_array().unwrap() {
            for event in clause["events"].as_array().unwrap() {
                if !core::temporal_allows_forecast(old, core::field(event, "id")) {
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
    tasks.insert(
        0,
        core::search::world_set_task(&identities.iter().map(|s| json!(s)).collect::<Vec<_>>()),
    );
    let mut program = core::plan(updated["nodes"].as_array().unwrap())?;
    for key in [
        "results",
        "evaluations",
        "round",
        "rounds",
        "baseline_status",
        "temporal_decomposition_requested",
        "last_error",
        "combination_search",
        "world_audits",
        "world_set_audits",
        "world_set_audit",
        "world_refinement",
        "independent_challenge",
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
    program["tasks"] = json!(tasks);
    program["active_world_ids"] = json!(identities);
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
fn bounded_text(value: &Value, max: usize) -> Result<(), String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= max)
        .map(|_| ())
        .ok_or("Missing or oversized world text".into())
}
fn bounded_texts(value: &Value, min: usize, max: usize, chars: usize) -> Result<(), String> {
    let values = value
        .as_array()
        .filter(|v| (min..=max).contains(&v.len()))
        .ok_or("Invalid world text list")?;
    for value in values {
        bounded_text(value, chars)?;
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
    let probability_warnings: Vec<String> = answer["outcomes"].as_array().into_iter().flatten()
        .flat_map(|o| o["audit"]["probability_coherence"]["findings"].as_array().into_iter().flatten())
        .map(|f| format!("Independent estimates conflict: whole world {:.1}% exceeds a required event at {:.1}%; raw estimates are unchanged, not calibrated.", f["joint_probability"].as_f64().unwrap()*100.0, f["component_probability"].as_f64().unwrap()*100.0)).collect();
    answer["world_set_audit"] = if program["world_set_audit"].is_object() {
        program["world_set_audit"].clone()
    } else {
        json!({"task_id":core::search::world_set_task(program["active_world_ids"].as_array().unwrap_or(&vec![]))["nodeId"],"revision":program["world_revision"],"world_ids":program["active_world_ids"],"verdict":"uncertain","evaluation":null,"correction_status":"unavailable"})
    };
    answer["baseline"] = program["baseline"].clone();
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
        "results",
        "evaluations",
        "baseline",
        "baseline_status",
        "temporal_decomposition_requested",
        "rounds",
        "http_calls",
        "transition_count",
        "independent_challenge",
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

fn exploration_correction(phase: &str, old: &Value, error: &str) -> Result<Value, String> {
    let attempt = old["response_correction"]["attempt"].as_u64().unwrap_or(0) + 1;
    if attempt > 2 {
        return Err(format!(
            "{phase} rejected after two corrective attempts: {error}"
        ));
    }
    let mut program = old.clone();
    let instruction = if phase == "challenge" {
        "The challenge response was not applied. Return the complete corrected challenge JSON against the unchanged visible catalog. Every premise must link existing prior hypotheses to new alternative hypotheses; every new hypothesis must be linked. Do not invent references, evidence, or evaluations. You may return empty premises and hypotheses with an honest explanation."
    } else {
        "The exploration response was not applied. Return the complete corrected exploration JSON against the unchanged visible catalog. Parent denotes hypothesis lineage and must reference a scenario/revision or a new hypothesis in this batch; source evidence is support, not a parent. Preserve the distinction between source observations and future hypotheses. Do not invent references or evaluations, and cite only research actually retrieved."
    };
    program["response_correction"] =
        json!({"attempt":attempt,"validation_error":error,"instruction":instruction});
    Ok(program)
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
            program["response_correction"] = json!({"attempt":attempt,
                "instruction":"The previous response was not a JSON object and was not applied. Return the complete JSON object required by this phase. Tool-call prose is not an executed tool call or a final result. Use actual tools if research is needed, then return the required JSON. Do not claim new evidence or judgments unless they were obtained."});
            Ok(Err(program))
        }
    }
}

fn run_inner(ctx: &Context) -> Result<(), String> {
    let phase = core::field(&ctx.entity_state, "phase");
    let old = core::parse(core::field(&ctx.entity_state, "program_json"))?;
    let mut snapshot = core::parse(core::field(&ctx.entity_state, "snapshot_json"))?;
    let response = core::field(&ctx.entity_state, "reasoning_result");
    let generated = match generated_response(response, &old) {
        Ok(Ok(value)) => value,
        Err(error) => {
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
    let old = core::parse(core::field(&ctx.entity_state, "program_json"))?;
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
        if let Err(error) = expand(&mut snapshot, &generated, phase, &old) {
            if !matches!(phase, "challenge" | "explore") {
                return Err(error);
            }
            let program = exploration_correction(phase, &old, &error)?;
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
        replan(&snapshot, &old, &core::parse(raw)?, added)?
    };
    if phase == "challenge" {
        record_challenge(&snapshot, before, &core::parse(raw)?, &mut program)?;
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
    use super::*;
    fn world_fixture() -> (Value, Value, Value) {
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-19","target_date":"2027-09-19"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed baseline","edges":"[]"},{"Id":"a","kind":"scenario","statement":"Component A","edges":"[]"},{"Id":"b","kind":"revision","statement":"Component B","edges":"[]"},{"Id":"c","kind":"scenario","statement":"Component C","edges":"[]"},{"Id":"d","kind":"scenario","statement":"Counter D","edges":"[]"}]});
        let world = json!({"id":"one","trajectory_answer":"One organization of the entire system","title":"A whole world","statement":"A and B and C occur jointly","mechanism":"A enables B enables C","component_ids":["ref_0002","ref_0003","ref_0004"],"counter_ids":["ref_0005"],"scene":"An imagined day","narrative":"A causes B and C but D may prevent it","what_you_can_do":[],"signals":["Observe A"],"falsifiers":["Observe D"],"facets":[{"id":"f1","title":"First change","description":"A changes daily life","component_ids":["ref_0002"]},{"id":"f2","title":"Second change","description":"B changes software","component_ids":["ref_0003"]},{"id":"f3","title":"Third change","description":"C changes economic choices","component_ids":["ref_0004"]}],"chain":[{"id":"l1","from_ids":["ref_0002"],"to_id":"ref_0003","mechanism":"A makes B possible","by":"2027-03-01"},{"id":"l2","from_ids":["ref_0003"],"to_id":"ref_0004","mechanism":"B enables C","by":"2027-09-01"}],"assumptions":["The mechanism persists"]});
        let mut second = world.clone();
        second["id"] = json!("two");
        second["trajectory_answer"] =
            json!("A different organization with different downstream consequences");
        let generated = json!({"shared_question":"How do the interacting constraints change the system?","baseline":{"as_of":"2026-09-19","observed":[{"claim":"Observed baseline","evidence_ids":["ref_0001"]}],"assumptions":[],"unknowns":[]},"worlds":[world,second]});
        let program = json!({"results":{"a":{"estimate_likelihood":"0.9"},"b":{"estimate_likelihood":"0.8"}},"evaluations":{},"rounds":[],"round":6,"stop_reason":"exploration_converged"});
        (snapshot, generated, program)
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
        let first = exploration_correction("explore", &old, &error).unwrap();
        assert!(
            !first["response_correction"]["instruction"]
                .as_str()
                .unwrap()
                .contains("premise")
        );
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(&first[key], value);
        }
        let second = exploration_correction("explore", &first, &error).unwrap();
        assert!(
            exploration_correction("explore", &second, &error)
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
            exploration_correction("challenge", &old, "Unknown prior hypothesis: missing").unwrap();
        assert_eq!(first["response_correction"]["attempt"], 1);
        assert_eq!(
            first["response_correction"]["validation_error"],
            "Unknown prior hypothesis: missing"
        );
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(&first[key], value);
        }
        let second =
            exploration_correction("challenge", &first, "Missing alternative group").unwrap();
        assert_eq!(second["response_correction"]["attempt"], 2);
        assert!(
            exploration_correction("challenge", &second, "Still invalid")
                .unwrap_err()
                .contains("after two corrective attempts")
        );
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
        record_challenge(&updated, 2, &generated, &mut program).unwrap();
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
        let old = json!({"http_calls":17,"rounds":[],"evidence_ids":["e"],"results":{"h":{"classify_gap":"evidence","estimate_likelihood":"0.4","estimate_conditional":"0.7","evaluate_novelty":"2"}},"evaluations":{"h":{"classify_gap":{"selected":"evidence"},"estimate_likelihood":{"probability":0.4},"estimate_conditional":{"probability":0.7}}}});
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
        p["results"] = json!({"e":{"classify_gap":"none","choose_next_operation":"monitor"},"r1-h":{"classify_temporal":"future_change","classify_gap":"evidence","estimate_likelihood":"0.37","evaluate_novelty":"0.8","decision_value":"0.7","choose_next_operation":"connect"}});
        expand(&mut s, &g, "explore", &p).unwrap();
        let next = replan(&s, &p, &g, 1).unwrap();
        assert_eq!(next["round"], 2);
        assert_eq!(next["tasks"].as_array().unwrap().len(), 5);
        assert_eq!(s["nodes"][0], original);
        assert_eq!(next["results"], p["results"]);
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
