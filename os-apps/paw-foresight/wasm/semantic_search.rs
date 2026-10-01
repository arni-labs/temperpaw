// Search relationships before composition; audit complete worlds before presentation.
// These are model judgments, not proofs or identified causal effects.
use super::{MODEL, field};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn ids(value: &Value) -> Result<Vec<&str>, String> {
    value
        .as_array()
        .ok_or("Missing identity list")?
        .iter()
        .map(|id| {
            id.as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Invalid identity".into())
        })
        .collect()
}
fn text(value: &Value, max: usize) -> bool {
    value
        .as_str()
        .is_some_and(|s| !s.trim().is_empty() && s.len() <= max)
}
pub(crate) fn date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return false;
    }
    let Ok(year) = value[..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..].parse::<u32>() else {
        return false;
    };
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    day > 0 && day <= days
}

/// Facets must account for the defining events; causal links must form a dated DAG.
pub fn validate_world(world: &Value, snapshot: &Value) -> Result<(), String> {
    let components: BTreeSet<_> = ids(&world["component_ids"])?.into_iter().collect();
    if components.len() < 3 {
        return Err("A multifaceted world needs at least three defining events".into());
    }
    let facets = world["facets"]
        .as_array()
        .filter(|v| (3..=12).contains(&v.len()))
        .ok_or("World needs three to twelve facets")?;
    let mut covered = BTreeSet::new();
    let mut facet_ids = BTreeSet::new();
    for facet in facets {
        if !text(&facet["id"], 80)
            || !facet_ids.insert(field(facet, "id"))
            || !text(&facet["title"], 100)
            || !text(&facet["description"], 800)
        {
            return Err("Invalid world facet".into());
        }
        let refs = ids(&facet["component_ids"])?;
        if refs.is_empty() || refs.iter().any(|id| !components.contains(id)) {
            return Err("Facet references a non-defining event".into());
        }
        covered.extend(refs);
    }
    if covered != components {
        return Err("Every defining event must belong to a facet".into());
    }
    let assumptions = world["assumptions"]
        .as_array()
        .filter(|v| v.len() <= 12)
        .ok_or("Missing world assumptions")?;
    if assumptions.iter().any(|v| !text(v, 600)) {
        return Err("Invalid world assumption".into());
    }
    let links = world["chain"]
        .as_array()
        .filter(|v| v.len() <= 24)
        .ok_or("World needs zero to twenty-four claimed causal links")?;
    let horizon = field(&snapshot["world"], "target_date");
    let baseline = field(&snapshot["world"], "last_ingest_date");
    let mut graph: BTreeMap<&str, BTreeSet<&str>> =
        components.iter().map(|id| (*id, BTreeSet::new())).collect();
    let mut incoming: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    let mut link_ids = BTreeSet::new();
    for link in links {
        let to = field(link, "to_id");
        let from = ids(&link["from_ids"])?;
        let by = field(link, "by");
        if !text(&link["id"], 80)
            || !link_ids.insert(field(link, "id"))
            || !text(&link["mechanism"], 800)
            || !components.contains(to)
            || from.is_empty()
            || from.iter().any(|id| *id == to || !components.contains(id))
        {
            return Err("Invalid causal link".into());
        }
        if !date(by)
            || (!horizon.is_empty() && by > horizon)
            || (!baseline.is_empty() && by < baseline)
        {
            return Err("Causal link date is outside the world interval".into());
        }
        incoming.entry(to).or_default().push(link);
        graph
            .get_mut(to)
            .ok_or("Missing causal target")?
            .extend(from.iter().copied());
    }
    let mut date_conflicts = vec![];
    // Inherit only an unambiguous incoming route. Multiple contributory routes
    // do not imply that all routes occurred or share a single milestone.
    for child in links {
        for source in ids(&child["from_ids"])? {
            if let Some(parents) = incoming.get(source)
                && let [parent] = parents.as_slice()
                && field(parent, "by") > field(child, "by")
            {
                date_conflicts.push(format!("link '{}' ({}) -> link '{}' ({}): prerequisite event '{}' reaches downstream event '{}'",field(parent,"id"),field(parent,"by"),field(child,"id"),field(child,"by"),source,field(child,"to_id")));
            }
        }
    }
    if !date_conflicts.is_empty() {
        return Err(format!(
            "World '{}': causal-link milestones must be nondecreasing along each claimed dependency. {}. These are link by dates, not event resolve_by dates. Parallel developments need no causal link.",
            field(world, "Id"),
            date_conflicts.join("; ")
        ));
    }
    let mut visited = BTreeSet::new();
    loop {
        let ready: Vec<_> = graph
            .iter()
            .filter(|(id, parents)| {
                !visited.contains(**id) && parents.iter().all(|p| visited.contains(p))
            })
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            break;
        }
        visited.extend(ready);
    }
    if visited.len() != components.len() {
        return Err("Causal chain contains a cycle".into());
    }
    // Parallel developments need not cause one another. Facets cover the whole
    // world; pair and whole-set judgments assess coherence without invented edges.
    Ok(())
}

fn pair_id(a: &str, b: &str) -> String {
    // Length prefixes prevent collisions even when user-owned identities contain separators.
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    format!("pair:{}:{a}:{}:{b}", a.len(), b.len())
}
pub fn world_tasks(world: &Value) -> Vec<Value> {
    let id = field(world, "Id");
    let components = ids(&world["component_ids"]).unwrap_or_default();
    let mut tasks = vec![];
    for (i, a) in components.iter().enumerate() {
        for b in &components[i + 1..] {
            tasks.push(json!({"nodeId":format!("{id}/{}",pair_id(a,b)),"world_id":id,"function":"check_pair","pair_ids":[a,b],"depth":0}));
        }
    }
    for link in world["chain"].as_array().into_iter().flatten() {
        for function in ["check_transition", "conditional_on", "conditional_off"] {
            tasks.push(json!({"nodeId":format!("{id}/link/{}",field(link,"id")),"world_id":id,"link_id":link["id"],"function":function,"depth":0}));
        }
    }
    tasks.push(json!({"nodeId":id,"world_id":id,"function":"check_world_consistency","depth":0}));
    // This must follow all audits, so the fresh joint estimate sees the weak links.
    tasks.push(json!({"nodeId":id,"function":"estimate_likelihood","depth":0}));
    tasks
}

/// Immutable hypothetical history for a link, independent of model answers.
fn branch_state(
    world: &Value,
    link: &Value,
    function: &str,
    as_of: &Value,
) -> Result<Value, String> {
    let links = world["chain"].as_array().ok_or("Missing causal chain")?;
    let mut incoming: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for item in links {
        incoming.entry(field(item, "to_id")).or_default().push(item);
    }
    fn visit<'a>(
        id: &'a str,
        incoming: &BTreeMap<&'a str, Vec<&'a Value>>,
        active: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
        order: &mut Vec<&'a str>,
    ) -> Result<(), String> {
        if done.contains(id) {
            return Ok(());
        }
        if !active.insert(id) {
            return Err("Cyclic hypothetical branch".into());
        }
        if let Some(routes) = incoming.get(id)
            && let [link] = routes.as_slice()
        {
            for parent in ids(&link["from_ids"])? {
                visit(parent, incoming, active, done, order)?;
            }
        }
        active.remove(id);
        done.insert(id);
        order.push(id);
        Ok(())
    }
    let target = field(link, "to_id");
    let direct = ids(&link["from_ids"])?;
    let mut order = vec![];
    let mut done = BTreeSet::new();
    let mut active = BTreeSet::from([target]);
    for parent in &direct {
        visit(parent, &incoming, &mut active, &mut done, &mut order)?;
    }
    let on = function != "conditional_off";
    let state_id = |link: &Value| {
        format!(
            "{}/branch/{}/conditional_on",
            field(world, "Id"),
            field(link, "id")
        )
    };
    let unique = |id: &str| {
        incoming.get(id).and_then(|routes| {
            if routes.len() == 1 {
                Some(routes[0])
            } else {
                None
            }
        })
    };
    let unassigned_routes: Vec<_> = order
        .iter()
        .filter_map(|id| {
            incoming
                .get(id)
                .filter(|routes| routes.len() > 1)
                .map(|routes| json!({"target_id":id,"candidate_links":routes,"assumed":false}))
        })
        .collect();
    let history:Vec<_>=order.iter().filter_map(|id|unique(id)).filter(|prior|on || !direct.contains(&field(prior,"to_id"))).map(|prior|json!({"state_id":state_id(prior),"link_id":prior["id"],"by":prior["by"],"from_ids":prior["from_ids"],"to_id":prior["to_id"]})).collect();
    let assignments: Vec<_> = order
        .iter()
        .filter(|id| on || !direct.contains(id))
        .map(|id| {
            let by = unique(id).map(|item| field(item, "by")).unwrap_or_else(|| {
                links
                    .iter()
                    .filter(|item| {
                        (done.contains(field(item, "to_id")) || field(item, "to_id") == target)
                            && item["from_ids"].as_array().is_some_and(|parents| {
                                parents.iter().any(|p| p.as_str() == Some(id))
                            })
                    })
                    .map(|item| field(item, "by"))
                    .min()
                    .unwrap_or(field(link, "by"))
            });
            json!({"node_id":id,"occurs":true,"by":by})
        })
        .collect();
    let mut result = json!({"id":format!("{}/branch/{}/{}",field(world,"Id"),field(link,"id"),function),"world_id":world["Id"],"link_id":link["id"],"baseline_ref":"state.baseline","as_of":as_of,"by":link["by"],"target_id":target,"hypothetical":true,"assignments":assignments,"unassigned_parent_ids":if on {vec![]}else{direct.clone()},"condition":{"kind":if on {"all_occurring"}else{"not_all_occurring"},"event_ids":direct},"parent_state_ids":direct.iter().filter_map(|id|unique(id)).filter(|prior|on || !direct.contains(&field(prior,"to_id"))).map(state_id).collect::<Vec<_>>(),"history":history});
    if !unassigned_routes.is_empty() {
        result["unassigned_upstream_routes"] = json!(unassigned_routes);
    }
    Ok(result)
}

