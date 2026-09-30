// Jev primitives evaluate explicit hypotheses; their distributions are not empirical calibration.
use super::{MODEL, field, parse};
use serde_json::{Value, json};
mod definitions {
    include!("semantic_definitions.rs");
}
fn digest(node: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "Id",
        "kind",
        "title",
        "mechanism",
        "source_quote",
        "quote",
        "observed_at",
        "claim_type",
        "evidence_metadata",
        "evidence_note",
        "signal",
        "statement",
        "date",
        "timestamp",
        "sources",
        "source_refs",
        "resolve_by",
        "provenance",
        "resolution",
        "probability",
        "evidence",
        "research_question",
        "component_ids",
        "counter_ids",
        "scene",
        "narrative",
        "signals",
        "falsifiers",
        "what_you_can_do",
        "facets",
        "chain",
        "assumptions",
        "branch_conditions",
    ] {
        if let Some(value) = node.get("fields").unwrap_or(node).get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    Value::Object(out)
}
/// Request-only lossless encoding: repeated provenance sets are stored once.
/// Typed evaluation values stay alongside references; persisted program is untouched.
pub fn compact_evaluation_contexts(state: &mut Value) {
    fn visit(value: &mut Value, sets: &mut Vec<Value>) {
        match value {
            Value::Object(object) => {
                if let Some(context) = object.get_mut("context").and_then(Value::as_object_mut)
                    && let Some(ids) = context.get_mut("evidence_ids")
                    && ids.is_array()
                    && ids.to_string().len() > 128
                {
                    let index = sets.iter().position(|v| v == ids).unwrap_or_else(|| {
                        sets.push(ids.clone());
                        sets.len() - 1
                    });
                    *ids = json!({"$evidence_set_ref":index});
                }
                for child in object.values_mut() {
                    visit(child, sets);
                }
            }
            Value::Array(items) => {
                for child in items {
                    visit(child, sets);
                }
            }
            _ => {}
        }
    }
    let mut sets = vec![];
    visit(state, &mut sets);
    if !sets.is_empty() {
        state["evidence_sets"] = json!(sets);
        state["context_encoding"] = json!(
            "lossless-evidence-sets-v1: a context.evidence_ids object with $evidence_set_ref index means the exact ordered ID array at state.evidence_sets[index]. All typed evaluations and source claims are unchanged."
        );
    }
}

