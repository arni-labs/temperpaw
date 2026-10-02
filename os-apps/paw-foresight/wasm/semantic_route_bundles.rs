// Structural route compatibility, not a likelihood or feasibility judgment.
use super::{MAX_WORLD_COMPONENTS, field, search};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PREFIXES: usize = 4096;

fn graph(endpoint: &Value, routes: &[&Value]) -> Result<Value, String> {
    let mut components = BTreeSet::new();
    let mut links = BTreeMap::new();
    for route in routes {
        for id in route["component_ids"]
            .as_array()
            .ok_or("Missing route components")?
        {
            components.insert(id.as_str().ok_or("Invalid route component")?);
        }
        for link in route["chain"].as_array().ok_or("Missing route chain")? {
            let id = field(link, "id");
            if let Some(prior) = links.insert(id, link)
                && prior != link
            {
                return Err(format!("Conflicting stored causal link {id}"));
            }
        }
    }
    if components.len() > MAX_WORLD_COMPONENTS {
        return Err(format!(
            "Joint route union exceeds {MAX_WORLD_COMPONENTS} components"
        ));
    }
    Ok(
        json!({"Id":endpoint["id"],"component_ids":components.into_iter().collect::<Vec<_>>(),"chain":links.into_values().collect::<Vec<_>>()}),
    )
}

fn find_bundle(
    snapshot: &Value,
    search: &Value,
    endpoint: &Value,
    proposed: &[Value],
    assessed: bool,
    limit: usize,
) -> Value {
    let mut groups = vec![];
    let mut missing = vec![];
    for commitment in endpoint["commitments"].as_array().into_iter().flatten() {
        let forced: Vec<_> = proposed
            .iter()
            .filter(|r| {
                r["endpoint_id"] == endpoint["id"] && r["commitment_id"] == commitment["id"]
            })
            .collect();
        if forced.len() > 1 {
            return json!({"endpoint_id":endpoint["id"],"status":"invalid_proposal","reason":"Select at most one proposed route per commitment"});
        }
        let mut options: Vec<_> = if forced.is_empty() {
            search["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| {
                    r["endpoint_id"] == endpoint["id"] && r["commitment_id"] == commitment["id"]
                })
                .collect()
        } else {
            forced
        };
        if assessed {
            options.retain(|r| {
                r["amendment_id"].is_null()
                    || search["amendments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|a| a["id"] == r["amendment_id"] && a["assessment"] == "preserved")
            });
        }
        options.sort_by_key(|r| {
            (
                match field(r, "status") {
                    "checked" => 0,
                    "unresolved" => 1,
                    _ => 2,
                },
                field(r, "id"),
            )
        });
        if options.is_empty() {
            missing.push(commitment["id"].clone());
        } else {
            groups.push(options);
        }
    }
    if !missing.is_empty() {
        return json!({"endpoint_id":endpoint["id"],"status":"incomplete","missing_commitment_ids":missing});
    }
    groups.sort_by_key(Vec::len);
    struct Search<'a> {
        snapshot: &'a Value,
        endpoint: &'a Value,
        prefixes: usize,
        limit: usize,
        exhausted: bool,
        first_error: Option<String>,
    }
    fn visit<'a>(
        state: &mut Search<'_>,
        groups: &[Vec<&'a Value>],
        chosen: &mut Vec<&'a Value>,
    ) -> Option<Vec<String>> {
        if chosen.len() == groups.len() {
            let result = graph(state.endpoint, chosen)
                .and_then(|g| search::validate_chain_with_limit(&g, state.snapshot, 8 * 24));
            match result {
                Ok(()) => return Some(chosen.iter().map(|r| field(r, "id").to_owned()).collect()),
                Err(error) => {
                    if state.first_error.is_none() {
                        state.first_error = Some(error);
                    }
                    return None;
                }
            }
        }
        for route in &groups[chosen.len()] {
            if state.prefixes >= state.limit {
                state.exhausted = true;
                return None;
            }
            state.prefixes += 1;
            chosen.push(route);
            // Date inference is not monotone: a later route may add another incoming
            // mechanism. Validate chronology only at a complete bundle, never a prefix.
            let found = visit(state, groups, chosen);
            chosen.pop();
            if found.is_some() {
                return found;
            }
        }
        None
    }
    let mut state = Search {
        snapshot,
        endpoint,
        prefixes: 0,
        limit,
        exhausted: false,
        first_error: None,
    };
    let checked_groups: Vec<Vec<&Value>> = groups
        .iter()
        .map(|g| {
            g.iter()
                .copied()
                .filter(|r| r["status"] == "checked")
                .collect()
        })
        .collect();
    let checked = if checked_groups.iter().all(|g| !g.is_empty()) {
        visit(&mut state, &checked_groups, &mut vec![])
    } else {
        None
    };
    let selected = checked.or_else(|| {
        if state.exhausted {
            None
        } else {
            visit(&mut state, &groups, &mut vec![])
        }
    });
    json!({"endpoint_id":endpoint["id"],"status":if selected.is_some(){"compatible"}else if state.exhausted{"unexamined_limit"}else{"incompatible"},"selected_route_ids":selected,"examined_prefixes":state.prefixes,"prefix_limit":limit,"reason":state.first_error,"semantics":"Structural compatibility only; by dates are deadlines, and reversed milestones require an explicit timing refinement rather than proving the future impossible. No likelihood judgment."})
}

