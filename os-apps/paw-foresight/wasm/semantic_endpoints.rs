// Endpoint-first search is persisted in SemanticRun; proposals are not observations.
use super::{field, search};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_WORLD_COMPONENTS: usize = 32;
mod bundles { include!("semantic_route_bundles.rs"); }
pub use bundles::{composition_bundles, validate_selection};


pub fn enabled(program: &Value) -> bool {
    program["world_search_contract"] == 1
}
fn text(value: &Value, max: usize) -> Result<&str, String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= max)
        .ok_or_else(|| format!("Endpoint text must contain 1–{max} characters"))
}
fn identifier(value: &Value) -> Result<&str, String> {
    let id = text(value, 120)?;
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || id.starts_with("ref_")
    {
        return Err("Invalid endpoint or route identity".into());
    }
    Ok(id)
}
fn list(value: &Value, max: usize) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .filter(|a| a.len() <= max)
        .ok_or("Invalid endpoint reference list")?;
    let mut seen = BTreeSet::new();
    for item in items {
        let id = text(item, 300)?;
        if !seen.insert(id.to_owned()) {
            return Err("Duplicate endpoint reference".into());
        }
    }
    Ok(items
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect())
}

/// Freeze imagined endpoint commitments before any prerequisite generation.
pub fn imagine(snapshot: &Value, old: &Value, generated: &Value) -> Result<Value, String> {
    if !enabled(old) || old["endpoint_search"].is_object() {
        return Err("Endpoints must be imagined once before backward search".into());
    }
    if ["hypotheses", "routes", "worlds"].iter().any(|key| {
        generated[*key]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    }) {
        return Err("Imagine endpoints before generating components or routes".into());
    }
    let mut generated = generated.clone();
    if super::proposals::pool::enabled(old) {
        super::references_for_endpoints::References::new(snapshot)?
            .resolve_generated(&mut generated);
    }
    let (min, max) = if super::proposals::pool::enabled(old) {
        if old["proposal_pool"]["stage"] == "enrich" {
            (3, 5)
        } else {
            (8, 12)
        }
    } else {
        (2, 6)
    };
    let endpoints = validate_proposals(&generated["endpoints"], min, max)?;
    if super::proposals::pool::enabled(old) {
        return super::proposals::pool::receive(snapshot, old, endpoints);
    }
    if old["endpoint_proposal_contract"] == 1 {
        return super::proposals::plan(snapshot, old, endpoints);
    }
    let mut program = old.clone();
    program["endpoint_search"] =
        json!({"status":"imagined","endpoints":endpoints,"routes":[],"amendments":[],"rounds":[]});
    program["tasks"] = json!([]);
    program["cursor"] = json!(0);
    program["stage"] = json!("exploration");
    Ok(program)
}

pub fn validate_proposals(proposals: &Value, min: usize, max: usize) -> Result<Vec<Value>, String> {
    let proposals = proposals
        .as_array()
        .filter(|v| (min..=max).contains(&v.len()))
        .ok_or("Candidate count outside the supplied bounded range")?;
    let mut ids = BTreeSet::new();
    let mut endpoints = vec![];
    for proposal in proposals {
        let id = identifier(&proposal["id"])?;
        if !ids.insert(id) {
            return Err("Duplicate endpoint identity".into());
        }
        let commitments = proposal["commitments"]
            .as_array()
            .filter(|v| (3..=8).contains(&v.len()))
            .ok_or("Endpoint needs three to eight load-bearing commitments")?;
        let mut claims = BTreeSet::new();
        for claim in commitments {
            if !claims.insert(identifier(&claim["id"])?) {
                return Err("Duplicate endpoint commitment".into());
            }
            text(&claim["statement"], 1000)?;
        }
        text(&proposal["title"], 100)?;
        text(&proposal["original_statement"], 1000)?;
        text(&proposal["original_narrative"], 2400)?;
        for key in ["signals", "falsifiers"] {
            for value in proposal[key]
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 8)
                .ok_or("Missing endpoint signals/falsifiers")?
            {
                text(value, 240)?;
            }
        }
        let mut endpoint = proposal.clone();
        endpoint["status"] = json!("imagined");
        endpoints.push(endpoint);
    }
    Ok(endpoints)
}

fn endpoint<'a>(program: &'a Value, id: &str) -> Result<&'a Value, String> {
    program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["id"] == id)
        .ok_or("Unknown original endpoint".into())
}
fn commitment<'a>(endpoint: &'a Value, id: &str) -> Result<&'a Value, String> {
    endpoint["commitments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"] == id)
        .ok_or("Unknown original commitment".into())
}

/// A connected authored path is not proof that its premises or mechanism hold.
fn validate_root_connection(anchor: &Value) -> Result<Vec<String>, String> {
    let refs = list(&anchor["evidence_ids"], 16)?;
    if refs.is_empty() {
        text(&anchor["unresolved_question"], 400)?;
        // With no claimed bridge, an absent mechanism is honest. Any supplied
        // mechanism must still satisfy the same size/type contract.
        if !anchor["mechanism"].is_null() && anchor["mechanism"] != "" {
            text(&anchor["mechanism"], 800)?;
        }
    } else {
        text(&anchor["mechanism"], 800)?;
    }
    Ok(refs)
}

fn invalid_route_components(
    route: &Value,
    snapshot: &Value,
    path: &str,
) -> Result<Vec<String>, String> {
    let nodes = snapshot["nodes"].as_array().ok_or("Missing route graph")?;
    let refs = super::references_for_endpoints::References::new(snapshot)?;
    let mut errors = vec![];
    for (index, id) in list(&route["component_ids"], 12)?.iter().enumerate() {
        let node = nodes.iter().find(|n| n["Id"] == *id);
        if !node.is_some_and(|n| matches!(field(n, "kind"), "scenario" | "revision")) {
            let shown = refs.project(&json!({"component_id":id}));
            errors.push(format!(
                "{path}.component_ids[{index}]={} has kind {}",
                field(&shown, "component_id"),
                node.map(|n| field(n, "kind")).unwrap_or("missing")
            ));
        }
    }
    Ok(errors)
}

fn route_component_error(errors: Vec<String>) -> String {
    format!(
        "Route components must name candidate events: {}. Only scenario/revision nodes belong in component_ids and chain. Put source evidence in root_connections.evidence_ids or grounding_evidence_ids; state the conjectural bridge as a separate hypothesis, never relabel an observation as a future event.",
        errors.join("; ")
    )
}

