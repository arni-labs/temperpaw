use super::*;
fn fixture() -> (Value, Value, Value) {
    let declaration = json!({"id":"bridge","from_ids":["a"],"to_id":"b","by":"2028-06-30","mechanism":"Executable interpreter enables isolated runtime tests","assumptions":["Bounded action vocabulary"],"endpoint_dependencies":["e"]});
    let mut search = json!({"endpoints":[{"id":"e","original_statement":"Rich original world","commitments":[{"id":"c","statement":"Original claim"}]}],"routes":[],"amendments":[]});
    let mut reply = json!({"shared_bridge_contract":1,"bridges":[declaration],"routes":[{"endpoint_id":"e","chain":[{"bridge_ref":"bridge"}]}]});
    materialize(&mut reply, &mut search).unwrap();
    let mut link = reply["routes"][0]["chain"][0].clone();
    link["id"] = json!("l");
    let world = json!({"Id":"r1","kind":"world","route_only":true,"component_ids":["a","b"],"counter_ids":[],"chain":[link],"assumptions":[],"edges":"[]","endpoint_id":"e"});
    let mut other = world.clone();
    other["Id"] = json!("r2");
    let snapshot = json!({"world":{"last_ingest_date":"2026-10-03"},"branches":[],"nodes":[{"Id":"a","kind":"scenario","statement":"Interpreter exists by2027","edges":"[]"},{"Id":"b","kind":"scenario","statement":"Runtime generation exists by2028","edges":"[]"},world,other]});
    let task = |id: &str| json!({"nodeId":format!("{id}/link/l"),"world_id":id,"link_id":"l","function":"check_transition","depth":0});
    let program = json!({"endpoint_search":search,"baseline":{"as_of":"2026-10-03"},"results":{"a":{"classify_temporal":"future"}},"evaluations":{"a":{"classify_temporal":{"selected":"future","answer":{"choice":"future"}}}},"cursor":0,"tasks":[task("r1"),task("r2")]});
    (snapshot, program, reply)
}
#[test]
fn explicit_materialization_is_immutable_and_rejects_unknown_semantics() {
    let (_, p, reply) = fixture();
    assert_eq!(
        reply["routes"][0]["chain"][0]["mechanism"],
        "Executable interpreter enables isolated runtime tests"
    );
    let mut registry = p["endpoint_search"].clone();
    let mut duplicate =
        json!({"shared_bridge_contract":1,"bridges":[registry["bridges"]["bridge"]],"routes":[]});
    assert!(
        materialize(&mut duplicate, &mut registry)
            .unwrap_err()
            .contains("immutable")
    );
    let mut invalid = json!({"shared_bridge_contract":1,"bridges":[],"routes":[{"endpoint_id":"e","chain":[{"bridge_ref":"bridge","by":"2099"}]}]});
    assert!(
        materialize(&mut invalid, &mut registry)
            .unwrap_err()
            .contains("Unknown")
    );
    let mut missing = json!({"shared_bridge_contract":1,"bridges":[],"routes":[{"endpoint_id":"other","chain":[{"bridge_ref":"bridge"}]}]});
    assert!(
        materialize(&mut missing, &mut registry)
            .unwrap_err()
            .contains("dependency")
    );
}
#[test]
fn exact_context_fanout_has_one_question_and_cache_hit_not_new_judgment() {
    let (snapshot, mut program, _) = fixture();
    let batch = super::super::super::batch::prepare(&snapshot, &program, 16).unwrap();
    assert_eq!(batch.tasks.len(), 2);
    assert_eq!(batch.request["questions"].as_object().unwrap().len(), 1);
    assert_eq!(batch.question_key(0), batch.question_key(1));
    let response = json!({"model":super::super::super::MODEL,"answers":{"q0":{"type":"choice","choice":"plausible","probabilities":{"plausible":0.7,"uncertain":0.3,"conflict":0.0}}}});
    let answers = super::super::super::batch::answers(&batch, &response).unwrap();
    assert_eq!(answers.len(), 2);
    let (decision, evaluation, _) = &answers[0];
    record(
        &mut program,
        &batch.individual[0],
        evaluation,
        decision,
        7,
        &json!(3),
    );
    program["cursor"] = json!(1);
    assert!(reuse_current(&snapshot, &mut program).unwrap());
    assert_eq!(program["cursor"], 2);
    assert_eq!(
        program["evaluations"]["r2/link/l"]["check_transition"]["context"]["shared_bridge_reuse"]["trace_index"],
        7
    );
}
#[test]
fn semantic_changes_prevent_reuse_and_unknown_fields_survive() {
    let (snapshot, program, _) = fixture();
    let request =
        super::super::super::search::request(&snapshot, &program, &program["tasks"][0]).unwrap();
    let original = fingerprint(&request);
    for field in [
        "baseline",
        "definitions",
        "branch_state",
        "declared_dependencies",
        "source_evidence",
        "prerequisite_judgments",
        "ancestor_mechanisms",
        "event_qualifications",
        "root_connections",
    ] {
        let mut changed = request.clone();
        changed["state"][field] = json!({"changed":true});
        assert_ne!(fingerprint(&changed), original, "{field}");
    }
    let mut deadline = request.clone();
    deadline["state"]["bridge"]["by"] = json!("2029-01-01");
    assert_ne!(fingerprint(&deadline), original);
    let mut changed = snapshot.clone();
    changed["nodes"][3]["unknown_semantic_condition"] = json!("Requires public ownership");
    let second =
        super::super::super::search::request(&changed, &program, &program["tasks"][1]).unwrap();
    assert_ne!(fingerprint(&second), original);
    assert_eq!(
        second["state"]["route_context_extensions"]["unknown_semantic_condition"],
        "Requires public ownership"
    );
    let mut changed = snapshot;
    changed["nodes"][3]["chain"][0]["by"] = json!("2029-01-01");
    assert!(
        super::super::super::search::request(&changed, &program, &program["tasks"][1]).is_err()
    );
}
#[test]
#[ignore = "requires frozen pass16 via FORESIGHT_SHARED_BRIDGE_CAPTURE"]
fn captured_native_materialization_preserves_full_bridge_and_reuses_exact_context() {
    let file = std::env::var("FORESIGHT_SHARED_BRIDGE_CAPTURE").unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let snapshot: Value =
        serde_json::from_str(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
    let mut program: Value =
        serde_json::from_str(record["fields"]["program_json"].as_str().unwrap()).unwrap();
    program["endpoint_search"]["backward_batch_contract"] = Value::Null;
    let originals = program["endpoint_search"]["routes"].as_array().unwrap();
    let first = originals
        .iter()
        .find(|r| r["id"] == "route_runtime_v1")
        .unwrap();
    let second = originals
        .iter()
        .find(|r| r["id"] == "route_history_v1")
        .unwrap();
    let link = first["chain"][0].clone();
    let declaration = json!({"id":"shared-runtime","from_ids":link["from_ids"],"to_id":link["to_id"],"by":link["by"],"mechanism":link["mechanism"],"assumptions":[],"endpoint_dependencies":[first["endpoint_id"]]});
    let mut routes = vec![first.clone(), second.clone()];
    for (index, route) in routes.iter_mut().enumerate() {
        route["alternative_to"] = route["id"].clone();
        route["id"] = json!(format!("shared-copy-{index}"));
        route["chain"][0] = json!({"bridge_ref":"shared-runtime"});
        // Explicitly share the same root bridge too; different grounding
        // would correctly keep these as different assessment inputs.
        route["root_connections"][0] = first["root_connections"][0].clone();
    }
    let generated = json!({"shared_bridge_contract":1,"bridges":[declaration],"routes":routes,"hypotheses":[],"research_evidence":[],"amendments":[]});
    let mut after = snapshot.clone();
    let search = super::super::add_routes(&snapshot, &mut after, &program, &generated).unwrap();
    program["endpoint_search"] = search;
    let added = program["endpoint_search"]["routes"]
        .as_array()
        .unwrap()
        .clone();
    let mut requests = Vec::new();
    for route in added
        .iter()
        .filter(|r| field(r, "id").starts_with("shared-copy-"))
    {
        assert_eq!(route["chain"][0]["mechanism"], link["mechanism"]);
        let world = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["Id"] == route["world_node_id"])
            .unwrap();
        let task = super::super::super::search::audit_tasks(world, &program)
            .into_iter()
            .find(|t| {
                t["function"] == "check_transition" && t["link_id"] == route["chain"][0]["id"]
            })
            .unwrap();
        requests.push(super::super::super::search::request(&after, &program, &task).unwrap());
    }
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    let source_id = requests[0]["state"]["source_evidence"][0]["Id"].clone();
    let mut changed = after.clone();
    let source = changed["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["Id"] == source_id)
        .unwrap();
    source["statement"] = json!(format!(
        "Corrected contrary observation: {}",
        field(source, "statement")
    ));
    let route = added.iter().find(|r| r["id"] == "shared-copy-0").unwrap();
    let world = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["Id"] == route["world_node_id"])
        .unwrap();
    let task = super::super::super::search::audit_tasks(world, &program)
        .into_iter()
        .find(|t| t["function"] == "check_transition" && t["link_id"] == route["chain"][0]["id"])
        .unwrap();
    let changed_request = super::super::super::search::request(&changed, &program, &task).unwrap();
    assert_ne!(
        fingerprint(&requests[0]),
        fingerprint(&changed_request),
        "changed source must invalidate shared result"
    );

    let mut repair = program.clone();
    repair["active_world_ids"] = json!([]);
    repair["endpoint_search"]["routes"] = json!(
        added
            .iter()
            .filter(|r| field(r, "id").starts_with("shared-copy-"))
            .collect::<Vec<_>>()
    );
    let raw = json!({"type":"choice","choice":"plausible","probabilities":{"plausible":0.59,"uncertain":0.30,"conflict":0.11}});
    for route in repair["endpoint_search"]["routes"]
        .as_array()
        .unwrap()
        .clone()
    {
        let world = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["Id"] == route["world_node_id"])
            .unwrap();
        let task = super::super::super::search::audit_tasks(world, &repair)
            .into_iter()
            .find(|t| {
                t["function"] == "check_transition" && t["link_id"] == route["chain"][0]["id"]
            })
            .unwrap();
        let node = field(&task, "nodeId");
        repair["results"][node]["check_transition"] = json!("uncertain");
        repair["evaluations"][node]["check_transition"] =
            json!({"selected":"uncertain","answer":raw});
        repair["route_basis"][field(world, "Id")] =
            super::super::candidate_basis(&after, &repair, field(world, "Id"));
    }
    let queue = super::super::super::backward::repair_obligations(&after, &repair).unwrap();
    let shared: Vec<_> = queue
        .iter()
        .filter(|item| item["bridge_ref"] == "shared-runtime")
        .collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(
        shared[0]["endpoint_commitments"].as_array().unwrap().len(),
        2
    );
    for observed in shared[0]["observed_results"].as_array().unwrap() {
        assert_eq!(observed["result"], "uncertain");
        assert_eq!(observed["evaluation"]["answer"], raw);
    }
    if let Ok(path) = std::env::var("FORESIGHT_SHARED_BRIDGE_OUTPUT") {
        std::fs::write(path,serde_json::to_vec(&json!({"snapshot":after,"program":program,"requests":requests,"disclosure":"Captured pass16 native materialization with explicit shared declarations; not a new provider evaluation"})).unwrap()).unwrap();
    }
}

