use super::*;

fn fixture() -> (Value, Value) {
    let snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2028-01-01"},"nodes":[
        {"Id":"a","kind":"scenario","statement":"A occurs","edges":"[]"},
        {"Id":"b","kind":"scenario","statement":"B occurs","edges":"[]"},
        {"Id":"c","kind":"scenario","statement":"C occurs","edges":"[]"},
        {"Id":"source","kind":"evidence","statement":"Observed fact","edges":"[]"},
        {"Id":"w","kind":"world","statement":"A B C occur together","component_ids":["a","b","c"],"counter_ids":[],"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","mechanism":"A enables B","by":"2027-01-01"}],"assumptions":[],"edges":"[]"}]});
    let program = json!({"audit_policy_version":2,"active_world_ids":["w"],"world_pass":1,"results":{},"evaluations":{},"baseline":{"as_of":"2026-10-01"},"evidence_ids":["source"]});
    (snapshot, program)
}
fn fill(snapshot: &Value, program: &mut Value) {
    for world in snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|world| world["kind"] == "world")
    {
        for task in audit_tasks(world, program) {
            let id = field(&task, "nodeId");
            let function = field(&task, "function");
            let result = match function {
                "estimate_likelihood" | "conditional_on" | "conditional_off" => "0.3",
                "check_transition" => "uncertain",
                _ => "compatible",
            };
            program["results"][id][function] = json!(result);
            if function != "estimate_likelihood" {
                let input = request(snapshot, program, &task).unwrap();
                program["evaluations"][id][function] = json!({"selected":result,"context":{"audit_input_fingerprint":audit_input_fingerprint(snapshot,&task,&input),"world_pass":program["world_pass"]}});
            } else {
                program["evaluations"][id][function] = json!({"type":"noul","probability":0.3,"context":{"world_pass":program["world_pass"]}});
            }
        }
    }
}
#[test]
fn mandatory_coverage_precedes_conditional_depth_and_legacy_is_unchanged() {
    let (snapshot, program) = fixture();
    let world = &snapshot["nodes"][4];
    let planned = audit_tasks(world, &program);
    assert_eq!(planned.len(), 6);
    assert_eq!(
        planned
            .iter()
            .filter(|task| task["function"] == "check_pair")
            .count(),
        3
    );
    assert_eq!(planned.last().unwrap()["function"], "estimate_likelihood");
    assert!(!planned.iter().any(is_diagnostic));
    let mut legacy = program.clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("audit_policy_version");
    assert_eq!(audit_tasks(world, &legacy), world_tasks(world));
    assert_eq!(audit_tasks(world, &legacy).len(), 8);
    assert_eq!(audit_diagnostics(world, &program)["status"], "not_run");
    let mut route = world.clone();
    route["route_only"] = json!(true);
    let route_tasks = mandatory_audit_tasks(&route, &program);
    assert_eq!(route_tasks.len(), 2);
    assert_eq!(route_tasks[0]["function"], "check_route_grounding");
    assert_eq!(route_tasks[1]["function"], "check_transition");
}
#[test]
fn unresolved_diagnostics_are_selected_only_after_all_worlds_complete() {
    let (mut snapshot, mut program) = fixture();
    let mut second = snapshot["nodes"][4].clone();
    second["Id"] = json!("second");
    snapshot["nodes"].as_array_mut().unwrap().push(second);
    program["active_world_ids"] = json!(["w", "second"]);
    fill(&snapshot, &mut program);
    let mut incomplete = program.clone();
    incomplete["results"]["second"]
        .as_object_mut()
        .unwrap()
        .remove("estimate_likelihood");
    assert!(!refine_worlds(&snapshot, &mut incomplete, 20, 1000, ""));
    assert!(incomplete["audit_diagnostic_plan"].is_null());
    assert!(refine_worlds(&snapshot, &mut program, 20, 1000, ""));
    assert_eq!(
        program["world_refinement"]["w"]["rounds"][0]["complete"],
        true
    );
    assert_eq!(
        program["world_refinement"]["w"]["rounds"][0]["diagnostics"]["status"],
        "not_run"
    );
    assert_eq!(
        program["audit_diagnostic_plan"]["w"]["link_ids"],
        json!(["ab"])
    );
    assert_eq!(
        program["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|task| is_diagnostic(task))
            .count(),
        4
    );
    assert_eq!(
        audit_diagnostics(&snapshot["nodes"][4], &program)["status"],
        "incomplete"
    );
}
#[test]
fn skipped_mandatory_transition_is_not_a_completed_audit() {
    let (snapshot, mut program) = fixture();
    fill(&snapshot, &mut program);
    program["results"]["w/link/ab"]
        .as_object_mut()
        .unwrap()
        .remove("check_transition");
    let audit = audit_world(&snapshot["nodes"][4], &program);
    assert_eq!(audit["planned_checks"], 5);
    assert_eq!(audit["completed_checks"], 4);
    assert_eq!(audit["status"], "uncertain");
    assert!(refine_worlds(&snapshot, &mut program, 20, 1000, ""));
    assert!(
        program["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["function"] == "check_transition")
    );
    assert_eq!(
        program["world_refinement"]["w"]["rounds"][0]["complete"],
        false
    );
    assert!(program["audit_diagnostic_plan"].is_null());
}
#[test]
fn changed_source_invalidates_required_checks_before_budget_refusal() {
    let (mut snapshot, mut program) = fixture();
    fill(&snapshot, &mut program);
    snapshot["nodes"][3]["statement"] = json!("Changed observed fact");
    assert!(!refine_worlds(
        &snapshot,
        &mut program,
        20,
        1000,
        "provider_error"
    ));
    assert!(program["results"]["w/link/ab"]["check_transition"].is_null());
    assert!(program["results"]["w"]["estimate_likelihood"].is_null());
    assert_eq!(
        audit_world(&snapshot["nodes"][4], &program)["completed_checks"],
        0
    );
}
#[test]
fn pending_work_uses_executor_candidate_policy_and_exposes_contingency() {
    let (snapshot, mut program) = fixture();
    program["world_search_contract"] = json!(1);
    program["endpoint_search"] =
        json!({"backward_batch_contract":2,"endpoints":[{"id":"unexamined-endpoint"}],"routes":[]});
    program["endpoint_novelty"] = json!({"unexamined-endpoint":{"status":"provisional"}});
    let workload = super::super::endpoints::pending_mandatory_work(&snapshot, &program).unwrap();
    let tasks = workload["candidate_tasks"].as_array().unwrap();
    assert!(tasks.iter().all(|task| !matches!(
        field(task, "function"),
        "evaluate_novelty" | "decision_value"
    )));
    assert_eq!(tasks.len(), 13);
    assert!(workload["contingent_questions"].as_u64().unwrap() > 0);
    assert_eq!(workload["questions"], 14);
    assert_eq!(
        workload["conservative_http_requests"].as_u64().unwrap(),
        workload["known_packed_http_requests"].as_u64().unwrap()
            + workload["contingent_questions"].as_u64().unwrap()
    );
    assert_eq!(workload["packing_known"], false);
}

#[test]
#[ignore = "requires captured pass14 input via FORESIGHT_AUDIT_POLICY_CAPTURE"]
fn captured_policy_reduces_initial_work_without_skipping_mandatory_checks() {
    let record: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_AUDIT_POLICY_CAPTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let snapshot: Value =
        serde_json::from_str(record["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
    let legacy: Value =
        serde_json::from_str(record["fields"]["program_json"].as_str().unwrap()).unwrap();
    let mut program = legacy.clone();
    program["audit_policy_version"] = json!(2);
    program
        .as_object_mut()
        .unwrap()
        .remove("audit_diagnostic_plan");
    let mut old_count = 0;
    let mut new_count = 0;
    for world in snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "world")
    {
        let old = audit_tasks(world, &legacy);
        let new = audit_tasks(world, &program);
        assert_eq!(
            new,
            old.iter()
                .filter(|task| !is_diagnostic(task))
                .cloned()
                .collect::<Vec<_>>()
        );
        old_count += old.len();
        new_count += new.len();
    }
    eprintln!(
        "captured initial audit questions {old_count} -> {new_count}; diagnostics deferred, not passed"
    );
    assert!(new_count * 2 < old_count);
    super::super::endpoints::finish_routes(&snapshot, &mut program);
    let world = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["Id"] == "world-r1-travelling_companions")
        .unwrap();
    let audit = audit_world(world, &program);
    assert_eq!(audit["diagnostics"]["status"], "not_run");
    assert!(
        audit["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| !matches!(
                check["kind"].as_str(),
                Some("conditional_on" | "conditional_off")
            ))
    );
    if let Ok(path) = std::env::var("FORESIGHT_AUDIT_POLICY_OUTPUT") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"snapshot":snapshot,"program":program,"audit":audit,"disclosure":"Captured native judgments projected through the new Rust audit policy; not a live policy-v2 run."})).unwrap()).unwrap();
    }
}
