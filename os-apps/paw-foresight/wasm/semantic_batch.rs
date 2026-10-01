// Independent questions share payloads without changing their individual contexts.
use serde_json::{Value, json};
pub struct Batch {
    pub request: Value,
    pub tasks: Vec<Value>,
    pub individual: Vec<Value>,
}
impl Batch {
    pub fn question_key(&self, index: usize) -> String {
        if self.request["state"]["cases"].is_object() {
            format!("q{index}")
        } else {
            "result".into()
        }
    }
}
/// A provider-confirmed token overflow only changes packing, never context or tasks.
pub fn reduce_cap(program: &mut Value, batch: &Batch) -> bool {
    if batch.tasks.len() <= 1 {
        return false;
    }
    let old = program["batch_byte_cap"].as_u64().unwrap_or(128 * 1024) as usize;
    let next = old.min(batch.request.to_string().len()) / 2;
    if next == 0 || next >= old {
        return false;
    }
    program["batch_byte_cap"] = json!(next);
    true
}
pub fn prepare(snapshot: &Value, program: &Value, remaining: usize) -> Result<Batch, String> {
    let cap = program["batch_byte_cap"]
        .as_u64()
        .unwrap_or(128 * 1024)
        .min(128 * 1024) as usize;
    let cursor = program["cursor"].as_u64().ok_or("Missing cursor")? as usize;
    let tasks = program["tasks"].as_array().ok_or("Missing tasks")?;
    let first = tasks.get(cursor).ok_or("Task cursor exhausted")?;
    let structural = super::search::is_structural(first);
    let mut batch = Batch {
        request: json!({"model":super::MODEL,"state":{"common":{},"cases":{}},"questions":{}}),
        tasks: vec![],
        individual: vec![],
    };
    for (_index, task) in tasks
        .iter()
        .enumerate()
        .take(tasks.len().min(cursor + 16).min(cursor + remaining))
        .skip(cursor)
    {
        if (!batch.tasks.is_empty()
            && ((first["function"] == "check_world_set" || task["function"] == "check_world_set")
                && !(first["focal_world_id"].is_string() && task["focal_world_id"].is_string())))
            || structural != super::search::is_structural(task)
            || (!structural
                && (task["function"] != first["function"]
                    || (task["function"] != "classify_temporal"
                        && task["depth"] != first["depth"])))
        {
            break;
        }
        // An ineligible item ends the contiguous batch; native skipping advances
        // it on the next call without consuming a provider judgment.
        if !structural
            && program["stage"] != "worlds"
            && matches!(
                super::field(task, "function"),
                "estimate_likelihood"
                    | "estimate_conditional"
                    | "evaluate_novelty"
                    | "decision_value"
            )
            && !super::temporal_allows_forecast(program, super::field(task, "nodeId"))
        {
            break;
        }
        // Structural requests take their task explicitly; avoid cloning the whole
        // accumulated program for every independent question.
        let individual = super::evaluation::request_task(snapshot, program, task)?;
        let key = format!("q{}", batch.tasks.len());
        let mut state = individual["state"].clone();
        let mut common = json!({});
        for field in [
            "world_question",
            "source_evidence",
            "baseline",
            "world",
            "previous_world_judgments",
            "proposed_worlds",
            "components",
            "shared_question",
        ] {
            if let Some(value) = state.as_object_mut().and_then(|s| s.remove(field)) {
                common[field] = value;
            }
        }
        // Shared context is identical within this immutable checkpoint.
        if !batch.tasks.is_empty() && batch.request["state"]["common"] != common {
            break;
        }
        let mut candidate = batch.request.clone();
        candidate["state"]["common"] = common;
        if let Some(comparisons) = state.get("comparisons").and_then(Value::as_array).cloned()
            && !comparisons.is_empty()
        {
            if !candidate["state"]["comparison_catalog"].is_array() {
                candidate["state"]["comparison_catalog"] = json!([]);
            }
            let catalog = candidate["state"]["comparison_catalog"]
                .as_array_mut()
                .unwrap();
            let indices: Vec<_> = comparisons
                .into_iter()
                .map(|comparison| {
                    if let Some(index) = catalog.iter().position(|value| value == &comparison) {
                        index
                    } else {
                        catalog.push(comparison);
                        catalog.len() - 1
                    }
                })
                .collect();
            state.as_object_mut().unwrap().remove("comparisons");
            state["comparison_refs"] = json!(indices);
        }
        // Intern whole values, not IDs: two revisions of the same event must
        // remain distinct. Catalog entries are data, never extra case premises.
        for field in [
            "events",
            "prerequisite_events",
            "ancestor_events",
            "unassigned_route_events",
        ] {
            if let Some(values) = state.get(field).and_then(Value::as_array).cloned()
                && !values.is_empty()
            {
                let refs: Vec<_> = values
                    .into_iter()
                    .map(|value| intern_event(&mut candidate["state"], value))
                    .collect();
                state.as_object_mut().unwrap().remove(field);
                state[format!("{field}_refs")] = json!(refs);
            }
        }
        if let Some(value) = state.as_object_mut().unwrap().remove("target_event") {
            state["target_event_ref"] = json!(intern_event(&mut candidate["state"], value));
        }
        candidate["state"]["cases"][&key] = state;
        let mut question = individual["questions"]["result"].clone();
        question["instructions"] = json!(format!(
            "For this question, first expand state.cases.{key}: if it contains comparison_refs, replace that field with comparisons containing exactly the entries of state.comparison_catalog at those zero-based indices, in order. Then state means ONLY that expanded case combined with state.common; no other catalog entries belong to this case. This is lossless reference encoding, not additional evidence. Other cases are separate hypothetical questions, not assumed facts. {}",
            super::field(&question, "instructions")
        ));
        if candidate["state"]["event_catalog"].is_array() {
            question["instructions"] = json!(format!(
                "Expand ONLY state.cases.{key}: replace comparison_refs with comparisons from state.comparison_catalog; replace events_refs, prerequisite_events_refs, ancestor_events_refs and unassigned_route_events_refs with their named arrays from state.event_catalog; replace target_event_ref with target_event from state.event_catalog. Indices are zero-based; preserve exact values and array order. Combine the expanded case with state.common. No unreferenced catalog entries or other cases are assumed facts. This is lossless encoding, not additional evidence. {}",
                super::field(&individual["questions"]["result"], "instructions")
            ));
        }
        candidate["questions"][&key] = question;
        let candidate_bytes = candidate.to_string().len();
        if candidate_bytes > cap && !batch.tasks.is_empty() {
            break;
        }
        if candidate_bytes > 128 * 1024 {
            if batch.tasks.is_empty() {
                return Err("Batched semantic request exceeds 128 KB".into());
            }
            break;
        }
        batch.request = candidate;
        batch.tasks.push(task.clone());
        batch.individual.push(individual);
    }
    if batch.tasks.is_empty() {
        return Err("No question budget remains".into());
    }
    Ok(batch)
}
fn intern_event(state: &mut Value, value: Value) -> usize {
    if !state["event_catalog"].is_array() {
        state["event_catalog"] = json!([]);
    }
    let catalog = state["event_catalog"].as_array_mut().unwrap();
    if let Some(index) = catalog.iter().position(|entry| entry == &value) {
        index
    } else {
        catalog.push(value);
        catalog.len() - 1
    }
}

