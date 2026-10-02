//! Native fresh-world spawn parameters must satisfy the complete dependency contract.
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use temper_spec::automaton::{LintSeverity, lint_automata_bundle, parse_automaton};

fn sources() -> BTreeMap<String, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    [
        ("World", "paw-foresight/specs/world.ioa.toml"),
        ("SemanticRun", "paw-foresight/specs/semantic_run.ioa.toml"),
        ("Session", "paw-agent/specs/session.ioa.toml"),
    ]
    .into_iter()
    .map(|(name, path)| {
        (
            name.into(),
            std::fs::read_to_string(root.join("os-apps").join(path)).unwrap(),
        )
    })
    .collect()
}

#[test]
fn fresh_world_supplies_empty_resume_to_native_spawn() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(42);
    let source = sources();
    let automata = source
        .iter()
        .map(|(name, text)| (name.clone(), parse_automaton(text).unwrap()))
        .collect();
    let errors: Vec<_> = lint_automata_bundle(&automata)
        .into_iter()
        .filter(|f| f.severity == LintSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let world = parse_automaton(&source["World"]).unwrap();
    assert!(
        !world
            .actions
            .iter()
            .find(|a| a.name == "Configure")
            .unwrap()
            .params
            .iter()
            .any(|p| p.name() == "resume_run_id")
    );
    let resume = world
        .state
        .iter()
        .find(|v| v.name == "resume_run_id")
        .unwrap();
    assert_eq!(resume.initial, "");
    let table = temper_jit::table::TransitionTable::from_ioa_source(&source["World"]);
    let mut state: temper_server::entity_actor::EntityState = serde_json::from_value(json!({
        "entity_type":"World", "entity_id":"new-world", "status":"Active", "item_count":0,
        "booleans":{"semantic_exploration_requested":true}, "fields":{"resume_run_id":resume.initial}
    }))
    .unwrap();
    let result = temper_server::entity_actor::process_action(
        &mut state,
        &table,
        "StartSemanticExploration",
        &json!({}),
    );
    assert!(result.success);
    assert_eq!(result.spawn_requests.len(), 1);
    assert_eq!(
        result.spawn_requests[0].initial_action.as_deref(),
        Some("Start")
    );
    assert_eq!(
        result.spawn_requests[0]
            .copied_field_values
            .get("resume_run_id"),
        Some(&json!(""))
    );
    let old_source = source["World"].replace("copy_fields=\"resume_run_id\"", "copy_fields=\"\"");
    let mut broken = automata;
    broken.insert("World".into(), parse_automaton(&old_source).unwrap());
    assert!(
        lint_automata_bundle(&broken)
            .iter()
            .any(|f| f.severity == LintSeverity::Error && f.message.contains("resume_run_id"))
    );
}
