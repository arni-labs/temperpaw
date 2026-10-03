use serde_json::{Value, json};
fn fixture() -> (Value, Value) {
    let snapshot = json!({"world":{"description":"Question"},"nodes":[
 {"Id":"grandchild","kind":"scenario","statement":"Later event","edges":"[{\"kind\":\"requires\",\"to_id\":\"child\"}]"},
 {"Id":"child","kind":"scenario","statement":"Dependent event","edges":"[{\"kind\":\"requires\",\"to_id\":\"parent\"}]"},
 {"Id":"parent","kind":"scenario","statement":"Prior event","edges":"[]"},
 {"Id":"unrelated","kind":"scenario","statement":"Unrelated event","edges":"[]"}]});
    let mut program = json!({"world_search_contract":1,"baseline":{},"results":{"parent":{"estimate_likelihood":"0.2"}},"evaluations":{}});
    let before = program.clone();
    super::invalidate_changed_candidates(&snapshot, &mut program, &before);
    for (id, probability) in [("child", "0.3"), ("grandchild", "0.1")] {
        let task = json!({"nodeId":id,"function":"estimate_likelihood"});
        let request = super::super::evaluation::request_task(&snapshot, &program, &task).unwrap();
        let fingerprint =
            super::super::evaluation::prerequisite_input_fingerprint(&request).unwrap();
        assert_eq!(
            Some(fingerprint.clone()),
            super::super::evaluation::candidate_prerequisite_fingerprint(&snapshot, &program, id)
                .unwrap()
        );
        program["results"][id]["estimate_likelihood"] = json!(probability);
        program["evaluations"][id]["estimate_likelihood"] = json!({"type":"noul","probability":probability,"context":{"prerequisite_input_fingerprint":fingerprint}});
    }
    (snapshot, program)
}
#[test]
fn changed_prerequisite_evaluation_invalidates_child_and_descendant_regardless_of_order() {
    let (snapshot, mut program) = fixture();
    let before = program.clone();
    program["results"]["parent"]["estimate_likelihood"] = json!("0.8");
    super::invalidate_changed_candidates(&snapshot, &mut program, &before);
    assert!(program["results"]["child"]["estimate_likelihood"].is_null());
    assert!(program["evaluations"]["child"]["estimate_likelihood"].is_null());
    assert!(program["results"]["grandchild"]["estimate_likelihood"].is_null());
    assert_eq!(program["results"]["parent"]["estimate_likelihood"], "0.8");
}
#[test]
fn unchanged_shared_candidate_and_unrelated_judgment_reuse_exact_assessments() {
    let (snapshot, mut program) = fixture();
    let before = program.clone();
    program["results"]["unrelated"]["estimate_likelihood"] = json!("0.9");
    program["endpoint_search"]["routes"] = json!([{"id":"route-a","component_ids":["parent","child"]},{"id":"route-b","component_ids":["parent","child"]}]);
    super::invalidate_changed_candidates(&snapshot, &mut program, &before);
    for id in ["child", "grandchild"] {
        for key in ["results", "evaluations"] {
            assert_eq!(program[key][id], before[key][id]);
        }
    }
}
#[test]
fn old_assessment_without_recorded_prerequisite_input_is_not_assumed_current() {
    let (snapshot, mut program) = fixture();
    program["evaluations"]["child"]["estimate_likelihood"]["context"]
        .as_object_mut()
        .unwrap()
        .clear();
    let before = program.clone();
    super::invalidate_changed_candidates(&snapshot, &mut program, &before);
    assert!(program["results"]["child"]["estimate_likelihood"].is_null());
}
#[test]
fn evidence_set_encoding_does_not_change_prerequisite_fingerprint() {
    let ids: Vec<_> = (0..30).map(|i| format!("evidence-{i}")).collect();
    let mut request = json!({"state":{"prerequisites":[{"id":"p","evaluations":{"estimate_likelihood":{"context":{"evidence_ids":ids}}}}]}});
    let before = super::super::evaluation::prerequisite_input_fingerprint(&request);
    super::super::evaluation::compact_evaluation_contexts(&mut request["state"]);
    assert!(request["state"]["evidence_sets"].is_array());
    assert_eq!(
        before,
        super::super::evaluation::prerequisite_input_fingerprint(&request)
    );
}

#[test]
fn admission_without_prerequisite_judgments_survives_parent_reassessment() {
    let (snapshot, mut program) = fixture();
    program["results"]["child"]["classify_temporal"] = json!("future_change");
    program["results"]["child"]["classify_claim_role"] = json!("event");
    let before = program.clone();
    program["results"]["parent"]["estimate_likelihood"] = json!("0.8");
    super::invalidate_changed_candidates(&snapshot, &mut program, &before);
    assert_eq!(
        program["results"]["child"]["classify_temporal"],
        "future_change"
    );
    assert_eq!(program["results"]["child"]["classify_claim_role"], "event");
    assert!(program["results"]["child"]["estimate_likelihood"].is_null());
}
