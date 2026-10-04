// Bound a generation turn, not the ambition or number of original worlds.
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_COMMITMENTS: usize = 3;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub fn enabled(program: &Value) -> bool {
    matches!(
        program["endpoint_search"]["backward_batch_contract"].as_u64(),
        Some(1 | 2)
    )
}


/// Rank actual unresolved checks, never model-invented explanations. Shared
/// obligations merge only when the exact assessment input is identical.
pub fn repair_obligations(snapshot: &Value, program: &Value) -> Result<Vec<Value>, String> {
    use sha2::{Digest, Sha256};
    let mut obligations: std::collections::BTreeMap<String, Value> =
        std::collections::BTreeMap::new();
    let nodes = snapshot["nodes"].as_array().ok_or("Missing repair graph")?;
    let active_routes: BTreeSet<_> = nodes
        .iter()
        .filter(|node| {
            program["active_world_ids"]
                .as_array()
                .is_some_and(|ids| ids.contains(&node["Id"]))
        })
        .flat_map(|node| {
            node["selected_route_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    for route in program["endpoint_search"]["routes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !active_routes.is_empty() && !active_routes.contains(super::field(route, "id")) {
            continue;
        }

        let world_id = super::field(route, "world_node_id");
        let Some(world) = nodes.iter().find(|n| n["Id"] == world_id) else {
            continue;
        };
        if program["route_basis"][world_id]
            != super::endpoints::candidate_basis(snapshot, program, world_id)
        {
            continue;
        }
        for task in super::search::mandatory_audit_tasks(world, program) {
            let function = super::field(&task, "function");
            let result = &program["results"][super::field(&task, "nodeId")][function];
            if !matches!(result.as_str(), Some("gap" | "conflict" | "uncertain")) {
                continue;
            }
            let (kind, subject, bridge_ref) = if function == "check_route_grounding" {
                let roots: Vec<_> = route["root_connections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|root| root["evidence_ids"].as_array().is_none_or(Vec::is_empty))
                    .cloned()
                    .collect();
                // A whole-route uncertain result does not identify a particular
                // root. Keep its scope exact instead of inventing a culprit.
                (
                    "root_gap",
                    json!({"root_connections":if roots.is_empty(){route["root_connections"].clone()}else{json!(roots)}}),
                    Value::Null,
                )
            } else if function == "check_transition" {
                let Some(link) = world["chain"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|l| l["id"] == task["link_id"])
                else {
                    continue;
                };
                ("bridge", link.clone(), link["bridge_ref"].clone())
            } else {
                continue;
            };
            let request = super::evaluation::request_task(snapshot, program, &task)?;
            let fingerprint = super::endpoints::bridges::fingerprint(&request)
                .unwrap_or_else(|| format!("{:x}", Sha256::digest(request.to_string().as_bytes())));
            let key = format!("{kind}-{fingerprint}");
            let item=obligations.entry(key.clone()).or_insert_with(||json!({
                "obligation_id":key,"input_fingerprint":fingerprint,"kind":kind,"bridge_ref":bridge_ref,
                "subject":subject,"endpoint_commitments":[],"observed_results":[],"attempts":0,
                "semantics":"Recorded categorical result and raw distribution only; no generated Jev explanation. A root-group judgment does not identify which root is responsible."
            }));
            let commitment = json!({"endpoint_id":route["endpoint_id"],"commitment_id":route["commitment_id"],"route_id":route["id"]});
            if !item["endpoint_commitments"]
                .as_array()
                .unwrap()
                .contains(&commitment)
            {
                item["endpoint_commitments"]
                    .as_array_mut()
                    .unwrap()
                    .push(commitment);
            }
            item["observed_results"].as_array_mut().unwrap().push(json!({"task":task,"result":result,"evaluation":program["evaluations"][super::field(&task,"nodeId")][function]}));
        }
    }
    let mut queue: Vec<_> = obligations
        .into_values()
        .filter_map(|mut item| {
            let attempts: Vec<_> = program["targeted_repair_history"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|prior| prior["input_fingerprint"] == item["input_fingerprint"])
                .collect();
            if attempts
                .iter()
                .any(|prior| matches!(prior["status"].as_str(), Some("no_change" | "failed")))
            {
                return None;
            }
            item["attempts"] = json!(attempts.len());
            Some(item)
        })
        .collect();
    // Root repair changes a shared future prerequisite, so cover every selected
    // commitment that uses that exact root. These are separate route judgments,
    // not equal-input reuse and not a claim that Jev isolated the root as cause.
    for item in &mut queue {
        if item["kind"] != "root_gap" { continue; }
        let roots: BTreeSet<String> = item["subject"]["root_connections"].as_array().into_iter().flatten()
            .filter(|root| root["evidence_ids"].as_array().is_none_or(Vec::is_empty))
            .filter_map(|root| root["component_id"].as_str().map(str::to_owned)).collect();
        if roots.is_empty() { continue; }
        for route in program["endpoint_search"]["routes"].as_array().into_iter().flatten() {
            if !active_routes.contains(super::field(route,"id")) { continue; }
            let world_id=super::field(route,"world_node_id");
            if program["route_basis"][world_id] != super::endpoints::candidate_basis(snapshot,program,world_id) { continue; }
            if !route["root_connections"].as_array().into_iter().flatten().any(|r| roots.contains(super::field(r,"component_id"))) { continue; }
            let commitment=json!({"endpoint_id":route["endpoint_id"],"commitment_id":route["commitment_id"],"route_id":route["id"]});
            if !item["endpoint_commitments"].as_array().unwrap().contains(&commitment) {
                item["endpoint_commitments"].as_array_mut().unwrap().push(commitment);
            }
        }
        item["scope_semantics"]=json!("All selected commitments using an identical ungrounded root are repaired together. Recorded route judgments remain separate; this scope is not an inferred explanation or probability.");
    }
    queue.sort_by_key(|item| {
        let explicit_root = item["kind"] == "root_gap"
            && item["observed_results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| matches!(r["result"].as_str(), Some("gap" | "conflict")));
        let affected: BTreeSet<_> = item["endpoint_commitments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    super::field(c, "endpoint_id"),
                    super::field(c, "commitment_id"),
                )
            })
            .collect();
        (
            !explicit_root,
            std::cmp::Reverse(affected.len()),
            item["attempts"].as_u64().unwrap_or(0),
            super::field(item, "obligation_id").to_owned(),
        )
    });
    Ok(queue)
}


/// One prospective repair reserves revalidation of current work, a bounded
/// generation, and another full finalization in the original run allowance.
pub fn repair_admission(
    snapshot: &Value,
    program: &Value,
    elapsed_ms: u64,
) -> Result<Value, String> {
    // This patch freezes existing semantic context. Only genuinely pending work
    // belongs in the fixed cost; exact new-route contexts are priced on receipt.
    let work = super::endpoints::pending_mandatory_work(snapshot, program)?;
    let fixed = work["required_transitions"]
        .as_u64()
        .ok_or("Missing repair work cost")?;
    let ids = program["active_world_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut tasks = Vec::new();
    for id in &ids {
        let world = snapshot["nodes"]
            .as_array()
            .and_then(|nodes| nodes.iter().find(|node| node["Id"] == *id))
            .ok_or("Missing assessed world for repair finalization")?;
        tasks.extend(super::search::audit_tasks(world, program));
    }
    tasks.extend(super::search::world_set_tasks(&ids));
    let final_audit = super::search::refinement_admission(snapshot, program, &tasks);
    let audit_cost = final_audit["required_transitions"]
        .as_u64()
        .ok_or("Cannot reserve repair finalization audit")?;
    let finalization = super::finalization_transition_reserve(audit_cost);
    let finalization_ms = super::WORLD_TIME_RESERVE_MS.max(
        super::generation_duration(program, "compose")
            .saturating_add(super::presentation_time_reserve(program)),
    );
    let required = fixed + super::REASONING_ADMISSION_RESERVE + finalization + 16;
    let remaining = super::MAX_APP_TRANSITIONS.saturating_sub(super::transition_count(program));
    let generation = super::generation_duration(program, "backward");
    let time_ok = elapsed_ms
        .saturating_add(generation)
        .saturating_add(super::ENDPOINT_EVALUATION_DRAIN_MS)
        .saturating_add(finalization_ms)
        < super::MAX_MS;
    Ok(
        json!({"admitted":time_ok&&required<=remaining,"reason":if !time_ok{"time_budget"}else if required>remaining{"transition_budget"}else{"reserved"},"required_transitions":required,"remaining_transitions":remaining,"existing_graph_revalidation_transitions":fixed,"new_work_minimum_reserve":16,"generation_transition_reserve":super::REASONING_ADMISSION_RESERVE,"finalization_transition_reserve":finalization,"finalization_time_reserve_ms":finalization_ms,"finalization_audit":final_audit,"generation_ms":generation,"elapsed_ms":elapsed_ms,"original_deadline_ms":super::MAX_MS,"evaluation_transition_capacity":remaining.saturating_sub(super::REASONING_ADMISSION_RESERVE+finalization),"patch_contract":"append_only_unchanged_evidence_v1","output_cost_known":false,"semantics":"Unchanged existing assessment context is reused exactly. New repair output must fit the same remaining capacity before application; no acceptance, timing guarantee or fresh allowance is implied."}),
    )
}

/// Price the exact prospective replacement bundle, without asserting that its
/// unperformed checks pass or mutating the currently displayed worlds.
pub fn patch_finalization(snapshot: &Value, old: &Value, program: &Value) -> Result<Value, String> {
    let mut worlds = Vec::new();
    let routes = program["endpoint_search"]["routes"]
        .as_array()
        .ok_or("Missing patch routes")?;
    for id in old["active_world_ids"].as_array().into_iter().flatten() {
        let mut world = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["Id"] == *id)
            .ok_or("Missing saved world")?
            .clone();
        for selected in world["selected_route_ids"]
            .as_array_mut()
            .ok_or("Missing saved route selection")?
        {
            let replacements: Vec<_> = routes
                .iter()
                .filter(|r| {
                    r["alternative_to"] == *selected
                        && !old["endpoint_search"]["routes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|prior| prior["id"] == r["id"])
                })
                .collect();
            if replacements.len() > 1 {
                return Err("Patch must select one alternative per affected original route".into());
            }
            if let Some(replacement) = replacements.first() {
                *selected = replacement["id"].clone();
            }
        }
        for key in ["statement", "component_ids", "chain", "commitment_bindings"] {
            world.as_object_mut().unwrap().remove(key);
        }
        worlds.push(world);
    }
    let mut generated = json!({"worlds":worlds});
    super::endpoints::preserve_omitted_originals(program, &mut generated);
    super::endpoints::assemble_composition(program, &mut generated)?;
    let mut projected = snapshot.clone();
    let mut tasks = Vec::new();
    for world in generated["worlds"].as_array().unwrap() {
        let node = projected["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["Id"] == world["Id"])
            .unwrap();
        *node = world.clone();
        tasks.extend(super::search::audit_tasks(node, program));
    }
    tasks.extend(super::search::world_set_tasks(
        old["active_world_ids"]
            .as_array()
            .ok_or("Missing active worlds")?,
    ));
    let mut audit_program = program.clone();
    // Price the eventual world executor, not the current candidate stage. This
    // affects admission eligibility only and records no completed assessment.
    audit_program["stage"] = json!("worlds");
    let audit = super::search::refinement_admission(&projected, &audit_program, &tasks);
    let cost = audit["required_transitions"].as_u64().ok_or_else(|| {
        format!(
            "Cannot price patched finalization: {}",
            audit["planning_error"]
        )
    })?;
    Ok(
        json!({"audit":audit,"required_transitions":super::finalization_transition_reserve(cost),"semantics":"Prospective exact route union; no check has been performed by this estimate"}),
    )
}

/// Deterministic from persisted state: retries select the same obligations.
/// Missing commitments precede alternatives; least explored alternatives go first.
pub fn batch(snapshot: &Value, program: &Value) -> Value {
    if matches!(
        program["targeted_repair"]["status"].as_str(),
        Some("admitted" | "generating")
    ) {
        let obligation = &program["targeted_repair"]["obligation"];
        let mut selected = BTreeSet::new();
        let commitments:Vec<_>=obligation["endpoint_commitments"].as_array().into_iter().flatten().filter(|c|selected.insert((super::field(c,"endpoint_id"),super::field(c,"commitment_id")))).map(|c| {
            let statement=program["endpoint_search"]["endpoints"].as_array().into_iter().flatten().find(|e|e["id"]==c["endpoint_id"]).and_then(|e|e["commitments"].as_array()).and_then(|cs|cs.iter().find(|item|item["id"]==c["commitment_id"])).map(|c|c["statement"].clone()).unwrap_or(Value::Null);
            json!({"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"statement":statement,"alternative_to":c["route_id"],"reason":"targeted_recorded_gap","repair_fingerprint":obligation["input_fingerprint"]})
        }).collect();
        return json!({"contract":program["endpoint_search"]["backward_batch_contract"],"mode":"alternatives","commitments":commitments,"repair_obligation":obligation,"patch_contract":program["targeted_repair"]["admission"]["patch_contract"],"limits":{"routes":commitments.len(),"hypotheses":24,"research_evidence":8,"response_bytes":MAX_RESPONSE_BYTES}});
    }
    let routes: Vec<_> = program["endpoint_search"]["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let mut obligations = vec![];
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for commitment in endpoint["commitments"].as_array().into_iter().flatten() {
            let related: Vec<_> = routes
                .iter()
                .filter(|r| {
                    r["endpoint_id"] == endpoint["id"] && r["commitment_id"] == commitment["id"]
                })
                .collect();
            let unresolved = related.iter().find(|r| {
                matches!(r["status"].as_str(), Some("blocked" | "unresolved"))
                    && !routes.iter().any(|a| a["alternative_to"] == r["id"])
            });
            let priority = if related.is_empty() {
                0
            } else if unresolved.is_some() {
                1
            } else {
                2
            };
            obligations.push((priority, related.len(), json!({"endpoint_id":endpoint["id"],"commitment_id":commitment["id"],"statement":commitment["statement"],"alternative_to":unresolved.map(|r|r["id"].clone()),"reason":if priority == 0 {"missing_route"} else if priority == 1 {"unresolved_route"} else {"further_alternative"}})));
        }
    }
    let current_passes: BTreeSet<_> = if program["endpoint_search"]["backward_batch_contract"] == 2
    {
        program["endpoint_search"]["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|endpoint| {
                let id = endpoint["id"].as_str()?;
                super::proposals::pool::current_novelty_passed(snapshot, program, id).then_some(id)
            })
            .collect()
    } else {
        BTreeSet::new()
    };
    obligations.sort_by_key(|(priority, attempts, item)| {
        let provisional = program["endpoint_search"]["backward_batch_contract"] == 2
            && *priority == 0
            && !current_passes.contains(item["endpoint_id"].as_str().unwrap_or(""));
        (*priority, provisional, *attempts)
    });
    let complete_original = program["endpoint_search"]["backward_batch_contract"] == 2
        && obligations
            .first()
            .is_some_and(|(priority, _, _)| *priority == 0);
    let selected: Vec<Value> = if complete_original {
        // New runs cover two complete originals before diagnostic depth. The
        // same 24-node/64-KiB bounds apply to their shared representation.
        let count = if program["audit_policy_version"] == 2 && routes.is_empty() {
            2
        } else {
            1
        };
        let mut endpoints = Vec::new();
        for (priority, _, item) in &obligations {
            if *priority == 0
                && !endpoints.contains(&item["endpoint_id"])
                && endpoints.len() < count
            {
                endpoints.push(item["endpoint_id"].clone());
            }
        }
        obligations
            .into_iter()
            .filter(|(priority, _, item)| {
                *priority == 0 && endpoints.contains(&item["endpoint_id"])
            })
            .map(|(_, _, item)| item)
            .collect()
    } else {
        obligations
            .into_iter()
            .take(MAX_COMMITMENTS)
            .map(|(_, _, item)| item)
            .collect()
    };
    let route_limit = if program["endpoint_search"]["backward_batch_contract"] == 1 {
        MAX_COMMITMENTS
    } else {
        selected.len()
    };
    json!({"contract":program["endpoint_search"]["backward_batch_contract"],"mode":if complete_original {"complete_original"} else {"alternatives"},"commitments":selected,"limits":{"routes":route_limit,"hypotheses":24,"research_evidence":8,"response_bytes":MAX_RESPONSE_BYTES}})
}


pub fn repair_disposition(
    program: &Value,
    generated: &Value,
) -> Result<Option<&'static str>, String> {
    if !matches!(
        program["targeted_repair"]["status"].as_str(),
        Some("admitted" | "generating")
    ) {
        return Ok(None);
    }
    let disposition = &generated["repair_disposition"];
    if disposition["input_fingerprint"]
        != program["targeted_repair"]["obligation"]["input_fingerprint"]
    {
        return Err("Repair disposition must name the exact supplied input_fingerprint".into());
    }
    if !disposition["note"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty() && s.len() <= 800)
    {
        return Err("Repair disposition needs a nonempty note of at most800 characters".into());
    }
    let changed = [
        "routes",
        "hypotheses",
        "research_evidence",
        "amendments",
        "branches",
        "source_corrections",
        "bridges",
    ]
    .iter()
    .any(|key| generated[*key].as_array().is_some_and(|a| !a.is_empty()));
    match disposition["status"].as_str() {
        Some("no_change") if !changed && (generated["baseline_delta"].is_null()||generated["baseline_delta"].as_object().is_some_and(|o|o.is_empty())) && (generated["baseline"].is_null()||generated["baseline"]==program["baseline"]) =>Ok(Some("no_change")),
        Some("proposed") if changed && ["routes","hypotheses","research_evidence","amendments","source_corrections"].iter().any(|key|generated[*key].as_array().is_some_and(|a|!a.is_empty()))=>Ok(Some("proposed")),
        _=>Err("Repair must declare proposed material work, or no_change with no new evidence, graph or baseline mutation; unchanged work cannot renew the attempt".into())
    }
}

/// Patch replies may append hypotheses and alternatives, never change the
/// assessment context whose exact receipts made the admission affordable.
pub fn validate_patch_reply(program: &Value, generated: &Value) -> Result<(), String> {
    if program["targeted_repair"]["admission"]["patch_contract"]
        != "append_only_unchanged_evidence_v1"
    {
        return Ok(());
    }
    for key in [
        "research_evidence",
        "source_corrections",
        "amendments",
        "branches",
        "endpoint_revisions",
    ] {
        if generated[key].as_array().is_some_and(|a| !a.is_empty())
            || (!generated[key].is_null() && !generated[key].is_array())
        {
            return Err(format!(
                "Unchanged-evidence patch cannot change {key}; declare no_change if new research or revisions are necessary"
            ));
        }
    }
    for key in [
        "baseline",
        "baseline_delta",
        "scope_review",
        "scope_disposition",
        "world",
        "endpoints",
    ] {
        if !generated[key].is_null() {
            return Err(format!("Unchanged-evidence patch must omit {key}"));
        }
    }
    Ok(())
}

pub fn validate_patch_state(before: &Value, after: &Value, program: &Value) -> Result<(), String> {
    if program["targeted_repair"]["admission"]["patch_contract"]
        != "append_only_unchanged_evidence_v1"
    {
        return Ok(());
    }
    if before["world"] != after["world"] {
        return Err("Patch changed original question context".into());
    }
    for node in before["nodes"].as_array().into_iter().flatten() {
        if !after["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|candidate| candidate == node)
        {
            return Err(format!(
                "Patch changed existing node {}",
                super::field(node, "Id")
            ));
        }
    }
    Ok(())
}

pub fn validate_patch_program(old: &Value, new: &Value) -> Result<(), String> {
    if old["targeted_repair"]["admission"]["patch_contract"] != "append_only_unchanged_evidence_v1"
    {
        return Ok(());
    }
    for key in ["baseline", "scope_review"] {
        if old[key] != new[key] {
            return Err(format!("Patch changed accepted {key}"));
        }
    }
    for key in ["endpoints", "amendments"] {
        if old["endpoint_search"][key] != new["endpoint_search"][key] {
            return Err(format!("Patch changed original {key}"));
        }
    }
    Ok(())
}

/// Called before any route mutations. Historical runs retain their old contract.
pub fn validate(snapshot: &Value, program: &Value, generated: &Value) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    if generated.to_string().len() > MAX_RESPONSE_BYTES {
        return Err("Backward batch exceeds 64 KiB; return only the selected commitments and concise shared pieces".into());
    }
    validate_patch_reply(program, generated)?;
    repair_disposition(program, generated)?;
    let selected = batch(snapshot, program);
    let route_limit = selected["limits"]["routes"].as_u64().unwrap_or(0) as usize;
    for (key, max) in [
        ("routes", route_limit),
        ("hypotheses", 24),
        ("research_evidence", 8),
        ("amendments", route_limit),
        ("branches", 24),
    ] {
        if generated[key]
            .as_array()
            .is_some_and(|items| items.len() > max)
        {
            return Err(format!("Backward batch exceeds {max} {key}"));
        }
    }
    let mut seen = BTreeSet::new();
    for route in generated["routes"].as_array().into_iter().flatten() {
        let obligation = selected["commitments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["endpoint_id"] == route["endpoint_id"]
                    && c["commitment_id"] == route["commitment_id"]
            })
            .ok_or("Backward route is outside the selected commitment batch")?;
        let key = (
            route["endpoint_id"].to_string(),
            route["commitment_id"].to_string(),
        );
        if !seen.insert(key) {
            return Err(
                "Return at most one route for each selected commitment in this turn".into(),
            );
        }
        if !obligation["alternative_to"].is_null()
            && route["alternative_to"] != obligation["alternative_to"]
        {
            return Err("Backward batch must address the selected unresolved route with its exact alternative_to ID".into());
        }
    }
    if program["audit_policy_version"] == 2
        && (selected["mode"] == "complete_original" || selected["patch_contract"] == "append_only_unchanged_evidence_v1")
        && seen.len() != selected["commitments"].as_array().unwrap().len()
    {
        return Err("Return one route for every commitment of both selected originals within the shared bounds; do not leave a partial initial reconstruction".into());
    }
    for amendment in generated["amendments"].as_array().into_iter().flatten() {
        if !selected["commitments"].as_array().unwrap().iter().any(|c| {
            c["endpoint_id"] == amendment["endpoint_id"]
                && c["commitment_id"] == amendment["commitment_id"]
        }) {
            return Err("Backward amendment is outside the selected commitment batch".into());
        }
    }
    Ok(())
}

/// World-first routing chooses obligations by coverage and failed mechanisms,
/// not forward-search novelty/value rankings. Keep every factual and causal check.
pub fn retain_route_assessments(program: &mut Value) {
    if program["world_search_contract"] == 1
        && program["endpoint_search"]["backward_batch_contract"] == 2
        && let Some(tasks) = program["tasks"].as_array_mut()
    {
        tasks.retain(|task| {
            !matches!(
                task["function"].as_str(),
                Some("evaluate_novelty" | "decision_value")
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program() -> Value {
        json!({"endpoint_search":{"backward_batch_contract":1,"endpoints":(0..6).map(|e|json!({"id":format!("e{e}"),"commitments":(0..8).map(|c|json!({"id":format!("c{c}"),"statement":format!("Original {e}/{c}")})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"routes":[]}})
    }
    #[test]
    fn saved_larger_finalization_reserve_survives_execution_and_fallback() {
        let mut p = json!({"audit_policy_version":2,"stage":"routes","admitted_work":{"admitted":true,"status":"checking","finalization_transition_reserve":190,"finalization_time_reserve_ms":1400000}});
        assert_eq!(super::super::transition_limit(&p), 290);
        assert_eq!(super::super::time_limit(&p), 2200000);
        p["admitted_work"]["admitted"] = json!(false);
        p["route_finalization"] = json!({"admitted":true});
        assert_eq!(super::super::transition_limit(&p), 290);
        assert_eq!(super::super::time_limit(&p), 2200000);
    }
    #[test]
    fn append_only_patch_preserves_existing_context_and_complete_group() {
        let mut p = json!({"endpoint_search":{"backward_batch_contract":2,"endpoints":[],"routes":[]},"targeted_repair":{"status":"admitted","admission":{"patch_contract":"append_only_unchanged_evidence_v1"},"obligation":{"input_fingerprint":"same","endpoint_commitments":(0..4).map(|i|json!({"endpoint_id":"e","commitment_id":format!("c{i}"),"route_id":format!("r{i}")})).collect::<Vec<_>>()}}});
        let before = json!({"world":{"question":"Original"},"nodes":[{"Id":"source","statement":"Present fact"}]});
        assert_eq!(
            batch(&before, &p)["commitments"].as_array().unwrap().len(),
            4
        );
        let good = json!({"hypotheses":[{"id":"new"}],"routes":[{"id":"alternative"}]});
        validate_patch_reply(&p, &good).unwrap();
        for key in [
            "research_evidence",
            "source_corrections",
            "amendments",
            "branches",
            "endpoint_revisions",
            "baseline",
            "baseline_delta",
            "scope_review",
        ] {
            let mut bad = good.clone();
            bad[key] = json!([{"changed":true}]);
            assert!(validate_patch_reply(&p, &bad).is_err(), "{key}");
        }
        let mut after = before.clone();
        after["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"new","statement":"Future hypothesis"}));
        validate_patch_state(&before, &after, &p).unwrap();
        after["nodes"][0]["statement"] = json!("Changed present fact");
        assert!(validate_patch_state(&before, &after, &p).is_err());
        p["targeted_repair"]["admission"]["patch_contract"] = Value::Null;
        validate_patch_reply(&p, &json!({"research_evidence":[{"id":"legacy"}]})).unwrap();
    }

    #[test]
    fn new_policy_covers_two_complete_originals_under_shared_bounds() {
        let mut p = program();
        p["endpoint_search"]["backward_batch_contract"] = json!(2);
        p["audit_policy_version"] = json!(2);
        let b = batch(&Value::Null, &p);
        assert_eq!(b["commitments"].as_array().unwrap().len(), 16);
        assert_eq!(b["limits"]["hypotheses"], 24);
        assert_eq!(b["limits"]["response_bytes"], 65536);
        let routes: Vec<_> = b["commitments"].as_array().unwrap().iter().enumerate().map(|(i,c)|json!({"id":format!("r{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})).collect();
        assert!(validate(&Value::Null, &p, &json!({"routes":routes[..8]})).is_err());
        validate(&Value::Null, &p, &json!({"routes":routes})).unwrap();
        p["endpoint_search"]["routes"] = json!(routes);
        let next = batch(&Value::Null, &p);
        assert_eq!(next["commitments"].as_array().unwrap().len(), 8);
        assert_eq!(next["commitments"][0]["endpoint_id"], "e2");
    }

    #[test]
    fn pass15_untouched_initial_receipt_prioritizes_its_original_context() {
        let fixture: Value =
            serde_json::from_str(include_str!("semantic_pass15_order_fixture.json")).unwrap();
        let receipt = &fixture["captured_initial_receipt"];
        let state = &receipt["request"]["state"];
        assert_eq!(state["source_evidence"].as_array().unwrap().len(), 17);
        let snapshot = json!({"world":state["world"],"nodes":state["source_evidence"]});
        let mut program = fixture["program"].clone();
        program["baseline"] = state["baseline"].clone();
        program["endpoint_novelty"] =
            json!({"games-across-games":{"status":"passed","initial_check":receipt}});
        let selected = batch(&snapshot, &program);
        assert_eq!(
            selected["commitments"][0]["endpoint_id"],
            "games-across-games"
        );
        assert_eq!(selected["commitments"].as_array().unwrap().len(), 4);
        assert_eq!(
            program["endpoint_novelty"]["games-across-games"]["initial_check"],
            *receipt
        );
        assert!(program["endpoint_novelty"]["games-across-games"]["final_check"].is_null());
        let mut changed = snapshot;
        changed["nodes"][0]["statement"] = json!("Changed current evidence");
        assert_eq!(
            batch(&changed, &program)["commitments"][0]["endpoint_id"],
            "games-without-builds"
        );
    }

    #[test]
    fn pass15_current_passes_receive_complete_routes_before_provisional_originals() {
        let fixture: Value =
            serde_json::from_str(include_str!("semantic_pass15_order_fixture.json")).unwrap();
        let snapshot = &fixture["snapshot"];
        let mut p = fixture["program"].clone();
        let current = json!({"endpoints":p["endpoint_search"]["endpoints"],"baseline":p["baseline"],"world":snapshot["world"],"source_evidence":super::super::evidence::active_sources(snapshot)});
        for id in ["games-as-checkable-rules", "games-across-games"] {
            let request = super::super::proposals::pool::request(
                &current,
                &json!({"function":"check_proposal_change","endpoint_id":id}),
            )
            .unwrap();
            p["endpoint_novelty"][id]["final_check"] = json!({"passed":true,"request":request});
        }
        let original = p.clone();
        for expected in [
            "games-as-checkable-rules",
            "games-across-games",
            "games-without-builds",
            "player-built-continuity",
            "live-game-troupes",
        ] {
            let b = batch(snapshot, &p);
            assert_eq!(b["mode"], "complete_original");
            assert!(
                b["commitments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["endpoint_id"] == expected)
            );
            let endpoint = p["endpoint_search"]["endpoints"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == expected)
                .unwrap();
            assert_eq!(
                b["commitments"].as_array().unwrap().len(),
                endpoint["commitments"].as_array().unwrap().len()
            );
            let routes: Vec<_> = b["commitments"].as_array().unwrap().iter().map(|c|json!({"id":format!("{}/{}",expected,c["commitment_id"]),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})).collect();
            validate(snapshot, &p, &json!({"routes":routes})).unwrap();
            p["endpoint_search"]["routes"]
                .as_array_mut()
                .unwrap()
                .extend(routes);
        }
        assert_eq!(
            p["endpoint_search"]["endpoints"],
            original["endpoint_search"]["endpoints"]
        );
        assert_eq!(batch(snapshot, &p)["mode"], "alternatives");
        let mut stale = original.clone();
        stale["baseline"]["as_of"] = json!("2026-10-04");
        assert_eq!(
            batch(snapshot, &stale)["commitments"][0]["endpoint_id"],
            "games-without-builds"
        );
        let mut changed = snapshot.clone();
        changed["world"]["question"] = json!("A changed question");
        assert_eq!(
            batch(&changed, &original)["commitments"][0]["endpoint_id"],
            "games-without-builds"
        );
        let mut changed_source = snapshot.clone();
        changed_source["nodes"][0]["statement"] = json!("Changed present evidence");
        assert_eq!(
            batch(&changed_source, &original)["commitments"][0]["endpoint_id"],
            "games-without-builds"
        );
        let mut legacy = original;
        legacy["endpoint_search"]["backward_batch_contract"] = json!(1);
        assert_eq!(
            batch(snapshot, &legacy)["commitments"][0]["endpoint_id"],
            "games-without-builds"
        );
    }

    #[test]
    fn chunks_cover_every_original_before_retrying_failed_routes() {
        let mut p = program();
        let mut covered = BTreeSet::new();
        for round in 0..16 {
            let b = batch(&Value::Null, &p);
            assert_eq!(b, batch(&Value::Null, &p));
            let routes:Vec<_>=b["commitments"].as_array().unwrap().iter().enumerate().map(|(i,c)| {
                assert_eq!(c["reason"],"missing_route");
                assert!(covered.insert((c["endpoint_id"].to_string(),c["commitment_id"].to_string())));
                json!({"id":format!("r{round}_{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})
            }).collect();
            validate(&Value::Null, &p, &json!({"routes":routes})).unwrap();
            p["endpoint_search"]["routes"]
                .as_array_mut()
                .unwrap()
                .extend(routes);
        }
        assert_eq!(covered.len(), 48);
        let first = batch(&Value::Null, &p);
        for (i, c) in first["commitments"].as_array().unwrap().iter().enumerate() {
            assert_eq!(c["reason"], "unresolved_route");
            p["endpoint_search"]["routes"].as_array_mut().unwrap().push(json!({"id":format!("alt{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"alternative_to":c["alternative_to"],"status":"unresolved"}));
        }
        assert_ne!(
            batch(&Value::Null, &p)["commitments"][0]["commitment_id"],
            first["commitments"][0]["commitment_id"]
        );
    }
    #[test]
    fn complete_original_batches_cover_six_worlds_in_six_turns_before_alternatives() {
        let mut p = program();
        p["endpoint_search"]["backward_batch_contract"] = json!(2);
        for round in 0..6 {
            let selected = batch(&Value::Null, &p);
            assert_eq!(selected["mode"], "complete_original");
            assert_eq!(selected["commitments"].as_array().unwrap().len(), 8);
            assert_eq!(selected["limits"]["hypotheses"], 24);
            let routes: Vec<_> = selected["commitments"].as_array().unwrap().iter().enumerate().map(|(i,c)| {
                assert_eq!(c["endpoint_id"], format!("e{round}"));
                json!({"id":format!("r{round}_{i}"),"endpoint_id":c["endpoint_id"],"commitment_id":c["commitment_id"],"status":"unresolved"})
            }).collect();
            validate(&Value::Null, &p, &json!({"routes":routes})).unwrap();
            p["endpoint_search"]["routes"]
                .as_array_mut()
                .unwrap()
                .extend(routes);
        }
        let alternatives = batch(&Value::Null, &p);
        assert_eq!(alternatives["mode"], "alternatives");
        assert_eq!(alternatives["commitments"].as_array().unwrap().len(), 3);
        assert!(
            alternatives["commitments"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["reason"] == "unresolved_route")
        );
        assert!(validate(&Value::Null, &p, &json!({"routes":vec![json!({});4]})).is_err());
    }

    #[test]
    fn route_planning_removes_only_unused_rankings_and_preserves_legacy() {
        let functions = [
            "classify_claim_role",
            "classify_temporal",
            "classify_gap",
            "estimate_likelihood",
            "estimate_conditional",
            "check_transition",
            "evaluate_novelty",
            "decision_value",
        ];
        let mut p = json!({"world_search_contract":1,"endpoint_search":{"backward_batch_contract":2},"tasks":functions.iter().map(|f|json!({"function":f})).collect::<Vec<_>>()});
        let original = p.clone();
        retain_route_assessments(&mut p);
        assert_eq!(p["tasks"].as_array().unwrap().len(), 6);
        for function in &functions[..6] {
            assert!(
                p["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["function"] == *function)
            );
        }
        let mut legacy = original.clone();
        legacy["world_search_contract"] = json!(0);
        let before = legacy.clone();
        retain_route_assessments(&mut legacy);
        assert_eq!(legacy, before);
        let mut v1 = original;
        v1["endpoint_search"]["backward_batch_contract"] = json!(1);
        let before = v1.clone();
        retain_route_assessments(&mut v1);
        assert_eq!(v1, before);
    }

    #[test]
    fn rejects_oversized_outside_and_duplicate_responses_without_changing_state() {
        let p = program();
        let original = p.clone();
        let r = json!({"endpoint_id":"e0","commitment_id":"c0"});
        assert!(validate(&Value::Null, &p, &json!({"routes":[r.clone(),r.clone()]})).is_err());
        assert!(
            validate(
                &Value::Null,
                &p,
                &json!({"routes":[{"endpoint_id":"e5","commitment_id":"c7"}]})
            )
            .is_err()
        );
        assert!(validate(&Value::Null, &p, &json!({"hypotheses":vec![json!({});25]})).is_err());
        assert!(
            validate(
                &Value::Null,
                &p,
                &json!({"exploration_note":"x".repeat(MAX_RESPONSE_BYTES)})
            )
            .is_err()
        );
        assert_eq!(p, original);
        let mut legacy = p.clone();
        legacy["endpoint_search"]
            .as_object_mut()
            .unwrap()
            .remove("backward_batch_contract");
        assert!(validate(&Value::Null, &legacy, &json!({"routes":vec![r;48]})).is_ok());
    }
}
