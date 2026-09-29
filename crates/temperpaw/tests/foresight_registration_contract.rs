//! Runtime contract for immutable Forecast initialization and its declared queue.
use serde_json::json;
use std::collections::BTreeMap;
use temper_jit::TransitionTable;

#[test]
fn recovery_reenters_through_declared_system_actions() {
    for (source, source_action, target, states) in [
        (
            include_str!("../../../os-apps/paw-foresight/specs/path.ioa.toml"),
            "ResumeRepair",
            "StartRepair",
            &["Solving"][..],
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/path.ioa.toml"),
            "ResumeCosting",
            "EvaluateChallenge",
            &["Challenged"][..],
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/claim.ioa.toml"),
            "ResumeBridge",
            "EvaluateRoutes",
            &["Bridging"][..],
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/world.ioa.toml"),
            "ResumeWorldCascade",
            "EvaluateWorldCascade",
            &["Active", "RegisteringForecasts"][..],
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/endpoint.ioa.toml"),
            "ResumeWriter",
            "StartWriter",
            &["Sampled"][..],
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/endpoint.ioa.toml"),
            "ResumeEndpointScoring",
            "EvaluateEndpointScoring",
            &["UnderRepair"][..],
        ),
    ] {
        let table = TransitionTable::from_ioa_source(source);
        let spec: toml::Value = toml::from_str(source).unwrap();
        let action = spec["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"].as_str() == Some(source_action))
            .unwrap();
        let trigger = &action["triggers"].as_array().unwrap()[0];
        assert_eq!(trigger["kind"].as_str(), Some("entity"));
        assert_eq!(trigger["principal"].as_str(), Some("system"));
        assert_eq!(trigger["target_action"].as_str(), Some(target));
        assert_eq!(trigger["resolve_target"]["type"].as_str(), Some("same_id"));
        for state in states {
            for step in [source_action, target] {
                let result = table.evaluate(state, 0, step).unwrap();
                assert!(result.success, "{step} from {state}");
                assert_eq!(result.new_state, *state);
            }
        }
    }
}