fn validate_route(route: &Value, snapshot: &Value, program: &Value) -> Result<(), String> {
    let endpoint = endpoint(program, field(route, "endpoint_id"))?;
    let claim = commitment(endpoint, field(route, "commitment_id"))?;
    let components = list(&route["component_ids"], 12)?;
    let target = field(route, "target_component_id");
    if components.len() < 2 || !components.iter().any(|id| id == target) {
        return Err("Route needs target and conjectural prerequisites".into());
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing route graph")?;
    let active_sources = super::evidence::active_sources(snapshot);
    let component_errors = invalid_route_components(route, snapshot, "route")?;
    if !component_errors.is_empty() {
        return Err(route_component_error(component_errors));
    }
    let target_node = nodes
        .iter()
        .find(|n| n["Id"] == target)
        .ok_or("Missing route target")?;
    let expected = if let Some(amendment_id) = route["amendment_id"].as_str() {
        &program["endpoint_search"]["amendments"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| {
                a["id"] == amendment_id
                    && a["endpoint_id"] == endpoint["id"]
                    && a["commitment_id"] == claim["id"]
            })
            .ok_or("Unknown explicit commitment amendment")?["replacement_text"]
    } else {
        &claim["statement"]
    };
    if target_node["statement"] != *expected {
        return Err(
            "Route target silently changes original commitment; use an explicit amendment".into(),
        );
    }
    let grounds = list(&route["grounding_evidence_ids"], 16)?;
    for id in grounds {
        if !active_sources.iter().any(|n| n["Id"] == id) {
            return Err("Route grounding must reference supplied evidence, separately from conjectural prerequisites".into());
        }
    }
    let links = route["chain"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 24)
        .ok_or("Route needs a declared backward connection")?;
    let mut graph: BTreeMap<String, BTreeSet<String>> = components
        .iter()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    let mut ids = BTreeSet::new();
    for link in links {
        let id = identifier(&link["id"])?;
        if !ids.insert(id) {
            return Err("Duplicate route link".into());
        }
        text(&link["mechanism"], 800)?;
        let to = field(link, "to_id");
        let from = list(&link["from_ids"], 12)?;
        if from.is_empty()
            || !graph.contains_key(to)
            || from.iter().any(|id| id == to || !graph.contains_key(id))
        {
            return Err("Route link references invalid component".into());
        }
        graph.get_mut(to).unwrap().extend(from);
    }
    // Every declared component must contribute to the target, rather than a
    // disconnected impressive side story. Root hypotheses remain conjectural.
    let mut ancestors = BTreeSet::new();
    let mut pending = vec![target.to_owned()];
    while let Some(id) = pending.pop() {
        if ancestors.insert(id.clone()) {
            pending.extend(graph[&id].iter().cloned());
        }
    }
    if ancestors.len() != components.len() {
        let disconnected: Vec<_> = components
            .iter()
            .filter(|id| !ancestors.contains(*id))
            .cloned()
            .collect();
        let refs = super::references_for_endpoints::References::new(snapshot)?;
        let visible =
            refs.project(&json!({"component_ids":disconnected,"target_component_id":target}));
        return Err(format!(
            "Disconnected route components {} do not lead to commitment target {}. Connect each through an explicit causal chain or remove unrelated components; citations belong in grounding evidence, not extra chain nodes.",
            visible["component_ids"], visible["target_component_id"]
        ));
    }
    let mut completed = BTreeSet::new();
    loop {
        let ready: Vec<_> = graph
            .iter()
            .filter(|(id, parents)| {
                !completed.contains(*id) && parents.iter().all(|p| completed.contains(p))
            })
            .map(|(id, _)| id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        completed.extend(ready);
    }
    if completed.len() != components.len() {
        return Err("Cyclic backward route".into());
    }
    search::validate_chain(
        &json!({"Id":route["id"],"component_ids":components,"chain":links}),
        snapshot,
    )?;
    let roots: BTreeSet<_> = graph
        .iter()
        .filter(|(_, parents)| parents.is_empty())
        .map(|(id, _)| id.as_str())
        .collect();
    let anchors = route["root_connections"]
        .as_array()
        .ok_or("Declare present support or unresolved frontier for every route root")?;
    let mut covered = BTreeSet::new();
    for anchor in anchors {
        let root = field(anchor, "component_id");
        if !roots.contains(root) || !covered.insert(root) {
            return Err("Root connection must name each conjectural root exactly once".into());
        }
        let refs = validate_root_connection(anchor)?;
        for id in refs {
            if !active_sources
                .iter()
                .any(|n| n["Id"] == id && !super::evidence::is_projection(n))
            {
                return Err("Root support must cite existing present evidence".into());
            }
        }
    }
    if covered != roots {
        return Err("Route omitted a root's present connection or explicit frontier gap".into());
    }
    Ok(())
}

/// Preserve original endpoints; append routes and explicit amendments atomically.
pub fn add_routes(
    before: &Value,
    after: &mut Value,
    old: &Value,
    generated: &Value,
) -> Result<Value, String> {
    super::backward::validate(old, generated)?;
    let mut search = old["endpoint_search"].clone();
    if !search.is_object() {
        return Err("Imagine endpoints before generating prerequisites".into());
    }
    let round = old["round"].as_u64().unwrap_or(0) + 1;
    let locals: BTreeSet<_> = generated["hypotheses"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            generated["research_evidence"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .filter_map(|h| h["id"].as_str())
        .collect();
    let mut reply = generated.clone();
    super::references_for_endpoints::References::new(before)?.resolve_generated(&mut reply);
    fn resolve_local(v: &mut Value, field_name: &str, locals: &BTreeSet<&str>, round: u64) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    resolve_local(v, k, locals, round)
                }
            }
            Value::Array(a) => {
                for v in a {
                    resolve_local(v, field_name, locals, round)
                }
            }
            Value::String(s)
                if matches!(
                    field_name,
                    "target_component_id"
                        | "component_id"
                        | "component_ids"
                        | "from_ids"
                        | "to_id"
                        | "grounding_evidence_ids"
                        | "evidence_ids"
                ) && locals.contains(s.as_str()) =>
            {
                *s = format!("r{round}-{s}")
            }
            _ => (),
        }
    }
    resolve_local(&mut reply, "", &locals, round);
    // Local link labels are not global identities. Canonical content identities
    // let independently proposed routes compose without collisions, while exact
    // shared edges retain one identity and can be deduplicated losslessly.
    for route in reply["routes"].as_array_mut().into_iter().flatten() {
        for link in route["chain"].as_array_mut().into_iter().flatten() {
            use sha2::{Digest, Sha256};
            let definition = json!({"from_ids":link["from_ids"],"to_id":link["to_id"],"by":link["by"],"mechanism":link["mechanism"]});
            link["id"] = json!(format!(
                "link-{:x}",
                Sha256::digest(definition.to_string().as_bytes())
            ));
        }
    }

    for amendment in reply["amendments"].as_array().into_iter().flatten() {
        let id = identifier(&amendment["id"])?;
        let e = endpoint(old, field(amendment, "endpoint_id"))?;
        let c = commitment(e, field(amendment, "commitment_id"))?;
        if search["amendments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == id)
        {
            return Err("Amendment identity is immutable".into());
        }
        if amendment["original_text"] != c["statement"] {
            return Err("Amendment must quote frozen original commitment".into());
        }
        text(&amendment["replacement_text"], 1000)?;
        text(&amendment["reason"], 800)?;
        let active_sources = super::evidence::active_sources(after);
        let refs = list(&amendment["evidence_ids"], 16)?;
        for id in refs {
            if !active_sources.iter().any(|n| n["Id"] == id) {
                return Err("Amendment citation is not supplied evidence".into());
            }
        }
        let mut a = amendment.clone();
        a["assessment"] = json!("pending");
        search["amendments"].as_array_mut().unwrap().push(a);
    }
    let routes = reply["routes"]
        .as_array()
        .filter(|v| v.len() <= 48)
        .ok_or("Backward search must return bounded routes")?;
    let mut checking = old.clone();
    checking["endpoint_search"] = search.clone();
    // Independent routes can fail for different reasons. Report them together
    // while the snapshot/search are still unchanged, rather than spending one
    // correction session to discover each route's first actionable defect.
    let mut route_errors = vec![];
    for (index, route) in routes.iter().enumerate() {
        let result = invalid_route_components(route, after, &format!("routes[{index}]")).and_then(
            |errors| {
                if errors.is_empty() {
                    validate_route(route, after, &checking)
                } else {
                    Err(route_component_error(errors))
                }
            },
        );
        if let Err(error) = result {
            route_errors.push(format!(
                "routes[{index}] (id={}): {error}",
                field(route, "id")
            ));
        }
    }
    if !route_errors.is_empty() {
        return Err(route_errors.join("; "));
    }
    // A locally valid route can conflict with the other commitments' routes.
    // Check proposed joint paths before recording or evaluating any new route.
    bundles::validate_proposed(after, &search, routes)?;
    let mut new_nodes:Vec<Value>=reply["amendments"].as_array().into_iter().flatten().map(|a|json!({"Id":format!("amendment-{}",field(a,"id")),"kind":"world","route_only":true,"archived":true,"statement":format!("Proposed amendment: {}",field(a,"replacement_text")),"amendment":a,"edges":"[]"})).collect();
    for route in routes {
        let id = identifier(&route["id"])?;
        if search["routes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == id)
        {
            return Err("Route identity is immutable; propose another route".into());
        }
        if let Some(alternative) = route["alternative_to"].as_str() {
            let previous = search["routes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == alternative)
                .ok_or("Unknown alternative route")?;
            if previous["endpoint_id"] != route["endpoint_id"]
                || previous["commitment_id"] != route["commitment_id"]
            {
                return Err("Alternate route must address the same original commitment".into());
            }
        }
        use sha2::{Digest, Sha256};
        let route_definition = json!({"component_ids":route["component_ids"],"chain":route["chain"],"root_connections":route["root_connections"],"grounding_evidence_ids":route["grounding_evidence_ids"]});
        let node_id = format!(
            "route-{:x}",
            Sha256::digest(route_definition.to_string().as_bytes())
        );
        let existing = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .chain(new_nodes.iter())
            .any(|n| n["Id"] == node_id);
        let mut r = route.clone();
        r["status"] = json!("pending");
        r["audit"] = Value::Null;
        r["world_node_id"] = json!(node_id);
        if !existing {
            new_nodes.push(json!({"Id":node_id,"kind":"world","route_only":true,"archived":true,"statement":format!("Joint proposed route to {}: {}",field(route,"commitment_id"),field(after["nodes"].as_array().unwrap().iter().find(|n|n["Id"]==route["target_component_id"]).unwrap(),"statement")),"component_ids":route["component_ids"],"counter_ids":[],"chain":route["chain"],"assumptions":[],"edges":"[]","endpoint_id":route["endpoint_id"],"grounding_evidence_ids":route["grounding_evidence_ids"],"root_connections":route["root_connections"]}));
        }
        search["routes"].as_array_mut().unwrap().push(r);
    }
    if after["nodes"].as_array().unwrap().len() + new_nodes.len() > super::MAX_NODES {
        return Err("Backward routes exceed node budget".into());
    }
    after["nodes"].as_array_mut().unwrap().extend(new_nodes);
    search["status"] = json!("searching");
    search["rounds"].as_array_mut().unwrap().push(json!({"round":round,"route_ids":routes.iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"note":reply["exploration_note"]}));
    Ok(search)
}

pub fn amendment_request(snapshot: &Value, program: &Value, task: &Value) -> Result<Value, String> {
    let amendment = program["endpoint_search"]["amendments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["id"] == task["amendment_id"])
        .ok_or("Unknown amendment task")?;
    let mut request = json!({"model":super::MODEL,"state":{"world":snapshot["world"],"baseline":program["baseline"],"amendment":amendment,"evidence":super::evidence::active_sources(snapshot)},"question":{"type":"choice","instructions":"Compare the immutable original commitment and explicit amendment in their exact scopes. Does the revision retain the distinguishing commitment, weaken it toward an easier/common outcome, or change what world was imagined? Do not reward plausibility or higher odds. Missing clarity stays unresolved. This judges semantic drift, not truth or probability.","criteria":{"preserved":"The distinguishing endpoint commitment and scope remain intact.","weakened":"The revision relaxes or removes a load-bearing distinguishing commitment.","changed":"The revision changes the outcome or scope rather than providing another route to the same endpoint.","unresolved":"Meaning preservation cannot be established from the supplied definitions."}},"validation":{"selection_policy":"provider_argmax"}});
    let question = request.as_object_mut().unwrap().remove("question").unwrap();
    request["questions"] = json!({"result":question});
    if request.to_string().len() > 128 * 1024 {
        return Err("Endpoint assessment exceeds request budget".into());
    }
    Ok(request)
}

/// Route audit work follows shared event admission; never spend an unrelated
/// all-pairs sweep before attempting a backward alternative.
fn route_tasks(world: &Value) -> Vec<Value> {
    let mut tasks: Vec<_> = search::world_tasks(world)
        .into_iter()
        .filter(|t| {
            matches!(
                field(t, "function"),
                "check_transition" | "conditional_on" | "conditional_off"
            )
        })
        .collect();
    tasks.insert(0,json!({"nodeId":world["Id"],"world_id":world["Id"],"function":"check_route_grounding","depth":0}));
    tasks
}

pub fn grounding_request(snapshot: &Value, program: &Value, task: &Value) -> Result<Value, String> {
    let world = snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["Id"] == task["nodeId"] && n["route_only"] == true)
        .ok_or("Unknown route grounding task")?;
    let mut request = json!({"model":super::MODEL,"state":{"world_question":snapshot["world"],"baseline":program["baseline"],"route":world,"components":snapshot["nodes"].as_array().into_iter().flatten().filter(|n|world["component_ids"].as_array().into_iter().flatten().any(|id|*id==n["Id"])).collect::<Vec<_>>(),"source_evidence":super::evidence::active_sources(snapshot)},"question":{"type":"choice","instructions":"Inspect each root_connection from sourced present conditions to a conjectural first prerequisite. Does its declared mechanism form an assessable bridge, contradict the present, or leave a substantive missing step? The existence of citations or graph connectivity is not evidence that the future occurs. Roots are conjectures, never facts. Explicit unresolved frontiers must remain gaps. Preserve scope and source qualifications.","criteria":{"connected":"Each root has a stated mechanism from supplied present conditions; this is assessable proposed connectivity, not demonstrated feasibility or truth.","gap":"At least one root has an explicit or substantive missing connection to present conditions.","conflict":"A declared bridge contradicts supplied present conditions within the same scope.","uncertain":"The supplied definitions or evidence do not establish the bridge's meaning or connectivity."}}});
    let question = request.as_object_mut().unwrap().remove("question").unwrap();
    request["questions"] = json!({"result":question});
    if request.to_string().len() > 128 * 1024 {
        return Err("Endpoint assessment exceeds request budget".into());
    }
    Ok(request)
}

pub fn plan_routes(snapshot: &Value, program: &mut Value) -> bool {
    let mut tasks = vec![];
    let mut scheduled = BTreeSet::new();
    let amendments = program["endpoint_search"]["amendments"].clone();
    for amendment in amendments.as_array().into_iter().flatten() {
        let id = format!("amendment-{}", field(amendment, "id"));
        let basis = candidate_basis(snapshot, program, &id);
        if program["route_basis"][&id] != basis {
            for collection in ["results", "evaluations"] {
                if let Some(map) = program[collection].as_object_mut() {
                    map.remove(&id);
                }
            }
        }
        program["route_basis"][&id] = basis;
        if program["results"][&id]["classify_amendment"].is_null() {
            tasks.push(json!({"nodeId":id,"function":"classify_amendment","amendment_id":amendment["id"],"depth":0}));
        }
    }
    let routes = program["endpoint_search"]["routes"].clone();
    for route in routes.as_array().into_iter().flatten() {
        let Some(world) = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["Id"] == route["world_node_id"])
        else {
            continue;
        };
        if !world["component_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|id| {
                super::branches::future_eligible(snapshot, program, id.as_str().unwrap_or(""))
            })
        {
            continue;
        }
        let basis = candidate_basis(snapshot, program, field(world, "Id"));
        if program["route_basis"][field(world, "Id")] != basis {
            for task in route_tasks(world) {
                for key in ["results", "evaluations"] {
                    if let Some(v) = program[key][field(&task, "nodeId")].as_object_mut() {
                        v.remove(field(&task, "function"));
                    }
                }
            }
        }
        program["route_basis"][field(world, "Id")] = basis;
        for task in route_tasks(world) {
            if program["results"][field(&task, "nodeId")][field(&task, "function")].is_null()
                && scheduled.insert(task.to_string())
            {
                tasks.push(task);
            }
        }
    }
    if tasks.is_empty() {
        return false;
    }
    program["stage"] = json!("routes");
    program["tasks"] = json!(tasks);
    program["cursor"] = json!(0);
    true
}

pub fn finish_routes(snapshot: &Value, program: &mut Value) {
    let current = program.clone();
    for amendment in program["endpoint_search"]["amendments"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        amendment["assessment"] = current["results"]
            [format!("amendment-{}", field(amendment, "id"))]["classify_amendment"]
            .as_str()
            .map_or(json!("pending"), |v| json!(v));
    }
    for route in program["endpoint_search"]["routes"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        let Some(world) = snapshot["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["Id"] == route["world_node_id"])
        else {
            continue;
        };
        let mut audit = search::audit_world(world, &current);
        let checks = audit["checks"].as_array_mut().unwrap();
        checks.retain(|c| {
            matches!(
                field(c, "kind"),
                "check_transition" | "conditional_on" | "conditional_off"
            )
        });
        let grounding = &current["results"][field(world, "Id")]["check_route_grounding"];
        checks.insert(0,json!({"id":format!("{}/check_route_grounding",field(world,"Id")),"kind":"check_route_grounding","subject_ids":world["component_ids"],"result":grounding,"probability":null,"branch_state":null}));
        let completed = checks
            .iter()
            .filter(|c| c["result"].is_string() || c["result"].is_number())
            .count();
        let conflict = checks.iter().any(|c| c["result"] == "conflict");
        let unresolved_roots: Vec<_> = route["root_connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|root| root["evidence_ids"].as_array().is_none_or(Vec::is_empty))
            .map(|root| root["component_id"].clone())
            .collect();
        let uncertain = !unresolved_roots.is_empty()
            || checks.iter().any(|c| {
                c["result"].is_null() || matches!(c["result"].as_str(), Some("gap" | "uncertain"))
            });
        let planned = checks.len();
        audit["unresolved_root_ids"] = json!(unresolved_roots);
        audit["status"] = json!(if conflict {
            "conflicts_found"
        } else if completed == 0 {
            "not_tested"
        } else if uncertain {
            "uncertain"
        } else {
            "no_conflict_found"
        });
        audit["planned_checks"] = json!(planned);
        audit["completed_checks"] = json!(completed);
        let admitted = world["component_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|id| {
                super::branches::future_eligible(snapshot, &current, id.as_str().unwrap_or(""))
            });
        route["status"] = json!(if !admitted || audit["status"] == "conflicts_found" {
            "blocked"
        } else if audit["status"] == "no_conflict_found" {
            "checked"
        } else {
            "unresolved"
        });
        route["audit"] = audit;
    }
    let routes = program["endpoint_search"]["routes"].clone();
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        let all = endpoint["commitments"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|c| {
                routes.as_array().into_iter().flatten().any(|r| {
                    r["endpoint_id"] == endpoint["id"]
                        && r["commitment_id"] == c["id"]
                        && r["status"] == "checked"
                        && r["amendment_id"].is_null()
                })
            });
        endpoint["status"] = json!(if all { "evaluated" } else { "unresolved" });
    }
    program["endpoint_search"]["status"] = json!("evaluated");
}

pub fn alternative_needed(program: &Value) -> bool {
    let routes = program["endpoint_search"]["routes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for commitment in endpoint["commitments"].as_array().into_iter().flatten() {
            let related: Vec<_> = routes
                .iter()
                .filter(|r| {
                    r["endpoint_id"] == endpoint["id"] && r["commitment_id"] == commitment["id"]
                })
                .collect();
            if related.is_empty()
                || related.iter().any(|r| {
                    matches!(field(r, "status"), "blocked" | "unresolved")
                        && !routes.iter().any(|a| a["alternative_to"] == r["id"])
                })
            {
                return true;
            }
        }
    }
    false
}