#[test]
fn independent_bridges_still_batch_while_only_exact_aliases_share_answers() {
    let (mut snapshot, mut program, _) = fixture();
    let mut third = snapshot["nodes"][3].clone();
    third["Id"] = json!("r3");
    let mut definition = program["endpoint_search"]["bridges"]["bridge"].clone();
    definition["id"] = json!("other");
    definition["mechanism"] = json!("A different execution mechanism");
    program["endpoint_search"]["bridges"]["other"] = definition.clone();
    third["chain"][0]["bridge_ref"] = json!("other");
    third["chain"][0]["mechanism"] = definition["mechanism"].clone();
    snapshot["nodes"].as_array_mut().unwrap().push(third);
    program["tasks"].as_array_mut().unwrap().push(json!({"nodeId":"r3/link/l","world_id":"r3","link_id":"l","function":"check_transition","depth":0}));
    let batch = super::super::super::batch::prepare(&snapshot, &program, 16).unwrap();
    assert_eq!(batch.tasks.len(), 3);
    assert_eq!(batch.request["questions"].as_object().unwrap().len(), 2);
    assert_eq!(batch.question_key(0), "q0");
    assert_eq!(batch.question_key(1), "q0");
    assert_eq!(batch.question_key(2), "q2");
}
#[test]
fn changed_live_inputs_prevent_cache_hit_without_erasing_original_receipt() {
    let (snapshot, mut program, _) = fixture();
    let request =
        super::super::super::search::request(&snapshot, &program, &program["tasks"][0]).unwrap();
    record(
        &mut program,
        &request,
        &json!({"type":"choice","selected":"uncertain","answer":{"choice":"plausible"},"context":{}}),
        "uncertain",
        0,
        &json!(1),
    );
    program["cursor"] = json!(1);
    program["baseline"]["unknowns"] = json!(["New contrary observation"]);
    assert!(!reuse_current(&snapshot, &mut program).unwrap());
    assert_eq!(
        program["shared_bridge_receipts"].as_object().unwrap().len(),
        1
    );
    assert!(program["results"]["r2/link/l"].is_null());
}

