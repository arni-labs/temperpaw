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
