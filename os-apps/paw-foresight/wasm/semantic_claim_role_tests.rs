// Deterministic component tests: actual planner/request/validator/batcher with
// explicit provider responses. Native acceptance must separately test Jev's judgment.
use super::*;
const META: &str = "Inferred and unverified: the most misleading single picture of a 2030 weekend would be the affluent urban early-adopter version; current evidence is segmented by remote-capable work, U.S. shoppers, and TV households, so normality may vary sharply by income, caregiving load, climate exposure, country, age, and job type.";
fn fixture() -> Value {
    json!({"world":{"description":"What will a normal weekend look like in 2030?","last_ingest_date":"2026-10-01","target_date":"2030-12-31"},"nodes":[
        {"Id":"meta","kind":"scenario","provenance":"hypothesis","statement":META,"source_refs":"[\"source\"]","resolve_by":"2030-12-31","edges":"[]","probability":"0.61"},
        {"Id":"event","kind":"revision","statement":"The city closes the central shopping street to private cars on Saturdays by 2030.","edges":"[]"},
        {"Id":"persistence","kind":"scenario","statement":"The city keeps its central shopping street open to private cars on Saturdays through 2030.","edges":"[]"},
        {"Id":"unclear","kind":"scenario","statement":"A different normal wins.","edges":"[]"},
        {"Id":"dependent","kind":"revision","statement":"The city extends Saturday pedestrian hours by 2030.","edges":"[{\"kind\":\"requires\",\"to_id\":\"meta\"}]"}
    ]})
}
fn admit(program: &mut Value) {
    for id in ["meta", "event", "persistence", "unclear", "dependent"] {
        program["results"][id] = json!({"classify_claim_role":match id {"meta"=>"context","unclear"=>"unresolved",_=>"event"},"classify_temporal":"uncertain"});
    }
    program["baseline_status"] = json!("established");
}
#[test]
fn captured_commentary_is_retained_but_never_gets_event_probability() {
    let snapshot = fixture();
    let original = snapshot.clone();
    let mut program = plan(snapshot["nodes"].as_array().unwrap()).unwrap();
    admit(&mut program);
    program["results"]["meta"]["estimate_likelihood"] = json!("0.61");
    program["results"]["meta"]["estimate_conditional"] = json!("0.73");
    program["evaluations"]["meta"]["estimate_likelihood"] = json!({"probability":0.61});
    for id in ["meta", "unclear", "dependent"] {
        assert!(!forecast_allows(&program, id));
        assert!(!branches::future_eligible(&snapshot, &program, id));
        assert!(
            evaluation::request_task(
                &snapshot,
                &program,
                &json!({"nodeId":id,"function":"estimate_likelihood"})
            )
            .is_err()
        );
    }
    for id in ["event", "persistence"] {
        assert!(forecast_allows(&program, id));
        assert!(
            evaluation::request_task(
                &snapshot,
                &program,
                &json!({"nodeId":id,"function":"estimate_likelihood"})
            )
            .is_ok()
        );
    }
    program["cursor"] = json!(
        program["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .position(|t| t["nodeId"] == "meta" && t["function"] == "estimate_likelihood")
            .unwrap()
    );
    skip_nonfuture_tasks(&mut program).unwrap();
    assert!(program["results"]["meta"]["estimate_likelihood"].is_null());
    assert!(program["results"]["meta"]["estimate_conditional"].is_null());
    assert!(program["evaluations"]["meta"]["estimate_likelihood"].is_null());
    search::plan_combinations(&snapshot, &mut program, 100);
    assert_eq!(
        program["combination_search"]["candidate_ids"],
        json!(["event", "persistence"])
    );
    assert_eq!(snapshot, original); // no deletion, paraphrase, or source-reference loss
}
#[test]
fn role_batch_is_score_free_across_depths_and_never_exports_validation_policy() {
    let snapshot = fixture();
    let mut program = plan(snapshot["nodes"].as_array().unwrap()).unwrap();
    program["results"]["meta"] =
        json!({"estimate_likelihood":"0.61","classify_temporal":"uncertain"});
    assert!(!forecast_allows(&program, "event")); // pending is not an event verdict
    let batch = batch::prepare(&snapshot, &program, 16).unwrap();
    assert_eq!(batch.tasks.len(), 5);
    assert!(
        batch
            .tasks
            .iter()
            .all(|t| t["function"] == "classify_claim_role")
    );
    assert!(batch.tasks.iter().any(|t| t["depth"] == 1));
    assert!(!batch.request.to_string().contains("selection_policy"));
    assert!(!batch.request.to_string().contains("0.61"));
    let mut response = json!({"model":MODEL,"answers":{}});
    for (index, task) in batch.tasks.iter().enumerate() {
        let role = if task["nodeId"] == "meta" {
            "context"
        } else {
            "event"
        };
        let mut distribution = json!({"event":0.3,"context":0.3,"unresolved":0.3});
        distribution[role] = json!(0.4); // deliberately below old confidence fallback
        response["answers"][batch.question_key(index)] =
            json!({"type":"choice","choice":role,"probabilities":distribution});
        let request = &batch.individual[index];
        assert_eq!(request["validation"]["selection_policy"], "provider_argmax");
        assert!(request["state"].get("assessment").is_none());
        assert!(request["state"].get("evaluations").is_none());
        if task["nodeId"] == "meta" {
            assert_eq!(request["state"]["node"]["statement"], META);
            assert_eq!(request["state"]["node"]["source_refs"], "[\"source\"]");
        }
    }
    for ((selected, value, _), task) in batch::answers(&batch, &response)
        .unwrap()
        .iter()
        .zip(&batch.tasks)
    {
        assert_eq!(
            selected,
            if task["nodeId"] == "meta" {
                "context"
            } else {
                "event"
            }
        );
        assert_eq!(value["selected"], *selected);
    }
    response["answers"][batch.question_key(0)]["choice"] = json!("unsupported");
    assert!(batch::answers(&batch, &response).is_err());
}
#[test]
fn historical_programs_and_whole_worlds_keep_recorded_estimates() {
    let snapshot = json!({"world":{},"nodes":[{"Id":"w","kind":"world","statement":"Three admitted changes jointly occur by 2030","component_ids":[],"counter_ids":[],"edges":"[]"}]});
    let mut program = json!({"stage":"worlds","claim_role_contract":1,"cursor":0,"tasks":[{"nodeId":"w","function":"estimate_likelihood"}],"results":{"w":{"estimate_likelihood":"0.37"}}});
    let original = program.clone();
    skip_nonfuture_tasks(&mut program).unwrap();
    assert_eq!(program, original);
    batch::prepare(&snapshot, &program, 16).unwrap();
    let legacy = json!({"baseline_status":"established","results":{"old":{"classify_temporal":"uncertain","estimate_likelihood":"0.61"}}});
    assert!(forecast_allows(&legacy, "old"));
    assert_eq!(legacy["results"]["old"]["estimate_likelihood"], "0.61");
}

#[test]
fn context_cannot_be_a_conditional_branch_premise() {
    let mut snapshot = fixture();
    snapshot["nodes"][1]["branch_id"] = json!("depends-on-context");
    snapshot["branches"] = json!([{"id":"depends-on-context","condition":{"kind":"all_occurring","event_ids":["meta"]},"by":"2030-12-31"}]);
    let mut program = plan(snapshot["nodes"].as_array().unwrap()).unwrap();
    admit(&mut program);
    assert!(forecast_allows(&program, "event"));
    assert!(!branches::future_eligible(&snapshot, &program, "event"));
    for role in ["unresolved", "context"] {
        program["results"]["meta"]["classify_claim_role"] = json!(role);
        assert!(!branches::future_eligible(&snapshot, &program, "event"));
    }
    program["results"]["meta"]["classify_claim_role"] = json!("event");
    assert!(branches::future_eligible(&snapshot, &program, "event"));
}

#[test]
fn exploration_admission_does_not_erase_archived_world_estimates() {
    let snapshot = fixture();
    let mut program = plan(snapshot["nodes"].as_array().unwrap()).unwrap();
    program["results"]["world-r1-old"] = json!({"estimate_likelihood":"0.37"});
    program["evaluations"]["world-r1-old"] = json!({"estimate_likelihood":{"probability":0.37}});
    skip_nonfuture_tasks(&mut program).unwrap();
    assert_eq!(
        program["results"]["world-r1-old"]["estimate_likelihood"],
        "0.37"
    );
    assert_eq!(
        program["evaluations"]["world-r1-old"]["estimate_likelihood"]["probability"],
        0.37
    );
}