pub fn audit_world(world: &Value, program: &Value) -> Value {
    let tasks = world_tasks(world);
    let mut checks = vec![];
    let mut conflict = false;
    let mut unknown = false;
    let mut completed = 0;
    for task in tasks
        .iter()
        .filter(|t| t["function"] != "estimate_likelihood")
    {
        let id = field(task, "nodeId");
        let function = field(task, "function");
        let result = &program["results"][id][function];
        if result.is_string() {
            completed += 1;
        }
        conflict |= result == "conflict";
        unknown |= result.is_null() || result == "uncertain";
        let subject_ids = if task["pair_ids"].is_array() {
            task["pair_ids"].clone()
        } else if let Some(link) = world["chain"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|l| l["id"] == task["link_id"])
        {
            let mut ids = link["from_ids"].as_array().cloned().unwrap_or_default();
            ids.push(link["to_id"].clone());
            json!(ids)
        } else {
            world["component_ids"].clone()
        };
        let branch = world["chain"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|link| link["id"] == task["link_id"])
            .and_then(|link| {
                branch_state(world, link, function, &program["baseline"]["as_of"]).ok()
            });
        checks.push(json!({"id":format!("{id}/{function}"),"kind":function,"subject_ids":subject_ids,"result":result,"probability":program["evaluations"][id][function]["probability"],"branch_state":branch}));
    }
    let probability_coherence = super::coherence::audit(world, program);
    unknown |= probability_coherence["status"] == "inconsistent";
    let status = if conflict {
        "conflicts_found"
    } else if completed == 0 {
        "not_tested"
    } else if unknown {
        "uncertain"
    } else {
        "no_conflict_found"
    };
    json!({"status":status,"planned_checks":checks.len(),"completed_checks":completed,"checks":checks,"probability_coherence":probability_coherence})
}

/// Provider view removes repeated canonical task IDs, not audit judgments.
pub fn compact_world_audit(world: &Value, program: &Value) -> Value {
    let audit = audit_world(world, program);
    let components = world["component_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let checks: Vec<_> = audit["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| {
            let indices: Vec<_> = check["subject_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|id| components.iter().position(|c| c == id))
                .collect();
            json!([check["kind"], indices, check["result"]])
        })
        .collect();
    json!({"status":audit["status"],"planned_checks":audit["planned_checks"],"completed_checks":audit["completed_checks"],"checks":checks,"probability_coherence":audit["probability_coherence"],"encoding":"Each check is [kind, zero-based indices into state.node.component_ids, exact result]. Numeric conditional results are model estimates, not empirical causal effects."})
}

/// Round-robin distances cover the frontier before spending remaining budget on near duplicates.
pub fn plan_combinations(snapshot: &Value, program: &mut Value, remaining: usize) -> bool {
    let nodes = snapshot["nodes"].as_array().cloned().unwrap_or_default();
    let replaced: BTreeSet<_> = nodes.iter().filter_map(|n| n["parent"].as_str()).collect();
    let frontier: Vec<_> = nodes
        .iter()
        .filter(|n| {
            matches!(field(n, "kind"), "scenario" | "revision")
                && !replaced.contains(field(n, "Id"))
                && super::branches::future_eligible(snapshot, program, field(n, "Id"))
        })
        .map(|n| field(n, "Id"))
        .collect();
    let mut pairs = BTreeSet::new();
    let mut tasks = vec![];
    'budget: for distance in 1..frontier.len() {
        for i in 0..frontier.len() {
            if tasks.len() >= remaining {
                break 'budget;
            }
            let a = frontier[i];
            let b = frontier[(i + distance) % frontier.len()];
            let id = pair_id(a, b);
            if pairs.insert(id.clone()) {
                tasks.push(json!({"nodeId":id,"function":"check_pair","pair_ids":[a,b],"depth":0}));
            }
        }
    }
    program["combination_search"] = json!({"candidate_ids":frontier,"possible_pairs":frontier.len().saturating_mul(frontier.len().saturating_sub(1))/2,"planned_pairs":tasks.len(),"tested_pairs":0,"candidate_sets":[],"pairs":[]});
    program["tasks"] = json!(tasks);
    program["cursor"] = json!(0);
    program["stage"] = json!("combinations");
    program["stop_reason"] = json!("checking_combinations");
    !tasks.is_empty()
}