/// Preserve the current recorded path limitations independently of fresh
/// whole-world odds. Structural checks cannot clear a failed grounding check.
pub fn selected_route_audit(world: &Value, program: &Value) -> Option<Value> {
    if !enabled(program) || !world["endpoint_id"].is_string() || world["route_only"] == true {
        return None;
    }
    let routes: Vec<_> = world["selected_route_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|id| {
            let route = program["endpoint_search"]["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|route| route["id"] == *id && route["endpoint_id"] == world["endpoint_id"]);
            json!({"route_id":id,"commitment_id":route.map(|r|r["commitment_id"].clone()),
            "status":route.and_then(|r|r["status"].as_str()).unwrap_or("unresolved"),
            "root_connections":route.map(|r|r["root_connections"].clone()).unwrap_or(json!([])),
            "audit":route.map(|r|r["audit"].clone())})
        })
        .collect();
    let status = if routes.iter().any(|r| r["status"] == "blocked") {
        "blocked"
    } else if !routes.is_empty() && routes.iter().all(|r| r["status"] == "checked") {
        "checked"
    } else {
        "unresolved"
    };
    Some(json!({"status":status,"routes":routes}))
}

/// Omission is observable native state, not a fact the writer must invent.
/// Keep every unselected original and derive its limits from recorded routes.
pub fn preserve_omitted_originals(program: &Value, generated: &mut Value) {
    if !enabled(program) {
        return;
    }
    let mut omitted = vec![];
    for endpoint in program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if generated["worlds"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|world| world["endpoint_id"] == endpoint["id"])
        {
            continue;
        }
        if let Some(bundle) = program["composition_route_bundles"].as_array().into_iter().flatten()
            .find(|b| b["endpoint_id"] == endpoint["id"] && b["status"] != "compatible") {
            let reason = match field(bundle, "status") {
                "incomplete" => "Some defining commitments still lack eligible routes; the original remains unresolved.",
                "unexamined_limit" => "The bounded route search has not established a compatible set of paths. It has not shown that this future is impossible.",
                _ if field(bundle, "reason").contains("nondecreasing") => "The stored route deadlines do not establish a consistent order across all commitments. The path needs explicit timing refinement; this does not show that the future is impossible.",
                _ => "The stored paths do not yet form a consistent whole. Their graph needs explicit repair; this does not show that the future is impossible.",
            };
            omitted.push(json!({"endpoint_id":endpoint["id"],"reason":reason,"joint_route_receipt":bundle}));
            continue;
        }
        let mut missing = 0;
        let mut unchecked = 0;
        let mut checked = 0;
        let mut blocked = 0;
        for commitment in endpoint["commitments"].as_array().into_iter().flatten() {
            let routes: Vec<_> = program["endpoint_search"]["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|route| {
                    route["endpoint_id"] == endpoint["id"]
                        && route["commitment_id"] == commitment["id"]
                })
                .collect();
            if routes.is_empty() {
                missing += 1;
            } else if routes
                .iter()
                .any(|route| route["status"] == "checked" && route["amendment_id"].is_null())
            {
                checked += 1;
            } else if routes.iter().all(|route| route["status"] == "blocked") {
                blocked += 1;
            } else {
                unchecked += 1;
            }
        }
        omitted.push(json!({"endpoint_id":endpoint["id"],"reason":format!("This original world was not reconstructed. Of its defining commitments, {checked} have a checked unchanged route, {missing} have no recorded route, {blocked} have only blocked routes, and {unchecked} have unresolved checks or only amended routes. Its original text is preserved; no whole-world estimate was produced for it.")}));
    }
    generated["unreconstructed_endpoints"] = json!(omitted);
}

