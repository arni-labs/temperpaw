use serde_json::{Value, json};

#[test]
fn complete_overlapping_paths_fit_and_expand_to_exact_audits() {
    let mut snapshot = json!({"world":{"description":"Question","target_date":"2030-12-31","last_ingest_date":"2026-01-01"},"nodes":[{"Id":"ev","kind":"evidence","statement":"Present observation"}]});
    let mut worlds = vec![];
    for prefix in ["a", "b"] {
        let ids: Vec<_> = (0..16).map(|i| json!(format!("{prefix}{i}"))).collect();
        for id in &ids {
            snapshot["nodes"].as_array_mut().unwrap().push(
                json!({"Id":id,"kind":"scenario","statement":format!("Event {}",id),"edges":"[]"}),
            );
        }
        let links:Vec<_>=(1..16).map(|i|json!({"id":format!("{prefix}link{i}"),"from_ids":[ids[i-1]],"to_id":ids[i],"mechanism":format!("Distinct link {prefix}{i}: {}", "mechanism detail ".repeat(43)),"by":"2030-01-01"})).collect();
        let facets:Vec<_>=(0..3).map(|i|json!({"id":format!("facet{i}"),"title":"Aspect","description":"Change","component_ids":ids.iter().enumerate().filter(|(n,_)|n%3==i).map(|(_,v)|v.clone()).collect::<Vec<_>>()})).collect();
        worlds.push(json!({"Id":prefix,"kind":"world","statement":"World","shared_question":"Question","trajectory_answer":"Whole trajectory","comparison_contract":"v1","comparison_frame":{"description":"Same situation","original_question":"Question","horizon":"2030-12-31","evidence_ids":["ev"]},"trajectory_binding":{"organizing_component_ids":[ids[0]],"organizing_branch_ids":[],"downstream_component_ids":ids[4..].to_vec(),"counterpart_world_id":if prefix=="a"{"b"}else{"a"}},"component_ids":ids,"counter_ids":[],"chain":links,"facets":facets,"assumptions":[]}));
    }
    for w in &worlds {
        super::validate_world(w, &snapshot).unwrap();
        snapshot["nodes"].as_array_mut().unwrap().push(w.clone());
    }
    let mut program = json!({"active_world_ids":["a","b"],"comparison_bindings":{}});
    for w in &worlds {
        let audit = super::super::comparison::audit(&snapshot, w, &worlds);
        assert_eq!(audit["status"], "supported");
        println!(
            "{} chain={} audit={}",
            w["Id"],
            w["chain"].to_string().len(),
            audit.to_string().len()
        );
        program["comparison_bindings"][w["Id"].as_str().unwrap()] = audit;
    }
    let task = super::world_set_tasks(&[json!("a"), json!("b")])[0].clone();
    let original = program.clone();
    let request = super::request(&snapshot, &program, &task).unwrap();
    assert!(request.to_string().len() < 128 * 1024);
    assert_eq!(program, original);
    for (key, world) in [
        ("binding_audit", &worlds[0]),
        ("counterpart_binding_audit", &worlds[1]),
    ] {
        let mut restored = request["state"]["focal_comparison"][key].clone();
        for path in restored["paths"].as_array_mut().unwrap() {
            for link in path["links"].as_array_mut().unwrap() {
                let reference = &link["exact_world_chain_link"];
                assert_eq!(reference["world_id"], world["Id"]);
                *link = world["chain"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|candidate| candidate["id"] == reference["link_id"])
                    .unwrap()
                    .clone();
            }
        }
        assert_eq!(
            restored,
            program["comparison_bindings"][world["Id"].as_str().unwrap()]
        );
    }
}

#[test]
fn projection_preserves_changed_missing_ambiguous_and_branch_links() {
    let link = json!({"id":"l","from_ids":["a"],"to_id":"b","mechanism":"exact mechanism","by":"2030-01-01"});
    let world = json!({"Id":"w","chain":[link]});
    let mut changed = link.clone();
    changed["mechanism"] = json!("different mechanism");
    let absent = json!({"id":"missing","mechanism":"not in chain"});
    let audit = json!({"status":"supported","issues":[],"paths":[{"kind":"declared_chain","links":[link,changed,absent]},{"kind":"hypothetical_branch","conditions":[{"exact":"retained"}],"links":[link]}]});
    let projected = super::request_binding_audit(audit.clone(), &world);
    assert_eq!(
        projected["paths"][0]["links"][0],
        json!({"exact_world_chain_link":{"world_id":"w","link_id":"l"}})
    );
    assert_eq!(projected["paths"][0]["links"][1], changed);
    assert_eq!(projected["paths"][0]["links"][2], absent);
    assert_eq!(projected["paths"][1], audit["paths"][1]);
    let duplicate = json!({"Id":"w","chain":[link,link]});
    assert_eq!(
        super::request_binding_audit(audit.clone(), &duplicate),
        audit
    );
    assert_eq!(
        super::request_binding_audit(audit.clone(), &Value::Null),
        audit
    );
}
