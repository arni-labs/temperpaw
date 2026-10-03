// A fresh joint-event judgment is about the world, not the execution history of
// conditional experiments. Canonical nodes, route audits and receipts are untouched.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const NODE_FIELDS: &[&str] = &[
    "Id",
    "Status",
    "archived",
    "assumptions",
    "branch_conditions",
    "chain",
    "commitment_bindings",
    "comparison_contract",
    "comparison_frame",
    "component_ids",
    "counter_ids",
    "edges",
    "endpoint_id",
    "facets",
    "falsifiers",
    "falsifier",
    "kind",
    "mechanism",
    "narrative",
    "provenance",
    "revision",
    "scene",
    "selected_route_ids",
    "shared_question",
    "signals",
    "signal",
    "source_session_id",
    "statement",
    "title",
    "trajectory_answer",
    "trajectory_binding",
    "what_you_can_do",
    "parent",
    "research_status",
    "research_question",
    "evidence_note",
    "source_refs",
    "source_quote",
    "quote",
    "observed_at",
    "claim_type",
    "evidence_metadata",
    "source_correction",
    "projection_period",
    "date",
    "timestamp",
    "sources",
    "resolve_by",
    "resolution",
    "probability",
    "evidence",
    "branch_id",
    "grounding_evidence_ids",
    "root_connections",
    "route_only",
];

fn known_node(node: &Value) -> Result<(), String> {
    let fields = node
        .get("fields")
        .unwrap_or(node)
        .as_object()
        .ok_or("Invalid assessment node")?;
    for key in fields.keys() {
        if !NODE_FIELDS.contains(&key.as_str()) {
            return Err(format!(
                "Whole-world assessment has unsupported semantic node field: {key}"
            ));
        }
    }
    Ok(())
}

fn counter_definition(node: &Value) -> Result<Value, String> {
    let mut definition = node.clone();
    let fields = definition
        .as_object_mut()
        .ok_or("Invalid counter definition")?;
    for key in [
        "Status",
        "archived",
        "source_session_id",
        "parent",
        "revision",
        "research_status",
        "title",
        "scene",
        "narrative",
        "facets",
        "what_you_can_do",
        "probability",
    ] {
        fields.remove(key);
    }
    Ok(definition)
}

fn counter_qualification(snapshot: &Value, node: &Value) -> Result<Option<Value>, String> {
    match node.get("branch_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if id.is_empty() => Ok(None),
        Some(Value::String(id)) => Ok(Some(super::super::branches::state(
            snapshot,
            id,
            node["Id"].as_str(),
        )?)),
        _ => Err("Counter branch_id must be a string or null".into()),
    }
}