pub fn validate_proposed(
    snapshot: &Value,
    search: &Value,
    proposed: &[Value],
) -> Result<(), String> {
    let mut errors = vec![];
    for endpoint in search["endpoints"].as_array().into_iter().flatten() {
        if !proposed.iter().any(|r| r["endpoint_id"] == endpoint["id"]) {
            continue;
        }
        let mut seen = BTreeSet::new();
        let alternatives = proposed
            .iter()
            .filter(|r| r["endpoint_id"] == endpoint["id"])
            .any(|r| !seen.insert(r["commitment_id"].to_string()));
        let mut combined = search.clone();
        let proposals: Vec<Vec<Value>> = if alternatives {
            // Historical responses may return alternatives together. Each must
            // have a compatible bundle, never conjoin alternatives of one claim.
            combined["routes"]
                .as_array_mut()
                .unwrap()
                .extend_from_slice(proposed);
            proposed
                .iter()
                .filter(|r| r["endpoint_id"] == endpoint["id"])
                .map(|r| vec![r.clone()])
                .collect()
        } else {
            vec![proposed.to_vec()]
        };
        let mut remaining = MAX_PREFIXES;
        for forced in proposals {
            let bundle = find_bundle(snapshot, &combined, endpoint, &forced, false, remaining);
            remaining = remaining
                .saturating_sub(bundle["examined_prefixes"].as_u64().unwrap_or(0) as usize);
            if !matches!(field(&bundle, "status"), "compatible" | "incomplete") {
                errors.push(format!("Joint backward routes for {} are not ready for composition: {bundle}. Explicitly refine the proposed causal milestones or mechanism against unchanged routes. A late by deadline can precede an earlier deadline in reality, but this supplied path has not specified that timing. Do not silently rewrite stored routes; propose an alternative when an existing route must change.",field(endpoint,"id")));
                break;
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn composition_bundles(snapshot: &Value, program: &Value) -> Value {
    let mut eligible = program["endpoint_search"].clone();
    if let Some(routes) = eligible["routes"].as_array_mut() {
        routes.retain(|route| {
            route["component_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|id| {
                    super::super::branches::future_eligible(
                        snapshot,
                        program,
                        id.as_str().unwrap_or(""),
                    )
                })
        });
    }
    json!(
        eligible["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| find_bundle(snapshot, &eligible, e, &[], true, MAX_PREFIXES))
            .collect::<Vec<_>>()
    )
}

/// Native bundles guide selection; any complete structurally valid alternative is allowed.
pub fn validate_selection(
    world: &Value,
    snapshot: &Value,
    program: &Value,
    bundles: &Value,
) -> Result<(), String> {
    let endpoint_id = field(world, "endpoint_id");
    let validate = || -> Result<(), String> {
        let search = &program["endpoint_search"];
        let endpoint = search["endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| field(e, "id") == endpoint_id)
            .ok_or("Unknown original endpoint")?;
        let ids = world["selected_route_ids"]
            .as_array()
            .ok_or("Missing selected_route_ids")?;
        let mut seen = BTreeSet::new();
        let mut commitments = BTreeSet::new();
        let mut selected = vec![];
        for value in ids {
            let id = value.as_str().ok_or("Invalid selected route ID")?;
            if !seen.insert(id) {
                return Err(format!("Duplicate selected route {id}"));
            }
            let route = search["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| field(r, "id") == id)
                .ok_or_else(|| format!("Unknown selected route {id}"))?;
            if field(route, "endpoint_id") != endpoint_id {
                return Err(format!("Route {id} belongs to a different original"));
            }
            if !commitments.insert(field(route, "commitment_id")) {
                return Err(
                    "Select exactly one route per original commitment; do not conjoin alternatives"
                        .into(),
                );
            }
            selected.push(route);
        }
        let required: BTreeSet<_> = endpoint["commitments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| field(c, "id"))
            .collect();
        if commitments != required {
            return Err("Selected routes must cover every commitment exactly once".into());
        }
        search::validate_chain_with_limit(&graph(endpoint, &selected)?, snapshot, 8 * 24)
    };
    validate().map_err(|error| {
        let guidance = bundles.as_array().into_iter().flatten()
            .find(|b| field(b, "endpoint_id") == endpoint_id);
        format!("Original {endpoint_id}: {error}. Native route guidance: {}. Select a complete compatible set without changing stored milestones; guidance is structural, not a likelihood ranking.", guidance.unwrap_or(&Value::Null))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Vec<Value>) {
        let snapshot =
            json!({"world":{"last_ingest_date":"2026-10-01","target_date":"2030-12-31"}});
        let routes = vec![
            json!({"id":"a","endpoint_id":"e","commitment_id":"c1","status":"checked","amendment_id":null,"component_ids":["root","middle"],"chain":[{"id":"la","from_ids":["root"],"to_id":"middle","by":"2030-12-31","mechanism":"First capacity enables the middle event"}]}),
            json!({"id":"b","endpoint_id":"e","commitment_id":"c2","status":"checked","amendment_id":null,"component_ids":["middle","end"],"chain":[{"id":"lb","from_ids":["middle"],"to_id":"end","by":"2029-12-31","mechanism":"The middle event enables the final event"}]}),
        ];
        let search = json!({"endpoints":[{"id":"e","commitments":[{"id":"c1"},{"id":"c2"}]}],"routes":[],"amendments":[]});
        (snapshot, search, routes)
    }
    #[test]
    fn joint_milestones_reject_before_storage_and_alternatives_require_compatible_timing() {
        let (snapshot, mut search, routes) = fixture();
        for route in &routes {
            search::validate_chain(route, &snapshot).unwrap();
        }
        let original = routes.clone();
        let error = validate_proposed(&snapshot, &search, &routes).unwrap_err();
        assert!(error.contains("2030-12-31") && error.contains("2029-12-31"));
        assert!(error.contains("timing"));
        assert_eq!(routes, original);
        search["routes"] = json!([routes[0]]);
        let mut alternative = routes[1].clone();
        alternative["id"] = json!("b-alternative");
        alternative["alternative_to"] = json!("b");
        assert!(validate_proposed(&snapshot, &search, &[alternative.clone()]).is_err());
        alternative["chain"][0]["by"] = json!("2030-12-31");
        validate_proposed(&snapshot, &search, &[alternative.clone()]).unwrap();
        search["routes"].as_array_mut().unwrap().push(alternative);
        let program = json!({"endpoint_search":search});
        let bundles = json!([find_bundle(
            &snapshot,
            &program["endpoint_search"],
            &program["endpoint_search"]["endpoints"][0],
            &[],
            true,
            MAX_PREFIXES
        )]);
        assert_eq!(bundles[0]["status"], "compatible");
        let mut alternate_program = program.clone();
        let mut other = alternate_program["endpoint_search"]["routes"][1].clone();
        other["id"] = json!("b-other");
        alternate_program["endpoint_search"]["routes"]
            .as_array_mut()
            .unwrap()
            .push(other);
        let chosen = json!({"endpoint_id":"e","selected_route_ids":["a","b-other"]});
        validate_selection(&chosen, &snapshot, &alternate_program, &bundles).unwrap();
        assert_ne!(
            chosen["selected_route_ids"],
            bundles[0]["selected_route_ids"]
        );
        alternate_program["endpoint_search"]["routes"][2]["chain"][0]["by"] = json!("2029-12-31");
        let error =
            validate_selection(&chosen, &snapshot, &alternate_program, &bundles).unwrap_err();
        assert!(error.contains("2029-12-31") && error.contains("Native route guidance"));

        validate_selection(
            &json!({"endpoint_id":"e","selected_route_ids":["a","b-alternative"]}),
            &snapshot,
            &program,
            &bundles,
        )
        .unwrap();
        assert!(
            validate_selection(
                &json!({"endpoint_id":"e","selected_route_ids":["a"]}),
                &snapshot,
                &program,
                &bundles
            )
            .unwrap_err()
            .contains("cover every commitment")
        );
    }
    #[test]
    fn bounded_search_does_not_claim_impossibility_or_prune_partial_chronology() {
        let (snapshot, mut search, mut routes) = fixture();
        let mut third = routes[0].clone();
        third["id"] = json!("c");
        third["commitment_id"] = json!("c3");
        third["chain"][0]["id"] = json!("lc");
        third["chain"][0]["by"] = json!("2028-12-31");
        routes.push(third);
        search["endpoints"][0]["commitments"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c3"}));
        search["routes"] = json!(routes);
        let limited = find_bundle(&snapshot, &search, &search["endpoints"][0], &[], true, 0);
        assert_eq!(limited["status"], "unexamined_limit");
        let full = find_bundle(
            &snapshot,
            &search,
            &search["endpoints"][0],
            &[],
            true,
            MAX_PREFIXES,
        );
        assert_eq!(full["status"], "compatible", "{full}");
    }
    #[test]
    #[ignore = "requires explicit local captured pass5 games state"]
    fn captured_games_joint_routes_need_explicit_milestone_refinement() {
        let path = std::env::var("FORESIGHT_GAMES_BUNDLE_FIXTURE").unwrap();
        let captured: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let program: Value =
            serde_json::from_str(captured["fields"]["program_json"].as_str().unwrap()).unwrap();
        let snapshot: Value =
            serde_json::from_str(captured["fields"]["snapshot_json"].as_str().unwrap()).unwrap();
        let mut search = program["endpoint_search"].clone();
        let routes: Vec<_> = search["routes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["endpoint_id"] == "prompt-studios")
            .cloned()
            .collect();
        search["routes"] = json!([]);
        assert!(
            validate_proposed(&snapshot, &search, &routes)
                .unwrap_err()
                .contains("nondecreasing")
        );
        let mut repaired = routes.clone();
        for route in &mut repaired {
            for link in route["chain"].as_array_mut().unwrap() {
                link["by"] = json!("2030-12-31");
            }
        }
        // Explicit test proposal change, not a silent mutation of accepted routes.
        validate_proposed(&snapshot, &search, &repaired).unwrap();
        assert_ne!(repaired, routes);
        search["routes"] = json!(repaired);
        let bundle = find_bundle(
            &snapshot,
            &search,
            &search["endpoints"][0],
            &[],
            true,
            MAX_PREFIXES,
        );
        assert_eq!(bundle["status"], "compatible");
        let mut initial = program.clone();
        initial["endpoint_search"]["endpoints"] = json!([search["endpoints"][0]]);
        initial["endpoint_search"]["routes"] = json!([]);
        initial["endpoint_search"]["rounds"] = json!([]);
        let reply = |routes: &Value| json!({"routes":routes,"hypotheses":[],"research_evidence":[],"amendments":[],"exploration_note":"Captured initial routes, explicitly refined timing in the positive test only"});
        let mut unchanged = snapshot.clone();
        assert!(
            super::super::add_routes(&snapshot, &mut unchanged, &initial, &reply(&json!(routes)))
                .unwrap_err()
                .contains("nondecreasing")
        );
        assert_eq!(unchanged, snapshot);
        let mut after = snapshot.clone();
        let accepted =
            super::super::add_routes(&snapshot, &mut after, &initial, &reply(&search["routes"]))
                .unwrap();
        initial["endpoint_search"] = accepted;
        let mut assembled = json!({"worlds":[{"endpoint_id":"prompt-studios","selected_route_ids":bundle["selected_route_ids"]}]});
        super::super::assemble_composition(&initial, &mut assembled).unwrap();
        assert_eq!(
            assembled["worlds"][0]["statement"],
            search["endpoints"][0]["original_statement"]
        );
        assert_eq!(
            assembled["worlds"][0]["commitment_bindings"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        search::validate_chain_with_limit(&assembled["worlds"][0], &after, 192).unwrap();
    }
}