/// Validate every answer before advancing any cursor; malformed fan-out stays retryable.
pub fn answers(batch: &Batch, response: &Value) -> Result<Vec<(String, Value, Value)>, String> {
    if response["answers"].as_object().map(|a| a.len()) != Some(batch.tasks.len()) {
        return Err("Provider fan-out answer count mismatch".into());
    }
    batch.individual.iter().enumerate().map(|(index,request)| {
        let response=json!({"model":response["model"],"answers":{"result":response["answers"][batch.question_key(index)]}});
        Ok((super::validate(request,&response)?,super::evaluation_value(request,&response)?,response))
    }).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn assert_comparison_roundtrip(batch: &Batch) {
        for (i, individual) in batch.individual.iter().enumerate() {
            let mut state = batch.request["state"]["cases"][format!("q{i}")].clone();
            for (k, v) in batch.request["state"]["common"].as_object().unwrap() {
                state[k] = v.clone();
            }
            if let Some(refs) = state.as_object_mut().unwrap().remove("comparison_refs") {
                state["comparisons"] = json!(
                    refs.as_array()
                        .unwrap()
                        .iter()
                        .map(|r| batch.request["state"]["comparison_catalog"]
                            [r.as_u64().unwrap() as usize]
                            .clone())
                        .collect::<Vec<_>>()
                );
            }
            for field in [
                "events",
                "prerequisite_events",
                "ancestor_events",
                "unassigned_route_events",
            ] {
                if let Some(refs) = state
                    .as_object_mut()
                    .unwrap()
                    .remove(&format!("{field}_refs"))
                {
                    state[field] = json!(
                        refs.as_array()
                            .unwrap()
                            .iter()
                            .map(|r| batch.request["state"]["event_catalog"]
                                [r.as_u64().unwrap() as usize]
                                .clone())
                            .collect::<Vec<_>>()
                    );
                }
            }
            if let Some(index) = state.as_object_mut().unwrap().remove("target_event_ref") {
                state["target_event"] = batch.request["state"]["event_catalog"]
                    [index.as_u64().unwrap() as usize]
                    .clone();
            }
            assert_eq!(state, individual["state"]);
        }
    }
    #[test]
    fn event_catalog_preserves_order_full_values_and_response_identity() {
        let s = json!({"nodes":[
            {"Id":"a","kind":"scenario","statement":"α","edges":"[]","source_refs":[{"quote":"exact source"}]},
            {"Id":"b","kind":"scenario","statement":"β","edges":"[]"},
            {"Id":"c","kind":"scenario","statement":"γ","edges":"[]"}]});
        let p = json!({"cursor":0,"tasks":[
            {"nodeId":"pair:1:a:1:b","function":"check_pair","pair_ids":["a","b"]},
            {"nodeId":"pair:1:a:1:c","function":"check_pair","pair_ids":["a","c"]}]});
        let batch = prepare(&s, &p, 16).unwrap();
        assert_eq!(
            batch.request["state"]["event_catalog"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            batch.request["state"]["cases"]["q0"]["events_refs"],
            json!([0, 1])
        );
        assert_eq!(
            batch.request["state"]["cases"]["q1"]["events_refs"],
            json!([0, 2])
        );
        assert_comparison_roundtrip(&batch);
        let mut catalog = json!({});
        assert_eq!(
            intern_event(&mut catalog, json!({"Id":"a","statement":"before"})),
            0
        );
        assert_eq!(
            intern_event(&mut catalog, json!({"Id":"a","statement":"after"})),
            1
        );
        assert_eq!(
            intern_event(&mut catalog, json!({"Id":"a","statement":"before"})),
            0
        );
        let response = json!({"model":super::super::MODEL,"answers":{
            "q0":{"type":"choice","choice":"compatible","probabilities":{"compatible":1.0,"conflict":0.0,"uncertain":0.0}},
            "q1":{"type":"choice","choice":"conflict","probabilities":{"compatible":0.0,"conflict":1.0,"uncertain":0.0}}}});
        let answers = answers(&batch, &response).unwrap();
        assert_eq!(answers[0].0, "compatible");
        assert_eq!(answers[1].0, "conflict");
    }

    #[test]
    #[ignore = "Requires private captured transit checkpoint and old request reconstruction"]
    fn captured_transit_event_catalog_preserves_all_179_inputs() {
        let dir = std::path::PathBuf::from(std::env::var("FORESIGHT_EVENT_FIXTURE_DIR").unwrap());
        let read = |name: &str| -> Value {
            serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap()
        };
        let snapshot = read("focal-transit-final-snapshot.json");
        let original = read("focal-transit-final-checkpoint.json");
        let baseline = read("focal-packing-export.json");
        let mut p = original.clone();
        p["world_refinement"] = json!({});
        p["world_audits"] = json!({});
        let tasks = original["tasks"].as_array().unwrap();
        for task in tasks {
            for map in ["results", "evaluations"] {
                if let Some(m) = p[map][task["nodeId"].as_str().unwrap()].as_object_mut() {
                    m.remove(task["function"].as_str().unwrap());
                }
            }
        }
        assert_eq!(tasks.len(), 179);
        assert_eq!(p["batch_byte_cap"], 48523);
        let mut cursor = 0;
        let mut batches = 0;
        while cursor < tasks.len() {
            p["cursor"] = json!(cursor);
            let batch = prepare(&snapshot, &p, tasks.len() - cursor).unwrap();
            assert_comparison_roundtrip(&batch);
            if cursor == 5 {
                if let Ok(path) = std::env::var("FORESIGHT_EVENT_OUTPUT") {
                    std::fs::write(path,json!({"snapshot":snapshot,"program":p,"individual":batch.individual,"expected_tasks":batch.tasks.len(),"expected_request":batch.request,"fingerprints":batch.tasks.iter().zip(&batch.individual).map(|(t,r)|super::super::search::audit_input_fingerprint(&snapshot,t,r)).collect::<Vec<_>>(),"disclosure":"Reconstructed captured checkpoint with observed receipts replayed; actual WASM uses a separate synthetic execution clock, no native mutation"}).to_string()).unwrap();
                }
                assert!(batch.tasks.len() > 2, "old encoding fits only two pairs");
            }
            for (task, individual) in batch.tasks.iter().zip(&batch.individual) {
                assert_eq!(
                    individual, &baseline["rows"][cursor]["request"],
                    "task {cursor}"
                );
                let id = task["nodeId"].as_str().unwrap();
                let f = task["function"].as_str().unwrap();
                if !original["results"][id][f].is_null() {
                    p["results"][id][f] = original["results"][id][f].clone();
                    p["evaluations"][id][f] = original["evaluations"][id][f].clone();
                }
                cursor += 1;
            }
            batches += 1;
        }
        assert_eq!(batches, 71);
    }

    #[test]
    fn comparison_catalog_is_lossless_and_allows_multiple_deep_questions() {
        let nodes: Vec<_> = (0..16).map(|i|json!({"Id":format!("h{i:02}"),"kind":"scenario","statement":format!("event {i}"),"mechanism":"mechanism detail ".repeat(70),"edges":"[]"})).collect();
        let snapshot = json!({"world":{},"nodes":nodes});
        let mut p = super::super::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        if !p["tasks"].is_array() {
            p["tasks"] =
                super::super::plan(snapshot["nodes"].as_array().unwrap()).unwrap()["tasks"].clone();
        }
        p["batch_byte_cap"] = json!(51928);
        p["cursor"] = json!(
            p["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| t["function"] == "classify_gap")
                .unwrap()
        );
        let batch = prepare(&snapshot, &p, 16).unwrap();
        assert!(batch.tasks.len() > 1);
        assert!(batch.request.to_string().len() <= 51928);
        assert_comparison_roundtrip(&batch);
    }
    #[test]
    #[ignore = "Requires captured native UUID snapshot/program"]
    fn saved_uuid_deep_batch_interns_comparisons_losslessly() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_PACKING_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let f = raw.get("fields").unwrap_or(&raw);
        let snapshot: Value = serde_json::from_str(f["snapshot_json"].as_str().unwrap()).unwrap();
        let mut p: Value = serde_json::from_str(f["program_json"].as_str().unwrap()).unwrap();
        if !p["tasks"].is_array() {
            p["tasks"] =
                super::super::plan(snapshot["nodes"].as_array().unwrap()).unwrap()["tasks"].clone();
        }
        p["batch_byte_cap"] = json!(51928);
        p["cursor"] = json!(
            p["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| t["function"] == "classify_gap")
                .unwrap()
        );
        let batch = prepare(&snapshot, &p, 16).unwrap();
        if let Ok(path) = std::env::var("FORESIGHT_PACKING_OUTPUT") {
            std::fs::write(path,json!({"snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"individual":batch.individual,"expected_tasks":batch.tasks.len()}).to_string()).unwrap();
        }
        eprintln!(
            "actual deep tasks={} bytes={}",
            batch.tasks.len(),
            batch.request.to_string().len()
        );
        assert!(batch.tasks.len() > 1);
        assert!(batch.request.to_string().len() <= 51928);
        assert_comparison_roundtrip(&batch);
    }
    #[test]
    #[ignore = "Requires saved long-exploration checkpoint"]
    fn saved_combinations_checkpoint_packs_reduced_batches_without_losing_inputs() {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("FORESIGHT_PACKING_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let record = &raw["fields"];
        let snapshot: Value =
            serde_json::from_str(record["snapshot_json"].as_str().unwrap()).unwrap();
        let program: Value =
            serde_json::from_str(record["program_json"].as_str().unwrap()).unwrap();
        let batch = prepare(&snapshot, &program, 16).unwrap();
        let single = prepare(&snapshot, &program, 1).unwrap();
        eprintln!(
            "reduced batch questions={} bytes={} single_bytes={} common_bytes={}",
            batch.tasks.len(),
            batch.request.to_string().len(),
            single.request.to_string().len(),
            single.request["state"]["common"].to_string().len()
        );
        assert!(batch.tasks.len() < 16);
        assert_eq!(
            single.tasks[0],
            program["tasks"][program["cursor"].as_u64().unwrap() as usize]
        );
        assert_eq!(
            single.individual[0]["state"]["source_evidence"],
            batch.individual[0]["state"]["source_evidence"]
        );
    }
    #[test]
    fn world_batches_share_immutable_world_and_feedback_without_dropping_case_events() {
        let world = json!({"Id":"w","kind":"world","statement":"A, B and C jointly occur","component_ids":["a","b","c"],"counter_ids":[],"chain":[],"facets":[{"description":"long context ".repeat(1000)}],"assumptions":[],"edges":"[]"});
        let snapshot = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},world]});
        let program = json!({"cursor":0,"tasks":super::super::search::world_tasks(&world)});
        let batch = prepare(&snapshot, &program, 3).unwrap();
        assert_eq!(batch.tasks.len(), 3);
        assert_eq!(batch.request["state"]["common"]["world"], world);
        for index in 0..batch.individual.len() {
            let case = &batch.request["state"]["cases"][format!("q{index}")];
            assert!(case["world"].is_null());
            assert!(case["previous_world_judgments"].is_null());
        }
        assert_comparison_roundtrip(&batch);
    }

    #[test]
    fn whole_world_likelihood_does_not_require_component_temporal_classification() {
        let snapshot = json!({"world":{},"nodes":[{"Id":"world-r1-a","kind":"world","statement":"Joint world","edges":"[]","component_ids":[],"counter_ids":[]}]});
        let program = json!({"stage":"worlds","baseline_status":"established","cursor":0,"tasks":[{"nodeId":"world-r1-a","function":"estimate_likelihood"}],"results":{}});
        let batch = prepare(&snapshot, &program, 16).unwrap();
        assert_eq!(batch.tasks.len(), 1);
        assert_eq!(batch.individual[0]["questions"]["result"]["type"], "noul");
    }
    #[test]
    fn ordinary_waves_batch_without_crossing_assessment_or_parent_dependencies() {
        let nodes = vec![
            json!({"Id":"a","kind":"scenario","statement":"A","edges":"[]"}),
            json!({"Id":"b","kind":"scenario","statement":"B","edges":"[]"}),
            json!({"Id":"child","kind":"revision","statement":"Child","edges":"[{\"kind\":\"requires\",\"to_id\":\"a\"}]"}),
        ];
        let snapshot = json!({"world":{},"nodes":nodes});
        let mut p = super::super::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        // Temporal prefix may cross depth boundaries only in separate requests.
        for id in ["a", "b", "child"] {
            p["results"][id]["classify_temporal"] = json!("future_change");
        }
        let tasks = p["tasks"].as_array().unwrap().clone();
        let start = tasks
            .iter()
            .position(|t| t["function"] == "classify_gap")
            .unwrap();
        p["cursor"] = json!(start);
        let batch = prepare(&snapshot, &p, 16).unwrap();
        assert_eq!(batch.tasks.len(), 2);
        assert!(
            batch
                .tasks
                .iter()
                .all(|t| t["function"] == "classify_gap" && t["depth"] == 0)
        );
        assert_eq!(batch.question_key(0), "q0");
        for (i, task) in batch.tasks.iter().enumerate() {
            assert_eq!(
                batch.individual[i],
                super::super::evaluation::request_task(&snapshot, &p, task).unwrap()
            );
        }
        let parent_last = tasks
            .iter()
            .rposition(|t| t["nodeId"] == "a" && t["function"] != "classify_temporal")
            .unwrap();
        let child_first = tasks
            .iter()
            .position(|t| t["nodeId"] == "child" && t["function"] != "classify_temporal")
            .unwrap();
        assert!(parent_last < child_first);
        let answer = json!({"type":"choice","choice":"none","probabilities":{"none":0.8,"prerequisite":0.05,"evidence":0.05,"timing":0.05,"uncertain":0.05}});
        let response = json!({"model":super::super::MODEL,"answers":{"q0":answer,"q1":answer}});
        assert_eq!(answers(&batch, &response).unwrap().len(), 2);
    }
    #[test]
    fn batches_independent_pairs_but_stops_before_dependent_likelihood() {
        let s = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"}]});
        let p = json!({"cursor":0,"tasks":[{"nodeId":"pair:1:a:1:b","function":"check_pair","pair_ids":["a","b"]},{"nodeId":"pair:1:a:1:c","function":"check_pair","pair_ids":["a","c"]},{"nodeId":"a","function":"estimate_likelihood"}]});
        let batch = prepare(&s, &p, 10).unwrap();
        assert_eq!(batch.tasks.len(), 2);
        for (index, individual) in batch.individual.iter().enumerate() {
            let mut old_view = p.clone();
            old_view["cursor"] = json!(index);
            assert_eq!(*individual, super::super::request(&s, &old_view).unwrap());
        }
        let answer = json!({"type":"choice","choice":"compatible","probabilities":{"compatible":0.8,"conflict":0.1,"uncertain":0.1}});
        let mut response = json!({"model":super::super::MODEL,"answers":{"q0":answer,"q1":answer}});
        assert_eq!(answers(&batch, &response).unwrap().len(), 2);
        response["answers"]["q1"]["probabilities"]["conflict"] = json!(0.9);
        assert!(answers(&batch, &response).is_err());
        assert_eq!(prepare(&s, &p, 1).unwrap().tasks.len(), 1);
    }
}

#[cfg(test)]
#[test]
fn adaptive_packing_preserves_context_cursor_and_all_pending_tasks() {
    let snapshot = json!({"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"}]});
    let mut program = json!({"cursor":0,"tasks":[{"nodeId":"pair:1:a:1:b","function":"check_pair","pair_ids":["a","b"]},{"nodeId":"pair:1:a:1:c","function":"check_pair","pair_ids":["a","c"]}]});
    let original = program.clone();
    let first = prepare(&snapshot, &program, 10).unwrap();
    assert_eq!(first.tasks.len(), 2);
    assert!(reduce_cap(&mut program, &first));
    assert_eq!(program["cursor"], original["cursor"]);
    assert_eq!(program["tasks"], original["tasks"]);
    let smaller = prepare(&snapshot, &program, 10).unwrap();
    assert_eq!(smaller.tasks.len(), 1);
    assert_eq!(smaller.individual[0], first.individual[0]);
    assert!(
        !reduce_cap(&mut program, &smaller),
        "single request cannot retry forever"
    );
    program["cursor"] = json!(1);
    let rest = prepare(&snapshot, &program, 10).unwrap();
    assert_eq!(rest.individual[0], first.individual[1]);
}

#[cfg(test)]
#[test]
#[ignore = "Requires captured live large baseline"]
fn captured_live_component_wave_preserves_sources_and_packs_multiple_questions() {
    let fields: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("FORESIGHT_WAVE_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let snapshot = super::parse(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let program = super::parse(fields["program_json"].as_str().unwrap()).unwrap();
    let batch = prepare(&snapshot, &program, 16).unwrap();
    eprintln!(
        "actual component wave: {} questions, {} bytes, {} shared bytes",
        batch.tasks.len(),
        batch.request.to_string().len(),
        batch.request["state"]["common"].to_string().len()
    );
    assert!(batch.tasks.len() > 1);
    for (i, task) in batch.tasks.iter().enumerate() {
        let exact = super::evaluation::request_task(&snapshot, &program, task).unwrap();
        assert_eq!(batch.individual[i], exact);
        assert_eq!(
            batch.request["state"]["common"]["source_evidence"],
            exact["state"]["source_evidence"]
        );
    }
}