#[test]
fn provider_contract_endpoint_model_and_question_are_bound_to_cache_identity() {
    let (snapshot, program, _) = fixture();
    let request =
        super::super::super::search::request(&snapshot, &program, &program["tasks"][0]).unwrap();
    let identity = fingerprint(&request);
    for (provider, endpoint, version) in [
        ("another-provider", PROVIDER_ENDPOINT, PROVIDER_CONTRACT),
        ("typesafe", "https://example.invalid/v2", PROVIDER_CONTRACT),
        ("typesafe", PROVIDER_ENDPOINT, "typesafe.systemone.v2"),
    ] {
        assert_ne!(
            identity,
            fingerprint_for(&request, provider, endpoint, version)
        );
    }
    for key in ["model", "questions"] {
        let mut changed = request.clone();
        changed[key] = json!("changed");
        assert_ne!(identity, fingerprint(&changed));
    }
}

#[test]
#[ignore = "requires frozen pass17 food via FORESIGHT_BRIDGE_OVERFLOW_CAPTURE"]
fn captured_bridge_overflow_request_replay() {
    use sha2::{Digest, Sha256};
    let record: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_BRIDGE_OVERFLOW_CAPTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let snapshot: Value =
        serde_json::from_str(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
    let mut program: Value =
        serde_json::from_str(record["fields"]["program_json"].as_str().unwrap()).unwrap();
    let trace: Value =
        serde_json::from_str(record["fields"]["trace_json"].as_str().unwrap()).unwrap();
    let failed = trace.as_array().unwrap().last().unwrap();
    // finish_routes/deferred comparison mutated only these fields after the
    // failed HTTP request. The recorded provider hash verifies this replay.
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array_mut()
        .unwrap()
    {
        if endpoint["id"] == "kitchens-cook-themselves" {
            endpoint["status"] = json!("imagined");
        }
    }
    let novelty = &mut program["endpoint_novelty"]["kitchens-cook-themselves"];
    novelty["status"] = json!("passed");
    novelty["final_check"] = Value::Null;
    novelty.as_object_mut().unwrap().remove("reason");
    program["tasks"] = json!([failed["task"]]);
    program["cursor"] = json!(0);
    let batch = super::super::super::batch::prepare(&snapshot, &program, 1).unwrap();
    let raw = batch.request.to_string();
    let legacy: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_BRIDGE_LEGACY_REQUEST").unwrap()).unwrap(),
    )
    .unwrap();
    let old_raw = legacy["request"].to_string();
    assert_eq!(old_raw.len(), 90961);
    assert_eq!(
        format!("{:x}", Sha256::digest(old_raw.as_bytes())),
        "13edef59bdecdeb9925c61199d11d7b1bafd6eae8d3bb98367bdb3268c3c97ae"
    );
    let old = &legacy["individual"]["state"];
    let new = &batch.individual[0]["state"];
    for key in [
        "definitions",
        "bridge",
        "branch_state",
        "ancestor_mechanisms",
        "root_connections",
        "declared_dependencies",
        "baseline",
        "source_evidence",
        "event_qualifications",
        "snapshot_branches",
        "route_context_extensions",
    ] {
        assert_eq!(new[key], old[key], "lost semantic field {key}");
    }
    for (role, old_field) in [
        ("direct_prerequisite_ids", "prerequisite_events"),
        ("ancestor_ids", "ancestor_events"),
        ("unassigned_route_event_ids", "unassigned_route_events"),
    ] {
        let ids: Vec<_> = old[old_field]
            .as_array()
            .into_iter()
            .flatten()
            .map(|n| n["Id"].clone())
            .collect();
        assert_eq!(new["event_roles"][role], json!(ids));
    }
    assert_eq!(new["event_roles"]["target_id"], old["target_event"]["Id"]);
    for (id, prior) in old["prerequisite_judgments"].as_object().unwrap() {
        let current = &new["prerequisite_judgments"][id];
        assert_eq!(current["normalized"], prior["normalized"]);
        for (function, evaluation) in prior["evaluations"].as_object().unwrap() {
            assert_eq!(
                current["evaluations"][function]["answer"],
                evaluation["answer"]
            );
            assert_eq!(
                current["evaluations"][function]["selected"],
                evaluation["selected"]
            );
            for key in ["evidence_ids", "branch_state", "probability_comparison"] {
                assert_eq!(
                    restored_context(
                        &current["evaluations"][function]["context"],
                        &new["source_evidence"]
                    )[key],
                    evaluation["context"][key]
                );
            }
        }
    }
    assert!(raw.len() < old_raw.len());
    eprintln!(
        "exact failed request {} -> {} bytes",
        old_raw.len(),
        raw.len()
    );
    if let Ok(path) = std::env::var("FORESIGHT_BRIDGE_OVERFLOW_OUTPUT") {
        std::fs::write(path,serde_json::to_vec(&json!({"request":batch.request,"individual":batch.individual[0],"task":failed["task"]})).unwrap()).unwrap();
    }
}

