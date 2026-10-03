// Synthetic graph and evaluator boundary for lifecycle tests; no provider claims.
    fn ready_pair() -> (Value, Value) {
        // Synthetic evaluator boundary: completed checks may honestly say gap.
        let mut snapshot = json!({"world":{"description":"What changes by 2030?","last_ingest_date":"2026-10-01","target_date":"2030-12-31"},"nodes":[{"Id":"root","kind":"scenario","statement":"A future prerequisite","edges":"[]"}]});
        let mut p = json!({"audit_policy_version":2,"world_search_contract":1,"stage":"routes","transition_count":160,"baseline":{"as_of":"2026-10-01","observed":[],"unknowns":[],"assumptions":[]},"endpoint_search":{"status":"searching","endpoints":[],"routes":[],"rounds":[{"round":1,"route_ids":[],"note":"Synthetic route round"}],"amendments":[]},"results":{},"evaluations":{},"candidate_basis":{},"route_basis":{},"endpoint_novelty":{},"continue_exploring":true});
        for id in ["a", "b"] {
            let target = format!("target-{id}");
            let world = format!("route-world-{id}");
            let chain = json!([{"id":format!("link-{id}"),"from_ids":["root"],"to_id":target,"by":"2029-01-01","mechanism":"The prerequisite enables this distinct consequence"}]);
            snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":target,"kind":"scenario","statement":format!("Consequence {id} by 2030"),"edges":"[]"}));
            snapshot["nodes"].as_array_mut().unwrap().push(json!({"Id":world,"kind":"world","statement":format!("Synthetic route for consequence {id}"),"route_only":true,"component_ids":["root",target],"chain":chain,"edges":"[]"}));
            p["endpoint_search"]["endpoints"].as_array_mut().unwrap().push(json!({"id":id,"original_statement":format!("World {id}"),"title":format!("World {id}"),"original_narrative":"Synthetic interacting consequences for lifecycle verification.","signals":["A synthetic signal"],"falsifiers":["A synthetic falsifier"],"commitments":[{"id":"c","statement":format!("Consequence {id} by 2030")},{"id":"d","statement":"Second defining consequence"},{"id":"e","statement":"Third defining consequence"}]}));
            p["endpoint_search"]["routes"].as_array_mut().unwrap().push(json!({"id":format!("route-{id}"),"endpoint_id":id,"commitment_id":"c","world_node_id":world,"target_component_id":target,"component_ids":["root",target],"chain":chain,"grounding_evidence_ids":[],"root_connections":[{"component_id":"root","evidence_ids":[],"unresolved_question":"Unknown bridge"}],"status":"unresolved"}));
        }
        // Shared routes are explicit for each defining commitment.
        let originals = p["endpoint_search"]["routes"].as_array().unwrap().clone();
        for route in originals {
            for commitment in ["d", "e"] {
                let mut copy = route.clone();
                copy["id"] = json!(format!("{}-{commitment}", core::field(&route,"id")));
                copy["commitment_id"] = json!(commitment);
                p["endpoint_search"]["routes"].as_array_mut().unwrap().push(copy);
            }
        }
        // Keep a third original; finishing this unit must not erase it.
        p["endpoint_search"]["endpoints"].as_array_mut().unwrap().push(json!({"id":"later","original_statement":"Another original","title":"Another original","original_narrative":"Synthetic later arrangement.","signals":["A signal"],"falsifiers":["A falsifier"],"commitments":[{"id":"c","statement":"A later consequence"},{"id":"d","statement":"A second later consequence"},{"id":"e","statement":"A third later consequence"}]}));
        let plan = core::plan(snapshot["nodes"].as_array().unwrap()).unwrap();
        for task in plan["tasks"].as_array().unwrap() {
            let value = match core::field(task, "function") {
                "classify_claim_role" => json!("event"),
                "classify_temporal" => json!("future_change"),
                "estimate_likelihood" => json!("true"),
                _ => json!("gap"),
            };
            p["results"][core::field(task, "nodeId")][core::field(task, "function")] = value;
        }
        for node in snapshot["nodes"].as_array().unwrap() {
            let id = core::field(node, "Id");
            if node["kind"] == "world" {
                p["route_basis"][id] = core::endpoints::candidate_basis(&snapshot, &p, id);
                for task in core::search::mandatory_audit_tasks(node, &p) {
                    p["results"][core::field(&task, "nodeId")][core::field(&task, "function")] =
                        json!("gap");
                }
            } else {
                p["candidate_basis"][id] = core::endpoints::candidate_basis(&snapshot, &p, id);
            }
        }
        let current = json!({"endpoints":p["endpoint_search"]["endpoints"],"baseline":p["baseline"],"world":snapshot["world"],"source_evidence":[]});
        for id in ["a", "b", "later"] {
            let task = json!({"nodeId":id,"function":"check_proposal_change","endpoint_id":id,"proposal_attempt":1,"other_endpoint_id":"","relation_index":0,"depth":0});
            let receipt=json!({"task":task,"result":"changed_arrangement","passed":true,"evaluation":{"type":"choice","selected":"changed_arrangement","answer":{"type":"choice","choice":"changed_arrangement","probabilities":{"changed_arrangement":1.0}}},"request":core::proposals::pool::request(&current,&task).unwrap()});
            p["endpoint_novelty"][id] = json!({"status":"passed","initial_check":receipt,"final_check":receipt});
        }
        (snapshot, p)
    }