pub fn request(
    snapshot: &Value,
    program: &Value,
    world: &Value,
    mut request: Value,
) -> Result<Value, String> {
    let nodes = snapshot["nodes"]
        .as_array()
        .ok_or("Missing assessment nodes")?;
    known_node(world)?;
    let components = world["component_ids"]
        .as_array()
        .ok_or("Missing assessment components")?;
    for id in components
        .iter()
        .chain(world["counter_ids"].as_array().into_iter().flatten())
    {
        known_node(
            nodes
                .iter()
                .find(|node| node["Id"] == *id)
                .ok_or("Missing assessment component")?,
        )?;
    }
    for source in super::super::evidence::active_sources(snapshot) {
        known_node(source)?;
        let supplied = super::digest(source);
        for key in source
            .as_object()
            .ok_or("Invalid assessment source")?
            .keys()
        {
            if supplied.get(key).is_none()
                && !matches!(
                    key.as_str(),
                    "Status"
                        | "archived"
                        | "edges"
                        | "source_session_id"
                        | "revision"
                        | "parent"
                        | "research_status"
                )
            {
                return Err(format!(
                    "Whole-world assessment has unsupported source field: {key}"
                ));
            }
        }
    }
    let state = request["state"]
        .as_object_mut()
        .ok_or("Missing assessment state")?;
    state.retain(|key, _| {
        matches!(
            key.as_str(),
            "world"
                | "node"
                | "prerequisites"
                | "counter_hypotheses"
                | "source_evidence"
                | "baseline"
        )
    });
    state.insert("node".into(), world.clone());
    state.insert(
        "prerequisites".into(),
        json!(components.iter().map(|id| {
        let node = nodes.iter().find(|node| node["Id"] == *id).unwrap(); // validated above
        json!({"id":id,"node":node,"evaluations":program["evaluations"][id.as_str().unwrap_or("")]})
    }).collect::<Vec<_>>()),
    );
    // These are presentation variants, not extra conjuncts of the recorded event.
    for key in [
        "title",
        "scene",
        "narrative",
        "facets",
        "what_you_can_do",
        "probability",
        "Status",
        "archived",
        "revision",
        "source_session_id",
        "comparison_contract",
        "comparison_frame",
        "trajectory_binding",
    ] {
        state["node"]
            .as_object_mut()
            .ok_or("Missing assessment world")?
            .remove(key);
    }
    for component in state
        .get_mut("prerequisites")
        .and_then(Value::as_array_mut)
        .ok_or("Missing assessment prerequisites")?
    {
        let raw = nodes
            .iter()
            .find(|node| node["Id"] == component["id"])
            .ok_or("Missing defining component")?;
        component["node"] = raw.clone();
        for key in [
            "Status",
            "archived",
            "source_session_id",
            "parent",
            "revision",
            "research_status",
        ] {
            component["node"]
                .as_object_mut()
                .ok_or("Invalid component")?
                .remove(key);
        }
        let gap = component["evaluations"]["classify_gap"]["selected"].clone();
        component
            .as_object_mut()
            .ok_or("Invalid assessment prerequisite")?
            .retain(|key, _| matches!(key.as_str(), "id" | "node"));
        if !gap.is_null() {
            component["recorded_gap"] = gap;
        }
        for key in [
            "title",
            "scene",
            "narrative",
            "facets",
            "what_you_can_do",
            "probability",
        ] {
            component["node"]
                .as_object_mut()
                .ok_or("Missing assessment component definition")?
                .remove(key);
        }
    }
    for counter in state
        .get_mut("counter_hypotheses")
        .and_then(Value::as_array_mut)
        .ok_or("Missing assessment counters")?
    {
        let original = nodes
            .iter()
            .find(|node| node["Id"] == counter["node"]["Id"])
            .ok_or("Missing original counter")?;
        *counter = json!({"node":counter_definition(original)?});
        if let Some(qualification) = counter_qualification(snapshot, original)? {
            counter["branch_qualification"] = qualification;
        }
    }
    let audit = super::super::search::audit_world(world, program);
    let limitations: Vec<_> = audit["checks"].as_array().into_iter().flatten()
        .filter(|check| !matches!(check["kind"].as_str(), Some("conditional_on" | "conditional_off")))
        .map(|check| json!({"kind":check["kind"],"subject_ids":check["subject_ids"],"result":check["result"]})).collect();
    let routes: Vec<_> = audit["selected_routes"]["routes"].as_array().into_iter().flatten()
        .map(|route| json!({"route_id":route["route_id"],"commitment_id":route["commitment_id"],"status":route["status"],"root_connections":route["root_connections"],"audit_status":route["audit"]["status"]})).collect();
    state.insert("world_audit".into(), json!({"status":audit["status"],"planned_checks":audit["planned_checks"],"completed_checks":audit["completed_checks"],"limitations":limitations,"selected_routes":routes,"interpretation":"Recorded qualitative model checks are warnings, not observed facts or likelihoods. Transition results concern the link under its hypothetical prerequisites; they do not establish that those prerequisites occur. Full conditional experiments remain in canonical audits and are deliberately not inputs to this fresh joint estimate."}));
    // Bind the selected endpoint's exact commitments, including any recorded amendments.
    if let Some(endpoint_id) = world["endpoint_id"].as_str() {
        let endpoint = program["endpoint_search"]["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|endpoint| endpoint["id"] == endpoint_id)
            .ok_or("Missing endpoint definition")?;
        state.insert("endpoint_definition".into(), json!({"id":endpoint_id,"statement":endpoint["statement"],"commitments":endpoint["commitments"],"original_statement":endpoint["original_statement"],"amendments":program["endpoint_search"]["amendments"].as_array().into_iter().flatten().filter(|amendment| amendment["endpoint_id"] == endpoint_id).collect::<Vec<_>>(),"novelty_status":program["endpoint_novelty"][endpoint_id]["status"],"novelty_reason":program["endpoint_novelty"][endpoint_id]["reason"]}));
    }
    state.insert(
        "assessment_contract".into(),
        json!("whole_world_assessment_v1"),
    );
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(Value::Object(state.clone()).to_string().as_bytes())
    );
    state.insert("assessment_fingerprint".into(), json!(fingerprint));
    request["questions"]["result"]["instructions"] = json!(
        "Estimate the probability that state.node.statement AND EVERY defining event in state.prerequisites occur within their stated deadlines and world horizon. Signed state.node.branch_conditions are additional uncertain conjuncts, NOT assumptions granted true. Use the exact causal mechanisms, assumptions, present baseline, all supplied typed evidence, contrary hypotheses and unresolved route warnings. Judge this whole event freshly: never average, multiply, inherit, or substitute component probabilities for a fresh joint estimate. Do not infer independence or treat coherent prose or qualitative checks as evidence of occurrence. Prior conditional probability experiments are intentionally excluded. Worlds can overlap and need not sum to one. A counter hypothesis is contrary context, not a defining component. Its branch_qualification restricts where that counter applies; never assume its clauses true or add them to the world conjuncts. Source projections remain projections, not observations. Return event likelihood, not confidence in the narrative."
    );
    validate_definition(snapshot, world, &request)?;
    Ok(request)
}

