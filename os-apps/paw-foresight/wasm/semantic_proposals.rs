// Preliminary whole-world proposals are judged before they become frozen endpoints.
// These are fallible model classifications, never proofs of novelty or truth.
use super::field;
use serde_json::{Value, json};

pub fn is_task(task: &Value) -> bool {
    matches!(
        field(task, "function"),
        "check_endpoint_delta" | "check_endpoint_set"
    )
}
fn tasks(attempt: &Value) -> Vec<Value> {
    let number = attempt["attempt"].as_u64().unwrap_or(0);
    let mut tasks: Vec<_>=attempt["endpoints"].as_array().into_iter().flatten().map(|endpoint|json!({"nodeId":format!("proposal-{number}-endpoint-{}",field(endpoint,"id")),"function":"check_endpoint_delta","proposal_attempt":number,"endpoint_id":endpoint["id"],"depth":0})).collect();
    tasks.push(json!({"nodeId":format!("proposal-{number}-set"),"function":"check_endpoint_set","proposal_attempt":number,"depth":0}));
    tasks
}
fn attempt<'a>(program: &'a Value, task: &Value) -> Option<&'a Value> {
    std::iter::once(&program["endpoint_proposal_attempt"])
        .chain(
            program["endpoint_proposal_history"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .find(|a| a["attempt"] == task["proposal_attempt"] && tasks(a).iter().any(|t| t == task))
}
pub fn valid_task(program: &Value, task: &Value) -> bool {
    is_task(task) && attempt(program, task).is_some()
}

pub fn plan(snapshot: &Value, old: &Value, mut endpoints: Vec<Value>) -> Result<Value, String> {
    if old["endpoint_search"].is_object() {
        return Err("Accepted original endpoints cannot be reproposed".into());
    }
    let number = old["endpoint_proposal_history"]
        .as_array()
        .map_or(0, Vec::len)
        + 1;
    if number > 3 {
        return Err("Endpoint proposal quality exhausted three attempts".into());
    }
    if old["endpoint_proposal_attempt"]["status"] == "checking" {
        return Err("Evaluate the pending proposal before replacing it".into());
    }
    for endpoint in &mut endpoints {
        endpoint.as_object_mut().unwrap().remove("status");
    }
    let attempt = json!({"attempt":number,"status":"checking","endpoints":endpoints,"baseline":old["baseline"],"source_evidence_ids":snapshot["nodes"].as_array().into_iter().flatten().filter(|n|matches!(field(n,"kind"),"evidence"|"research_evidence")).map(|n|n["Id"].clone()).collect::<Vec<_>>(),"checks":[]});
    let mut program = old.clone();
    program["tasks"] = json!(tasks(&attempt));
    program["cursor"] = json!(0);
    program["stage"] = json!("proposals");
    program["endpoint_proposal_attempt"] = attempt;
    if !program["endpoint_proposal_history"].is_array() {
        program["endpoint_proposal_history"] = json!([]);
    }
    program
        .as_object_mut()
        .unwrap()
        .remove("response_correction");
    Ok(program)
}
fn criteria(function: &str) -> Value {
    if function == "check_endpoint_delta" {
        json!({
            "consequential_change":"The distinguishing commitments specify a materially different future arrangement and interacting consequences beyond the supplied present. A specified adoption change counts only when its concrete consequences distinguish the world; missing source coverage alone is not novelty.",
            "consequential_persistence":"Under explicitly changed surrounding conditions, the world explains materially different consequences of an arrangement persisting. Merely saying people still supervise current tools or continue current practices does not qualify.",
            "present_or_adoption_only":"The defining proposal largely relabels capabilities or arrangements already in the sourced present, bundles current tools, adds a future date or longer scene, or suggests unspecified wider adoption without a materially different consequential world.",
            "unresolved":"The supplied commitments or baseline do not establish a distinguishable consequential future; do not invent a delta or equate missing evidence with novelty."
        })
    } else {
        json!({
            "alternative_trajectories":"The proposals are rich alternative trajectories answering the same question, with different interacting arrangements and consequences. They may overlap; they need not be mutually exclusive or span prescribed axes.",
            "complementary_slices":"The proposals principally cover separate topics, tools, actors or facets that could be parts of one world, rather than distinct whole future trajectories.",
            "duplicates":"Different wording or scenes disguise substantially the same trajectory and consequences.",
            "unresolved":"The proposals do not provide enough defined interacting consequences to distinguish alternative whole trajectories."
        })
    }
}
pub fn request(snapshot: &Value, program: &Value, task: &Value) -> Result<Value, String> {
    let attempt = attempt(program, task).ok_or("Unknown preliminary endpoint assessment")?;
    let proposal = if task["endpoint_id"].is_string() {
        attempt["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["id"] == task["endpoint_id"])
            .cloned()
            .ok_or("Missing endpoint proposal")?
    } else {
        Value::Null
    };
    let request = json!({"model":super::MODEL,"state":{"world":snapshot["world"],"baseline":attempt["baseline"],"source_evidence":snapshot["nodes"].as_array().into_iter().flatten().filter(|n|attempt["source_evidence_ids"].as_array().into_iter().flatten().any(|id|*id==n["Id"])).collect::<Vec<_>>(),"proposal":proposal,"proposals":if task["function"]=="check_endpoint_set" {attempt["endpoints"].clone()}else{Value::Null}},"questions":{"result":{"type":"choice","instructions":"Judge the exact distinguishing commitments and interacting consequences against the supplied sourced present, preserving dates, scope and source caveats. This screens proposal quality, not likelihood, truth, feasibility or global novelty. A longer scene, future year, vague broader adoption or bundle of present tools is not itself a consequential change. Do not reward technological optimism or force pessimistic/optimistic categories. Persistence can matter when surrounding changes produce materially different consequences. Evaluate what is actually specified; never rescue a weak proposal by inventing its missing delta. For the set, compare whole trajectories rather than rewarding coverage of different research topics.","criteria":criteria(field(task,"function"))}},"validation":{"selection_policy":"provider_argmax"}});
    if request.to_string().len() > 128 * 1024 {
        return Err("Endpoint proposal quality request exceeds context budget".into());
    }
    Ok(request)
}

/// Persist each completed attempt once. Resource admission is computed by the
/// native scheduler using the original run clock and transition count.
pub fn finish(
    program: &mut Value,
    allow_retry: bool,
    resource_exhausted: bool,
) -> Result<(), String> {
    let mut attempt = program["endpoint_proposal_attempt"].clone();
    if attempt["status"] != "checking" {
        return Ok(());
    }
    let mut checks = vec![];
    let mut accepted = true;
    for task in tasks(&attempt) {
        let result = program["results"][field(&task, "nodeId")][field(&task, "function")].clone();
        let receipt = &program["evaluations"][field(&task, "nodeId")][field(&task, "function")];
        let recorded = receipt["type"] == "choice"
            && receipt["selected"] == result
            && receipt["answer"]["choice"] == result
            && result.is_string();
        let allowed = if task["function"] == "check_endpoint_delta" {
            matches!(
                result.as_str(),
                Some("consequential_change" | "consequential_persistence")
            )
        } else {
            result == "alternative_trajectories"
        };
        accepted &= allowed && recorded;
        checks.push(json!({"task_id":task["nodeId"],"function":task["function"],"endpoint_id":task.get("endpoint_id").cloned().unwrap_or(Value::Null),"result":if recorded {result.clone()}else{Value::Null},"evaluation":if recorded{receipt.clone()}else{Value::Null},"critique":if recorded{criteria(field(&task,"function"))[result.as_str().unwrap_or("")].clone()}else{Value::Null}}));
    }
    attempt["checks"] = json!(checks);
    attempt["status"] = json!(if accepted {
        "accepted"
    } else if allow_retry && attempt["attempt"].as_u64().unwrap_or(3) < 3 {
        "revision_requested"
    } else {
        "unresolved"
    });
    attempt["reason"] = json!(if accepted {
        "proposal_quality"
    } else if resource_exhausted || !allow_retry {
        "resource_limit"
    } else if attempt["attempt"].as_u64().unwrap_or(3) >= 3 {
        "attempt_limit"
    } else {
        "proposal_quality"
    });
    if accepted {
        let mut endpoints = attempt["endpoints"].clone();
        for endpoint in endpoints.as_array_mut().unwrap() {
            endpoint["status"] = json!("imagined");
        }
        program["endpoint_search"] = json!({"status":"imagined","backward_batch_contract":2,"endpoints":endpoints,"routes":[],"amendments":[],"rounds":[]});
        program["stage"] = json!("exploration");
    }
    program["endpoint_proposal_history"]
        .as_array_mut()
        .ok_or("Missing proposal history")?
        .push(attempt.clone());
    program["endpoint_proposal_attempt"] = attempt;
    program["tasks"] = json!([]);
    program["cursor"] = json!(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Vec<Value>) {
        let snapshot = json!({"world":{"description":"How might creative work change?","last_ingest_date":"2026-10-02","target_date":"2030-12-31"},"nodes":[{"Id":"source","kind":"evidence","statement":"Supervised assistance and integrated tools already exist","edges":"[]"}]});
        let program = json!({"world_search_contract":1,"endpoint_proposal_contract":1,"claim_role_contract":1,"baseline":{"as_of":"2026-10-02","observed":[{"claim":"Supervised assistance and integrated tools already exist","evidence_ids":["source"]}],"assumptions":[],"unknowns":[]},"results":{},"evaluations":{},"rounds":[],"started_at_ms":"12345"});
        let endpoint = json!({"id":"one","title":"A proposed world","original_statement":"Creators supervise assistants by2030","original_narrative":"An imagined ordinary day with interacting consequences","commitments":[{"id":"a","statement":"Creators supervise assistants"},{"id":"b","statement":"Tools integrate workflows"},{"id":"c","statement":"People publish in platforms"}],"signals":["A change appears"],"falsifiers":["The mechanism fails"]});
        let mut second = endpoint.clone();
        second["id"] = json!("two");
        (snapshot, program, vec![endpoint, second])
    }
    fn record(snapshot: &Value, program: &mut Value, delta: &str, set: &str) {
        for task in program["tasks"].as_array().unwrap().clone() {
            let request = request(snapshot, program, &task).unwrap();
            let selected = if task["function"] == "check_endpoint_delta" {
                delta
            } else {
                set
            };
            let probabilities: serde_json::Map<String, Value> =
                request["questions"]["result"]["criteria"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|key| (key.clone(), json!(if key == selected { 0.4 } else { 0.2 })))
                    .collect();
            // All choice groups have four entries: selected0.4 still passes
            // provider argmax validation without an arbitrary0.65 cutoff.
            let response = json!({"model":super::super::MODEL,"answers":{"result":{"type":"choice","choice":selected,"probabilities":probabilities}}});
            let value = super::super::evaluation::validate(&request, &response).unwrap();
            let receipt = super::super::evaluation::evaluation_value(&request, &response).unwrap();
            program["results"][field(&task, "nodeId")][field(&task, "function")] = json!(value);
            program["evaluations"][field(&task, "nodeId")][field(&task, "function")] = receipt;
        }
    }
    #[test]
    fn present_topic_slices_are_revised_before_any_endpoint_is_frozen() {
        let (s, p, endpoints) = fixture();
        let mut checking =
            super::super::endpoints::imagine(&s, &p, &json!({"endpoints":endpoints})).unwrap();
        assert!(checking["endpoint_search"].is_null());
        assert_eq!(checking["stage"], "proposals");
        let request = request(&s, &checking, &checking["tasks"][0]).unwrap();
        assert_eq!(request["state"]["baseline"], p["baseline"]);
        assert_eq!(
            request["state"]["proposal"]["commitments"],
            endpoints[0]["commitments"]
        );
        let batch = super::super::batch::prepare(&s, &checking, 3).unwrap();
        assert_eq!(batch.tasks.len(), 2);
        assert!(batch.request.get("validation").is_none());
        record(
            &s,
            &mut checking,
            "present_or_adoption_only",
            "complementary_slices",
        );
        finish(&mut checking, true, false).unwrap();
        assert_eq!(
            checking["endpoint_proposal_attempt"]["status"],
            "revision_requested"
        );
        assert!(checking["endpoint_search"].is_null());
        assert_eq!(checking["started_at_ms"], "12345");
        let previous = checking["endpoint_proposal_history"][0].clone();
        let mut revised = plan(&s, &checking, endpoints).unwrap();
        record(
            &s,
            &mut revised,
            "consequential_change",
            "alternative_trajectories",
        );
        finish(&mut revised, true, false).unwrap();
        assert_eq!(revised["endpoint_proposal_history"][0], previous);
        assert_eq!(revised["endpoint_proposal_attempt"]["status"], "accepted");
        assert_eq!(
            revised["endpoint_search"]["endpoints"][0]["status"],
            "imagined"
        );
        assert!(plan(&s, &revised, vec![]).is_err());
        if let Ok(path) = std::env::var("FORESIGHT_PROPOSAL_FIXTURE") {
            std::fs::write(path,serde_json::to_string_pretty(&json!({"checking":super::super::endpoints::imagine(&s,&p,&json!({"endpoints":revised["endpoint_proposal_attempt"]["endpoints"]})).unwrap(),"rejected":checking,"accepted":revised,"snapshot":s})).unwrap()).unwrap();
        }
    }
    #[test]
    fn unresolved_missing_receipts_and_exhaustion_never_admit_proposals() {
        let (s, p, endpoints) = fixture();
        let mut current = plan(&s, &p, endpoints.clone()).unwrap();
        record(
            &s,
            &mut current,
            "consequential_persistence",
            "alternative_trajectories",
        );
        current["evaluations"] = json!({});
        finish(&mut current, true, false).unwrap();
        assert_eq!(
            current["endpoint_proposal_attempt"]["checks"][0]["result"],
            Value::Null
        );
        assert!(current["endpoint_search"].is_null());
        for _ in 0..2 {
            current = plan(&s, &current, endpoints.clone()).unwrap();
            record(&s, &mut current, "unresolved", "unresolved");
            finish(&mut current, true, false).unwrap();
        }
        assert_eq!(current["endpoint_proposal_attempt"]["status"], "unresolved");
        assert_eq!(
            current["endpoint_proposal_attempt"]["reason"],
            "attempt_limit"
        );
        assert_eq!(
            current["endpoint_proposal_history"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(plan(&s, &current, endpoints.clone()).is_err());
        let mut resource = plan(&s, &p, endpoints).unwrap();
        finish(&mut resource, false, true).unwrap();
        assert_eq!(
            resource["endpoint_proposal_attempt"]["reason"],
            "resource_limit"
        );
        assert!(resource["endpoint_search"].is_null());
    }
    #[test]
    fn endpoint_named_set_does_not_collide_with_set_judgment() {
        let (snapshot, program, mut endpoints) = fixture();
        endpoints[0]["id"] = json!("set");
        let planned = plan(&snapshot, &program, endpoints).unwrap();
        let tasks = planned["tasks"].as_array().unwrap();
        let ids: std::collections::BTreeSet<_> =
            tasks.iter().map(|task| field(task, "nodeId")).collect();
        assert_eq!(ids.len(), tasks.len());
        assert!(tasks.iter().all(|task| valid_task(&planned, task)));
    }

    #[test]
    fn stored_proposal_tasks_are_validated_without_fabricated_snapshot_nodes() {
        let (s, p, endpoints) = fixture();
        let mut program = plan(&s, &p, endpoints).unwrap();
        let original = program["tasks"][0].clone();
        assert!(valid_task(&program, &original));
        let mut bad = original.clone();
        bad["endpoint_id"] = json!("unknown");
        assert!(!valid_task(&program, &bad));
        bad = original.clone();
        bad["nodeId"] = json!("other");
        assert!(!valid_task(&program, &bad));
        record(
            &s,
            &mut program,
            "consequential_change",
            "alternative_trajectories",
        );
        finish(&mut program, true, false).unwrap();
        assert!(valid_task(&program, &original));
    }
}