/// Composition binds every original endpoint; a weakened descendant is labelled
/// explicitly and cannot replace the original commitment by changing prose.
/// A selected route is the authority for its graph. Composition supplies prose,
/// not a second independently authored copy of already validated prerequisites.
pub fn assemble_composition(program: &Value, generated: &mut Value) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    let mut assembled = generated.clone();
    let worlds = assembled["worlds"]
        .as_array_mut()
        .ok_or("Missing reconstructed worlds")?;
    for world in worlds {
        let original = endpoint(program, field(world, "endpoint_id"))?;
        let compact = ["statement", "component_ids", "chain", "commitment_bindings"]
            .iter()
            .any(|key| world.get(*key).is_none());
        let selected = list(&world["selected_route_ids"], 48)?;
        let mut components = BTreeSet::new();
        let mut links = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        for id in selected {
            let route = program["endpoint_search"]["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["id"] == id && r["endpoint_id"] == original["id"])
                .ok_or_else(|| {
                    format!("Selected route {id} is unknown or belongs to another endpoint")
                })?;
            let claim = commitment(original, field(route, "commitment_id"))?;
            let binding = json!({"commitment_id":claim["id"],"component_id":route["target_component_id"],"amendment_id":route["amendment_id"]});
            if let Some(prior) = bindings.insert(field(claim, "id").to_owned(), binding.clone()) {
                if compact {
                    return Err(format!(
                        "Select exactly one route for commitment {}; alternative routes are not conjunctive prerequisites",
                        field(claim, "id")
                    ));
                }
                if prior != binding {
                    return Err(format!(
                        "Selected routes disagree on target or amendment for commitment {}; select compatible routes",
                        field(claim, "id")
                    ));
                }
            }
            for component in list(&route["component_ids"], MAX_WORLD_COMPONENTS)? {
                components.insert(component);
            }
            for link in route["chain"]
                .as_array()
                .ok_or("Stored route has no causal chain")?
            {
                let id = field(link, "id").to_owned();
                if let Some(prior) = links.insert(id.clone(), link.clone())
                    && prior != *link
                {
                    return Err(format!("Selected routes disagree on causal link {id}"));
                }
            }
        }
        if bindings.len() != original["commitments"].as_array().unwrap().len() {
            return Err("Selected routes must account for every original commitment".into());
        }
        let canonical = json!({"statement":original["original_statement"],"component_ids":components.into_iter().collect::<Vec<_>>(),"chain":links.into_values().collect::<Vec<_>>(),"commitment_bindings":bindings.into_values().collect::<Vec<_>>()});
        for key in ["statement", "component_ids", "chain", "commitment_bindings"] {
            if let Some(supplied) = world.get(key) {
                // Ordering is not meaning; duplicates and changed records still reject.
                let normalized = |value: &Value| {
                    if let Some(items) = value.as_array() {
                        let mut items = items.clone();
                        items.sort_by_key(Value::to_string);
                        Value::Array(items)
                    } else {
                        value.clone()
                    }
                };
                if normalized(supplied) != normalized(&canonical[key]) {
                    return Err(format!(
                        "World {}: supplied {key} conflicts with selected stored routes; omit engine-owned fields",
                        field(world, "endpoint_id")
                    ));
                }
            }
            world[key] = canonical[key].clone();
        }
    }
    // Keep the original validators, including amendment provenance and omission rules.
    validate_composition(program, &assembled)?;
    *generated = assembled;
    Ok(())
}

/// The larger bound applies only to the exact union checked against stored routes,
/// never merely because the submitted world claims an endpoint identity.
pub fn validate_world(world: &Value, snapshot: &Value, program: &Value) -> Result<(), String> {
    if !enabled(program) {
        return search::validate_world(world, snapshot);
    }
    let original = endpoint(program, field(world, "endpoint_id"))?;
    let scoped = json!({"world_search_contract":1,"endpoint_search":{
        "endpoints":[original],"routes":program["endpoint_search"]["routes"],
        "amendments":program["endpoint_search"]["amendments"]}});
    let mut checked = json!({"worlds":[world]});
    assemble_composition(&scoped, &mut checked)?;
    // At most eight commitments, each with one <=24-link route. Dedup may shrink it.
    search::validate_world_with_chain_limit(world, snapshot, 8 * 24)
}

