use serde_json::{Value, json};
use std::sync::Arc;
use temper_jit::TransitionTable;
use temper_runtime::scheduler::SimActorHandler;
use temper_server::entity_actor::sim_handler::EntityActorHandler;
fn source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"
    ))
    .unwrap()
}
fn check_all_actions(src: &str) -> Result<(), String> {
    let spec: toml::Value = toml::from_str(src).unwrap();
    for action in spec["action"].as_array().unwrap() {
        let name = action["name"].as_str().unwrap();
        let initial = action["from"][0].as_str().unwrap();
        let fixture = src.replacen(
            "initial = \"Created\"",
            &format!("initial = \"{initial}\""),
            1,
        );
        let mut actor = EntityActorHandler::new(
            "SemanticRun",
            "fixture",
            Arc::new(TransitionTable::from_ioa_source(&fixture)),
        );
        actor.init().unwrap();
        let mut params = json!({});
        for param in action["params"].as_array().unwrap() {
            params[param.as_str().unwrap()] = json!("");
        }
        if matches!(name, "Prepared" | "Reason" | "ResumePrepared") {
            params["reasoning_phase_polls"] = json!(0);
        }
        let out = actor
            .handle_message(name, &params.to_string())
            .map_err(|e| format!("{name}: {e}"))?;
        if out["counters"]["transition_count"] != 1 {
            return Err(format!("{name} did not count"));
        }
    }
    Ok(())
}
#[test]
fn every_native_action_counts_and_missing_increment_is_detected() {
    let src = source();
    check_all_actions(&src).unwrap();
    let mutant = src.replacen(
        "{type=\"increment\",var=\"transition_count\"}",
        "{type=\"increment\",var=\"check_count\"}",
        1,
    );
    assert!(check_all_actions(&mutant).is_err());
}
#[test]
fn correction_and_retry_do_not_reset_phase_poll_budget() {
    let src = source().replacen("initial = \"Created\"", "initial = \"Reasoning\"", 1);
    let mut actor = EntityActorHandler::new(
        "SemanticRun",
        "fixture",
        Arc::new(TransitionTable::from_ioa_source(&src)),
    );
    actor.init().unwrap();
    let mut count = 0;
    let mut apply = |action: &str, params: Value| {
        count += 1;
        let out = actor.handle_message(action, &params.to_string()).unwrap();
        assert_eq!(out["counters"]["transition_count"], count);
        out
    };
    apply("CheckReasoning", json!({}));
    apply(
        "ReasoningRetry",
        json!({"last_retry_error":"transient","last_retry_session_id":"old"}),
    );
    apply("SpawnReasoning", json!({}));
    assert_eq!(
        apply("CheckReasoning", json!({}))["counters"]["reasoning_phase_polls"],
        2
    );
    apply("ReasoningComplete", json!({"reasoning_result":"{}"}));
    apply("CompositionRejected", json!({"program_json":"{}"}));
    apply(
        "LaunchReasoning",
        json!({"system_prompt":"x","user_message":"x","tools_enabled":"[]","tool_choice":"none","max_turns":"1"}),
    );
    apply("SpawnReasoning", json!({}));
    assert_eq!(
        apply("CheckReasoning", json!({}))["counters"]["reasoning_phase_polls"],
        3
    );
    apply("ReasoningComplete", json!({"reasoning_result":"{}"}));
    apply(
        "Expanded",
        json!({"snapshot_json":"{}","program_json":"{}","started_at_ms":"original"}),
    );
    assert_eq!(
        apply(
            "Reason",
            json!({"phase":"synthesize","program_json":"{}","trace_json":"[]","reasoning_phase_polls":0})
        )["counters"]["reasoning_phase_polls"],
        0
    );
}
#[test]
fn metadata_exposes_exact_phase_reset_parameters() {
    use std::collections::BTreeSet;
    let source = source();
    let ioa: toml::Value = toml::from_str(&source).unwrap();
    let xml = include_str!("../../../os-apps/paw-foresight/specs/model.csdl.xml");
    let document = temper_spec::csdl::parse_csdl(xml).unwrap();
    for name in ["Prepared", "Reason", "ResumePrepared"] {
        let expected: BTreeSet<_> = ioa["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .unwrap()["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        let actions: Vec<_> = document
            .schemas
            .iter()
            .flat_map(|s| &s.actions)
            .filter(|a| {
                a.name == name && a.binding_type() == Some("TemperPaw.Foresight.SemanticRun")
            })
            .collect();
        assert_eq!(actions.len(), 1);
        let actual: BTreeSet<_> = actions[0]
            .parameters
            .iter()
            .skip(1)
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(actual, expected);
        let mut mutant = actual;
        mutant.remove("reasoning_phase_polls");
        assert_ne!(mutant, expected);
    }
}

#[test]
fn ten_poll_phase_counts_worst_case_success_and_timeout_without_reset() {
    let source = source().replacen("initial = \"Created\"", "initial = \"Choosing\"", 1);
    let spec: toml::Value = toml::from_str(&source).unwrap();
    for name in ["SpawnReasoning", "ReasoningPending"] {
        let action = spec["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .unwrap();
        let schedule = action["effect"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e.get("action").and_then(toml::Value::as_str) == Some("CheckReasoning"))
            .unwrap();
        assert_eq!(
            schedule
                .get("delay_seconds")
                .and_then(toml::Value::as_integer),
            Some(30)
        );
    }
    let retry = spec["action"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"].as_str() == Some("ReasoningRetry"))
        .unwrap();
    assert!(
        retry["effect"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.get("delay_seconds").and_then(toml::Value::as_integer) == Some(15))
    );
    for timeout in [false, true] {
        let mut actor = EntityActorHandler::new(
            "SemanticRun",
            "fixture",
            Arc::new(TransitionTable::from_ioa_source(&source)),
        );
        actor.init().unwrap();
        let mut apply = |action: &str, params: Value| {
            actor.handle_message(action, &params.to_string()).unwrap()
        };
        apply(
            "Reason",
            json!({"phase":"compose","program_json":"{}","trace_json":"[]","reasoning_phase_polls":0}),
        );
        let launch = json!({"system_prompt":"fixture","user_message":"fixture","tools_enabled":"","tool_choice":"none","max_turns":"1"});
        apply("LaunchReasoning", launch.clone());
        apply("SpawnReasoning", json!({}));
        for _ in 0..3 {
            apply("CheckReasoning", json!({}));
            apply(
                "ReasoningRetry",
                json!({"last_retry_error":"HTTP 503","last_retry_session_id":"old"}),
            );
            apply("SpawnReasoning", json!({}));
        }
        for _ in 0..4 {
            apply("CheckReasoning", json!({}));
            apply("ReasoningComplete", json!({"reasoning_result":"{}"}));
            apply("CompositionRejected", json!({"program_json":"{}"}));
            apply("LaunchReasoning", launch.clone());
            apply("SpawnReasoning", json!({}));
        }
        for _ in 0..2 {
            apply("CheckReasoning", json!({}));
            apply("ReasoningPending", json!({}));
        }
        apply("CheckReasoning", json!({}));
        let result = if timeout {
            apply("ReasoningPending", json!({}));
            apply("CheckReasoning", json!({}));
            apply("Fail", json!({"error_message":"poll budget exhausted"}))
        } else {
            apply("ReasoningComplete", json!({"reasoning_result":"{}"}));
            apply(
                "Expanded",
                json!({"snapshot_json":"{}","program_json":"{}","started_at_ms":"original"}),
            )
        };
        assert_eq!(
            result["counters"]["transition_count"],
            if timeout { 40 } else { 39 }
        );
        assert_eq!(
            result["counters"]["reasoning_phase_polls"],
            if timeout { 11 } else { 10 }
        );
        assert_eq!(result["counters"]["reasoning_retry_count"], 3);
        if !timeout {
            assert_eq!(result["fields"]["started_at_ms"], "original");
        }
    }
}