/// Deterministic, diverse seeds, then Jev's pair judgments guide which events can be combined.
/// A compatible clique remains a candidate: higher-order consistency is tested separately.
pub fn finish_combinations(program: &mut Value) {
    let candidates: Vec<String> = program["combination_search"]["candidate_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let mut pairs = vec![];
    let mut compatible = BTreeSet::new();
    for task in program["tasks"].as_array().into_iter().flatten() {
        let id = field(task, "nodeId");
        let result = &program["results"][id]["check_pair"];
        if result.is_null() {
            continue;
        }
        if result == "compatible" {
            compatible.insert(id.to_owned());
        }
        pairs.push(json!({"pair_ids":task["pair_ids"],"result":result}));
    }
    let mut sets = BTreeSet::new();
    let mut candidates_out = vec![];
    for start in 0..candidates.len() {
        let mut selected = vec![candidates[start].clone()];
        for offset in 1..candidates.len() {
            let candidate = &candidates[(start + offset) % candidates.len()];
            if selected.len() < 12
                && selected
                    .iter()
                    .all(|id| compatible.contains(&pair_id(id, candidate)))
            {
                selected.push(candidate.clone());
            }
        }
        selected.sort();
        if selected.len() >= 3 && sets.insert(selected.clone()) {
            candidates_out.push(json!({"component_ids":selected,"status":"pairwise_candidate_not_whole_world_validation"}));
        }
    }
    program["combination_search"]["tested_pairs"] = json!(pairs.len());
    program["combination_search"]["pairs"] = json!(pairs);
    program["combination_search"]["candidate_sets"] = json!(candidates_out);
    program["combination_search"]["unresolved_candidates_retained"] = json!(true);
}

pub fn world_set_task(world_ids: &[Value]) -> Value {
    let key = world_ids
        .iter()
        .map(|id| {
            let s = id.as_str().unwrap_or("");
            format!("{}:{s}", s.len())
        })
        .collect::<Vec<_>>()
        .join(":");
    json!({"nodeId":format!("world-set:{key}"),"function":"check_world_set","world_ids":world_ids,"depth":0})
}

pub fn is_structural(task: &Value) -> bool {
    matches!(
        field(task, "function"),
        "check_pair"
            | "check_world_set"
            | "check_world_consistency"
            | "check_transition"
            | "conditional_on"
            | "conditional_off"
    )
}

/// Validate identity and structural subjects without constructing provider state.
/// Checkpoint reads must not serialize evidence and judgment histories per receipt.
pub fn validate_task(snapshot: &Value, task: &Value) -> Result<(), String> {
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    if task["function"] == "check_world_set" {
        let subjects = task["world_ids"].as_array().ok_or("Missing world set")?;
        if !(2..=6).contains(&subjects.len()) || world_set_task(subjects) != *task {
            return Err("Invalid world-set identity".into());
        }
        let mut unique = std::collections::BTreeSet::new();
        for id in subjects {
            let id = id.as_str().ok_or("Invalid world identity")?;
            if !unique.insert(id)
                || !nodes
                    .iter()
                    .any(|n| field(n, "Id") == id && n["kind"] == "world")
            {
                return Err("Invalid world-set subject".into());
            }
        }
        return Ok(());
    }
    let world = nodes.iter().find(|n| n["Id"] == task["world_id"]);
    if task["world_id"].is_string() && world.is_none_or(|w| field(w, "kind") != "world") {
        return Err("Structural task references missing world".into());
    }
    if let Some(world) = world
        && !world_tasks(world).iter().any(|expected| expected == task)
    {
        return Err("Structural task does not match its immutable world".into());
    }
    let exists = |id: &str| nodes.iter().any(|n| field(n, "Id") == id);
    match field(task, "function") {
        "check_pair" => {
            let pair = ids(&task["pair_ids"])?;
            if pair.len() != 2 || pair[0] == pair[1] {
                return Err("Pair needs two distinct events".into());
            }
            if pair.iter().any(|id| {
                !nodes.iter().any(|n| {
                    field(n, "Id") == *id && matches!(field(n, "kind"), "scenario" | "revision")
                })
            }) {
                return Err("Compatibility pairs must reference future hypotheses".into());
            }
            if world.is_none() && field(task, "nodeId") != pair_id(pair[0], pair[1]) {
                return Err("Pair task identity does not match its subjects".into());
            }
        }
        "check_world_consistency" => {
            let world = world.ok_or("Missing world for consistency test")?;
            if ids(&world["component_ids"])?.iter().any(|id| !exists(id)) {
                return Err("Missing structural subject".into());
            }
        }
        "check_transition" | "conditional_on" | "conditional_off" => {
            let world = world.ok_or("Missing world for causal link")?;
            let link = world["chain"]
                .as_array()
                .ok_or("Missing causal chain")?
                .iter()
                .find(|link| link["id"] == task["link_id"])
                .ok_or("Missing causal link")?;
            if ids(&link["from_ids"])?.iter().any(|id| !exists(id)) || !exists(field(link, "to_id"))
            {
                return Err("Missing structural subject".into());
            }
        }
        _ => return Err("Unknown structural function".into()),
    }
    Ok(())
}

// The set audit compares supplied trajectories, not their likelihood or source validity.
// Keep complete defining prose and qualifications; omit operational and presentation fields.
fn set_audit_record(node: &Value, fields: &[&str]) -> Value {
    let mut record = serde_json::Map::new();
    for key in fields {
        if let Some(value) = node.get(*key) {
            record.insert((*key).into(), value.clone());
        }
    }
    Value::Object(record)
}
fn set_audit_projection(state: &mut Value) {
    for (group, fields) in [
        (
            "proposed_worlds",
            &[
                "Id",
                "kind",
                "title",
                "statement",
                "definition",
                "mechanism",
                "narrative",
                "shared_question",
                "trajectory_answer",
                "assumptions",
                "facets",
                "chain",
                "branch_conditions",
                "component_ids",
                "counter_ids",
                "scope",
                "resolve_by",
                "date",
                "by",
            ][..],
        ),
        (
            "components",
            &[
                "Id",
                "kind",
                "statement",
                "definition",
                "mechanism",
                "scope",
                "resolve_by",
                "date",
                "by",
                "evidence_note",
                "provenance",
                "source_refs",
            ][..],
        ),
        (
            "source_evidence",
            &[
                "Id",
                "kind",
                "statement",
                "evidence_note",
                "evidence_metadata",
                "provenance",
                "claim_type",
                "resolution",
            ][..],
        ),
    ] {
        if let Some(records) = state[group].as_array() {
            state[group] = json!(
                records
                    .iter()
                    .map(|n| set_audit_record(n, fields))
                    .collect::<Vec<_>>()
            );
        }
    }
}

pub fn request(snapshot: &Value, program: &Value, task: &Value) -> Result<Value, String> {
    validate_task(snapshot, task)?;
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let world = nodes.iter().find(|n| n["Id"] == task["world_id"]);
    let get = |id: &str| {
        nodes
            .iter()
            .find(|n| field(n, "Id") == id)
            .cloned()
            .ok_or("Missing structural subject".to_owned())
    };
    let mut state = json!({"world_question":snapshot["world"],"baseline":program["baseline"],"world":world,"source_evidence":nodes.iter().filter(|n|matches!(field(n,"kind"),"evidence"|"research_evidence")).collect::<Vec<_>>()});
    if let Some(world) = world {
        state["previous_world_judgments"] = previous_world_judgments(program, world);
    }
    let mut question = match field(task, "function") {
        "check_world_set" => {
            if program["active_world_ids"].is_array()
                && program["active_world_ids"] != task["world_ids"]
            {
                return Err("World set does not match active revision".into());
            }
            state["proposed_worlds"] = json!(
                ids(&task["world_ids"])?
                    .into_iter()
                    .map(get)
                    .collect::<Result<Vec<_>, _>>()?
            );
            let proposed = state["proposed_worlds"].as_array().unwrap();
            // Historical worlds without this contract remain auditable. New worlds
            // carry one immutable comparison target, not a topic per card.
            if proposed.iter().any(|w| !w["shared_question"].is_null()) {
                let shared = &proposed[0]["shared_question"];
                if !text(shared, 800)
                    || proposed.iter().any(|w| {
                        &w["shared_question"] != shared || !text(&w["trajectory_answer"], 1000)
                    })
                {
                    return Err("World set needs one shared question and a trajectory answer from every world".into());
                }
                state["shared_question"] = shared.clone();
            }
            let required: std::collections::BTreeSet<&str> = state["proposed_worlds"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|world| {
                    ["component_ids", "counter_ids"]
                        .into_iter()
                        .flat_map(move |key| {
                            world[key]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                        })
                })
                .collect();
            state["components"] = json!(
                nodes
                    .iter()
                    .filter(|n| required.contains(field(n, "Id")))
                    .collect::<Vec<_>>()
            );
            set_audit_projection(&mut state);
            json!({"type":"choice","instructions":"This is a structural comparison of the supplied futures, not evidence verification or likelihood estimation. Source qualifications and baseline limits are retained, but full source bodies and prior scores are deliberately not inputs to this judgment. Assess this SET against the original question and its shared central question, which may involve interacting uncertainties. Compare each trajectory_answer with its actual definition and components: does it change the overall outcome through a different organizing mechanism, with consequential downstream differences? Different subject areas, stakeholders or mechanisms confined to separate subtopics are complementary slices even if each is coherent. Do not accept the author's assertion of difference when the defining events merely distribute a common account across topics. Shared events or simultaneous possibilities do not by themselves make worlds slices; the test is substantive alternative answers to the same question. Overlap is allowed; mutual exclusivity, prescribed axes, symmetry and artificial opposites are NOT required. Judge full definitions, assumptions and components, not different titles. Do not reward unsupported novelty or demand contradictions merely to create variety.","criteria":{"alternative_answers":"The set offers meaningfully different overall answers or trajectories to the question, though they may overlap.","complementary_slices":"The worlds mostly partition topics, sectors or use cases within the same overall answer or trajectory.","uncertain":"The supplied definitions and evidence do not establish whether the set offers materially different answers."}})
        }
        "check_pair" => {
            let pair = ids(&task["pair_ids"])?;
            state["events"] = json!([get(pair[0])?, get(pair[1])?]);
            json!({"type":"choice","instructions":"Can BOTH specified events occur in the SAME world within the stated scope and dates, including any supplied world assumptions? Check incompatible resource uses, mutually exclusive actors or outcomes, timing and prerequisites. Different subjects or sequential states can coexist. This is logical/causal compatibility, NOT whether either event is likely, novel or already observed. Missing proof of the future alone is not a conflict. Preserve ambiguity.","criteria":{"compatible":"No material contradiction identified in the supplied pair and assumptions.","conflict":"The supplied events cannot jointly hold as scoped or their stated mechanisms conflict.","uncertain":"A material ambiguity prevents judging coexistence."}})
        }
        "check_world_consistency" => {
            let world = world.ok_or("Missing world for consistency test")?;
            state["events"] = json!(
                ids(&world["component_ids"])?
                    .into_iter()
                    .map(get)
                    .collect::<Result<Vec<_>, _>>()?
            );
            json!({"type":"choice","instructions":"Test the WHOLE world, every defining event, all facets, chain links and assumptions jointly. Look for higher-order contradictions that pairwise checks miss: shared resource constraints, circular explanations, incompatible timelines, changes that destroy another component's prerequisites, or incompatible scopes. Do not substitute plausibility or lack of future evidence for contradiction. A no-conflict judgment is not proof that the world will happen.","criteria":{"compatible":"No material whole-set contradiction found in the supplied world.","conflict":"At least one material contradiction exists in the joint world.","uncertain":"Material missing scope or assumptions prevent a whole-set judgment."}})
        }
        "check_transition" | "conditional_on" | "conditional_off" => {
            let world = world.ok_or("Missing world for causal link")?;
            let link = world["chain"]
                .as_array()
                .ok_or("Missing causal chain")?
                .iter()
                .find(|l| l["id"] == task["link_id"])
                .ok_or("Missing causal link")?;
            let from = ids(&link["from_ids"])?;
            state["link"] = link.clone();
            state["prerequisite_events"] = json!(
                from.iter()
                    .map(|id| get(id))
                    .collect::<Result<Vec<_>, _>>()?
            );
            state["target_event"] = get(field(link, "to_id"))?;
            let as_of = program["baseline"]
                .get("as_of")
                .unwrap_or(&snapshot["world"]["last_ingest_date"]);
            let branch = branch_state(world, link, field(task, "function"), as_of)?;
            state["ancestor_events"] = json!(
                branch["assignments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| get(field(a, "node_id")))
                    .collect::<Result<Vec<_>, _>>()?
            );
            let route_ids: BTreeSet<_> = branch["unassigned_upstream_routes"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|group| group["candidate_links"].as_array().into_iter().flatten())
                .flat_map(|route| route["from_ids"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .collect();
            if !route_ids.is_empty() {
                state["unassigned_route_events"] = json!(
                    route_ids
                        .into_iter()
                        .map(get)
                        .collect::<Result<Vec<_>, _>>()?
                );
            }
            state["branch_state"] = branch;

            // Do not condition on the target or on downstream consequences merely because
            // they belong to this candidate world. Only the explicit hypothetical ancestor state applies.
            state["world"] = json!({"assumptions":world["assumptions"]});
            if task["function"] == "check_transition" {
                json!({"type":"choice","instructions":"Does this stated mechanism plausibly connect the explicit prerequisite events to the target, within the interval, under the supplied assumptions? Assess missing steps, reversed cause/effect, constraints and feedback. Do not infer a causal effect from correlation. A plausible hypothesis is not established causation.","criteria":{"plausible":"The supplied mechanism could connect these events within the interval without a specific missing step.","conflict":"A concrete contradiction, reversed dependency or timing impossibility breaks the link.","uncertain":"A necessary intermediate step or mechanism remains materially unspecified."}})
            } else {
                let on = task["function"] == "conditional_on";
                state["condition"] = json!(if on {
                    "All hypothetical ancestor assignments in branch_state hold, including every direct prerequisite; none are observations"
                } else {
                    "Earlier hypothetical ancestor assignments in branch_state hold, but NOT ALL direct prerequisites occur by the deadline. Direct parents remain individually unassigned; do not invent which fails"
                });
                json!({"type":"noul","instructions":"Estimate P(target event occurs by link.by | state.condition, world assumptions and supplied present evidence). The condition is hypothetical. Condition only on branch_state.assignments and branch_state.condition; earlier ancestors are inherited hypothetical events, never observations. Do not condition on the target itself, downstream events or unrelated future components. This is a conditional model estimate, not an identified intervention effect. Do not multiply component odds. Account for alternative paths and shared causes.","criteria":{"true":"The target event occurs by the deadline under the stated condition.","false":"The target event does not occur by the deadline under the stated condition."}})
            }
        }
        _ => return Err("Unsupported structural question".into()),
    };
    if state["branch_state"]["unassigned_upstream_routes"]
        .as_array()
        .is_some_and(|routes| !routes.is_empty())
    {
        question["instructions"] = json!(format!(
            "{} unassigned_upstream_routes lists exact candidate routes as context only: none is assumed to occur, collectively required, exhaustive or mutually exclusive.",
            field(&question, "instructions")
        ));
    }
    let request = json!({"model":MODEL,"state":state,"questions":{"result":question}});
    if request.to_string().len() > 128 * 1024 {
        return Err("Structural request exceeds 128 KB".into());
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let world = json!({"Id":"w","component_ids":["a","b","c"],"facets":[{"id":"f1","title":"One","description":"First consequence","component_ids":["a"]},{"id":"f2","title":"Two","description":"Second consequence","component_ids":["b"]},{"id":"f3","title":"Three","description":"Third consequence","component_ids":["c"]}],"assumptions":[],"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","mechanism":"A enables B","by":"2027-02-01"},{"id":"bc","from_ids":["b"],"to_id":"c","mechanism":"B enables C","by":"2027-09-01"}]});
        let mut world = world;
        world["kind"] = json!("world");
        let snapshot = json!({"world":{"target_date":"2027-09-01","last_ingest_date":"2026-09-20"},"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},world.clone()]});
        (world, snapshot)
    }
    #[test]
    fn contributory_routes_remain_separate_and_ambiguous_ancestors_unassigned() {
        let (mut world, mut snapshot) = fixture();
        world["component_ids"] = json!(["a", "b", "c", "d"]);
        world["facets"][2]["component_ids"] = json!(["c", "d"]);
        world["chain"] = json!([
          {"id":"ac","from_ids":["a"],"to_id":"c","mechanism":"A contributes C","by":"2027-02-01"},
          {"id":"bc","from_ids":["b"],"to_id":"c","mechanism":"B separately contributes C","by":"2027-08-01"},
          {"id":"cd","from_ids":["c"],"to_id":"d","mechanism":"C contributes D","by":"2027-05-01"}]);
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .insert(3, json!({"Id":"d","kind":"scenario"}));
        snapshot["nodes"][4] = world.clone();
        validate_world(&world, &snapshot).unwrap();
        for (index, parent) in [(0, "a"), (1, "b")] {
            let state = branch_state(
                &world,
                &world["chain"][index],
                "conditional_on",
                &json!("2026-09-20"),
            )
            .unwrap();
            assert_eq!(state["assignments"].as_array().unwrap().len(), 1);
            assert_eq!(state["assignments"][0]["node_id"], parent);
            assert_eq!(state["target_id"], "c");
        }
        let state = branch_state(
            &world,
            &world["chain"][2],
            "conditional_on",
            &json!("2026-09-20"),
        )
        .unwrap();
        assert_eq!(
            state["assignments"],
            json!([{"node_id":"c","occurs":true,"by":"2027-05-01"}])
        );
        assert_eq!(state["history"], json!([]));
        assert_eq!(state["parent_state_ids"], json!([]));
        assert_eq!(
            state["unassigned_upstream_routes"][0]["candidate_links"],
            json!([world["chain"][0], world["chain"][1]])
        );
        let off =
            branch_state(&world, &world["chain"][2], "conditional_off", &Value::Null).unwrap();
        assert_eq!(off["assignments"], json!([]));
        assert_eq!(off["condition"]["kind"], "not_all_occurring");
        assert_eq!(
            off["unassigned_upstream_routes"],
            state["unassigned_upstream_routes"]
        );
        let tasks = world_tasks(&world);
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "conditional_on")
                .count(),
            3
        );
        let mut diamond = world.clone();
        diamond["chain"][2]["from_ids"] = json!(["c", "a"]);
        let inherited = branch_state(
            &diamond,
            &diamond["chain"][2],
            "conditional_on",
            &Value::Null,
        )
        .unwrap();
        let assigned: BTreeSet<_> = inherited["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| field(v, "node_id"))
            .collect();
        assert_eq!(assigned, BTreeSet::from(["a", "c"]));
        snapshot["nodes"][4] = diamond.clone();
        let task = world_tasks(&diamond)
            .into_iter()
            .find(|t| t["link_id"] == "cd" && t["function"] == "conditional_on")
            .unwrap();
        let request = request(&snapshot, &json!({}), &task).unwrap();
        let descriptors: BTreeSet<_> = request["state"]["unassigned_route_events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| field(v, "Id"))
            .collect();
        assert_eq!(descriptors, BTreeSet::from(["a", "b"]));
        assert_eq!(
            request["state"]["branch_state"]["assignments"],
            inherited["assignments"]
        );
        assert_eq!(
            request["state"]["branch_state"]["condition"],
            json!({"kind":"all_occurring","event_ids":["c","a"]})
        );
        assert!(
            request["questions"]["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("none is assumed to occur")
        );
        if let Ok(path) = std::env::var("FORESIGHT_ROUTES_CONTEXT_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&request["state"]).unwrap()).unwrap();
        }

        snapshot["nodes"][4] = world.clone();
        let mut reversed = world.clone();
        reversed["chain"].as_array_mut().unwrap().remove(0);
        assert!(
            validate_world(&reversed, &snapshot)
                .unwrap_err()
                .contains("nondecreasing")
        );
        let mut cyclic = world.clone();
        cyclic["chain"].as_array_mut().unwrap().push(
            json!({"id":"da","from_ids":["d"],"to_id":"a","mechanism":"Cycle","by":"2027-05-01"}),
        );
        for link in cyclic["chain"].as_array_mut().unwrap() {
            link["by"] = json!("2027-05-01");
        }
        assert!(
            validate_world(&cyclic, &snapshot)
                .unwrap_err()
                .contains("cycle")
        );
        let mut outside = world.clone();
        outside["chain"][1]["by"] = json!("2028-01-01");
        assert!(
            validate_world(&outside, &snapshot)
                .unwrap_err()
                .contains("outside")
        );
    }
    #[test]
    fn structural_validation_matches_request_checks_without_building_evidence_payloads() {
        let (world, mut snapshot) = fixture();
        let tasks: Vec<_> = world_tasks(&world)
            .into_iter()
            .filter(is_structural)
            .collect();
        for task in &tasks {
            assert!(validate_task(&snapshot, task).is_ok());
            assert!(request(&snapshot, &json!({}), task).is_ok());
        }
        for (key, value) in [
            ("world_id", json!("missing")),
            ("nodeId", json!("forged")),
            ("pair_ids", json!(["a", "a"])),
            ("pair_ids", json!(["a", "missing"])),
            ("depth", json!(1)),
        ] {
            let mut bad = tasks[0].clone();
            bad[key] = value;
            assert!(validate_task(&snapshot, &bad).is_err());
            assert!(request(&snapshot, &json!({}), &bad).is_err());
        }
        let mut link = tasks
            .iter()
            .find(|t| t["function"] == "check_transition")
            .unwrap()
            .clone();
        link["link_id"] = json!("missing");
        assert!(validate_task(&snapshot, &link).is_err());
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"large","kind":"evidence","statement":"x".repeat(150_000)}));
        assert!(validate_task(&snapshot, &tasks[0]).is_ok());
        assert!(request(&snapshot, &json!({}), &tasks[0]).is_err());
    }

    #[test]
    fn date_diagnostic_lists_every_reversed_link_with_world_and_dates() {
        let (mut world, mut snapshot) = fixture();
        world["component_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!("d"));
        world["facets"][2]["component_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!("d"));
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"d","kind":"scenario"}));
        world["chain"][1]["by"] = json!("2027-01-01");
        world["chain"].as_array_mut().unwrap().push(json!({"id":"bd","from_ids":["b"],"to_id":"d","mechanism":"B enables D","by":"2027-01-15"}));
        let error = validate_world(&world, &snapshot).unwrap_err();
        for expected in [
            "World 'w'",
            "link 'ab' (2027-02-01) -> link 'bc' (2027-01-01)",
            "link 'ab' (2027-02-01) -> link 'bd' (2027-01-15)",
            "nondecreasing",
            "not event resolve_by",
            "Parallel developments need no causal link",
        ] {
            assert!(error.contains(expected), "{error}");
        }
        world["chain"][1]["by"] = json!("2027-02-01");
        world["chain"][2]["by"] = json!("2027-02-01");
        assert!(validate_world(&world, &snapshot).is_ok());
    }
    #[test]
    fn causal_cycles_reversed_time_and_missing_facets_fail() {
        let (world, snapshot) = fixture();
        assert!(validate_world(&world, &snapshot).is_ok());
        let mut cyclic = world.clone();
        cyclic["chain"][0]["from_ids"] = json!(["c"]);
        assert!(validate_world(&cyclic, &snapshot).is_err());
        let mut early = world.clone();
        early["chain"][1]["by"] = json!("2027-01-01");
        assert!(validate_world(&early, &snapshot).is_err());
        let mut missing = world.clone();
        missing["facets"][2]["component_ids"] = json!(["a"]);
        assert!(validate_world(&missing, &snapshot).is_err());
        let mut disconnected = world.clone();
        disconnected["component_ids"] = json!(["a", "b", "c", "d"]);
        disconnected["facets"][2]["component_ids"] = json!(["c", "d"]);
        let mut parallel_snapshot = snapshot.clone();
        parallel_snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"d","kind":"scenario","statement":"A parallel development"}));
        assert!(validate_world(&disconnected, &parallel_snapshot).is_ok());
    }
    #[test]
    fn parallel_world_keeps_all_components_in_joint_evaluation_without_fake_links() {
        let (mut world, snapshot) = fixture();
        world["chain"] = json!([]);
        world["assumptions"] = json!([
            "The developments share a background condition; neither is asserted to cause another."
        ]);
        assert!(validate_world(&world, &snapshot).is_ok());
        let tasks = world_tasks(&world);
        assert_eq!(tasks.len(), 5); // Three pairs, whole-set consistency, joint likelihood.
        assert_eq!(tasks.last().unwrap()["function"], "estimate_likelihood");
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "check_pair")
                .count(),
            3
        );
        assert!(
            !tasks
                .iter()
                .any(|t| t["function"] == "conditional_on" || t["function"] == "conditional_off")
        );
        let audit = audit_world(&world, &json!({"results":{}}));
        let whole = audit["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "check_world_consistency")
            .unwrap();
        assert_eq!(whole["subject_ids"], world["component_ids"]);
        let mut uncovered = world.clone();
        uncovered["facets"][2]["component_ids"] = json!(["a"]);
        assert!(validate_world(&uncovered, &snapshot).is_err());
    }
    #[test]
    fn pairwise_agreement_cannot_clear_a_whole_set_conflict() {
        let (world, _) = fixture();
        let mut p = json!({"results":{},"evaluations":{}});
        assert_eq!(audit_world(&world, &p)["status"], "not_tested");
        for task in world_tasks(&world) {
            let id = field(&task, "nodeId");
            let f = field(&task, "function");
            if p["results"][id].is_null() {
                p["results"][id] = json!({});
            }
            p["results"][id][f] = json!(if f == "check_world_consistency" {
                "conflict"
            } else {
                "compatible"
            });
        }
        assert_eq!(audit_world(&world, &p)["status"], "conflicts_found");
    }
    #[test]
    fn combination_search_excludes_contradictory_pairs_but_retains_uncertain_candidates() {
        let (_, snapshot) = fixture();
        let mut p = json!({"results":{}});
        assert!(plan_combinations(&snapshot, &mut p, 100));
        assert_eq!(p["tasks"].as_array().unwrap().len(), 3);
        for task in p["tasks"].as_array().unwrap().clone() {
            p["results"][field(&task, "nodeId")] = json!({"check_pair":"compatible"});
        }
        finish_combinations(&mut p);
        assert_eq!(
            p["combination_search"]["candidate_sets"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        p["results"][pair_id("a", "b")]["check_pair"] = json!("conflict");
        finish_combinations(&mut p);
        assert_eq!(p["combination_search"]["candidate_sets"], json!([]));
        assert_eq!(
            p["combination_search"]["candidate_ids"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
    #[test]
    fn conditional_requests_never_condition_on_the_outcome_or_all_world_events() {
        let (world, snapshot) = fixture();
        let task = world_tasks(&world)
            .into_iter()
            .find(|t| t["function"] == "conditional_on")
            .unwrap();
        let r = request(&snapshot, &json!({}), &task).unwrap();
        assert_eq!(r["state"]["target_event"]["Id"], "b");
        assert!(r["state"]["world"]["component_ids"].is_null());
        assert_eq!(r["state"]["prerequisite_events"][0]["Id"], "a");
    }
}

/// Exact semantic input identity for reusable world audits. Feedback is a prior
/// judgment, not new evidence; every other request field remains fingerprinted.
pub fn audit_input_fingerprint(snapshot: &Value, task: &Value, request: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut canonical = request.clone();
    if let Some(state) = canonical["state"].as_object_mut() {
        state.remove("previous_world_judgments");
    }
    let nodes = snapshot["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let world = nodes.iter().find(|n| n["Id"] == task["world_id"]);
    let defining: Vec<_> = nodes
        .iter()
        .filter(|node| {
            world.is_some_and(|w| {
                ["component_ids", "counter_ids"].iter().any(|key| {
                    w[key]
                        .as_array()
                        .is_some_and(|ids| ids.contains(&node["Id"]))
                })
            })
        })
        .collect();
    let input = json!({"version":1,"request":canonical,"task":task,"world":world,"defining_events":defining,"branches":snapshot["branches"]});
    format!("{:x}", Sha256::digest(input.to_string().as_bytes()))
}

fn reusable_audit(snapshot: &Value, program: &Value, task: &Value) -> bool {
    let node = field(task, "nodeId");
    let function = field(task, "function");
    let saved = &program["evaluations"][node][function]["context"]["audit_input_fingerprint"];
    function != "estimate_likelihood"
        && program["results"][node][function].is_string()
        && saved.is_string()
        && program["evaluations"][node][function]["context"]["world_pass"]
            .as_u64()
            .is_some_and(|round| round > 0 && round <= program["world_pass"].as_u64().unwrap_or(1))
        && request(snapshot, program, task).is_ok_and(|request| {
            saved.as_str() == Some(audit_input_fingerprint(snapshot, task, &request).as_str())
        })
}

pub const MAX_REFINEMENT_PASSES: u64 = 3;
/// Feedback is an earlier model judgment over the same immutable proposition,
/// never new evidence and never a target probability to reproduce.
pub fn previous_world_judgments(program: &Value, world: &Value) -> Value {
    let id = field(world, "Id");
    let record = &program["world_refinement"][id];
    if !record.is_object() {
        return Value::Null;
    }
    let tasks = world_tasks(world);
    let components = world["component_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let legend: Vec<_> = tasks
        .iter()
        .map(|task| {
            let mut entry = json!({"function":task["function"]});
            if let Some(pair) = task["pair_ids"].as_array() {
                entry["component_indices"] = json!(
                    pair.iter()
                        .filter_map(|id| components.iter().position(|c| c == id))
                        .collect::<Vec<_>>()
                );
            }
            if task["link_id"].is_string() {
                entry["link_id"] = task["link_id"].clone();
            }
            entry
        })
        .collect();
    let mut contexts = vec![];
    let rounds:Vec<_>=record["rounds"].as_array().into_iter().flatten().map(|round| {
        let context=contexts.iter().position(|v|v==&round["evidence_ids"]).unwrap_or_else(||{contexts.push(round["evidence_ids"].clone());contexts.len()-1});
        let values:Vec<_>=tasks.iter().map(|task|round["assessments"][field(task,"nodeId")][field(task,"function")].clone()).collect();
        json!({"round":round["round"],"evaluation_mode":round["evaluation_mode"],"fresh_check_count":round["fresh_check_count"],"reused_check_count":round["reused_check_count"],"probability":round["probability"],"audit_status":round["audit_status"],"probability_coherence":round["probability_coherence"],"complete":round["complete"],"evidence_context":context,"judgments":values})
    }).collect();
    json!({"world_id":id,"definition":record["definition"],"component_ids":components,"question_legend":legend,"evidence_contexts":contexts,"rounds":rounds,"interpretation":"Each round's judgments vector corresponds by index to question_legend; component_indices are zero-based in component_ids. Counts distinguish fresh judgments from unchanged audits reused by exact input fingerprint; repeated values are not independent confirmations. Prior model judgments are not observations or ground truth. Reconsider them against supplied evidence, causal conditions and structural conflicts. Keep or revise either upward or downward; do not manufacture agreement or greater confidence."})
}

// Size the next pass with its newly captured history and the actual packer.
// Current judgments are sizing proxies only; no scratch request is sent or saved.
pub fn refinement_admission(snapshot: &Value, program: &Value, tasks: &[Value]) -> Value {
    let mut scratch = program.clone();
    scratch["tasks"] = json!(tasks);
    scratch["cursor"] = json!(0);
    scratch["world_pass"] = json!(program["world_pass"].as_u64().unwrap_or(1) + 1);
    let mut batches = 0u64;
    let mut cursor = 0usize;
    while cursor < tasks.len() {
        scratch["cursor"] = json!(cursor);
        match super::batch::prepare(snapshot, &scratch, tasks.len() - cursor) {
            Ok(batch) => {
                cursor += batch.tasks.len();
                batches += 1;
            }
            Err(error) => return json!({"admitted":false,"planning_error":error}),
        }
    }
    let limit = super::MAX_APP_TRANSITIONS - super::REASONING_TRANSITION_RESERVE;
    let remaining = limit.saturating_sub(super::transition_count(program));
    // One SearchPlanned callback, then Evaluate/Recorded for each HTTP attempt.
    let set_checkpoint = u64::from(
        tasks
            .iter()
            .any(|task| task["function"] == "check_world_set"),
    );
    // Reserve nominal packed work plus two extra attempts for this pass. Actual
    // retries, repacking and growth remain bounded by the live global limits;
    // admission estimates capacity, it does not guarantee completion.
    let retry_headroom_attempts = 2;
    let required = 1 + set_checkpoint + batches * 2 + retry_headroom_attempts * 2;
    json!({"admitted":required <= remaining,"estimated_batches":batches,
        "remaining_transitions":remaining,"required_transitions":required,
        "retry_headroom_attempts":retry_headroom_attempts,"completion_guaranteed":false,"writing_reserve":super::REASONING_TRANSITION_RESERVE,
        "response_growth_bounded":false,"adaptive_repacking_bounded":false})
}

/// Record a pass before deciding whether to schedule another. History and trace
/// are immutable; only current values are cleared for the re-evaluation.
pub fn refine_worlds(
    snapshot: &Value,
    program: &mut Value,
    calls: usize,
    elapsed: u64,
    blocked: &str,
) -> bool {
    let pass = program["world_pass"].as_u64().unwrap_or(1);
    let active = program["active_world_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let worlds: Vec<_> = snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| active.contains(&n["Id"]))
        .collect();
    if worlds.is_empty() {
        return false;
    }
    if !program["world_refinement"].is_object() {
        program["world_refinement"] = json!({});
    }
    // A same-ID edit is not an optional confidence pass. Invalidate its current
    // results before any budget refusal; completed historical receipts stay intact.
    let changed_worlds: Vec<_> = worlds
        .iter()
        .filter(|world| {
            world_tasks(world).iter().any(|task| {
                let saved = &program["evaluations"][field(task, "nodeId")][field(task, "function")]
                    ["context"]["audit_input_fingerprint"];
                saved.is_string() && !reusable_audit(snapshot, program, task)
            })
        })
        .copied()
        .collect();
    for world in &changed_worlds {
        for task in world_tasks(world) {
            for collection in ["results", "evaluations"] {
                if let Some(values) = program[collection][field(&task, "nodeId")].as_object_mut() {
                    values.remove(field(&task, "function"));
                }
            }
        }
    }
    let refresh_required = !changed_worlds.is_empty();
    let mut all_stable = true;
    let mut all_complete = true;
    let mut tasks = vec![];
    for world in &worlds {
        let id = field(world, "Id");
        let mut assessments = json!({});
        let mut evaluations = json!({});
        let world_tasks = world_tasks(world);
        let complete = world_tasks.iter().all(|task| {
            program["results"][field(task, "nodeId")][field(task, "function")].is_string()
        });
        for task in &world_tasks {
            let node = field(task, "nodeId");
            let function = field(task, "function");
            assessments[node][function] = program["results"][node][function].clone();
            evaluations[node][function] = program["evaluations"][node][function].clone();
        }
        let probability = program["results"][id]["estimate_likelihood"]
            .as_str()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p));
        let reused_checks: Vec<_> = world_tasks
            .iter()
            .filter_map(|task| {
                let node = field(task, "nodeId");
                let function = field(task, "function");
                let context = &program["evaluations"][node][function]["context"];
                let source_round = context["world_pass"].as_u64()?;
                (source_round < pass && reusable_audit(snapshot, program, task)).then(|| {
                    json!({
                        "node_id":node,"function":function,"source_round":source_round,
                        "input_fingerprint":context["audit_input_fingerprint"]
                    })
                })
            })
            .collect();
        let fresh_check_count = world_tasks.iter().filter(|task|
            program["results"][field(task,"nodeId")][field(task,"function")].is_string()
            && program["evaluations"][field(task,"nodeId")][field(task,"function")]["context"]["world_pass"].as_u64() == Some(pass)
        ).count();
        let mut receipt = json!({"round":pass,"probability":probability,"audit_status":audit_world(world,program)["status"],"probability_coherence":super::coherence::audit(world,program),"complete":complete,"evidence_ids":program["evidence_ids"],"assessments":assessments,"evaluations":evaluations});
        if !program["world_refinement"][id].is_object() {
            program["world_refinement"][id] = json!({"world_id":id,"definition":world["statement"],"rounds":[],"stop_reason":"in_progress","converged":false,"accuracy_verified":false});
        }
        let reuse = world_tasks
            .iter()
            .filter(|t| field(t, "function") != "estimate_likelihood")
            .all(|task| reusable_audit(snapshot, program, task));
        // Legacy records remain readable without inventing provenance/counts.
        let accounted = fresh_check_count + reused_checks.len();
        let present = world_tasks
            .iter()
            .filter(|task| {
                program["results"][field(task, "nodeId")][field(task, "function")].is_string()
            })
            .count();
        if accounted == present && (reuse || present == 0) {
            receipt["evaluation_mode"] = json!("fresh_estimate_with_exact_audit_reuse");
            receipt["fresh_check_count"] = json!(fresh_check_count);
            receipt["reused_check_count"] = json!(reused_checks.len());
            receipt["reused_checks"] = json!(reused_checks);
        }
        let history = program["world_refinement"][id]["rounds"]
            .as_array_mut()
            .unwrap();
        // Resuming an interrupted pass updates that pass, not the round count.
        // Its previous checkpoint and every HTTP attempt remain in the old run
        // and immutable trace. Completed round receipts are never rewritten.
        history.retain(|prior| prior["round"] != pass || prior["complete"] == true);
        let current = history
            .iter()
            .find(|prior| prior["round"] == pass)
            .unwrap_or(&receipt);
        let stable = history
            .iter()
            .rev()
            .find(|prior| prior["round"].as_u64().is_some_and(|round| round < pass))
            .is_some_and(|previous| {
                previous["complete"] == true
                    && current["complete"] == true
                    && reuse
                    && world_tasks.iter().filter(|t| field(t, "function") != "estimate_likelihood").all(|task| {
                        let node = field(task, "nodeId");
                        let function = field(task, "function");
                        let old = &previous["evaluations"][node][function]["context"]["audit_input_fingerprint"];
                        old.is_string() && old == &current["evaluations"][node][function]["context"]["audit_input_fingerprint"]
                    })
                    && previous["probability"]
                        .as_f64()
                        .zip(current["probability"].as_f64())
                        .is_some_and(|(old, now)| (old - now).abs() <= 0.02)
            });
        if !history.iter().any(|prior| prior["round"] == pass) {
            history.push(receipt);
        }
        all_stable &= stable;
        all_complete &= complete;
        // Reuse only when every audit still matches. A changed or legacy input
        // requires a complete fresh audit, never a mixture of stale constraints.
        tasks.extend(
            world_tasks
                .into_iter()
                .filter(|t| !reuse || field(t, "function") == "estimate_likelihood"),
        );
    }
    let mut reason = if !blocked.is_empty() {
        blocked
    } else if !all_complete && !refresh_required {
        "incomplete_pass"
    } else if pass >= 2 && all_stable {
        "stable_world_estimates"
    } else if pass >= MAX_REFINEMENT_PASSES {
        "max_refinement_passes"
    } else if super::MAX_CALLS.saturating_sub(calls) < tasks.len() {
        "call_budget"
    } else if elapsed >= super::MAX_MS.saturating_sub(120_000) {
        "time_budget"
    } else {
        "in_progress"
    };
    if reason == "in_progress" {
        let admission = refinement_admission(snapshot, program, &tasks);
        if admission["admitted"] != true {
            reason = "transition_budget";
        }
        program["refinement_admission"] = admission;
    }
    for world in &worlds {
        let id = field(world, "Id");
        program["world_refinement"][id]["stop_reason"] = json!(reason);
        program["world_refinement"][id]["converged"] = json!(reason == "stable_world_estimates");
    }
    if reason != "in_progress" {
        return false;
    }
    for task in &tasks {
        for collection in ["results", "evaluations"] {
            if let Some(values) = program[collection][field(task, "nodeId")].as_object_mut() {
                values.remove(field(task, "function"));
            }
        }
    }
    program["tasks"] = json!(tasks);
    program["cursor"] = json!(0);
    program["world_pass"] = json!(pass + 1);
    program["stop_reason"] = json!("world_refinement_in_progress");
    true
}

#[cfg(test)]
mod refinement_tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let world = json!({"Id":"w","kind":"world","statement":"A and B and C jointly occur","component_ids":["a","b","c"],"counter_ids":[],"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","mechanism":"A enables B","by":"2027-01-01"}],"assumptions":[],"edges":"[]"});
        let snapshot = json!({"world":{},"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},world]});
        let program = json!({"active_world_ids":["w"],"world_pass":1,"results":{},"evaluations":{},"evidence_ids":["source"]});
        (snapshot, program)
    }
    #[test]
    fn exact_audits_reuse_but_changed_inputs_invalidate_even_when_budget_declines() {
        let (mut snapshot, mut program) = fixture();
        snapshot["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"source","kind":"evidence","statement":"Original source"}));
        fill(&snapshot, &mut program, 0.4);
        let legacy = {
            let mut p = program.clone();
            for functions in p["evaluations"].as_object_mut().unwrap().values_mut() {
                for evaluation in functions.as_object_mut().unwrap().values_mut() {
                    evaluation["context"]
                        .as_object_mut()
                        .unwrap()
                        .remove("audit_input_fingerprint");
                }
            }
            p
        };
        let mut old = legacy.clone();
        assert!(refine_worlds(&snapshot, &mut old, 20, 1000, ""));
        assert!(
            old["tasks"].as_array().unwrap().len() > 1,
            "legacy cannot reuse"
        );
        assert!(old["world_refinement"]["w"]["rounds"][0]["evaluation_mode"].is_null());
        let original = program.clone();
        assert!(refine_worlds(&snapshot, &mut program, 20, 1000, ""));
        assert_eq!(program["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(program["tasks"][0]["function"], "estimate_likelihood");
        let history = program["world_refinement"].clone();
        for mutation in 0..6 {
            let mut changed = snapshot.clone();
            match mutation {
                0 => changed["nodes"][4]["statement"] = json!("Changed source, same ID"),
                1 => changed["nodes"][3]["chain"][0]["mechanism"] = json!("Changed mechanism"),
                2 => changed["nodes"][3]["chain"][0]["by"] = json!("2027-02-01"),
                3 => changed["nodes"][3]["assumptions"] = json!(["New assumption"]),
                4 => changed["branches"] = json!([{"id":"changed"}]),
                _ => changed["world"]["question"] = json!("Different question"),
            }
            let mut p = original.clone();
            p["world_refinement"] = history.clone();
            p["transition_count"] = json!(435);
            assert!(!refine_worlds(&changed, &mut p, 20, 1000, ""));
            assert!(
                p["results"]["w"]["estimate_likelihood"].is_null(),
                "mutation {mutation}"
            );
            assert_eq!(p["world_refinement"]["w"]["rounds"], history["w"]["rounds"]);
            assert_eq!(
                p["world_refinement"]["w"]["stop_reason"],
                "transition_budget"
            );
            let mut renewed = original.clone();
            renewed["world_refinement"] = history.clone();
            assert!(refine_worlds(&changed, &mut renewed, 20, 1000, ""));
            assert!(renewed["tasks"].as_array().unwrap().len() > 1);
            fill(&changed, &mut renewed, 0.4);
            assert!(
                refine_worlds(&changed, &mut renewed, 40, 2000, ""),
                "equal odds on changed basis must not establish stability"
            );
            assert_eq!(renewed["world_refinement"]["w"]["converged"], false);
        }
    }

    #[test]
    fn refinement_uses_pass_contingency_without_claiming_completion() {
        let (snapshot, mut program) = fixture();
        fill(&snapshot, &mut program, 0.4);
        let tasks = world_tasks(&snapshot["nodes"][3]);
        let initial = refinement_admission(&snapshot, &program, &tasks);
        let nominal = 1 + initial["estimated_batches"].as_u64().unwrap() * 2;
        program["transition_count"] = json!(
            super::super::MAX_APP_TRANSITIONS
                - super::super::REASONING_TRANSITION_RESERVE
                - nominal
                - 4
        );
        let admission = refinement_admission(&snapshot, &program, &tasks);
        assert_eq!(admission["admitted"], true);
        assert_eq!(admission["completion_guaranteed"], false);
        assert_eq!(admission["retry_headroom_attempts"], 2);
        program["transition_count"] = json!(program["transition_count"].as_u64().unwrap() + 1);
        assert_eq!(
            refinement_admission(&snapshot, &program, &tasks)["admitted"],
            false
        );
    }
    fn fill(snapshot: &Value, program: &mut Value, probability: f64) {
        for task in world_tasks(&snapshot["nodes"][3]) {
            let id = field(&task, "nodeId");
            let function = field(&task, "function");
            let numeric = matches!(
                function,
                "estimate_likelihood" | "conditional_on" | "conditional_off"
            );
            program["results"][id][function] = json!(if numeric {
                probability.to_string()
            } else {
                "compatible".into()
            });
            program["evaluations"][id][function] = if numeric {
                json!({"probability":probability})
            } else {
                json!({"selected":"compatible"})
            };
            let mut context = json!({"world_pass":program["world_pass"]});
            if function != "estimate_likelihood" {
                context["audit_input_fingerprint"] = json!(audit_input_fingerprint(
                    snapshot,
                    &task,
                    &request(snapshot, program, &task).unwrap()
                ));
            }
            program["evaluations"][id][function]["context"] = context;
        }
    }
    #[test]
    fn resumed_partial_pass_does_not_create_duplicate_rounds_or_false_convergence() {
        let (snapshot, mut program) = fixture();
        assert!(!refine_worlds(
            &snapshot,
            &mut program,
            1,
            1000,
            "provider_error"
        ));
        assert!(!refine_worlds(
            &snapshot,
            &mut program,
            2,
            2000,
            "provider_error"
        ));
        assert_eq!(
            program["world_refinement"]["w"]["rounds"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // Repair historical repeated incomplete receipts from older releases.
        let partial = program["world_refinement"]["w"]["rounds"][0].clone();
        program["world_refinement"]["w"]["rounds"]
            .as_array_mut()
            .unwrap()
            .push(partial);
        fill(&snapshot, &mut program, 0.23);
        assert!(refine_worlds(&snapshot, &mut program, 20, 3000, ""));
        assert_eq!(program["world_pass"], 2);
        let history = &program["world_refinement"]["w"]["rounds"];
        assert_eq!(history.as_array().unwrap().len(), 1);
        assert_eq!(history[0]["round"], 1);
        assert_eq!(history[0]["complete"], true);
        assert_eq!(program["world_refinement"]["w"]["converged"], false);
        let first = history[0].clone();
        assert!(!refine_worlds(
            &snapshot,
            &mut program,
            21,
            4000,
            "provider_error"
        ));
        fill(&snapshot, &mut program, 0.24);
        assert!(!refine_worlds(&snapshot, &mut program, 40, 5000, ""));
        assert_eq!(program["world_refinement"]["w"]["rounds"][0], first);
        assert_eq!(
            program["world_refinement"]["w"]["rounds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(program["world_refinement"]["w"]["converged"], true);
    }

    #[test]
    fn second_pass_consumes_prior_judgments_without_reusing_current_values() {
        let (snapshot, mut program) = fixture();
        fill(&snapshot, &mut program, 0.23);
        assert!(refine_worlds(&snapshot, &mut program, 20, 1000, ""));
        let first = program["world_refinement"]["w"]["rounds"][0].clone();
        assert_eq!(program["world_pass"], 2);
        assert!(program["results"]["w"]["estimate_likelihood"].is_null());
        let request = super::super::request(&snapshot, &program).unwrap();
        assert_eq!(
            request["state"]["previous_world_judgments"]["rounds"][0]["probability"],
            0.23
        );
        let feedback = &request["state"]["previous_world_judgments"];
        let index = feedback["question_legend"]
            .as_array()
            .unwrap()
            .iter()
            .position(|q| q["function"] == "check_world_consistency")
            .unwrap();
        assert_eq!(feedback["rounds"][0]["judgments"][index], "compatible");
        fill(&snapshot, &mut program, 0.24);
        assert!(!refine_worlds(&snapshot, &mut program, 40, 2000, ""));
        assert_eq!(program["world_refinement"]["w"]["rounds"][0], first);
        assert_eq!(
            program["world_refinement"]["w"]["stop_reason"],
            "stable_world_estimates"
        );
        assert_eq!(program["world_refinement"]["w"]["accuracy_verified"], false);
    }
    #[test]
    fn maximal_world_feedback_compacts_long_ids_without_dropping_judgments() {
        let ids: Vec<_> = (0..12)
            .map(|i| format!("component-{i}-{}", "x".repeat(80)))
            .collect();
        let chain:Vec<_>=(1..12).map(|i|json!({"id":format!("link-{i}"),"from_ids":[ids[i-1]],"to_id":ids[i],"mechanism":"m".repeat(800),"by":"2027-09-01"})).collect();
        let facets:Vec<_>=(0..12).map(|i|json!({"id":format!("f{i}"),"title":"t".repeat(100),"description":"d".repeat(800),"component_ids":[ids[i]]})).collect();
        let world = json!({"Id":format!("world-{}","w".repeat(80)),"kind":"world","statement":"s".repeat(1000),"component_ids":ids,"counter_ids":[],"facets":facets,"chain":chain,"assumptions":vec!["a".repeat(600);12],"edges":"[]"});
        let mut nodes: Vec<_> = ids
            .iter()
            .map(|id| json!({"Id":id,"kind":"scenario","statement":"future event","edges":"[]"}))
            .collect();
        for i in 0..35 {
            nodes.push(json!({"Id":format!("source-{i}"),"kind":"research_evidence","statement":"e".repeat(1900),"quote":"Actual source excerpt","edges":"[]"}));
        }
        nodes.push(world.clone());
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-19","target_date":"2027-09-19"},"nodes":nodes});
        validate_world(&world, &snapshot).unwrap();
        let mut program = json!({"world_pass":1,"active_world_ids":[world["Id"]],"results":{},"evaluations":{},"evidence_ids":(0..35).map(|i|format!("source-{i}")).collect::<Vec<_>>()});
        for pass in 1..=2 {
            for task in world_tasks(&world) {
                let function = field(&task, "function");
                program["results"][field(&task, "nodeId")][function] = json!(if matches!(
                    function,
                    "estimate_likelihood" | "conditional_on" | "conditional_off"
                ) {
                    format!("0.{pass}")
                } else {
                    "compatible".into()
                });
            }
            assert!(refine_worlds(&snapshot, &mut program, pass * 102, 1000, ""));
        }
        let feedback = previous_world_judgments(&program, &world);
        let count = world_tasks(&world).len();
        assert_eq!(feedback["question_legend"].as_array().unwrap().len(), count);
        assert!(
            feedback["rounds"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["judgments"].as_array().unwrap().len() == count)
        );
        assert!(feedback.to_string().len() < 20 * 1024);
        let task = world_tasks(&world)
            .into_iter()
            .find(|t| t["function"] == "check_world_consistency")
            .unwrap();
        let request = request(&snapshot, &program, &task).unwrap();
        assert!(request.to_string().len() < 128 * 1024);
        program["tasks"] = json!(world_tasks(&world));
        program["cursor"] = json!(world_tasks(&world).len() - 1);
        let likelihood = super::super::request(&snapshot, &program).unwrap();
        assert!(likelihood.to_string().len() < 128 * 1024);
    }

    #[test]
    fn changing_judgments_stop_after_three_and_partial_failure_never_converges() {
        let (snapshot, mut program) = fixture();
        for (index, probability) in [0.2, 0.7, 0.4].into_iter().enumerate() {
            fill(&snapshot, &mut program, probability);
            assert_eq!(
                refine_worlds(&snapshot, &mut program, 20 * (index + 1), 1000, ""),
                index < 2
            );
        }
        assert_eq!(
            program["world_refinement"]["w"]["rounds"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            program["world_refinement"]["w"]["stop_reason"],
            "max_refinement_passes"
        );
        assert_eq!(program["world_refinement"]["w"]["converged"], false);
        let (snapshot, mut failed) = fixture();
        fill(&snapshot, &mut failed, 0.23);
        assert!(refine_worlds(&snapshot, &mut failed, 20, 1000, ""));
        assert!(!refine_worlds(
            &snapshot,
            &mut failed,
            21,
            2000,
            "provider_error"
        ));
        assert_eq!(
            failed["world_refinement"]["w"]["stop_reason"],
            "provider_error"
        );
        assert!(failed["world_refinement"]["w"]["rounds"][1]["probability"].is_null());
        assert_eq!(
            failed["world_refinement"]["w"]["rounds"][1]["complete"],
            false
        );
        assert_eq!(failed["world_refinement"]["w"]["converged"], false);
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let world = json!({"Id":"w","kind":"world","component_ids":["a","b","c","d"],"assumptions":[],"chain":[{"id":"ab","from_ids":["a"],"to_id":"b","by":"2027-02-01","mechanism":"A enables B"},{"id":"bc","from_ids":["b"],"to_id":"c","by":"2027-05-01","mechanism":"B enables C"},{"id":"cd","from_ids":["c"],"to_id":"d","by":"2027-09-01","mechanism":"C enables D"}]});
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-29"},"nodes":[{"Id":"a","kind":"scenario"},{"Id":"b","kind":"scenario"},{"Id":"c","kind":"scenario"},{"Id":"d","kind":"scenario"},{"Id":"unrelated","kind":"scenario"},world.clone()]});
        (world, snapshot)
    }
    #[test]
    fn conditional_branches_inherit_ancestors_without_target_or_downstream_conditioning() {
        let (world, snapshot) = fixture();
        let program = json!({"baseline":{"as_of":"2026-09-29"}});
        let original = world.clone();
        let task = |f: &str| {
            world_tasks(&world)
                .into_iter()
                .find(|t| t["link_id"] == "bc" && t["function"] == f)
                .unwrap()
        };
        let on = request(&snapshot, &program, &task("conditional_on")).unwrap();
        let off = request(&snapshot, &program, &task("conditional_off")).unwrap();
        let a = &on["state"]["branch_state"];
        let b = &off["state"]["branch_state"];
        assert_eq!(
            a["assignments"],
            json!([{"node_id":"a","occurs":true,"by":"2027-02-01"},{"node_id":"b","occurs":true,"by":"2027-02-01"}])
        );
        assert_eq!(
            b["assignments"],
            json!([{"node_id":"a","occurs":true,"by":"2027-02-01"}])
        );
        assert_eq!(b["unassigned_parent_ids"], json!(["b"]));
        assert_eq!(b["history"], json!([]));
        assert_eq!(b["parent_state_ids"], json!([]));
        assert_eq!(
            b["condition"],
            json!({"kind":"not_all_occurring","event_ids":["b"]})
        );
        assert_eq!(a["as_of"], "2026-09-29");
        assert_eq!(a["parent_state_ids"], json!(["w/branch/ab/conditional_on"]));
        assert_eq!(a["history"].as_array().unwrap().len(), 1);
        for branch in [a, b] {
            assert!(
                branch["assignments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| !matches!(field(v, "node_id"), "c" | "d" | "unrelated"))
            );
        }
        assert_eq!(world, original);
        assert_eq!(
            request(&snapshot, &program, &task("conditional_on")).unwrap(),
            on
        );
        let audit = audit_world(&world, &program);
        let check = audit["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "w/link/bc/conditional_off")
            .unwrap();
        assert_eq!(check["branch_state"], *b);
    }
    #[test]
    fn off_joint_parents_are_unassigned_even_when_one_is_another_parents_ancestor() {
        let (mut world, _) = fixture();
        world["chain"][1]["from_ids"] = json!(["a", "b"]);
        let branch = branch_state(
            &world,
            &world["chain"][1],
            "conditional_off",
            &json!("2026-09-29"),
        )
        .unwrap();
        assert_eq!(branch["assignments"], json!([]));
        assert_eq!(branch["condition"]["event_ids"], json!(["a", "b"]));
        assert_eq!(branch["unassigned_parent_ids"], json!(["a", "b"]));
        world["chain"][0]["from_ids"] = json!(["c"]);
        assert!(branch_state(&world, &world["chain"][1], "conditional_on", &Value::Null).is_err());
    }
    #[test]
    fn combinations_exclude_observed_and_mixed_temporal_candidates() {
        let (_, snapshot) = fixture();
        let mut p = json!({"results":{"a":{"classify_temporal":"already_observed"},"b":{"classify_temporal":"mixed"},"c":{"classify_temporal":"future_change"},"d":{"classify_temporal":"uncertain"}}});
        assert!(plan_combinations(&snapshot, &mut p, 30));
        let ids = p["combination_search"]["candidate_ids"].as_array().unwrap();
        assert!(!ids.contains(&json!("a")));
        assert!(!ids.contains(&json!("b")));
        assert!(ids.contains(&json!("c")));
        assert!(ids.contains(&json!("d")));
    }
}

#[cfg(test)]
mod world_set_tests {
    use super::*;
    #[test]
    fn set_projection_preserves_external_premises_and_source_limitations() {
        let branch = json!([{"branch_id":"b","kind":"not_all_occurring","by":"2036-01-01","events":[{"id":"outside-components","statement":"A scoped future premise","by":"2035-01-01"}]}]);
        let mut state = json!({"baseline":{"unknowns":["Missing current evidence"]},"proposed_worlds":[{"Id":"w","statement":"Full definition","mechanism":"Full mechanism","branch_conditions":branch,"component_ids":["h"],"counter_ids":[],"source_session_id":"old","scene":"presentation"}],"components":[{"Id":"h","statement":"Full event","mechanism":"Full cause","evidence_note":"Limited support","source_refs":[{"quote":"Qualification"}],"branch_state":{"duplicate":true},"evaluations":{"score":9}}],"source_evidence":[{"Id":"e","statement":"Only a title was fetched; not proof of adoption","evidence_metadata":{"kind":"lead"},"provenance":"weak_signal","source_session_id":"old"}]});
        let original = state.clone();
        set_audit_projection(&mut state);
        assert_eq!(state["proposed_worlds"][0]["branch_conditions"], branch);
        assert_eq!(
            state["components"][0]["source_refs"],
            original["components"][0]["source_refs"]
        );
        assert_eq!(
            state["source_evidence"][0]["statement"],
            original["source_evidence"][0]["statement"]
        );
        assert_eq!(
            state["source_evidence"][0]["evidence_metadata"],
            original["source_evidence"][0]["evidence_metadata"]
        );
        assert_eq!(state["baseline"], original["baseline"]);
        let mut changed = original;
        changed["proposed_worlds"][0]["scene"] = json!("Different presentation");
        changed["components"][0]["evaluations"] = json!({"score":0});
        changed["source_evidence"][0]["source_session_id"] = json!("new");
        set_audit_projection(&mut changed);
        assert_eq!(state, changed);
        assert!(state["components"][0].get("branch_state").is_none());
    }

    #[test]
    fn set_identity_is_exact_and_every_world_remains_in_context() {
        let worlds = json!(["w1", "w2"]);
        let task = world_set_task(worlds.as_array().unwrap());
        let snapshot = json!({"world":{"name":"Open question"},"nodes":[{"Id":"w1","kind":"world","statement":"One direction","assumptions":["A"]},{"Id":"w2","kind":"world","statement":"Another direction","assumptions":["B"]}]});
        assert!(validate_task(&snapshot, &task).is_ok());
        let request = request(
            &snapshot,
            &json!({"baseline":{"as_of":"2026-09-30"}}),
            &task,
        )
        .unwrap();
        assert_eq!(request["state"]["proposed_worlds"], snapshot["nodes"]);
        let mut modern = snapshot.clone();
        for (i, world) in modern["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            world["shared_question"] = json!("How does control of the system change?");
            world["trajectory_answer"] =
                json!(format!("Trajectory {i} changes the overall system"));
        }
        let modern_request = super::request(&modern, &json!({}), &task).unwrap();
        assert_eq!(
            modern_request["state"]["shared_question"],
            modern["nodes"][0]["shared_question"]
        );
        modern["nodes"][1]["shared_question"] = json!("A different narrow topic");
        assert!(super::request(&modern, &json!({}), &task).is_err());
        let mut forged = task.clone();
        forged["world_ids"][1] = json!("invented");
        assert!(validate_task(&snapshot, &forged).is_err());
        let duplicate = world_set_task(&[json!("w1"), json!("w1")]);
        assert!(validate_task(&snapshot, &duplicate).is_err());
    }
}