pub fn validate_composition(program: &Value, generated: &Value) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    let worlds = generated["worlds"]
        .as_array()
        .ok_or("Missing reconstructed worlds")?;
    for e in program["endpoint_search"]["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !worlds.iter().any(|w| w["endpoint_id"] == e["id"]) {
            let report=generated["unreconstructed_endpoints"].as_array().into_iter().flatten().find(|r|r["endpoint_id"]==e["id"]).ok_or("Every omitted endpoint needs an explicit unreconstructed receipt; originals remain visible")?;
            text(&report["reason"], 800)?;
            let joint_unavailable = program["composition_route_bundles"].as_array().into_iter().flatten()
                .any(|b| b["endpoint_id"] == e["id"] && b["status"] != "compatible");
            if e["status"] == "evaluated" && !joint_unavailable {
                return Err("Cannot silently discard a fully connected original endpoint".into());
            }
        }
    }
    for w in worlds {
        let e = endpoint(program, field(w, "endpoint_id"))?;
        if w["statement"] != e["original_statement"] {
            return Err("Keep the frozen endpoint statement; explicit commitment amendments are recorded separately".into());
        }
        let bindings = w["commitment_bindings"]
            .as_array()
            .ok_or("Missing original commitment bindings")?;
        if bindings.len() != e["commitments"].as_array().unwrap().len() {
            return Err("Reconstructed world must account for every original commitment".into());
        }
        let selected = list(&w["selected_route_ids"], 48)?;
        let mut seen = BTreeSet::new();
        for binding in bindings {
            let claim = commitment(e, field(binding, "commitment_id"))?;
            if !seen.insert(field(binding, "commitment_id")) {
                return Err("Duplicate commitment binding".into());
            }
            if !w["component_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|id| *id == binding["component_id"])
            {
                return Err("Commitment binding is absent from defining components".into());
            }
            let routes: Vec<_> = program["endpoint_search"]["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| {
                    selected.iter().any(|id| r["id"] == *id)
                        && r["endpoint_id"] == e["id"]
                        && r["commitment_id"] == claim["id"]
                        && r["target_component_id"] == binding["component_id"]
                        && r["amendment_id"] == binding["amendment_id"]
                })
                .collect();
            if routes.is_empty() {
                return Err("Commitment needs an explicitly selected backward route, including unresolved routes honestly".into());
            }
            if let Some(id) = binding["amendment_id"].as_str() {
                let amendment = program["endpoint_search"]["amendments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|a| a["id"] == id)
                    .ok_or("Unknown amendment binding")?;
                if amendment["assessment"] != "preserved" {
                    return Err("Only an explicitly evaluated meaning-preserving amendment can reconstruct this endpoint; weakened/changed proposals remain separately recorded".into());
                }
            }
        }
        for id in selected {
            let route = program["endpoint_search"]["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["id"] == id && r["endpoint_id"] == e["id"])
                .ok_or("Selected route belongs to another endpoint")?;
            // Alternative routes are alternatives, not a conjunction of every
            // attempted prerequisite. The chosen route's parts must be retained.
            for link in route["chain"].as_array().into_iter().flatten() {
                if !w["chain"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate == link)
                {
                    return Err(
                        "Reconstructed world omitted or changed a selected causal link".into(),
                    );
                }
            }
            for component in route["component_ids"].as_array().into_iter().flatten() {
                if !w["component_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|id| id == component)
                {
                    return Err("Reconstructed world omitted selected route prerequisite".into());
                }
            }
        }
    }
    Ok(())
}

/// Basis for canonical-ID candidate reuse excludes operational provenance and unrelated
/// later candidates, but includes all declared meaning, ancestry and present evidence.
pub fn candidate_basis(snapshot: &Value, program: &Value, id: &str) -> Value {
    let nodes = snapshot["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut required = BTreeSet::new();
    let mut pending = vec![id.to_owned()];
    let mut definitions = vec![];
    while let Some(id) = pending.pop() {
        if !required.insert(id.clone()) {
            continue;
        }
        if let Some(node) = nodes.iter().find(|n| n["Id"] == id) {
            let mut definition = node.clone();
            for key in ["source_session_id", "probability", "Status"] {
                definition.as_object_mut().unwrap().remove(key);
            }
            for edge in super::parse(field(node, "edges"))
                .ok()
                .and_then(|e| e.as_array().cloned())
                .unwrap_or_default()
            {
                if edge["kind"] == "requires" {
                    pending.push(field(&edge, "to_id").to_owned());
                }
            }
            pending.extend(
                node["component_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
            if let Some(branch_id) = node["branch_id"].as_str() {
                definition["canonical_branch_state"] =
                    super::branches::state(snapshot, branch_id, Some(field(node, "Id")))
                        .unwrap_or(Value::Null);
            }
            definitions.push(definition);
        }
    }
    use sha2::{Digest, Sha256};
    let value = json!({"evaluator_contract":"endpoint-search-v1","model":super::MODEL,"world":snapshot["world"],"claim_role_contract":program["claim_role_contract"],"definitions":definitions,"baseline":program["baseline"],"evidence":nodes.iter().filter(|n|matches!(field(n,"kind"),"evidence"|"research_evidence")).collect::<Vec<_>>()});
    json!(format!(
        "{:x}",
        Sha256::digest(value.to_string().as_bytes())
    ))
}

pub fn invalidate_changed_candidates(snapshot: &Value, program: &mut Value, old: &Value) {
    if !enabled(program) {
        return;
    }
    let mut basis = json!({});
    for node in snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| matches!(field(n, "kind"), "scenario" | "revision"))
    {
        let id = field(node, "Id");
        let current = candidate_basis(snapshot, program, id);
        if old["candidate_basis"][id] != current {
            for collection in ["results", "evaluations"] {
                if let Some(map) = program[collection].as_object_mut() {
                    map.remove(id);
                }
            }
        }
        // Novelty is relative to a dynamic comparison sample. Its exact request
        // context can change even when this candidate and its evidence do not.
        let task = json!({"nodeId":id,"function":"evaluate_novelty"});
        if let Ok(request) = super::evaluation::request_task(snapshot, program, &task) {
            let sample = request["state"]["comparisons"].clone();
            if old["novelty_basis"][id] != sample {
                for collection in ["results", "evaluations"] {
                    if let Some(map) = program[collection][id].as_object_mut() {
                        map.remove("evaluate_novelty");
                    }
                }
            }
            program["novelty_basis"][id] = sample;
        }
        basis[id] = current;
    }
    program["candidate_basis"] = basis;
    invalidate_changed_prerequisite_inputs(snapshot, program);
}

/// Refresh cached descendants immediately after a provider batch. Preserve the
/// completed prefix; only pending candidates use the existing dependency order.
pub fn refresh_changed_candidate_inputs(
    snapshot: &Value,
    program: &mut Value,
) -> Result<(), String> {
    if !enabled(program) {
        return Ok(());
    }
    let removed = invalidate_changed_prerequisite_inputs(snapshot, program);
    if removed.is_empty() {
        return Ok(());
    }
    let cursor = program["cursor"]
        .as_u64()
        .ok_or("Missing candidate cursor")? as usize;
    let tasks = program["tasks"]
        .as_array()
        .ok_or("Missing candidate tasks")?;
    if cursor > tasks.len() {
        return Err("Invalid candidate cursor".into());
    }
    let plan = super::plan(snapshot["nodes"].as_array().ok_or("Missing nodes")?)?;
    let canonical = plan["tasks"]
        .as_array()
        .ok_or("Missing planned candidate tasks")?;
    let key = |t: &Value| {
        (
            field(t, "nodeId").to_owned(),
            field(t, "function").to_owned(),
        )
    };
    let mut wanted = removed;
    wanted.extend(tasks[cursor..].iter().map(key));
    let mut pending: Vec<Value> = canonical
        .iter()
        .filter(|t| wanted.contains(&key(t)))
        .cloned()
        .collect();
    let ordered: BTreeSet<_> = pending.iter().map(key).collect();
    pending.extend(
        tasks[cursor..]
            .iter()
            .filter(|t| !ordered.contains(&key(t)))
            .cloned(),
    );
    let mut combined = tasks[..cursor].to_vec();
    combined.extend(pending);
    program["tasks"] = json!(combined);
    Ok(())
}

// Clearing a stale parent changes its descendants' inputs too. Reach a fixed
// point so snapshot ordering cannot leave an indirectly stale child reusable.
fn invalidate_changed_prerequisite_inputs(
    snapshot: &Value,
    program: &mut Value,
) -> BTreeSet<(String, String)> {
    let mut removed = BTreeSet::new();
    let candidates: Vec<_> = snapshot["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| matches!(field(n, "kind"), "scenario" | "revision"))
        .map(|n| field(n, "Id").to_owned())
        .collect();
    loop {
        let mut stale = vec![];
        for id in &candidates {
            let Ok(Some(current)) =
                super::evaluation::candidate_prerequisite_fingerprint(snapshot, program, id)
            else {
                continue;
            };
            for function in program["results"][id]
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
            {
                // Admission requests intentionally omit prerequisite judgments.
                if matches!(
                    function.as_str(),
                    "classify_claim_role" | "classify_temporal"
                ) {
                    continue;
                }
                if program["evaluations"][id][function]["context"]["prerequisite_input_fingerprint"]
                    != current
                {
                    stale.push((id.clone(), function.clone()));
                }
            }
        }
        if stale.is_empty() {
            break;
        }
        for (id, function) in stale {
            removed.insert((id.clone(), function.clone()));
            for collection in ["results", "evaluations"] {
                if let Some(values) = program[collection][&id].as_object_mut() {
                    values.remove(&function);
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Value) {
        let snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2030-12-31"},"nodes":[{"Id":"source","kind":"research_evidence","statement":"Observed initial condition","edges":"[]"},{"Id":"root","kind":"scenario","statement":"A conjectural prerequisite","edges":"[]"},{"Id":"target","kind":"scenario","statement":"An unusual future outcome by 2030","edges":"[]"}]});
        let program = json!({"world_search_contract":1,"round":0,"baseline":{"as_of":"2026-10-01","observed":[{"claim":"Observed initial condition","evidence_ids":["source"]}],"assumptions":[],"unknowns":[]},"endpoint_search":{"status":"imagined","endpoints":[{"id":"e","original_statement":"Original world","commitments":[{"id":"c","statement":"An unusual future outcome by 2030"}]}],"routes":[],"amendments":[],"rounds":[]},"results":{"root":{"classify_claim_role":"event","classify_temporal":"future_change"},"target":{"classify_claim_role":"event","classify_temporal":"future_change"}},"evaluations":{}});
        let route = json!({"id":"r","endpoint_id":"e","commitment_id":"c","component_ids":["root","target"],"target_component_id":"target","chain":[{"id":"link","from_ids":["root"],"to_id":"target","mechanism":"The prerequisite enables the target","by":"2029-01-01"}],"grounding_evidence_ids":["source"],"root_connections":[{"component_id":"root","evidence_ids":["source"],"mechanism":"Observed capacity could be expanded"}],"alternative_to":null,"amendment_id":null});
        (snapshot, program, route)
    }
    #[test]
    fn captured_unresolved_music_roots_remain_gaps_even_when_provider_says_connected() {
        let roots: Vec<Value> =
            serde_json::from_str(include_str!("semantic_unresolved_roots_fixture.json")).unwrap();
        assert_eq!(roots.len(), 7);
        for root in &roots {
            validate_root_connection(root).unwrap();
            let mut sourced = root.clone();
            sourced["evidence_ids"] = json!(["source"]);
            assert!(validate_root_connection(&sourced).is_err());
        }
        let (snapshot, mut program, mut route) = fixture();
        route["root_connections"][0]["mechanism"] = json!("");
        route["root_connections"][0]["evidence_ids"] = json!([]);
        route["root_connections"][0]["unresolved_question"] =
            roots[0]["unresolved_question"].clone();
        let mut after = snapshot.clone();
        program["endpoint_search"]=add_routes(&snapshot,&mut after,&program,&json!({"routes":[route],"amendments":[],"exploration_note":"Recorded explicit unresolved frontier"})).unwrap();
        let node_id = program["endpoint_search"]["routes"][0]["world_node_id"]
            .as_str()
            .unwrap();
        let world = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["Id"] == node_id)
            .unwrap();
        for task in route_tasks(world) {
            let result = match field(&task, "function") {
                "check_route_grounding" => "connected",
                "check_transition" => "plausible",
                _ => "0.4",
            };
            program["results"][field(&task, "nodeId")][field(&task, "function")] = json!(result);
        }
        finish_routes(&after, &mut program);
        assert_eq!(
            program["endpoint_search"]["routes"][0]["status"],
            "unresolved"
        );
        assert_eq!(
            program["endpoint_search"]["routes"][0]["audit"]["status"],
            "uncertain"
        );
        assert!(alternative_needed(&program));
        let mut invalid_chain = program["endpoint_search"]["routes"][0].clone();
        invalid_chain["chain"][0]["mechanism"] = json!("");
        assert!(validate_route(&invalid_chain, &after, &program).is_err());
    }

    #[test]
    fn superseded_or_projection_sources_cannot_anchor_present_roots() {
        let (mut snapshot, program, mut route) = fixture();
        let mut corrected = snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["Id"] == "source")
            .unwrap()
            .clone();
        corrected["Id"] = json!("corrected-source");
        corrected["claim_type"] = json!("source_projection");
        corrected["source_correction"] = json!({"source_id":"source","corrected_source_id":"corrected-source","kind":"source_projection","verified":false});
        snapshot["nodes"].as_array_mut().unwrap().push(corrected);
        assert!(validate_route(&route, &snapshot, &program).is_err());
        route["grounding_evidence_ids"] = json!(["corrected-source"]);
        route["root_connections"][0]["evidence_ids"] = json!(["corrected-source"]);
        assert!(
            validate_route(&route, &snapshot, &program)
                .unwrap_err()
                .contains("present evidence")
        );
        route["root_connections"][0]["evidence_ids"] = json!([]);
        route["root_connections"][0]["unresolved_question"] =
            json!("The source only projects this future; what present evidence grounds the route?");
        validate_route(&route, &snapshot, &program).unwrap();
    }

    #[test]
    fn oversized_or_unselected_backward_batch_cannot_mutate_snapshot() {
        let (before, mut program, route) = fixture();
        program["endpoint_search"]["backward_batch_contract"] = json!(1);
        let mut after = before.clone();
        let oversized = json!({"routes":vec![route.clone();4]});
        assert!(
            add_routes(&before, &mut after, &program, &oversized)
                .unwrap_err()
                .contains("exceeds 3 routes")
        );
        assert_eq!(after, before);
        let mut outside = route;
        outside["commitment_id"] = json!("not-selected");
        assert!(
            add_routes(&before, &mut after, &program, &json!({"routes":[outside]}))
                .unwrap_err()
                .contains("outside")
        );
        assert_eq!(after, before);
    }
    #[test]
    fn evidence_components_report_all_route_locations_without_mutation() {
        let (before, program, mut route) = fixture();
        route["component_ids"] = json!(["source", "target"]);
        let mut second = route.clone();
        second["id"] = json!("second");
        second["component_ids"] = json!(["missing-component", "target"]);
        let mut after = before.clone();
        let error = add_routes(
            &before,
            &mut after,
            &program,
            &json!({"routes":[route,second]}),
        )
        .unwrap_err();
        for detail in [
            "routes[0].component_ids[0]=ref_0001",
            "research_evidence",
            "routes[1].component_ids[0]=missing-component",
            "kind missing",
            "root_connections.evidence_ids",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        assert_eq!(before, after);
    }

    #[test]
    fn independent_route_defects_are_reported_together_before_mutation() {
        let (mut before, program, route) = fixture();
        before["nodes"].as_array_mut().unwrap().push(
            json!({"Id":"orphan","kind":"scenario","statement":"Unrelated event","edges":"[]"}),
        );
        let mut disconnected = route.clone();
        disconnected["id"] = json!("disconnected");
        disconnected["component_ids"] = json!(["root", "target", "orphan"]);
        let mut bad_link = route;
        bad_link["id"] = json!("bad-link");
        bad_link["chain"][0]["to_id"] = json!("root");
        let mut after = before.clone();
        let error = add_routes(
            &before,
            &mut after,
            &program,
            &json!({"routes":[disconnected,bad_link]}),
        )
        .unwrap_err();
        for detail in [
            "routes[0] (id=disconnected)",
            "Disconnected route components",
            "ref_0004",
            "ref_0003",
            "routes[1] (id=bad-link)",
            "Route link references invalid component",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        assert_eq!(after, before);
    }

    #[test]
    fn producer_fixture_preserves_amendment_nodes_and_route_receipts() {
        let (snapshot, mut old, route) = fixture();
        old.as_object_mut().unwrap().remove("endpoint_search");
        let endpoint = json!({"id":"e","title":"An imagined endpoint","original_statement":"Original world","original_narrative":"Three interacting changes in an imagined ordinary day.","commitments":[{"id":"c","statement":"An unusual future outcome by 2030"},{"id":"c2","statement":"A second scoped change occurs by 2030"},{"id":"c3","statement":"A third scoped change occurs by 2030"}],"signals":["An observable change"],"falsifiers":["The mechanism fails"]});
        let mut second = endpoint.clone();
        second["id"] = json!("other");
        second["title"] = json!("Another imagined endpoint");
        let proposal = json!({"endpoints":[endpoint,second]});
        let mut program = imagine(&snapshot, &old, &proposal).unwrap();
        let imagined = program.clone();
        let amendment = json!({"id":"a","endpoint_id":"e","commitment_id":"c","original_text":"An unusual future outcome by 2030","replacement_text":"An unusual future outcome in a specified scope by 2030","reason":"Clarify the scope without removing the commitment","evidence_ids":["source"]});
        let generated = json!({"routes":[route],"amendments":[amendment],"hypotheses":[],"research_evidence":[],"exploration_note":"An alternative route remains worth testing"});
        let mut after = snapshot.clone();
        program["endpoint_search"] =
            add_routes(&snapshot, &mut after, &program, &generated).unwrap();
        let node = after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["amendment"].is_object())
            .unwrap();
        assert!(node["statement"].is_string());
        plan_routes(&after, &mut program);
        for task in program["tasks"].as_array().unwrap().clone() {
            let result = match field(&task, "function") {
                "classify_amendment" => "preserved",
                "check_route_grounding" => "connected",
                "check_transition" => "plausible",
                _ => "0.4",
            };
            program["results"][field(&task, "nodeId")][field(&task, "function")] = json!(result);
        }
        finish_routes(&after, &mut program);
        assert_eq!(
            program["endpoint_search"]["routes"][0]["audit"]["completed_checks"],
            4
        );
        if let Ok(path) = std::env::var("FORESIGHT_PRODUCER_FIXTURE") {
            std::fs::write(path,serde_json::to_string_pretty(&json!({"imagined_program":imagined,"program":program,"snapshot":after,"trace":[]})).unwrap()).unwrap();
        }
    }

    #[test]
    fn realistic_three_endpoint_route_fanout_leaves_room_for_alternatives_and_worlds() {
        let (mut snapshot, mut program, prototype) = fixture();
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":"otherroot","kind":"scenario","statement":"A different prerequisite route","edges":"[]"}));
        let mut endpoints = vec![];
        let mut routes = vec![];
        for e in 0..3 {
            let endpoint_id = format!("endpoint{e}");
            let mut claims = vec![];
            for c in 0..3 {
                let target = format!("target{e}_{c}");
                let statement = format!("Distinct commitment {c} of endpoint {e} occurs by 2030");
                snapshot["nodes"].as_array_mut().unwrap().push(
                    json!({"Id":target,"kind":"scenario","statement":statement,"edges":"[]"}),
                );
                claims.push(json!({"id":format!("claim{c}"),"statement":statement}));
                for a in 0..2 {
                    let mut route = prototype.clone();
                    let root = if a == 0 { "root" } else { "otherroot" };
                    route["id"] = json!(format!("route{e}_{c}_{a}"));
                    route["endpoint_id"] = json!(endpoint_id);
                    route["commitment_id"] = json!(format!("claim{c}"));
                    route["target_component_id"] = json!(target);
                    route["component_ids"] = json!([root, target]);
                    route["chain"][0]["from_ids"] = json!([root]);
                    route["chain"][0]["to_id"] = json!(target);
                    route["root_connections"][0]["component_id"] = json!(root);
                    if a == 1 {
                        route["alternative_to"] = json!(format!("route{e}_{c}_0"));
                    }
                    routes.push(route);
                }
            }
            endpoints.push(json!({"id":endpoint_id,"original_statement":format!("World {e}"),"commitments":claims}));
        }
        program["endpoint_search"]["endpoints"] = json!(endpoints);
        let mut after = snapshot.clone();
        program["endpoint_search"]=add_routes(&snapshot,&mut after,&program,&json!({"routes":routes,"hypotheses":[],"research_evidence":[],"amendments":[],"exploration_note":"Alternative mechanisms for three rich endpoints"})).unwrap();
        for node in after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "scenario")
        {
            program["results"][field(node, "Id")] =
                json!({"classify_claim_role":"event","classify_temporal":"future_change"});
        }
        assert!(plan_routes(&after, &mut program));
        let tasks = program["tasks"].as_array().unwrap().len();
        assert_eq!(tasks, 72);
        let mut batches = 0;
        let mut cursor = 0;
        while cursor < tasks {
            program["cursor"] = json!(cursor);
            let batch = super::super::batch::prepare(&after, &program, tasks - cursor).unwrap();
            assert!(!batch.tasks.is_empty());
            cursor += batch.tasks.len();
            batches += 1;
        }
        // Three reasoning phases (imagine and two backward turns), directed
        // route calls, and the untouched final-world/writer reserve. Candidate
        // assessment has its own share; no theoretical5000-call claim here.
        let nominal = 3 * super::super::REASONING_ADMISSION_RESERVE + 2 * batches as u64;
        assert!(
            nominal + 2 * super::super::REASONING_ADMISSION_RESERVE
                < super::super::MAX_APP_TRANSITIONS
        );
        eprintln!(
            "world-first fanout:18routes,{tasks}Jevquestions,{batches}HTTPbatches,{nominal}transitions including3reasoningreserves;finalreserve={} remains",
            2 * super::super::REASONING_ADMISSION_RESERVE
        );
    }

    #[test]
    fn backward_routes_require_grounded_or_explicitly_unresolved_roots_and_valid_calendar() {
        let (s, p, r) = fixture();
        validate_route(&r, &s, &p).unwrap();
        for date in ["2029-02-30", "2029-13-01", "2025-01-01", "2031-01-01"] {
            let mut invalid = r.clone();
            invalid["chain"][0]["by"] = json!(date);
            assert!(validate_route(&invalid, &s, &p).is_err(), "{date}");
        }
        let mut missing = r.clone();
        missing["root_connections"] = json!([]);
        assert!(validate_route(&missing, &s, &p).is_err());
        missing = r.clone();
        missing["root_connections"][0]["evidence_ids"] = json!([]);
        assert!(validate_route(&missing, &s, &p).is_err());
        missing["root_connections"][0]["unresolved_question"] =
            json!("What connects the present to this premise?");
        validate_route(&missing, &s, &p).unwrap();
        let mut disconnected = s.clone();
        disconnected["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Id":"unrelated","kind":"scenario"}));
        missing["component_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!("unrelated"));
        assert!(validate_route(&missing, &disconnected, &p).is_err());
        let mut changed = s.clone();
        changed["nodes"][2]["statement"] = json!("A safer ordinary outcome");
        assert!(
            validate_route(&r, &changed, &p)
                .unwrap_err()
                .contains("silently")
        );
    }
    #[test]
    fn exact_shared_routes_evaluate_once_and_changed_evidence_invalidates() {
        let (s, mut p, r) = fixture();
        let mut alternate = r.clone();
        alternate["id"] = json!("another");
        alternate["alternative_to"] = json!("r");
        let generated = json!({"routes":[r,alternate],"hypotheses":[],"research_evidence":[],"amendments":[],"exploration_note":"Try shared pieces"});
        let mut after = s.clone();
        p["endpoint_search"] = add_routes(&s, &mut after, &p, &generated).unwrap();
        assert_eq!(
            after["nodes"].as_array().unwrap().len(),
            4,
            "exact route graph shares one immutable node"
        );
        assert!(plan_routes(&after, &mut p));
        assert_eq!(
            p["tasks"].as_array().unwrap().len(),
            4,
            "grounding plus one directed edge and two conditionals; no pair sweep"
        );
        for t in p["tasks"].as_array().unwrap().clone() {
            let req = super::super::evaluation::request_task(&after, &p, &t).unwrap();
            assert!(req["questions"]["result"].is_object());
            let value = match field(&t, "function") {
                "check_route_grounding" => json!("connected"),
                "check_transition" => json!("plausible"),
                _ => json!("0.4"),
            };
            p["results"][field(&t, "nodeId")][field(&t, "function")] = value;
        }
        assert!(!plan_routes(&after, &mut p));
        finish_routes(&after, &mut p);
        assert_eq!(p["endpoint_search"]["routes"][0]["status"], "checked");
        assert_eq!(
            p["endpoint_search"]["routes"][0]["audit"]["completed_checks"],
            4
        );
        after["nodes"][0]["statement"] =
            json!("Contradictory new evidence with the same source ID");
        assert!(plan_routes(&after, &mut p));
        assert_eq!(p["tasks"].as_array().unwrap().len(), 4);
    }
    #[test]
    fn shared_candidate_reuse_invalidates_meaning_baseline_and_ancestry() {
        let (mut s, mut p, _) = fixture();
        p["claim_role_contract"] = json!(1);
        let prior = p.clone();
        invalidate_changed_candidates(&s, &mut p, &prior);
        p["results"]["target"]["estimate_likelihood"] = json!("0.41");
        let original = p.clone();
        invalidate_changed_candidates(&s, &mut p, &original);
        assert_eq!(p["results"]["target"]["estimate_likelihood"], "0.41");
        s["nodes"][2]["statement"] = json!("Changed target meaning");
        invalidate_changed_candidates(&s, &mut p, &original);
        assert!(p["results"]["target"].is_null());
        let (s, _, _) = fixture();
        p = original.clone();
        p["baseline"]["unknowns"] = json!(["New uncertainty"]);
        invalidate_changed_candidates(&s, &mut p, &original);
        assert!(p["results"]["target"].is_null());
        let mut linked = s;
        linked["nodes"][2]["edges"] = json!("[{\"kind\":\"requires\",\"to_id\":\"root\"}]");
        let before = candidate_basis(&linked, &original, "target");
        linked["nodes"][1]["statement"] = json!("Changed prerequisite");
        assert_ne!(candidate_basis(&linked, &original, "target"), before);
    }
    #[test]
    fn novelty_and_amendment_reuse_follow_their_actual_context() {
        let (mut snapshot, mut program, route) = fixture();
        program["claim_role_contract"] = json!(1);
        for node in snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "scenario")
        {
            program["candidate_basis"][field(node, "Id")] =
                candidate_basis(&snapshot, &program, field(node, "Id"));
        }
        let prior = program.clone();
        invalidate_changed_candidates(&snapshot, &mut program, &prior);
        program["results"]["target"]["evaluate_novelty"] = json!("3");
        program["results"]["target"]["estimate_likelihood"] = json!("0.4");
        let prior = program.clone();
        snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":"new","kind":"scenario","statement":"Another unusual future outcome by2030","edges":"[]"}));
        invalidate_changed_candidates(&snapshot, &mut program, &prior);
        assert!(program["results"]["target"]["evaluate_novelty"].is_null());
        assert_eq!(program["results"]["target"]["estimate_likelihood"], "0.4");
        let amendment = json!({"id":"fix","endpoint_id":"e","commitment_id":"c","original_text":"An unusual future outcome by 2030","replacement_text":"A scoped unusual future outcome by 2030","reason":"Explicit scope","evidence_ids":["source"]});
        let before = snapshot.clone();
        program["endpoint_search"] = add_routes(
            &before,
            &mut snapshot,
            &program,
            &json!({"routes":[route],"amendments":[amendment]}),
        )
        .unwrap();
        plan_routes(&snapshot, &mut program);
        program["results"]["amendment-fix"]["classify_amendment"] = json!("preserved");
        plan_routes(&snapshot, &mut program);
        assert_eq!(
            program["results"]["amendment-fix"]["classify_amendment"],
            "preserved"
        );
        program["baseline"]["unknowns"] = json!(["Different present context"]);
        plan_routes(&snapshot, &mut program);
        assert!(program["results"]["amendment-fix"]["classify_amendment"].is_null());
    }

    #[test]
    fn blocked_route_requires_alternative_before_any_endpoint_downgrade() {
        let (_, mut p, mut r) = fixture();
        r["status"] = json!("unresolved");
        p["endpoint_search"]["routes"] = json!([r]);
        assert!(alternative_needed(&p));
        let mut alternative = p["endpoint_search"]["routes"][0].clone();
        alternative["id"] = json!("alternate");
        alternative["alternative_to"] = json!("r");
        alternative["status"] = json!("checked");
        p["endpoint_search"]["routes"]
            .as_array_mut()
            .unwrap()
            .push(alternative);
        assert!(!alternative_needed(&p));
        assert_eq!(
            p["endpoint_search"]["endpoints"][0]["original_statement"],
            "Original world"
        );
    }
    #[test]
    fn native_omission_receipts_do_not_allow_discarding_connected_originals() {
        let (_, mut program, _) = fixture();
        let mut generated = json!({"worlds":[],"unreconstructed_endpoints":[{"endpoint_id":"e","reason":"Invented explanation"}]});
        preserve_omitted_originals(&program, &mut generated);
        assert!(
            generated["unreconstructed_endpoints"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("1 have no recorded route")
        );
        validate_composition(&program, &generated).unwrap();
        program["endpoint_search"]["endpoints"][0]["status"] = json!("evaluated");
        preserve_omitted_originals(&program, &mut generated);
        assert!(
            validate_composition(&program, &generated)
                .unwrap_err()
                .contains("fully connected")
        );
        program["world_search_contract"] = Value::Null;
        let unchanged = generated.clone();
        preserve_omitted_originals(&program, &mut generated);
        assert_eq!(generated, unchanged);
    }

    #[test]
    fn stored_route_union_can_exceed_legacy_chain_bound_without_trusting_markers() {
        let snapshot = json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2030-12-31"}});
        let components: Vec<_> = (0..26).map(|i|json!(format!("n{i}"))).collect();
        let links: Vec<_> = (0..25).map(|i|json!({"id":format!("l{i}"),"from_ids":[format!("n{i}")],"to_id":format!("n{}",i+1),"mechanism":"Earlier capacity enables the next step","by":"2029-01-01"})).collect();
        let mut routes = vec![];
        let mut commitments = vec![];
        for (i, (start, end)) in [(0,10),(10,20),(20,25)].into_iter().enumerate() {
            commitments.push(json!({"id":format!("c{i}"),"statement":"Original defining commitment"}));
            routes.push(json!({"id":format!("r{i}"),"endpoint_id":"e","commitment_id":format!("c{i}"),"target_component_id":format!("n{end}"),"component_ids":components[start..=end],"chain":links[start..end],"amendment_id":null}));
            search::validate_chain(routes.last().unwrap(), &snapshot).unwrap();
        }
        let program = json!({"world_search_contract":1,"endpoint_search":{"endpoints":[{"id":"e","original_statement":"Frozen bold world","commitments":commitments}],"routes":routes,"amendments":[]}});
        let mut generated = json!({"worlds":[{"endpoint_id":"e","selected_route_ids":["r0","r1","r2"],"assumptions":[],"facets":[
            {"id":"a","title":"First changes","description":"First interacting changes","component_ids":components[0..10]},
            {"id":"b","title":"Next changes","description":"Next interacting changes","component_ids":components[10..20]},
            {"id":"c","title":"Later changes","description":"Later interacting changes","component_ids":components[20..26]}
        ]}]});
        assemble_composition(&program, &mut generated).unwrap();
        let world = &generated["worlds"][0];
        validate_world(world, &snapshot, &program).unwrap();
        assert!(search::validate_world(world, &snapshot).unwrap_err().contains("24"));
        assert!(validate_world(world, &snapshot, &json!({})).is_err());
        let mut forged = world.clone();
        forged["chain"][0]["mechanism"] = json!("Forged mechanism");
        assert!(validate_world(&forged, &snapshot, &program).unwrap_err().contains("conflicts"));
        for violation in ["cycle", "date"] {
            let mut invalid_program = program.clone();
            let link = &mut invalid_program["endpoint_search"]["routes"][0]["chain"][0];
            if violation == "cycle" { link["from_ids"] = json!(["n25"]); } else { link["by"] = json!("2031-01-01"); }
            let mut invalid_world = world.clone();
            for key in ["statement","component_ids","chain","commitment_bindings"] { invalid_world.as_object_mut().unwrap().remove(key); }
            let mut draft = json!({"worlds":[invalid_world]});
            assemble_composition(&invalid_program, &mut draft).unwrap();
            assert!(validate_world(&draft["worlds"][0], &snapshot, &invalid_program).is_err());
        }
    }

    #[test]
    fn selected_routes_assemble_exact_shared_graph_and_reject_conflicts() {
        let (_, mut p, r) = fixture();
        let mut second = r.clone();
        second["id"] = json!("r2");
        second["commitment_id"] = json!("c2");
        p["endpoint_search"]["endpoints"][0]["commitments"].as_array_mut().unwrap()
            .push(json!({"id":"c2","statement":"Second commitment"}));
        p["endpoint_search"]["routes"] = json!([r, second]);
        let compact = json!({"worlds":[{"endpoint_id":"e","selected_route_ids":["r","r2"]}]});
        let mut full = compact.clone();
        assemble_composition(&p, &mut full).unwrap();
        assert_eq!(full["worlds"][0]["component_ids"], json!(["root", "target"]));
        assert_eq!(full["worlds"][0]["chain"].as_array().unwrap().len(), 1);
        assert_eq!(full["worlds"][0]["commitment_bindings"].as_array().unwrap().len(), 2);
        assert_eq!(full["worlds"][0]["statement"], "Original world");
        for key in ["statement", "component_ids", "chain", "commitment_bindings"] {
            let mut changed = full.clone();
            changed["worlds"][0][key] = json!("changed");
            let before = changed.clone();
            assert!(assemble_composition(&p, &mut changed).unwrap_err().contains(key));
            assert_eq!(changed, before);
        }
        let mut changed = compact.clone();
        changed["worlds"][0]["selected_route_ids"] = json!(["missing"]);
        assert!(assemble_composition(&p, &mut changed).unwrap_err().contains("missing"));
        let mut alternate = p["endpoint_search"]["routes"][0].clone();
        alternate["id"] = json!("alt");
        p["endpoint_search"]["routes"].as_array_mut().unwrap().push(alternate);
        let mut changed = compact.clone();
        changed["worlds"][0]["selected_route_ids"] = json!(["r", "r2", "alt"]);
        assert!(assemble_composition(&p, &mut changed).unwrap_err().contains("exactly one"));
        p["world_search_contract"] = Value::Null;
        let before = changed.clone();
        assemble_composition(&p, &mut changed).unwrap();
        assert_eq!(changed, before);
    }

    #[test]
    #[ignore = "requires explicit local captured checkpoint"]
    fn captured_food_selected_routes_roundtrip() {
        let path = std::env::var("FORESIGHT_COMPOSITION_FIXTURE").unwrap();
        let captured: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let p = &captured["program"];
        let worlds: Vec<_> = captured["snapshot"]["nodes"].as_array().unwrap().iter()
            .filter(|n| n["kind"] == "world" && n["archived"] != true).cloned().collect();
        assert_eq!(worlds.len(), 2);
        let mut compact = json!({"worlds":worlds});
        preserve_omitted_originals(p, &mut compact);
        let full = compact.clone();
        for world in compact["worlds"].as_array_mut().unwrap() {
            for key in ["statement", "component_ids", "chain", "commitment_bindings"] {
                world.as_object_mut().unwrap().remove(key);
            }
        }
        assemble_composition(p, &mut compact).unwrap();
        // Legacy payload must agree exactly (array order may differ).
        let mut checked_full = full;
        assemble_composition(p, &mut checked_full).unwrap();
        assert_eq!(compact, checked_full);
        assert_eq!(compact["worlds"][0]["component_ids"].as_array().unwrap().len(), 14);
        assert_eq!(compact["worlds"][1]["component_ids"].as_array().unwrap().len(), 10);
    }

    #[test]
    fn composition_rejects_silent_commitment_weakening_and_accounts_for_omissions() {
        let (_, mut p, r) = fixture();
        p["endpoint_search"]["routes"] = json!([r]);
        let mut generated = json!({"worlds":[{"endpoint_id":"e","statement":"Original world","selected_route_ids":["r"],"component_ids":["root","target"],"commitment_bindings":[{"commitment_id":"c","component_id":"target","amendment_id":null}],"chain":p["endpoint_search"]["routes"][0]["chain"]}]});
        validate_composition(&p, &generated).unwrap();
        let valid = generated.clone();
        generated["worlds"][0]["chain"] = json!([]);
        assert!(validate_composition(&p, &generated).is_err());
        generated = valid;
        generated["worlds"][0]["commitment_bindings"] = json!([]);
        assert!(validate_composition(&p, &generated).is_err());
        generated["worlds"] = json!([]);
        assert!(validate_composition(&p, &generated).is_err());
        generated["unreconstructed_endpoints"] = json!([{"endpoint_id":"e","reason":"Both proposed routes remain blocked; retain original as unresolved"}]);
        validate_composition(&p, &generated).unwrap();
    }
}

#[cfg(test)]
mod candidate_reuse_tests {
    include!("semantic_candidate_reuse_tests.rs");
}