#[test]
fn prior_outcome_projection_preserves_unknown_context_and_raw_decisions() {
    let original = json!({"answer":{"choice":"plausible","probabilities":{"plausible":0.59}},"selected":"uncertain","future_semantic_field":"preserve","context":{"task":{"nodeId":"execution-address"},"round":4,"evidence_ids":["contrary-source"],"branch_state":{"condition":"not all"},"unknown_qualification":"only independently observed trials"}});
    let projected = outcome_context(original.clone());
    assert_eq!(projected["answer"], original["answer"]);
    assert_eq!(projected["selected"], "uncertain");
    assert_eq!(projected["future_semantic_field"], "preserve");
    for key in ["evidence_ids", "branch_state", "unknown_qualification"] {
        assert_eq!(projected["context"][key], original["context"][key]);
    }
    assert!(projected["context"].get("task").is_none());
    assert!(projected["context"].get("round").is_none());
}

fn restored_context(context: &Value, sources: &Value) -> Value {
    let mut restored = context.clone();
    if restored["assessed_against_current_sources"] == true {
        restored
            .as_object_mut()
            .unwrap()
            .remove("assessed_against_current_sources");
        restored["evidence_ids"] = json!(
            sources
                .as_array()
                .unwrap()
                .iter()
                .map(|source| source["Id"].clone())
                .collect::<Vec<_>>()
        );
    }
    restored
}
#[test]
fn current_source_provenance_requires_exact_order_and_preserves_other_context() {
    let ids = json!(["a", "b"]);
    let sources = json!([{"Id":"a"},{"Id":"b"}]);
    let evaluation = json!({"answer":{"choice":"gap","probabilities":{"gap":0.6,"uncertain":0.4}},"selected":"uncertain","context":{"evidence_ids":ids,"branch_state":{"condition":"not all"},"unknown_semantic_constraint":"public ownership","probability_comparison":{"scope":"regional"}}});
    let projected = outcomes(json!({"classify_gap":evaluation}), &ids);
    assert_eq!(
        projected["classify_gap"]["context"]["assessed_against_current_sources"],
        true
    );
    assert_eq!(
        restored_context(&projected["classify_gap"]["context"], &sources),
        evaluation["context"]
    );
    assert_eq!(projected["classify_gap"]["answer"], evaluation["answer"]);
    assert_eq!(
        projected["classify_gap"]["selected"],
        evaluation["selected"]
    );
    for historical in [json!(["b", "a"]), json!(["a", "c"]), json!(["a"])] {
        let mut old = evaluation.clone();
        old["context"]["evidence_ids"] = historical;
        assert_eq!(
            outcomes(json!({"classify_gap":old}), &ids)["classify_gap"],
            old
        );
    }
    let mut collision = evaluation.clone();
    collision["context"]["assessed_against_current_sources"] = json!(false);
    assert_eq!(
        outcomes(json!({"classify_gap":collision}), &ids)["classify_gap"],
        collision
    );
}
#[test]
fn full_prior_context_remains_bound_to_bridge_cache_identity() {
    let (snapshot, mut program, _) = fixture();
    let event = program["endpoint_search"]["bridges"]["bridge"]["from_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    // A prior receipt's provenance remains part of identity even for fields
    // intentionally absent from model state.
    program["evaluations"][&event]["classify_gap"] =
        json!({"selected":"uncertain","context":{"task":{"nodeId":"old-address"}}});
    let first =
        super::super::super::search::request(&snapshot, &program, &program["tasks"][0]).unwrap();
    program["evaluations"][&event]["classify_gap"]["context"]["task"]["nodeId"] =
        json!("new-address");
    let second =
        super::super::super::search::request(&snapshot, &program, &program["tasks"][0]).unwrap();
    assert_eq!(first["state"], second["state"]);
    assert_ne!(fingerprint(&first), fingerprint(&second));
}
#[test]
#[ignore = "requires frozen pass18 captures and exact old native request replays"]
fn captured_current_source_projection_preserves_games_and_food_semantics() {
    use sha2::{Digest, Sha256};
    let captures = std::env::var("FORESIGHT_PASS18_CAPTURE_DIR").unwrap();
    let requests = std::env::var("FORESIGHT_PASS18_REQUEST_DIR").unwrap();
    for (topic, request_file, bytes, hash) in [
        (
            "games",
            "pass18-failed-replay.json",
            87729,
            "c55d41833825e3a41c4ae4a650fb52037cc41de4b39e57af4dddf298335d9709",
        ),
        (
            "food",
            "pass18-food-replay.json",
            90252,
            "65acfd7ea5787bd14bcea2de6a36b029985962f56919c4faa6f3e41b5dba98db",
        ),
    ] {
        let record: Value =
            serde_json::from_slice(&std::fs::read(format!("{captures}/{topic}.json")).unwrap())
                .unwrap();
        let snapshot: Value =
            serde_json::from_str(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
        let mut program: Value =
            serde_json::from_str(record["fields"]["program_json"].as_str().unwrap()).unwrap();
        let canonical = program["evaluations"].clone();
        let trace: Value =
            serde_json::from_str(record["fields"]["trace_json"].as_str().unwrap()).unwrap();
        program["tasks"] = json!([trace.as_array().unwrap().last().unwrap()["task"]]);
        program["cursor"] = json!(0);
        let old: Value =
            serde_json::from_slice(&std::fs::read(format!("{requests}/{request_file}")).unwrap())
                .unwrap();
        let raw = old["request"].to_string();
        assert_eq!(raw.len(), bytes);
        assert_eq!(format!("{:x}", Sha256::digest(raw.as_bytes())), hash);
        let batch = super::super::super::batch::prepare(&snapshot, &program, 1).unwrap();
        let mut reconstructed = batch.individual[0]["state"].clone();
        let sources = reconstructed["source_evidence"].clone();
        reconstructed
            .as_object_mut()
            .unwrap()
            .remove("prior_source_provenance_contract");
        for (_, prior) in reconstructed["prerequisite_judgments"]
            .as_object_mut()
            .unwrap()
        {
            for (_, evaluation) in prior["evaluations"].as_object_mut().unwrap() {
                evaluation["context"] = restored_context(&evaluation["context"], &sources);
            }
        }
        assert_eq!(
            reconstructed, old["individual"]["state"],
            "all substantive fields roundtrip for {topic}"
        );
        assert_eq!(
            program["evaluations"], canonical,
            "canonical receipts unchanged"
        );
        assert!(batch.request.get("validation").is_none());
        assert!(batch.request.to_string().len() < bytes);
        eprintln!(
            "{topic}: {bytes} -> {} provider bytes",
            batch.request.to_string().len()
        );
    }
}

#[test]
fn context_limit_is_exact_execution_state_not_a_judgment() {
    let (snapshot, mut p, _) = fixture();
    p["endpoint_search"]["routes"] = json!([{"id":"route-a","endpoint_id":"e","commitment_id":"c","world_node_id":"r1"},{"id":"route-b","endpoint_id":"e","commitment_id":"c","world_node_id":"r2"}]);
    let task = p["tasks"][0].clone();
    let request = super::super::super::evaluation::request_task(&snapshot, &p, &task).unwrap();
    let results = p["results"].clone();
    let evaluations = p["evaluations"].clone();
    super::super::super::execution_limits::record(&mut p, &task, &request, 90000, 3, 4);
    assert_eq!(p["results"], results);
    assert_eq!(p["evaluations"], evaluations);
    let work = super::super::pending_mandatory_work(&snapshot, &p).unwrap();
    assert_eq!(work["context_limited_checks"], 2);
    assert!(
        work["route_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["function"] != "check_transition")
    );
    assert!(super::super::super::execution_limits::skip_current(&snapshot, &mut p).unwrap());
    assert!(
        super::super::super::execution_limits::skip_current(&snapshot, &mut p).unwrap(),
        "Exact bridge alias reuses inability, not a judgment"
    );
    let world = &snapshot["nodes"][2];
    let mut audit = super::super::super::search::audit_world(world, &p);
    super::super::super::execution_limits::annotate_audit(&snapshot, world, &p, &mut audit);
    assert_eq!(audit["context_limited_checks"], 1);
    assert_eq!(audit["completed_checks"], 0);
    assert_ne!(audit["status"], "no_conflict_found");
    let check = audit["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["execution"].is_object())
        .unwrap();
    assert!(check["result"].is_null() && check["probability"].is_null());
    let mut changed = snapshot.clone();
    changed["nodes"][0]["statement"] = json!("Opposite prerequisite with a changed deadline");
    assert!(super::super::super::execution_limits::lookup(&changed, &p, &task).is_none());
    let mut changed_request = request.clone();
    changed_request["model"] = json!("changed-provider-model");
    assert_ne!(
        super::super::super::execution_limits::fingerprint(&request),
        super::super::super::execution_limits::fingerprint(&changed_request)
    );
}

#[test]
fn unverified_existing_current_source_marker_is_not_reinterpreted() {
    let (snapshot, mut program, _) = fixture();
    program["evaluations"]["a"]["classify_gap"] =
        json!({"selected":"uncertain","context":{"assessed_against_current_sources":true}});
    let saved = program["evaluations"].clone();
    let error = super::super::super::search::request(&snapshot, &program, &program["tasks"][0])
        .unwrap_err();
    assert!(error.contains("unverified current-source marker"));
    assert_eq!(program["evaluations"], saved);
}