pub fn request(snapshot: &Value, program: &Value) -> Result<Value, String> {
    let cursor = program["cursor"].as_u64().ok_or("Missing cursor")? as usize;
    request_task(snapshot, program, &program["tasks"][cursor])
}
pub fn request_task(snapshot: &Value, program: &Value, task: &Value) -> Result<Value, String> {
    if super::search::is_structural(task) {
        return super::search::request(snapshot, program, task);
    }
    let id = task["nodeId"].as_str().ok_or("Missing task identity")?;
    let nodes = snapshot["nodes"]
        .as_array()
        .ok_or("Missing snapshot nodes")?;
    let node = nodes
        .iter()
        .find(|n| field(n, "Id") == id)
        .ok_or("Task references absent node")?;
    if matches!(field(node, "kind"), "scenario" | "revision")
        && matches!(
            field(task, "function"),
            "estimate_likelihood" | "estimate_conditional" | "evaluate_novelty" | "decision_value"
        )
        && !super::temporal_allows_forecast(program, field(task, "nodeId"))
    {
        return Err("Observed or mixed claims require separation before future evaluation".into());
    }
    let edges = parse(field(node, "edges"))?;
    let mut prerequisites: Vec<Value> = edges.as_array().ok_or("Invalid edges")?.iter().filter(|e| e["kind"] == "requires").map(|edge| {
        let target = edge["to_id"].as_str().unwrap_or("");
        json!({"id":target,"node":nodes.iter().find(|n|field(n,"Id")==target).map(digest),"assessment":program["results"][target],"evaluations":program["evaluations"][target]})
    }).collect();
    let candidates: Vec<_> = nodes
        .iter()
        .filter(|n| field(n, "Id") != id && matches!(field(n, "kind"), "scenario" | "revision"))
        .collect();
    let words: std::collections::BTreeSet<_> = field(node, "statement")
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(str::to_lowercase)
        .collect();
    let mut nearest = candidates.clone();
    nearest.sort_by_key(|n| {
        std::cmp::Reverse(
            field(n, "statement")
                .split_whitespace()
                .filter(|w| words.contains(&w.to_lowercase()))
                .count(),
        )
    });
    let mut selected = std::collections::BTreeSet::new();
    let mut comparisons: Vec<_> = candidates
        .iter()
        .rev()
        .take(8)
        .chain(nearest.iter())
        .filter(|n| selected.insert(field(n, "Id")))
        .take(16)
        .map(|n| digest(n))
        .collect();
    let mut question = match task["function"].as_str().ok_or("Missing function")? {
        "classify_temporal" => {
            json!({"type":"choice","instructions":"Compare the exact scoped claim against the supplied dated present baseline and source evidence as of world.last_ingest_date. Classify its temporal role before forecasting. Distinguish an existing capability from a future change in scale, access, adoption or consequences. Do not assume a cited prediction has already happened, use remembered later events in a hindcast, or call a conjecture false merely because evidence is absent. This judgment is not verification of source truth. Preserve uncertainty.","criteria":super::temporal_criteria()})
        }
        "classify_gap" => {
            json!({"type":"choice","instructions":"Identify the most consequential causal gap in this hypothesis using actual supplied evidence. Future events are hypotheses, not false observations. Source URLs alone do not prove contents. A coherent mechanism does not imply a likely outcome.","criteria":super::gap_criteria()})
        }
        "estimate_conditional" => {
            json!({"type":"noul","instructions":"Estimate P(the target event occurs within its explicit scope and stated deadline (or question horizon when no narrower deadline is stated) | every hypothetical clause in state.branch_state.conditions). These clauses are assumptions for THIS conditional estimate only, never evidence. all_occurring requires all listed events by their deadlines; not_all_occurring requires at least one fails, without identifying which. Inherit every ancestor clause. Do not assume target, descendants or unrelated future events. This is not the marginal event probability or a causal intervention estimate.","criteria":{"true":"Target event occurs under the supplied condition.","false":"Target event does not occur under the supplied condition."}})
        }
        "estimate_likelihood" => {
            json!({"type":"noul","instructions":"Estimate whether the explicit event described by state.node will occur within its stated date or horizon, conditioned on the supplied world question, evidence and prerequisite assessments. This is an event proposition, not a question about coherence, novelty, confidence, or whether the text asserts the event. Account for unsupported premises and contrary evidence. Preserve uncertainty; overlapping hypotheses need not sum to one.","criteria":{"true":"The described event occurs within its stated horizon.","false":"The described event does not occur within its stated horizon."}})
        }
        "evaluate_novelty" => {
            json!({"type":"score","instructions":"Assess how much distinct explanatory or decision-relevant information this hypothesis adds compared with the supplied comparison sample. Novelty is relative to this sample, not a claim of global originality. Reward a new causal mechanism, not surprising wording.","criteria":definitions::evaluate_novelty()})
        }
        "decision_value" => {
            json!({"type":"score","instructions":"Assess the value of investigating or monitoring this hypothesis for decisions implied by the world question. Consider consequences, tractable uncertainty and actionable discriminating signals; likelihood alone is not decision value.","criteria":definitions::decision_value()})
        }
        "choose_next_operation" => {
            json!({"type":"choice","instructions":"Recommend the next useful operation using the accumulated assessments and evaluations. This does not claim that an operation executed. Exploration should follow information value, unanswered causal questions and evidence, without a prescribed phase sequence.","criteria":definitions::choose_next_operation()})
        }
        _ => return Err("Unsupported semantic function".into()),
    };
    let is_world = field(node, "kind") == "world";
    if is_world && task["function"] == "estimate_likelihood" {
        // Joint-event likelihood needs its defining components and explicit counters,
        // not the novelty comparison sample or decision-utility scores.
        comparisons.clear();
        for prerequisite in &mut prerequisites {
            for field in ["assessment", "evaluations"] {
                if let Some(values) = prerequisite[field].as_object_mut() {
                    values.retain(|function, _| {
                        matches!(function.as_str(), "classify_gap" | "estimate_likelihood")
                    });
                }
            }
        }
    }

    let mut counter_hypotheses = vec![];
    let evidence: Vec<_> = nodes
        .iter()
        .filter(|n| matches!(field(n, "kind"), "evidence" | "research_evidence"))
        .map(digest)
        .collect();
    if is_world {
        for id in node["counter_ids"]
            .as_array()
            .ok_or("Missing world counters")?
        {
            let target = id.as_str().ok_or("Invalid counter identity")?;
            let counter = nodes
                .iter()
                .find(|n| field(n, "Id") == target)
                .ok_or("Missing counter hypothesis")?;
            counter_hypotheses
                .push(json!({"node":digest(counter),"assessment":program["results"][target]}));
        }
        // Supply actual source claims, including provenance and quoted excerpts,
        // rather than treating component likelihoods as evidence for a joint event.
        if task["function"] == "estimate_likelihood" {
            question["instructions"] = json!(
                "Estimate the probability of the WHOLE JOINT WORLD defined by state.node.statement AND ALL its defining component events in state.prerequisites, within the world horizon. Every defining component must occur for this joint world to occur. Evaluate causal interactions, correlations, shared assumptions, supplied source evidence, baseline unknowns, and counter hypotheses. The signed clauses in state.node.branch_conditions are ALSO defining events in this JOINT probability, not conditions assumed true. Include their uncertainty. Component estimates are context only: never average, multiply, inherit, or substitute them for a fresh assessment of the joint world. Counter hypotheses are contrary context, not required events. This is event likelihood, not narrative coherence or confidence. Worlds may overlap and need not sum to one."
            );
        }
    }
    let audit = if is_world {
        super::search::compact_world_audit(node, program)
    } else {
        Value::Null
    };
    let mut request = json!({"model":MODEL,"state":{"world":snapshot["world"],"node":digest(node),"prerequisites":prerequisites,"counter_hypotheses":counter_hypotheses,"source_evidence":evidence,"baseline":program["baseline"],"world_audit":audit,"previous_world_judgments":if is_world { super::search::previous_world_judgments(program,node) } else { Value::Null },"comparisons":comparisons,"assessment":program["results"][id],"evaluations":program["evaluations"][id]},"questions":{"result":question}});
    if task["function"] == "estimate_conditional" {
        request["state"]["branch_state"] =
            super::branches::state(snapshot, field(node, "branch_id"), Some(id))?;
        let mut judgments = json!({});
        for clause in request["state"]["branch_state"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for event in clause["events"].as_array().into_iter().flatten() {
                let premise = field(event, "id");
                judgments[premise] = program["results"][premise].clone();
            }
        }
        request["state"]["premise_judgments"] = judgments;
    }

    if task["function"] == "classify_temporal" {
        request["state"].as_object_mut().unwrap().retain(|key, _| {
            matches!(
                key.as_str(),
                "world" | "node" | "source_evidence" | "baseline"
            )
        });
    }
    compact_evaluation_contexts(&mut request["state"]);
    if request.to_string().len() > 128 * 1024 {
        return Err("Semantic request exceeds 128 KB".into());
    }
    Ok(request)
}
fn distribution(answer: &Value, keys: &[String]) -> Result<Vec<f64>, String> {
    let values = answer["probabilities"]
        .as_object()
        .ok_or("Missing distribution")?;
    if values.len() != keys.len() {
        return Err("Wrong option count".into());
    }
    let probabilities: Vec<_> = keys
        .iter()
        .map(|key| {
            values
                .get(key)
                .and_then(Value::as_f64)
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .ok_or("Invalid probability".to_owned())
        })
        .collect::<Result<_, _>>()?;
    if (probabilities.iter().sum::<f64>() - 1.0).abs() > 0.005 * keys.len() as f64 + f64::EPSILON {
        return Err("Invalid probability mass".into());
    }
    Ok(probabilities)
}
pub fn validate(request: &Value, response: &Value) -> Result<String, String> {
    if response["model"] != MODEL {
        return Err("Provider model mismatch".into());
    }
    let answer = &response["answers"]["result"];
    let question = &request["questions"]["result"];
    // Missing type is supported only for the historical choice test/request format.
    let kind = question["type"].as_str().unwrap_or("choice");
    if answer["type"] != kind {
        return Err("Provider answer type mismatch".into());
    }
    match kind {
        "noul" => answer["noul"]
            .as_f64()
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .map(|p| p.to_string())
            .ok_or("Invalid event probability".into()),
        "score" => {
            let levels = question["criteria"]
                .as_array()
                .ok_or("Missing score criteria")?;
            if !(2..=10).contains(&levels.len()) {
                return Err("Invalid score levels".into());
            }
            let keys: Vec<_> = (0..levels.len()).map(|i| i.to_string()).collect();
            let p = distribution(answer, &keys)?;
            let score = answer["score"]
                .as_f64()
                .filter(|s| s.is_finite() && *s >= 0.0 && *s <= (levels.len() - 1) as f64)
                .ok_or("Invalid score")?;
            let expected: f64 = p.iter().enumerate().map(|(i, p)| i as f64 * p).sum();
            let tolerance = 0.005 * (0..levels.len()).sum::<usize>() as f64 + 0.01;
            if (score - expected).abs() > tolerance {
                return Err("Score does not match distribution".into());
            }
            Ok(score.to_string())
        }
        "choice" => {
            let options = question["criteria"].as_object().ok_or("Missing criteria")?;
            let keys: Vec<_> = options.keys().cloned().collect();
            let probabilities = distribution(answer, &keys)?;
            let selected = answer["choice"].as_str().ok_or("Missing choice")?;
            let max = probabilities.iter().copied().fold(0.0, f64::max);
            if answer["probabilities"][selected].as_f64() != Some(max) {
                return Err(format!(
                    "Choice is not argmax: selected={selected}, selected_probability={}, max_probability={max}",
                    answer["probabilities"][selected]
                ));
            }
            Ok(if max < 0.65 && options.contains_key("uncertain") {
                "uncertain"
            } else {
                selected
            }
            .into())
        }
        _ => Err("Unsupported answer type".into()),
    }
}
pub fn evaluation_value(request: &Value, response: &Value) -> Result<Value, String> {
    let selected = validate(request, response)?;
    let answer = &response["answers"]["result"];
    Ok(match answer["type"].as_str() {
        Some("noul") => {
            json!({"type":"noul","answer":answer,"probability":answer["noul"],"interpretation":"model_implied_event_probability_not_empirically_calibrated"})
        }
        Some("score") => json!({"type":"score","answer":answer,"score":answer["score"]}),
        _ => json!({"type":"choice","answer":answer,"selected":selected}),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temporal_check_uses_dated_baseline_without_candidate_scores() {
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-29"},"nodes":[{"Id":"h","kind":"scenario","statement":"A tool is available","edges":"[]"},{"Id":"e","kind":"evidence","statement":"Dated source claim","edges":"[]"}]});
        let p = json!({"cursor":0,"tasks":[{"nodeId":"h","function":"classify_temporal"}],"baseline":{"as_of":"2026-09-29","observed":[{"claim":"Tool already available","evidence_ids":["e"]}]},"results":{"h":{"estimate_likelihood":"0.99"}},"evaluations":{"h":{"evaluate_novelty":{"score":4}}}});
        let r = request(&snapshot, &p).unwrap();
        assert_eq!(r["state"]["baseline"], p["baseline"]);
        assert_eq!(r["state"]["source_evidence"][0]["Id"], "e");
        assert!(r["state"].get("assessment").is_none());
        assert!(r["state"].get("evaluations").is_none());
        assert!(r["state"].get("comparisons").is_none());
        assert_eq!(
            r["questions"]["result"]["criteria"],
            super::super::temporal_criteria()
        );
    }
    #[test]
    fn event_probability_is_not_thresholded_or_choice_confidence() {
        let q = json!({"questions":{"result":{"type":"noul"}}});
        for p in [0.0, 0.37, 1.0] {
            let r = json!({"model":MODEL,"answers":{"result":{"type":"noul","noul":p}}});
            assert_eq!(evaluation_value(&q, &r).unwrap()["probability"], p);
        }
        for p in [-0.1, 1.1] {
            assert!(
                validate(
                    &q,
                    &json!({"model":MODEL,"answers":{"result":{"type":"noul","noul":p}}})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn score_requires_consistent_bounded_distribution() {
        let q = json!({"questions":{"result":{"type":"score","criteria":["low","medium","high"]}}});
        let mut r = json!({"model":MODEL,"answers":{"result":{"type":"score","score":1.5,"probabilities":{"0":0.1,"1":0.3,"2":0.6},"legend":["low","medium","high"]}}});
        assert_eq!(validate(&q, &r).unwrap(), "1.5");
        r["answers"]["result"]["score"] = json!(0.2);
        assert!(validate(&q, &r).is_err());
        r["answers"]["result"]["type"] = json!("choice");
        assert!(validate(&q, &r).is_err());
    }
    #[test]
    fn requests_chain_prior_numeric_evaluations() {
        let snapshot = json!({"world":{"question":"What happens by 2027?"},"nodes":[{"Id":"h","kind":"scenario","statement":"Event occurs by 2027","edges":"[]"}]});
        let p = json!({"cursor":0,"tasks":[{"nodeId":"h","function":"choose_next_operation"}],"results":{"h":{"classify_gap":"evidence"}},"evaluations":{"h":{"estimate_likelihood":{"probability":0.2}}}});
        let r = request(&snapshot, &p).unwrap();
        assert_eq!(
            r["state"]["evaluations"]["estimate_likelihood"]["probability"],
            0.2
        );
        assert!(
            r["questions"]["result"]["criteria"]
                .get("challenge")
                .is_some()
        );
    }
}
#[cfg(test)]
mod comparison_tests {
    use super::*;
    #[test]
    fn comparison_sample_includes_recent_and_relevant_mechanisms() {
        let mut nodes:Vec<_>=(0..40).map(|i|json!({"Id":format!("h{i}"),"kind":"scenario","statement":if i==15{"semiconductor supply bottleneck"}else{"different unrelated path"},"edges":"[]"})).collect();
        nodes.push(json!({"Id":"target","kind":"scenario","statement":"semiconductor supply bottleneck reverses","source_quote":"Direct quoted observation","quote":"Source text","observed_at":"2026-09-19","claim_type":"reported_observation","mechanism":"Supply constrains adoption","edges":"[]"}));
        let r = request(
            &json!({"world":{},"nodes":nodes}),
            &json!({"cursor":0,"tasks":[{"nodeId":"target","function":"evaluate_novelty"}]}),
        )
        .unwrap();
        let ids: Vec<_> = r["state"]["comparisons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["Id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"h39") && ids.contains(&"h15"));
        assert_eq!(ids.len(), 16);
        assert_eq!(
            r["state"]["node"]["source_quote"],
            "Direct quoted observation"
        );
        assert_eq!(
            r["state"]["node"]["mechanism"],
            "Supply constrains adoption"
        );
        assert_eq!(r["state"]["node"]["quote"], "Source text");
        assert_eq!(r["state"]["node"]["observed_at"], "2026-09-19");
        assert_eq!(r["state"]["node"]["claim_type"], "reported_observation");
    }
}

#[cfg(test)]
#[test]
fn provenance_context_encoding_is_lossless_and_preserves_typed_values() {
    let ids: Vec<_> = (0..35)
        .map(|i| format!("evidence-id-with-realistic-length-{i}"))
        .collect();
    let original = json!({"prerequisites":(0..6).map(|i|json!({"id":format!("h{i}"),"evaluations":{"likelihood":{"type":"noul","probability":0.23,"answer":{"type":"noul","probability":0.23},"context":{"round":2,"evidence_ids":ids}},"gap":{"type":"choice","selected":"evidence","context":{"round":2,"evidence_ids":ids}}}})).collect::<Vec<_>>(),"source_evidence":[{"quote":"Keep this exact dated quotation."}]});
    let mut packed = original.clone();
    compact_evaluation_contexts(&mut packed);
    assert!(packed.to_string().len() * 2 < original.to_string().len());
    let sets = packed["evidence_sets"].clone();
    for node in packed["prerequisites"].as_array_mut().unwrap() {
        for value in node["evaluations"].as_object_mut().unwrap().values_mut() {
            let index = value["context"]["evidence_ids"]["$evidence_set_ref"]
                .as_u64()
                .unwrap() as usize;
            value["context"]["evidence_ids"] = sets[index].clone();
        }
    }
    packed.as_object_mut().unwrap().remove("evidence_sets");
    packed.as_object_mut().unwrap().remove("context_encoding");
    assert_eq!(packed, original);
}

#[cfg(test)]
#[test]
fn joint_likelihood_preserves_sources_components_and_counters_not_novelty_sample() {
    let snapshot = json!({"world":{"Id":"question"},"nodes":[{"Id":"h","kind":"scenario","statement":"Defining event","edges":"[]"},{"Id":"other","kind":"scenario","statement":"Unrelated novelty comparison","edges":"[]"},{"Id":"counter","kind":"scenario","statement":"Counter event","edges":"[]"},{"Id":"source","kind":"evidence","source_quote":"Exact source quote","edges":"[]"},{"Id":"w","kind":"world","statement":"Joint event","component_ids":["h"],"counter_ids":["counter"],"edges":"[{\"kind\":\"requires\",\"to_id\":\"h\"}]"}]});
    let judgments = json!({"classify_gap":{"selected":"evidence"},"estimate_likelihood":{"probability":0.4},"evaluate_novelty":{"score":2},"decision_value":{"score":3}});
    let program = json!({"cursor":0,"tasks":[{"nodeId":"w","function":"estimate_likelihood"}],"results":{"h":{"classify_gap":"evidence","estimate_likelihood":"0.4","evaluate_novelty":"2","decision_value":"3"}},"evaluations":{"h":judgments}});
    let request = request(&snapshot, &program).unwrap();
    assert_eq!(request["state"]["comparisons"], json!([]));
    assert_eq!(
        request["state"]["prerequisites"][0]["node"]["statement"],
        "Defining event"
    );
    assert_eq!(
        request["state"]["counter_hypotheses"][0]["node"]["Id"],
        "counter"
    );
    assert_eq!(
        request["state"]["source_evidence"][0]["source_quote"],
        "Exact source quote"
    );
    assert_eq!(
        request["state"]["prerequisites"][0]["evaluations"],
        json!({"classify_gap":{"selected":"evidence"},"estimate_likelihood":{"probability":0.4}})
    );
    assert_eq!(program["evaluations"]["h"], judgments);
}