#[test]
fn background_claim_search_reopens_once_and_retains_first_pass_routes() {
    use std::sync::Arc;
    use temper_runtime::scheduler::SimActorHandler;
    use temper_server::entity_actor::sim_handler::EntityActorHandler;
    for terminal in ["Settle", "MarkUnreachable"] {
        let table = TransitionTable::from_ioa_source(include_str!(
            "../../../os-apps/paw-foresight/specs/claim.ioa.toml"
        ));
        let mut claim = EntityActorHandler::new("Claim", "claim-background", Arc::new(table));
        claim.init().unwrap();
        claim.handle_message("SubmitForBridge", "{}").unwrap();
        claim
            .handle_message(
                "RoutesAttached",
                r#"{"path_ids":"[\"path-first\"]","route_count":"1"}"#,
            )
            .unwrap();
        let params = if terminal == "Settle" {
            r#"{"settled_route_id":"path-first","best_route_cost":"50","classification":"reachable"}"#
        } else {
            r#"{"unreachable_reason":"first-pass budget"}"#
        };
        claim.handle_message(terminal, params).unwrap();
        let deep = claim
            .handle_message("BeginDeepening", r#"{"settled_route_id":""}"#)
            .unwrap();
        assert_eq!(deep["status"], "Bridging");
        assert_eq!(deep["fields"]["path_ids"], "[\"path-first\"]");
        assert_eq!(deep["fields"]["route_count"], "1");
        assert_eq!(deep["fields"]["settled_route_id"], "");
        assert_eq!(deep["booleans"]["deepening_started"], true);
        claim.handle_message(terminal, params).unwrap();
        assert!(
            claim
                .handle_message("BeginDeepening", r#"{"settled_route_id":""}"#)
                .is_err()
        );
    }
}

#[test]
fn durable_path_and_claim_verdicts_drive_world_progress() {
    let cases = [
        (
            include_str!("../../../os-apps/paw-foresight/specs/path.ioa.toml"),
            "ClassifyCanonical",
            "RequestForecastRegistration",
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/path.ioa.toml"),
            "ClassifyTail",
            "RequestForecastRegistration",
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/claim.ioa.toml"),
            "Settle",
            "ResumeWorldCascade",
        ),
        (
            include_str!("../../../os-apps/paw-foresight/specs/claim.ioa.toml"),
            "MarkUnreachable",
            "ResumeWorldCascade",
        ),
    ];
    for (source, name, target) in cases {
        let spec: toml::Value = toml::from_str(source).unwrap();
        let action = spec["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .unwrap();
        let matches = |a: &toml::Value| {
            a.get("triggers")
                .and_then(toml::Value::as_array)
                .is_some_and(|ts| {
                    ts.iter().any(|t| {
                        t.get("kind").and_then(toml::Value::as_str) == Some("entity")
                            && t.get("principal").and_then(toml::Value::as_str) == Some("system")
                            && t.get("target_entity").and_then(toml::Value::as_str) == Some("World")
                            && t.get("target_action").and_then(toml::Value::as_str) == Some(target)
                            && t["resolve_target"]["field"].as_str() == Some("world_id")
                    })
                })
        };
        assert!(
            matches(action),
            "{name} must notify after its durable commit"
        );
        let mut missing = action.clone();
        missing.as_table_mut().unwrap().remove("triggers");
        assert!(!matches(&missing));
    }
    let table = TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ));
    for status in ["Active", "RegisteringForecasts"] {
        let result = table.evaluate(status, 0, "ResumeWorldCascade").unwrap();
        assert!(result.success);
        assert_eq!(
            result.new_state, status,
            "claim completion must not interrupt registration"
        );
    }
}

#[test]
fn progressive_registration_coalesces_requests_and_rejects_stale_batches() {
    use std::sync::Arc;
    use temper_runtime::scheduler::SimActorHandler;
    use temper_server::entity_actor::sim_handler::EntityActorHandler;
    let table = TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ));
    let spec: toml::Value = toml::from_str(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ))
    .unwrap();
    for trigger in spec["action"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|action| action.get("triggers").and_then(toml::Value::as_array))
        .flatten()
    {
        if matches!(
            trigger.get("module").and_then(toml::Value::as_str),
            Some("register_forecasts" | "aggregate_costs")
        ) {
            assert!(
                trigger.get("on_failure").is_none(),
                "a refused old callback must not compensate the current batch"
            );
        }
    }
    let mut world = EntityActorHandler::new("World", "world-progressive", Arc::new(table));
    world.init().unwrap();
    world.handle_message("Seed", "{}").unwrap();
    world.handle_message("SeedComplete", "{}").unwrap();
    let requested = world
        .handle_message("RequestForecastRegistration", "{}")
        .unwrap();
    assert_eq!(requested["status"], "Active");
    assert_eq!(requested["booleans"]["forecast_registration_pending"], true);
    let start = r#"{"forecast_registration_key":""}"#;
    let first = world
        .handle_message("StartForecastRegistration", start)
        .unwrap();
    assert_eq!(first["counters"]["registration_attempt"], 1);
    assert_eq!(first["booleans"]["forecast_registration_pending"], false);
    assert!(
        world
            .handle_message("StartForecastRegistration", start)
            .is_err()
    );
    for _ in 0..3 {
        let queued = world
            .handle_message("RequestForecastRegistration", "{}")
            .unwrap();
        assert_eq!(queued["status"], "RegisteringForecasts");
        assert_eq!(queued["booleans"]["forecast_registration_pending"], true);
    }
    // Final world weighting may settle while an earlier path's forecasts commit.
    let settled = world
        .handle_message(
            "PathsScored",
            r#"{"canonical_path_id":"path-final","exploration_phase":"first_pass","expected_exploration_phase":"first_pass"}"#,
        )
        .unwrap();
    assert_eq!(settled["status"], "RegisteringForecasts");
    assert_eq!(settled["booleans"]["first_pass_complete"], true);
    world
        .handle_message(
            "ForecastRegistrationComplete",
            r#"{"expected_registration_attempt":1,"error_message":"","learning_mode":"observed"}"#,
        )
        .unwrap();
    let next = world
        .handle_message("StartForecastRegistration", start)
        .unwrap();
    assert_eq!(next["counters"]["registration_attempt"], 2);
    assert_eq!(next["booleans"]["forecast_registration_pending"], false);
    for (action, payload) in [
        (
            "RegistrationSnapshotPrepared",
            json!({"expected_registration_attempt":1,"registration_nodes_json":"[]"}),
        ),
        (
            "CommitForecastSnapshot",
            json!({"expected_registration_attempt":1}),
        ),
        (
            "ForecastPrepared",
            json!({"expected_registration_attempt":1,"forecast_id":"late"}),
        ),
        (
            "ForecastRegistrationComplete",
            json!({"expected_registration_attempt":1,"error_message":"","learning_mode":"observed"}),
        ),
        (
            "ForecastRegistrationFailed",
            json!({"expected_registration_attempt":1,"error_message":"old"}),
        ),
    ] {
        assert!(
            world.handle_message(action, &payload.to_string()).is_err(),
            "{action}"
        );
        assert_eq!(world.current_status(), "RegisteringForecasts");
        // The exact same payload passes validation only when its attempt fence
        // is removed: unrelated strict-parameter errors cannot mask this test.
        let mut mutant = TransitionTable::from_ioa_source(include_str!(
            "../../../os-apps/paw-foresight/specs/world.ioa.toml"
        ));
        mutant
            .action_contracts
            .get_mut(action)
            .unwrap()
            .constraints
            .retain(|c| c.param() != "expected_registration_attempt");
        assert!(
            mutant
                .validate_action_params(
                    action,
                    &payload,
                    &json!({}),
                    &BTreeMap::from([("registration_attempt".into(), 2)]),
                    &BTreeMap::new()
                )
                .is_ok()
        );
    }
    assert!(
        world
            .handle_message("ForecastRegistered", r#"{"registration_key":"old-key"}"#)
            .is_err()
    );
    world
        .handle_message(
            "ForecastRegistrationComplete",
            r#"{"expected_registration_attempt":2,"error_message":"","learning_mode":"observed"}"#,
        )
        .unwrap();
    assert!(
        world
            .handle_message("StartForecastRegistration", start)
            .is_err(),
        "drained requests must not spin"
    );
    let deep =
        r#"{"deepening_claim_ids":"[]","deepening_cursor":"0","exploration_phase":"deepening"}"#;
    world.handle_message("BeginDeepening", deep).unwrap();
    assert!(world.handle_message("PathsScored", r#"{"canonical_path_id":"stale","exploration_phase":"first_pass","expected_exploration_phase":"first_pass"}"#).is_err(), "late first-pass callbacks must not rewind deepening");
    let completed = world.handle_message("PathsScored", r#"{"canonical_path_id":"final","exploration_phase":"complete","expected_exploration_phase":"deepening"}"#).unwrap();
    assert_eq!(completed["fields"]["exploration_phase"], "complete");

    assert!(
        world.handle_message("BeginDeepening", deep).is_err(),
        "background exploration starts once"
    );
}

#[test]
fn strict_forecast_initializes_only_through_declared_register() {
    let source = include_str!("../../../os-apps/paw-foresight/specs/forecast.ioa.toml");
    let table = TransitionTable::from_ioa_source(source);
    assert!(table.strict_action_params);
    assert_eq!(table.initial_state, "Created");
    assert!(
        table
            .validate_initial_fields(&json!({"Id":"forecast-test"}))
            .is_ok()
    );
    assert!(
        table
            .validate_initial_fields(&json!({"probability":"0.55"}))
            .is_err()
    );
    let registration = json!({"registration_key":"key","world_id":"world","event_node_id":"event","question":"Future question?","probability":"0.55","base_probability":"0.55","model_version":"identity","learning_run_id":"","previous_forecast_id":"","evidence_kind":"simulated","resolve_by":"2025-06-01T00:00:00Z","market_ref":"","engine_version":"test","registered_at":"2025-03-01T00:00:00Z"});
    assert!(
        table
            .validate_action_params(
                "Register",
                &registration,
                &json!({}),
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .is_ok()
    );
    for forbidden in ["outcome", "brier", "resolved_at", "outcome_evidence_kind"] {
        let mut injected = registration.clone();
        injected[forbidden] = json!("forged");
        assert!(
            table
                .validate_action_params(
                    "Register",
                    &injected,
                    &json!({}),
                    &BTreeMap::new(),
                    &BTreeMap::new()
                )
                .is_err()
        );
    }
    let spec: toml::Value = toml::from_str(source).unwrap();
    for action in spec["action"].as_array().unwrap() {
        let accepts_prediction = action["params"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| p.as_str() == Some("probability")));
        if accepts_prediction {
            assert_eq!(
                action["from"].as_array().unwrap(),
                &vec![toml::Value::String("Created".into())]
            );
            assert_eq!(action["to"].as_str(), Some("Preregistered"));
        }
    }
}

#[test]
fn recovery_and_adoption_preserve_operational_state() {
    let table = TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ));
    let recovered = table
        .evaluate("RegisteringForecasts", 0, "ForecastRegistrationFailed")
        .unwrap();
    assert!(recovered.success);
    assert_eq!(recovered.new_state, "Active");
    for state in ["Active", "Updating", "RegisteringForecasts"] {
        let adopted = table.evaluate(state, 0, "AdoptModel").unwrap();
        assert!(adopted.success, "{state}");
        assert_eq!(adopted.new_state, state);
    }
    for state in ["Created", "Archived", "Failed"] {
        assert!(
            !table
                .evaluate(state, 0, "AdoptModel")
                .is_some_and(|r| r.success)
        );
    }
}

#[test]
fn hundred_batch_scores_have_one_declared_learning_trigger() {
    let source = include_str!("../../../os-apps/paw-foresight/specs/forecast.ioa.toml");
    let table = TransitionTable::from_ioa_source(source);
    let forecast: toml::Value = toml::from_str(source).unwrap();
    let action = forecast["action"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"].as_str() == Some("ScoreBatch"))
        .unwrap();
    let mut spawned = 0;
    for _ in 0..100 {
        let scored = table.evaluate("Resolved", 0, "ScoreBatch").unwrap();
        assert!(scored.success);
        assert_eq!(scored.new_state, "Scored");
        spawned += action
            .get("triggers")
            .and_then(toml::Value::as_array)
            .map_or(0, Vec::len);
    }
    assert_eq!(spawned, 0);
    let hindcast: toml::Value = toml::from_str(include_str!(
        "../../../os-apps/paw-foresight/specs/hindcast.ioa.toml"
    ))
    .unwrap();
    let done = hindcast["action"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"].as_str() == Some("ScoreComplete"))
        .unwrap();
    let triggers = done["triggers"].as_array().unwrap();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0]["target_entity"].as_str(), Some("LearningRun"));
    assert_eq!(triggers[0]["target_action"].as_str(), Some("Start"));
    assert!(triggers[0].get("guard").is_some());
}

#[test]
fn changed_action_metadata_matches_ioa_and_detects_missing_parameters() {
    use std::collections::BTreeSet;
    use temper_spec::{
        automaton::parse_automaton,
        csdl::{CsdlDocument, parse_csdl},
    };
    let xml = include_str!("../../../os-apps/paw-foresight/specs/model.csdl.xml");
    let targets = [
        (
            "World",
            include_str!("../../../os-apps/paw-foresight/specs/world.ioa.toml"),
            &[
                "Configure",
                "ResearchSessionStarted",
                "CheckResearchSession",
                "ResearchPending",
                "ResearchIncomplete",
                "ResearchFailed",
                "ForecastPrepared",
                "ForecastRegistrationComplete",
                "ForecastRegistrationFailed",
                "PathsScored",
                "RequestForecastRegistration",
                "StartForecastRegistration",
                "ForecastRegistrationTimedOut",
                "MaybeDeepen",
                "BeginDeepening",
                "ContinueDeepening",
                "DeepeningClaimPrepared",
                "EvaluateWorldCascade",
                "RegistrationSnapshotPrepared",
                "CommitForecastSnapshot",
            ][..],
        ),
        (
            "Path",
            include_str!("../../../os-apps/paw-foresight/specs/path.ioa.toml"),
            &["StartRepair"][..],
        ),
        (
            "Claim",
            include_str!("../../../os-apps/paw-foresight/specs/claim.ioa.toml"),
            &["EvaluateRoutes", "BeginDeepening"][..],
        ),
        (
            "Endpoint",
            include_str!("../../../os-apps/paw-foresight/specs/endpoint.ioa.toml"),
            &["StartWriter", "EvaluateEndpointScoring"][..],
        ),
        (
            "Forecast",
            include_str!("../../../os-apps/paw-foresight/specs/forecast.ioa.toml"),
            &["ScoreBatch"][..],
        ),
        (
            "Hindcast",
            include_str!("../../../os-apps/paw-foresight/specs/hindcast.ioa.toml"),
            &["ScoreComplete"][..],
        ),
    ];
    let mut expected = BTreeMap::new();
    for (entity, source, names) in targets {
        let ioa = parse_automaton(source).unwrap();
        for name in names {
            let action = ioa.actions.iter().find(|a| a.name == *name).unwrap();
            let params: BTreeSet<String> =
                action.params.iter().map(|p| p.name().to_string()).collect();
            expected.insert(
                (format!("TemperPaw.Foresight.{entity}"), name.to_string()),
                params,
            );
        }
    }
    let matches = |document: &CsdlDocument| {
        expected.iter().all(|((binding, name), params)| {
            let actions: Vec<_> = document
                .schemas
                .iter()
                .flat_map(|s| &s.actions)
                .filter(|a| a.name == *name && a.binding_type() == Some(binding.as_str()))
                .collect();
            actions.len() == 1
                && actions[0]
                    .parameters
                    .iter()
                    .skip(1)
                    .map(|p| p.name.clone())
                    .collect::<BTreeSet<_>>()
                    == *params
        })
    };
    assert!(matches(&parse_csdl(xml).unwrap()));
    // Exercise each changed contract's missing-parameter failure with the real parser.
    for (entity, name, parameter) in [
        ("World", "Configure", "last_ingest_date"),
        ("World", "ResearchSessionStarted", "research_session_id"),
        ("World", "ResearchSessionStarted", "research_session_attempt"),
        ("World", "ResearchPending", "expected_research_session_id"),
        ("World", "ResearchIncomplete", "research_session_attempt"),
        ("World", "ResearchFailed", "expected_research_attempt"),
        (
            "World",
            "ResearchSessionStarted",
            "expected_research_attempt",
        ),
        ("World", "ForecastPrepared", "registration_model_json"),
        ("World", "ForecastPrepared", "registration_nodes_json"),
        ("World", "ForecastPrepared", "expected_registration_attempt"),
        (
            "World",
            "StartForecastRegistration",
            "forecast_registration_key",
        ),
        (
            "World",
            "DeepeningClaimPrepared",
            "expected_deepening_cursor",
        ),
        ("World", "ForecastPrepared", "learning_mode"),
        ("World", "ForecastRegistrationComplete", "learning_mode"),
        ("World", "ForecastRegistrationFailed", "error_message"),
        ("Forecast", "ScoreBatch", "brier"),
        ("Hindcast", "ScoreComplete", "learning_as_of"),
    ] {
        let mut mutant = parse_csdl(xml).unwrap();
        let binding = format!("TemperPaw.Foresight.{entity}");
        let action = mutant
            .schemas
            .iter_mut()
            .flat_map(|s| &mut s.actions)
            .find(|a| a.name == name && a.binding_type() == Some(binding.as_str()))
            .unwrap();
        let before = action.parameters.len();
        action.parameters.retain(|p| p.name != parameter);
        assert_eq!(action.parameters.len() + 1, before);
        assert!(
            !matches(&mutant),
            "{entity}.{name} missing {parameter} escaped the check"
        );
    }
}

#[test]
fn research_session_callback_preserves_world_progress_and_is_declared_on_each_seed() {
    let source = include_str!("../../../os-apps/paw-foresight/specs/world.ioa.toml");
    let table = TransitionTable::from_ioa_source(source);
    for state in [
        "Seeding",
        "Active",
        "Updating",
        "RegisteringForecasts",
        "Archived",
        "Failed",
    ] {
        let result = table.evaluate(state, 0, "ResearchSessionStarted").unwrap();
        assert!(result.success, "{state}");
        assert_eq!(result.new_state, state);
    }
    for state in ["Created", "OpeningReplay"] {
        assert!(
            !table
                .evaluate(state, 0, "ResearchSessionStarted")
                .is_some_and(|r| r.success)
        );
    }
    assert!(
        table
            .validate_initial_fields(&json!({"research_session_id":"forged"}))
            .is_err()
    );
    let spec: toml::Value = toml::from_str(source).unwrap();
    for name in ["Seed", "ResumeSeed"] {
        let action = spec["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["name"].as_str() == Some(name))
            .unwrap();
        let trigger = action["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|trigger| trigger["module"].as_str() == Some("seed_world"))
            .unwrap();
        assert_eq!(
            trigger.get("on_success").and_then(toml::Value::as_str),
            Some("ResearchSessionStarted")
        );
    }
}

#[test]
fn older_research_attempt_cannot_replace_the_current_session() {
    use std::sync::Arc;
    use temper_runtime::scheduler::SimActorHandler;
    use temper_server::entity_actor::sim_handler::EntityActorHandler;

    fn reverse_callbacks(
        table: TransitionTable,
        completed: bool,
    ) -> Result<serde_json::Value, String> {
        let mut world = EntityActorHandler::new("World", "world-1", Arc::new(table));
        world.init().unwrap();
        let first = world.handle_message("Seed", "{}").unwrap();
        assert_eq!(first["counters"]["research_attempt"], 1);
        let second = world.handle_message("ResumeSeed", "{}").unwrap();
        assert_eq!(second["counters"]["research_attempt"], 2);
        let current = world
            .handle_message(
                "ResearchSessionStarted",
                &json!({"research_session_id":"session-current","expected_research_attempt":2})
                    .to_string(),
            )
            .unwrap();
        assert_eq!(current["fields"]["research_session_id"], "session-current");
        if completed {
            let current = world.handle_message("SeedComplete",
                &json!({"skeleton_node_count":"1","graph_snapshot_file_id":"","uncertainty_axes":"[]"}).to_string()).unwrap();
            assert_eq!(current["status"], "Active");
        }
        let delayed = world.handle_message(
            "ResearchSessionStarted",
            &json!({"research_session_id":"session-old","expected_research_attempt":1}).to_string(),
        );
        assert_eq!(
            world.current_status(),
            if completed { "Active" } else { "Seeding" }
        );
        delayed
    }

    let source = include_str!("../../../os-apps/paw-foresight/specs/world.ioa.toml");
    for completed in [false, true] {
        assert!(reverse_callbacks(TransitionTable::from_ioa_source(source), completed).is_err());
        // Removing the guard must reproduce the overwrite with the same actor driver.
        let mut mutant = TransitionTable::from_ioa_source(source);
        let constraints = &mut mutant
            .action_contracts
            .get_mut("ResearchSessionStarted")
            .unwrap()
            .constraints;
        let before = constraints.len();
        constraints.retain(|constraint| constraint.param() != "expected_research_attempt");
        assert_eq!(constraints.len() + 1, before);
        let overwritten = reverse_callbacks(mutant, completed).unwrap();
        assert_eq!(overwritten["fields"]["research_session_id"], "session-old");
    }
}

#[test]
fn failed_registration_drains_only_work_queued_after_its_start() {
    use std::sync::Arc;
    use temper_runtime::scheduler::SimActorHandler;
    use temper_server::entity_actor::sim_handler::EntityActorHandler;
    let source = include_str!("../../../os-apps/paw-foresight/specs/world.ioa.toml");
    let spec: toml::Value = toml::from_str(source).unwrap();
    for recovery in ["ForecastRegistrationFailed", "ForecastRegistrationTimedOut"] {
        let action = spec["action"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["name"].as_str() == Some(recovery))
            .unwrap();
        let continuation = action
            .get("triggers")
            .and_then(toml::Value::as_array)
            .and_then(|triggers| {
                triggers.iter().find(|trigger| {
                    trigger.get("target_action").and_then(toml::Value::as_str)
                        == Some("StartForecastRegistration")
                })
            })
            .unwrap_or_else(|| panic!("{recovery} must not strand a later queued request"));
        assert_eq!(continuation["principal"].as_str(), Some("system"));
        assert_eq!(continuation["guard"]["type"].as_str(), Some("bool_true"));
        assert_eq!(
            continuation["guard"]["field"].as_str(),
            Some("forecast_registration_pending")
        );
        for queued in [false, true] {
            let mut world = EntityActorHandler::new(
                "World",
                "world-recovery",
                Arc::new(TransitionTable::from_ioa_source(source)),
            );
            world.init().unwrap();
            world.handle_message("Seed", "{}").unwrap();
            world.handle_message("SeedComplete", "{}").unwrap();
            world
                .handle_message("RequestForecastRegistration", "{}")
                .unwrap();
            world
                .handle_message(
                    "StartForecastRegistration",
                    r#"{"forecast_registration_key":""}"#,
                )
                .unwrap();
            if queued {
                world
                    .handle_message("RequestForecastRegistration", "{}")
                    .unwrap();
            }
            let params = if recovery == "ForecastRegistrationFailed" {
                r#"{"error_message":"transient","expected_registration_attempt":1}"#
            } else {
                r#"{"error_message":"deadline"}"#
            };
            let recovered = world.handle_message(recovery, params).unwrap();
            assert_eq!(recovered["status"], "Active");
            assert_eq!(
                recovered["booleans"]["forecast_registration_pending"],
                queued
            );
            let next = world.handle_message(
                "StartForecastRegistration",
                r#"{"forecast_registration_key":""}"#,
            );
            assert_eq!(
                next.is_ok(),
                queued,
                "a failed batch must not create an unbounded retry loop"
            );
            if let Ok(next) = next {
                assert_eq!(next["counters"]["registration_attempt"], 2);
                assert_eq!(next["booleans"]["forecast_registration_pending"], false);
                assert!(
                    world
                        .handle_message(
                            "ForecastRegistrationFailed",
                            r#"{"error_message":"stale","expected_registration_attempt":1}"#
                        )
                        .is_err()
                );
                world.handle_message("ForecastRegistrationFailed", r#"{"error_message":"still unavailable","expected_registration_attempt":2}"#).unwrap();
                assert!(
                    world
                        .handle_message(
                            "StartForecastRegistration",
                            r#"{"forecast_registration_key":""}"#
                        )
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn every_foresight_spec_loads_with_the_runtime_parser() {
    let specs =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-foresight/specs");
    for entry in std::fs::read_dir(specs).unwrap() {
        let path = entry.unwrap().path();
        if path.to_string_lossy().ends_with(".ioa.toml") {
            let source = std::fs::read_to_string(&path).unwrap();
            temper_spec::automaton::parse_automaton(&source)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
    }
}

#[test]
fn verified_operator_can_start_worlds_but_cannot_write_machine_results() {
    use temper_authz::{AuthzEngine, SecurityContext};
    let engine = AuthzEngine::new(include_str!(
        "../../../os-apps/paw-foresight/policies/foresight.cedar"
    ))
    .unwrap();
    let operator = SecurityContext::from_resolved_identity("operator", "operator", None);
    let session = SecurityContext::from_resolved_identity("session", "agent", None);
    let attrs = std::collections::HashMap::new();
    for action in [
        "ConfigureLearning",
        "Configure",
        "RequestSemanticExploration",
        "Seed",
    ] {
        assert!(
            engine
                .authorize(&operator, action, "World", &attrs)
                .is_allowed(),
            "{action}"
        );
        assert!(
            !engine
                .authorize(&session, action, "World", &attrs)
                .is_allowed(),
            "{action}"
        );
    }
    for action in [
        "SeedComplete",
        "StartSeed",
        "StartSemanticExploration",
        "AdoptModel",
    ] {
        assert!(
            !engine
                .authorize(&operator, action, "World", &attrs)
                .is_allowed(),
            "{action}"
        );
    }
    assert!(
        !engine
            .authorize(&operator, "Complete", "SemanticRun", &attrs)
            .is_allowed()
    );
}

#[test]
fn operator_seed_enters_the_declared_system_research_action() {
    let spec: toml::Value = toml::from_str(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ))
    .unwrap();
    let actions = spec["action"].as_array().unwrap();
    let seed = actions
        .iter()
        .find(|a| a["name"].as_str() == Some("Seed"))
        .unwrap();
    let trigger = &seed["triggers"].as_array().unwrap()[0];
    assert_eq!(trigger["kind"].as_str(), Some("entity"));
    assert_eq!(trigger["principal"].as_str(), Some("system"));
    assert_eq!(trigger["target_action"].as_str(), Some("StartSeed"));
    assert_eq!(trigger["resolve_target"]["type"].as_str(), Some("same_id"));
    let start = actions
        .iter()
        .find(|a| a["name"].as_str() == Some("StartSeed"))
        .unwrap();
    assert_eq!(start["triggers"][0]["module"].as_str(), Some("seed_world"));
}

#[test]
fn reasoning_spawn_copies_the_durably_saved_prompt() {
    use temper_server::entity_actor::{EntityState, process_action};
    let _clock = temper_runtime::scheduler::install_deterministic_context(518);
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"),
    )
    .unwrap();
    let table = temper_jit::TransitionTable::from_ioa_source(&source);
    let mut state: EntityState = serde_json::from_value(serde_json::json!({
        "entity_type":"SemanticRun", "entity_id":"run-test", "status":"ReasoningSetup",
        "item_count":0, "fields":{"system_prompt":"", "user_message":"", "model":"gpt-5.5"}
    }))
    .unwrap();
    let saved = process_action(
        &mut state,
        &table,
        "LaunchReasoning",
        &serde_json::json!({
            "system_prompt":"Design the branching worlds.", "user_message":"Question and twelve sourced facts."
        }),
    );
    assert!(saved.success);
    assert!(
        saved.spawn_requests.is_empty(),
        "spawn must follow prompt persistence"
    );
    assert!(
        saved
            .scheduled_actions
            .iter()
            .any(|a| a.action == "SpawnReasoning")
    );
    let spawned = process_action(&mut state, &table, "SpawnReasoning", &serde_json::json!({}));
    assert!(spawned.success);
    assert_eq!(spawned.spawn_requests.len(), 1);
    let fields = &spawned.spawn_requests[0].copied_field_values;
    assert_eq!(fields["system_prompt"], "Design the branching worlds.");
    assert_eq!(fields["user_message"], "Question and twelve sourced facts.");
}

#[test]
fn native_graph_and_trace_remain_readable_above_default_overflow_size() {
    use temper_server::entity_actor::{EntityState, process_action};
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/specs/semantic_run.ioa.toml"),
    )
    .unwrap();
    let table = temper_jit::TransitionTable::from_ioa_source(&source);
    for (status, action, field, size) in [
        ("Expanding", "Expanded", "snapshot_json", 200_000),
        ("Calling", "Recorded", "trace_json", 1_000_000),
    ] {
        let mut state: EntityState = serde_json::from_value(serde_json::json!({
            "entity_type":"SemanticRun", "entity_id":"run-test", "status":status,
            "item_count":0, "fields":{}
        }))
        .unwrap();
        let payload = format!("[\"{}\"]", "x".repeat(size));
        let mut params = serde_json::json!({});
        params[field] = serde_json::json!(payload);
        let result = process_action(&mut state, &table, action, &params);
        assert!(result.success);
        assert_eq!(
            state.fields[field], payload,
            "{field} must stay available to the next module"
        );
    }
}
