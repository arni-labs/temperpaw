// Only independent structural questions share HTTP calls. Node decisions remain sequential.
use serde_json::{Value, json};
pub struct Batch {
    pub request: Value,
    pub tasks: Vec<Value>,
    pub individual: Vec<Value>,
}
impl Batch {
    pub fn question_key(&self, index: usize) -> String {
        if super::search::is_structural(&self.tasks[0]) {
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
    if !structural && remaining > 0 {
        let request = super::request(snapshot, program)?;
        return Ok(Batch {
            request: request.clone(),
            tasks: vec![first.clone()],
            individual: vec![request],
        });
    }
    let mut batch = Batch {
        request: json!({"model":super::MODEL,"state":{"common":{},"cases":{}},"questions":{}}),
        tasks: vec![],
        individual: vec![],
    };
    for (_index, task) in tasks
        .iter()
        .enumerate()
        .take(
            tasks
                .len()
                .min(cursor + if structural { 16 } else { 1 })
                .min(cursor + remaining),
        )
        .skip(cursor)
    {
        if structural && !super::search::is_structural(task) {
            break;
        }
        // Structural requests take their task explicitly; avoid cloning the whole
        // accumulated program for every independent question.
        let individual = super::search::request(snapshot, program, task)?;
        let key = format!("q{}", batch.tasks.len());
        let mut state = individual["state"].clone();
        let mut common = json!({});
        for field in [
            "world_question",
            "source_evidence",
            "baseline",
            "world",
            "previous_world_judgments",
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
        candidate["state"]["cases"][&key] = state;
        let mut question = individual["questions"]["result"].clone();
        question["instructions"] = json!(format!(
            "For this question, state means ONLY state.cases.{key} combined with state.common. Other cases are separate hypothetical questions, not assumed facts. {}",
            super::field(&question, "instructions")
        ));
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
        for (index, individual) in batch.individual.iter().enumerate() {
            let case = &batch.request["state"]["cases"][format!("q{index}")];
            assert!(case["world"].is_null());
            assert!(case["previous_world_judgments"].is_null());
            let mut restored = batch.request["state"]["common"]
                .as_object()
                .unwrap()
                .clone();
            restored.extend(case.as_object().unwrap().clone());
            assert_eq!(Value::Object(restored), individual["state"]);
        }
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