// The boundary fails closed if a projection accidentally drops an event or source.
fn validate_definition(snapshot: &Value, world: &Value, request: &Value) -> Result<(), String> {
    for key in [
        "statement",
        "component_ids",
        "branch_conditions",
        "chain",
        "assumptions",
    ] {
        if request["state"]["node"][key] != world[key] {
            return Err(format!(
                "Whole-world assessment changed defining field: {key}"
            ));
        }
    }
    for id in world["counter_ids"].as_array().into_iter().flatten() {
        let original = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|node| node["Id"] == *id)
            .ok_or("Missing original counter")?;
        let counter = request["state"]["counter_hypotheses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["node"]["Id"] == *id)
            .ok_or("Omitted counter hypothesis")?;
        if counter["node"] != counter_definition(original)? {
            return Err("Whole-world assessment changed counter definition".into());
        }
        let expected = counter_qualification(snapshot, original)?;
        if counter.get("branch_qualification") != expected.as_ref() {
            return Err("Whole-world assessment changed counter branch qualification".into());
        }
    }
    let sources = super::super::evidence::active_sources(snapshot);
    let supplied = request["state"]["source_evidence"]
        .as_array()
        .ok_or("Missing assessment evidence")?;
    let expected: Vec<_> = sources.into_iter().map(super::digest).collect();
    if supplied != &expected {
        return Err("Whole-world assessment omitted or changed current source evidence".into());
    }
    for id in world["component_ids"]
        .as_array()
        .ok_or("Missing defining components")?
    {
        let original = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|node| node["Id"] == *id)
            .ok_or("Missing original component")?;
        let component = request["state"]["prerequisites"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["id"] == *id)
            .ok_or("Omitted defining component")?;
        for key in [
            "statement",
            "mechanism",
            "resolve_by",
            "date",
            "branch_conditions",
            "falsifier",
            "evidence_note",
        ] {
            if component["node"][key] != original[key] {
                return Err(format!(
                    "Whole-world assessment changed component field: {key}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let snapshot = json!({"world":{"Id":"question","target_date":"2030-12-31"},"nodes":[
            {"Id":"event","kind":"scenario","statement":"Exact event by 2030","mechanism":"Requires scarce skilled maintainers","resolve_by":"2030-12-31","falsifier":"No maintainers participate","edges":"[]"},
            {"Id":"contrary","kind":"evidence","statement":"Maintainer numbers declined","provenance":"observed","evidence_metadata":{"kind":"finding","publication_date":"2026-10-01"},"edges":"[]"},
            {"Id":"world","kind":"world","statement":"The joint world occurs","component_ids":["event"],"counter_ids":[],"branch_conditions":[{"kind":"not_all_occurring","event_ids":["event"],"by":"2030-12-31"}],"assumptions":["No subsidy"],"chain":[],"edges":"[{\"kind\":\"requires\",\"to_id\":\"event\"}]"}]});
        let program = json!({"tasks":[{"nodeId":"world","function":"estimate_likelihood"}],"cursor":0,"baseline":{"as_of":"2026-10-03","unknowns":["Funding unknown"]}});
        (snapshot, program)
    }
    #[test]
    fn direct_contract_preserves_definition_and_contrary_evidence_without_mutating_owners() {
        let (snapshot, program) = fixture();
        let original = (snapshot.clone(), program.clone());
        let request = super::super::request(&snapshot, &program).unwrap();
        assert_eq!(
            request["state"]["assessment_contract"],
            "whole_world_assessment_v1"
        );
        assert_eq!(
            request["state"]["node"]["branch_conditions"],
            snapshot["nodes"][2]["branch_conditions"]
        );
        assert_eq!(
            request["state"]["source_evidence"][0]["statement"],
            "Maintainer numbers declined"
        );
        assert_eq!(
            request["state"]["prerequisites"][0]["node"]["mechanism"],
            "Requires scarce skilled maintainers"
        );
        assert!(request["state"].get("previous_world_judgments").is_none());
        assert_eq!((snapshot, program), original);
    }
    #[test]
    fn provider_receives_one_direct_world_without_catalog_indirection() {
        let (mut snapshot, mut program) = fixture();
        let mut second = snapshot["nodes"][2].clone();
        second["Id"] = json!("second-world");
        snapshot["nodes"].as_array_mut().unwrap().push(second);
        program["tasks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"nodeId":"second-world","function":"estimate_likelihood"}));
        let batch = super::super::super::batch::prepare(&snapshot, &program, 2).unwrap();
        assert_eq!(batch.tasks.len(), 1);
        assert_eq!(batch.question_key(0), "result");
        assert_eq!(batch.request, batch.individual[0]);
        assert!(batch.request["state"].get("encoded_input").is_none());
        assert!(batch.request["state"].get("cases").is_none());
    }

    #[test]
    fn dropped_condition_or_contrary_source_is_rejected() {
        let (snapshot, program) = fixture();
        let request = super::super::request(&snapshot, &program).unwrap();
        let mut missing = request.clone();
        missing["state"]["node"]["branch_conditions"] = json!([]);
        assert!(
            validate_definition(&snapshot, &snapshot["nodes"][2], &missing)
                .unwrap_err()
                .contains("branch_conditions")
        );
        missing = request;
        missing["state"]["source_evidence"] = json!([]);
        assert!(
            validate_definition(&snapshot, &snapshot["nodes"][2], &missing)
                .unwrap_err()
                .contains("source evidence")
        );
    }
    #[test]
    fn changed_fact_invalidates_fingerprint_and_unknown_field_is_not_silently_lost() {
        let (mut snapshot, program) = fixture();
        let before = super::super::request(&snapshot, &program).unwrap();
        snapshot["nodes"][1]["statement"] = json!("Maintainer numbers grew");
        let after = super::super::request(&snapshot, &program).unwrap();
        assert_ne!(
            before["state"]["assessment_fingerprint"],
            after["state"]["assessment_fingerprint"]
        );
        snapshot["nodes"][0]["new_semantic_constraint"] = json!("Only after 2035");
        assert!(
            super::super::request(&snapshot, &program)
                .unwrap_err()
                .contains("new_semantic_constraint")
        );
    }
    #[test]
    #[ignore = "requires native pass14 capture via FORESIGHT_DIRECT_CAPTURE"]
    fn captured_success_and_failure_preserve_every_defining_component_and_source() {
        let record: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("FORESIGHT_DIRECT_CAPTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let snapshot: Value =
            serde_json::from_str(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
        let program: Value =
            serde_json::from_str(record["fields"]["program_json"].as_str().unwrap()).unwrap();
        let trace: Value =
            serde_json::from_str(record["fields"]["trace_json"].as_str().unwrap()).unwrap();
        assert!(
            trace
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["requestHash"]
                    == "d15a6578ba37cfd2443433c13cee07e30e049153e2a685c0b2ff08dbbae52fb5"
                    && entry["requestBytes"] == 107063)
        );
        assert!(
            trace
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["nodeId"] == "world-r1-inhabited_worlds"
                    && entry["providerUsage"]["input_tokens"] == 32133)
        );
        for id in [
            "world-r1-inhabited_worlds",
            "world-r1-travelling_companions",
        ] {
            let task = json!({"nodeId":id,"function":"estimate_likelihood","depth":0});
            let request = super::super::request_task(&snapshot, &program, &task).unwrap();
            let world = snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["Id"] == id)
                .unwrap();
            validate_definition(&snapshot, world, &request).unwrap();
            assert_eq!(request["state"]["baseline"], program["baseline"]);
            assert_eq!(
                request["state"]["source_evidence"]
                    .as_array()
                    .unwrap()
                    .len(),
                32
            );
            assert_eq!(
                request["state"]["prerequisites"].as_array().unwrap().len(),
                world["component_ids"].as_array().unwrap().len()
            );
            assert!(request["state"].get("encoded_input").is_none());
            eprintln!("{id} direct assessment bytes={}", request.to_string().len());
        }
    }
    #[test]
    fn conditional_counter_keeps_falsifier_edges_and_inherited_signed_qualifications() {
        let (mut snapshot, program) = fixture();
        snapshot["world"]["last_ingest_date"] = json!("2026-10-03");
        snapshot["nodes"].as_array_mut().unwrap().extend([
            json!({"Id":"other","kind":"scenario","statement":"Alternative event occurs","edges":"[]"}),
            json!({"Id":"counter","kind":"scenario","statement":"Counter applies only on its branch","mechanism":"Competition blocks the joint world","falsifier":"The competitor withdraws","branch_id":"child","edges":"[{\"kind\":\"supports\",\"to_id\":\"contrary\"}]"})]);
        snapshot["nodes"][2]["counter_ids"] = json!(["counter"]);
        snapshot["branches"] = json!([
            {"id":"parent","condition":{"kind":"all_occurring","event_ids":["event"]},"by":"2028-01-01"},
            {"id":"child","parent_branch_id":"parent","condition":{"kind":"not_all_occurring","event_ids":["other"]},"by":"2029-01-01"}]);
        let request = super::super::request(&snapshot, &program).unwrap();
        let counter = &request["state"]["counter_hypotheses"][0];
        assert_eq!(counter["node"]["falsifier"], "The competitor withdraws");
        assert_eq!(counter["node"]["edges"], snapshot["nodes"][4]["edges"]);
        assert_eq!(counter["node"]["branch_id"], "child");
        assert_eq!(
            counter["branch_qualification"],
            super::super::super::branches::state(&snapshot, "child", Some("counter")).unwrap()
        );
        assert_eq!(
            counter["branch_qualification"]["conditions"][0]["kind"],
            "all_occurring"
        );
        assert_eq!(
            counter["branch_qualification"]["conditions"][1]["kind"],
            "not_all_occurring"
        );
        assert_eq!(
            counter["branch_qualification"]["conditions"][1]["by"],
            "2029-01-01"
        );
        assert_eq!(
            request["state"]["node"]["branch_conditions"],
            snapshot["nodes"][2]["branch_conditions"]
        );
        let mut old_digest = request.clone();
        old_digest["state"]["counter_hypotheses"][0]["node"] =
            super::super::digest(&snapshot["nodes"][4]);
        assert!(
            validate_definition(&snapshot, &snapshot["nodes"][2], &old_digest)
                .unwrap_err()
                .contains("counter definition")
        );
        let mut unqualified = request;
        unqualified["state"]["counter_hypotheses"][0]
            .as_object_mut()
            .unwrap()
            .remove("branch_qualification");
        assert!(
            validate_definition(&snapshot, &snapshot["nodes"][2], &unqualified)
                .unwrap_err()
                .contains("branch qualification")
        );
    }
}
