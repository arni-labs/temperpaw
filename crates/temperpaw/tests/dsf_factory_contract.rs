//! Run the installed IOA contracts through Temper's production actor evaluator.
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};
use temper_jit::table::TransitionTable;
use temper_runtime::scheduler::{FaultConfig, SimActorSystem, SimActorSystemConfig};
use temper_server::entity_actor::sim_handler::EntityActorHandler;
const ENTITIES: &[(&str, &str)] = &[
    ("railway_service_instance", "DsfRailwayServiceInstance"),
    ("vercel_project", "DsfVercelProject"),
    ("supabase_project", "DsfSupabaseProject"),
    ("cloudflare_r2_bucket", "DsfCloudflareR2Bucket"),
    ("datadog_monitor", "DsfDatadogMonitor"),
    ("media_pipeline", "DsfMediaPipeline"),
    ("flow", "DsfFlow"),
    ("participant", "DsfParticipant"),
    ("observation", "DsfObservation"),
    ("model_sync", "DsfModelSync"),
    ("experiment", "DsfExperiment"),
];
fn source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../os-apps/dsf-twin/specs/{name}.ioa.toml"));
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn legacy_resource_bootstrap_preserves_used_sequences() {
    use temper_server::entity_actor::{EntityState, effects::process_action};
    for (file, entity) in ENTITIES.iter().take(6) {
        let text = source(file);
        let table = TransitionTable::from_ioa_source(&text);
        let ioa = temper_spec::automaton::parse_automaton(&text).unwrap();
        let operation = ioa
            .actions
            .iter()
            .find(|action| {
                ["Deploy", "ApplyConfiguration", "SetAlias", "RetrySelected"]
                    .contains(&action.name.as_str())
            })
            .unwrap();
        let mut params: serde_json::Map<String, Value> = operation
            .params
            .iter()
            .map(|param| {
                (
                    param.name().to_owned(),
                    json!(format!("bound-{}", param.name())),
                )
            })
            .collect();
        params.insert("expected_operation_sequence".into(), json!(0));
        let params = Value::Object(params);
        // Recovered legacy state: no persisted operation counter or corresponding field.
        let mut state: EntityState = serde_json::from_value(json!({
            "entity_type":entity,"entity_id":"legacy","status":"Active","item_count":0,
            "fields":{},"counters":{"model_sequence":7,"observed_sequence":9}
        }))
        .unwrap();
        let before = serde_json::to_value(&state).unwrap();
        assert!(
            !process_action(&mut state, &table, &operation.name, &params).success,
            "{entity}"
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before, "{entity}");
        let start = text
            .find("[[action]]\nname = \"BootstrapOperationSequence\"")
            .unwrap();
        let end = start + text[start + 1..].find("[[action]]").unwrap() + 1;
        let without_bootstrap = format!("{}{}", &text[..start], &text[end..]);
        let old_table = TransitionTable::from_ioa_source(&without_bootstrap);
        assert!(!process_action(&mut state, &old_table, &operation.name, &params).success);
        assert!(
            !process_action(
                &mut state,
                &old_table,
                "BootstrapOperationSequence",
                &json!({})
            )
            .success
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before, "{entity}");
        for _ in 0..2 {
            let result =
                process_action(&mut state, &table, "BootstrapOperationSequence", &json!({}));
            assert!(result.success, "{entity}: {:?}", result.error);
            assert_eq!(state.counters["operation_sequence"], 0, "{entity}");
            assert_eq!(state.counters["model_sequence"], 7, "{entity}");
            assert_eq!(state.counters["observed_sequence"], 9, "{entity}");
        }
        assert!(
            process_action(&mut state, &table, &operation.name, &params).success,
            "{entity}"
        );
        assert_eq!(state.counters["operation_sequence"], 1, "{entity}");
        assert!(
            !process_action(&mut state, &table, "BootstrapOperationSequence", &json!({})).success
        );
        state.status = "Active".into();
        let used = serde_json::to_value(&state).unwrap();
        assert!(
            !process_action(&mut state, &table, "BootstrapOperationSequence", &json!({})).success
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), used, "{entity}");
    }
}

fn step(sim: &mut SimActorSystem, action: &str, params: Value) -> Value {
    sim.step("subject", action, &params.to_string())
        .unwrap_or_else(|error| panic!("{action}: {error}"))
}

fn simulator(name: &str, entity: &str, seed: u64) -> SimActorSystem {
    let ioa = source(name);
    let handler = EntityActorHandler::new(
        entity,
        "subject",
        Arc::new(TransitionTable::from_ioa_source(&ioa)),
    )
    .with_ioa_invariants(&ioa);
    let mut sim = SimActorSystem::new(SimActorSystemConfig {
        seed,
        faults: FaultConfig::none(),
        ..Default::default()
    });
    sim.register_actor("subject", Box::new(handler));
    sim
}
fn registered(seed: u64) -> SimActorSystem {
    let mut sim = simulator(
        "railway_service_instance",
        "DsfRailwayServiceInstance",
        seed,
    );
    step(
        &mut sim,
        "Register",
        json!({"project_id":"project-1", "service_id":"service-1", "environment_id":"production", "config_ref":"file-1", "config_sha256":"hash-1", "intended_configuration":"approved-config"}),
    );
    sim
}
fn request(sequence: u64) -> Value {
    json!({"operation_key":format!("operation-{sequence}"), "expected_operation_sequence":sequence, "effort_id":"effort-1", "request_revision":"revision-1", "request_configuration":"{}", "proof_ref":"proof-1"})
}
fn validation(sequence: u64) -> Value {
    json!({"operation_key":format!("operation-{sequence}"), "expected_operation_sequence":sequence+1, "validation_evidence_ref":"validation-1", "intended_revision":"revision-1"})
}
fn provider(sequence: u64) -> Value {
    json!({"operation_key":format!("operation-{sequence}"), "expected_operation_sequence":sequence+1, "provider_execution_id":"deployment-1", "provider_evidence_ref":"provider-query-1"})
}
fn verification(sequence: u64) -> Value {
    json!({"operation_key":format!("operation-{sequence}"), "expected_operation_sequence":sequence+1, "verified_resource_id":"subject", "verified_revision":"revision-1", "provider_evidence_ref":"provider-query-1", "flow_evidence_ref":"probe-1", "telemetry_evidence_ref":"datadog-1"})
}
fn observation(sequence: u64, time: u64) -> Value {
    json!({"observation_id":format!("sample-{time}"), "observed_configuration":"measured-config", "observed_revision":"old-revision", "coverage":"measured", "outcome":"drift", "provenance_ref":"provider-query", "observed_at_ms":time, "expected_sequence":sequence})
}
fn executing(sim: &mut SimActorSystem, sequence: u64) {
    step(sim, "Deploy", request(sequence));
    step(sim, "DeployValidationSucceeded", validation(sequence));
    step(sim, "DeployExecute", json!({}));
}
fn configured_experiment(database: &str, bucket: &str) -> SimActorSystem {
    let mut sim = simulator("experiment", "DsfExperiment", 14);
    step(
        &mut sim,
        "Configure",
        json!({
            "effort_id":"experiment-effort", "branch":"codex/variant-one", "source_revision":"revision-1",
            "computer_id":"experiment-computer", "database_id":database, "media_bucket":bucket,
            "media_namespace":"variant-one", "permitted_external_calls":"[]", "manifest_ref":"manifest-1", "manifest_sha256":"a".repeat(64)
        }),
    );
    step(&mut sim, "Validate", json!({}));
    step(
        &mut sim,
        "ValidationPrepared",
        json!({"expected_sequence":1,"exec_id":"exec-validation","command":"runner validate","phase_deadline_ms":"300000"}),
    );
    sim
}

fn isolation_evidence() -> Value {
    json!({"production_database_id":"production-db", "production_media_bucket":"production-media",
        "isolation_evidence_ref":"binding-check-1","expected_sequence":1})
}

#[test]
fn experiment_cannot_use_production_database_or_media_bucket() {
    for (database, bucket) in [
        ("production-db", "experiment-media"),
        ("experiment-db", "production-media"),
    ] {
        let mut sim = configured_experiment(database, bucket);
        assert!(
            sim.step(
                "subject",
                "IsolationSucceeded",
                &isolation_evidence().to_string()
            )
            .is_err()
        );
        assert!(sim.step("subject", "Run", "{}").is_err());
        sim.assert_status("subject", "Validating");
    }
    let mut sim = configured_experiment("experiment-db", "experiment-media");
    step(&mut sim, "IsolationSucceeded", isolation_evidence());
    assert!(
        sim.step("subject", "Run", r#"{"database_id":"production-db"}"#)
            .is_err()
    );
    step(&mut sim, "Run", json!({}));
    step(
        &mut sim,
        "RunPrepared",
        json!({"expected_sequence":2,"exec_id":"exec-run","command":"runner run","phase_deadline_ms":"1800000"}),
    );
    step(
        &mut sim,
        "RunSucceeded",
        json!({"result_ref":"variant-1", "test_evidence_ref":"tests-1", "expected_sequence":2}),
    );
    step(
        &mut sim,
        "Select",
        json!({"selection_ask_id":"ask-1", "delivery_effort_id":"delivery-1"}),
    );
    sim.assert_status("subject", "Selecting");
    step(
        &mut sim,
        "SelectionSucceeded",
        json!({"expected_sequence":1,"selection_evidence_ref":"ask-1-accepted-delivery"}),
    );
    assert!(sim.step("subject", "Deploy", "{}").is_err());
    step(&mut sim, "Cleanup", json!({}));
    step(
        &mut sim,
        "CleanupPrepared",
        json!({"expected_sequence":3,"exec_id":"exec-cleanup","command":"runner cleanup","phase_deadline_ms":"300000"}),
    );
    step(
        &mut sim,
        "CleanupSucceeded",
        json!({"cleanup_evidence_ref":"cleanup-1", "expected_sequence":3}),
    );
    sim.assert_status("subject", "Cleaned");
    assert!(!sim.has_violations());
}

#[test]
fn recorded_observations_are_immutable_for_every_coverage_outcome() {
    for coverage in ["Measured", "Absent", "Inaccessible", "Stale"] {
        let mut sim = simulator("observation", "DsfObservation", 15);
        let params = json!({"subject_type":"DsfResource", "subject_id":"api-production",
            "source":"datadog", "source_event_id":"query-1", "query":"service:dsf",
            "window_start":"2026-09-06T00:00:00Z", "window_end":"2026-09-06T01:00:00Z",
            "observed_at_ms":1000, "sample_kind":"sampled-spans", "outcome":"unknown",
            "summary":"query evidence", "evidence_ref":"query-1"});
        step(&mut sim, &format!("Record{coverage}"), params.clone());
        for replacement in ["Measured", "Absent", "Inaccessible", "Stale"] {
            assert!(
                sim.step(
                    "subject",
                    &format!("Record{replacement}"),
                    &params.to_string()
                )
                .is_err()
            );
        }
        sim.assert_status("subject", coverage);
        sim.assert_event_count("subject", 1);
        assert!(!sim.has_violations());
    }
}

fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn csdl_matches_every_declared_ioa_field_action_and_parameter() {
    use std::collections::BTreeMap;
    let csdl_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps/dsf-twin/specs/model.csdl.xml");
    let document = temper_spec::csdl::parse_csdl(&fs::read_to_string(csdl_path).unwrap()).unwrap();
    let schema = &document.schemas[0];
    assert_eq!(schema.entity_types.len(), ENTITIES.len());
    for (file, name) in ENTITIES {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        let entity = schema
            .entity_types
            .iter()
            .find(|entity| entity.name == *name)
            .unwrap();
        let field_types: BTreeMap<_, _> = entity
            .properties
            .iter()
            .filter(|field| !["Id", "Status"].contains(&field.name.as_str()))
            .map(|field| (field.name.clone(), field.type_name.clone()))
            .collect();
        let expected_types: BTreeMap<_, _> = ioa
            .state
            .iter()
            .map(|field| {
                let kind = match field.var_type.as_str() {
                    "counter" => "Edm.Int64",
                    "bool" => "Edm.Boolean",
                    "string" => "Edm.String",
                    other => panic!("unexpected field type {other}"),
                };
                (pascal_case(&field.name), kind.to_owned())
            })
            .collect();
        assert_eq!(field_types, expected_types, "{name} properties");
        let binding = format!("Dsf.Twin.{name}");
        let actions: BTreeMap<_, _> = schema
            .actions
            .iter()
            .filter(|action| {
                action
                    .parameters
                    .first()
                    .is_some_and(|parameter| parameter.type_name == binding)
            })
            .map(|action| (action.name.as_str(), action))
            .collect();
        assert_eq!(actions.len(), ioa.actions.len(), "{name} action count");
        for action in &ioa.actions {
            let served = actions[action.name.as_str()];
            let expected: Vec<_> = action.params.iter().map(|param| param.name()).collect();
            let actual: Vec<_> = served
                .parameters
                .iter()
                .skip(1)
                .map(|param| param.name.as_str())
                .collect();
            assert_eq!(actual, expected, "{name}.{} parameters", action.name);
        }
    }
}

#[test]
fn counter_assignments_only_read_declared_action_parameters() {
    for (file, name) in ENTITIES {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        for action in ioa.actions {
            for effect in action.effect {
                if let temper_spec::automaton::Effect::Assign {
                    op: temper_spec::predicate::AssignOp::Set,
                    value: temper_spec::predicate::Arg::Param(param),
                    ..
                } = effect
                {
                    assert!(
                        action
                            .params
                            .iter()
                            .any(|declared| declared.name() == param),
                        "{name}.{} reads missing counter parameter {param}",
                        action.name
                    );
                }
            }
        }
    }
}

#[test]
fn incoming_counter_values_have_explicit_assignment_effects() {
    for (file, name) in ENTITIES {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        for action in &ioa.actions {
            for parameter in &action.params {
                if ioa.state.iter().any(|state| {
                    state.name == parameter.name()
                        && state.var_type == temper_spec::automaton::VarType::Counter
                }) {
                    assert!(
                        action.effect.iter().any(|effect| matches!(effect,
                        temper_spec::automaton::Effect::Assign {
                            var,
                            op: temper_spec::predicate::AssignOp::Set,
                            value: temper_spec::predicate::Arg::Param(param),
                        } if var == parameter.name() && param == parameter.name())),
                        "{name}.{} must explicitly assign counter {}",
                        action.name,
                        parameter.name()
                    );
                }
            }
        }
    }
}

#[test]
fn resource_types_declare_only_their_own_provider_actions() {
    for (file, entity) in ENTITIES {
        let sim = simulator(file, entity, 467);
        sim.assert_status("subject", "Draft");
        assert!(!sim.has_violations());
        assert!(!source(file).contains("DsfResource\""));
        assert!(!source(file).contains("DsfOperation\""));
    }
    for (file, operations) in [
        (
            "railway_service_instance",
            &["Deploy", "ApplyConfiguration", "Rollback"][..],
        ),
        (
            "vercel_project",
            &["Deploy", "ApplyConfiguration", "Rollback", "SetAlias"][..],
        ),
        ("supabase_project", &["ApplyConfiguration"][..]),
        ("cloudflare_r2_bucket", &["ApplyConfiguration"][..]),
        ("datadog_monitor", &["ApplyConfiguration"][..]),
        ("media_pipeline", &["RetrySelected"][..]),
    ] {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        for operation in operations {
            assert!(ioa.actions.iter().any(|action| action.name == *operation));
            for stage in ["Execute", "Reconcile", "Verify", "VerificationSucceeded"] {
                assert!(
                    ioa.actions
                        .iter()
                        .any(|action| action.name == format!("{operation}{stage}"))
                );
            }
        }
        if !operations.contains(&"Deploy") {
            assert!(!ioa.actions.iter().any(|action| action.name == "Deploy"));
        }
    }
}

#[test]
fn observations_reject_replay_stale_data_and_desired_configuration_changes() {
    for seed in 1..=32 {
        let mut sim = registered(seed);
        let mut forged = observation(0, 1000);
        forged["intended_configuration"] = json!("forged");
        assert!(sim.step("subject", "Observe", &forged.to_string()).is_err());
        step(&mut sim, "Observe", observation(0, 1000));
        for input in [
            observation(0, 1000),
            observation(1, 999),
            observation(0, 2000),
        ] {
            assert!(sim.step("subject", "Observe", &input.to_string()).is_err());
        }
        let state = step(&mut sim, "Observe", observation(1, 2000));
        assert_eq!(state["fields"]["observed_sequence"], 2);
        assert_eq!(state["fields"]["intended_configuration"], "approved-config");
        assert!(!sim.has_violations());
    }
}

#[test]
fn unavailable_observation_preserves_previous_measured_values() {
    let mut sim = registered(467);
    step(&mut sim, "Observe", observation(0, 1000));
    let state = step(
        &mut sim,
        "ObserveUnavailable",
        json!({"observation_id":"unavailable-1","coverage":"inaccessible","provenance_ref":"http-403","observed_at_ms":2000,"expected_sequence":1}),
    );
    assert_eq!(state["fields"]["observed_configuration"], "measured-config");
    assert_eq!(state["fields"]["observation_available"], false);
}

#[test]
fn resource_lock_and_command_sequence_prevent_overlapping_or_replayed_writes() {
    let mut sim = registered(1);
    executing(&mut sim, 0);
    assert!(
        sim.step("subject", "Deploy", &request(0).to_string())
            .is_err()
    );
    assert!(
        sim.step("subject", "ApplyConfiguration", &request(1).to_string())
            .is_err()
    );
    for field in [
        "project_id",
        "service_id",
        "environment_id",
        "config_ref",
        "config_sha256",
        "request_revision",
    ] {
        let mut input = provider(0);
        input[field] = json!("replacement");
        assert!(
            sim.step("subject", "DeployExecutionSucceeded", &input.to_string())
                .is_err(),
            "{field}"
        );
    }
    step(&mut sim, "DeployExecutionSucceeded", provider(0));
    step(&mut sim, "DeployVerify", json!({}));
    step(&mut sim, "DeployVerificationSucceeded", verification(0));
    assert!(
        sim.step("subject", "Deploy", &request(0).to_string())
            .is_err()
    );
    executing(&mut sim, 1);
    assert!(
        sim.step(
            "subject",
            "DeployExecutionSucceeded",
            &provider(0).to_string()
        )
        .is_err()
    );
    let state = step(&mut sim, "DeployExecutionSucceeded", provider(1));
    assert_eq!(state["fields"]["execution_attempts"], 1);
}

#[test]
fn uncertain_provider_writes_require_reconciliation_and_keep_the_lock() {
    for seed in 1..=32 {
        let mut sim = registered(seed);
        executing(&mut sim, 0);
        step(
            &mut sim,
            "DeployExecutionUncertain",
            json!({"operation_key":"operation-0","expected_operation_sequence":1,"error_message":"timeout"}),
        );
        assert!(sim.step("subject", "DeployExecute", "{}").is_err());
        assert!(
            sim.step("subject", "Deploy", &request(1).to_string())
                .is_err()
        );
        step(&mut sim, "DeployReconcile", json!({}));
        let mut wrong = provider(0);
        wrong["operation_key"] = json!("unrelated");
        assert!(
            sim.step("subject", "DeployProviderFound", &wrong.to_string())
                .is_err()
        );
        step(&mut sim, "DeployProviderFound", provider(0));
        assert!(sim.step("subject", "DeployExecute", "{}").is_err());
        assert!(!sim.has_violations());
    }
}

#[test]
fn retry_budget_and_exact_verification_hold_across_successive_resource_operations() {
    let mut sim = registered(7);
    executing(&mut sim, 0);
    for attempt in 1..=3 {
        step(&mut sim, "DeployExecutingTimedOut", json!({}));
        step(&mut sim, "DeployReconcile", json!({}));
        let absent = json!({"operation_key":"operation-0","expected_operation_sequence":1,"absence_evidence_ref":"exact-correlation-absent"});
        if attempt == 3 {
            assert!(
                sim.step("subject", "DeployProviderAbsent", &absent.to_string())
                    .is_err()
            );
            step(&mut sim, "DeployProviderFound", provider(0));
        } else {
            step(&mut sim, "DeployProviderAbsent", absent);
            step(&mut sim, "DeployExecute", json!({}));
        }
    }
    step(&mut sim, "DeployVerify", json!({}));
    for field in [
        "verified_resource_id",
        "verified_revision",
        "flow_evidence_ref",
        "telemetry_evidence_ref",
    ] {
        let mut forged = verification(0);
        forged[field] = json!(if field.contains("verified") {
            "other"
        } else {
            ""
        });
        assert!(
            sim.step(
                "subject",
                "DeployVerificationSucceeded",
                &forged.to_string()
            )
            .is_err(),
            "{field}"
        );
    }
    step(&mut sim, "DeployVerificationSucceeded", verification(0));
    executing(&mut sim, 1);
    assert!(!sim.has_violations());
}

#[test]
fn pending_verification_is_bounded_without_releasing_uncertain_resource() {
    let mut sim = registered(9);
    executing(&mut sim, 0);
    step(&mut sim, "DeployExecutionSucceeded", provider(0));
    for _ in 0..40 {
        step(&mut sim, "DeployVerify", json!({}));
        step(&mut sim, "DeployVerifyingTimedOut", json!({}));
    }
    assert!(sim.step("subject", "DeployVerify", "{}").is_err());
    assert!(
        sim.step("subject", "Deploy", &request(1).to_string())
            .is_err()
    );
    assert!(!sim.has_violations());
}

fn reaction_simulator() -> temper_server::trigger::sim_dispatcher::SimReactionSystem {
    reaction_simulator_with_registration(
        json!({"project_id":"project-1","service_id":"service-1","environment_id":"production","config_ref":"file-1","config_sha256":"hash-1","intended_configuration":"approved-config"}),
    )
}

fn reaction_simulator_with_registration(
    registration: Value,
) -> temper_server::trigger::sim_dispatcher::SimReactionSystem {
    use temper_server::{registry::SpecRegistry, trigger::sim_dispatcher::SimReactionSystem};
    let xml = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/dsf-twin/specs/model.csdl.xml"),
    )
    .unwrap();
    let csdl = temper_spec::csdl::parse_csdl(&xml).unwrap();
    let sources: Vec<_> = ENTITIES
        .iter()
        .map(|(file, entity)| (*entity, source(file)))
        .collect();
    let borrowed: Vec<_> = sources
        .iter()
        .map(|(entity, ioa)| (*entity, ioa.as_str()))
        .collect();
    let mut registry = SpecRegistry::new();
    registry.register_tenant("dsf-test", csdl, xml, &borrowed);
    let mut sim = SimReactionSystem::new(
        SimActorSystemConfig {
            seed: 467,
            faults: FaultConfig::none(),
            ..Default::default()
        },
        registry.build_reaction_registry(),
        "dsf-test",
    );
    for (file, entity, actor, id) in [
        (
            "railway_service_instance",
            "DsfRailwayServiceInstance",
            "resource",
            "subject",
        ),
        ("observation", "DsfObservation", "observation", "sample-1"),
        ("model_sync", "DsfModelSync", "sync", "sync-1"),
    ] {
        let ioa = source(file);
        let handler =
            EntityActorHandler::new(entity, id, Arc::new(TransitionTable::from_ioa_source(&ioa)))
                .with_ioa_invariants(&ioa);
        sim.register_actor(actor, entity, id, Box::new(handler));
    }
    sim.step("resource", "Register", &registration.to_string())
        .unwrap();
    sim
}
fn collection() -> Value {
    json!({"expected_refresh_sequence":1,"collected_observation_id":"sample-1","collected_source_event_id":"provider-event-1","collected_query":"deployment identity","collected_window_start":"2026-09-06T00:00:00Z","collected_window_end":"2026-09-06T00:01:00Z","collected_sample_kind":"provider_read","collected_outcome":"healthy","collected_summary":"exact provider identity","collected_evidence_ref":"provider-resource-url","collected_observed_at_ms":2000,"collected_expected_resource_sequence":0,"collected_observed_configuration":"measured-config","collected_observed_revision":"revision-1"})
}

#[test]
fn collection_commits_immutable_evidence_before_projecting_typed_resource_facts() {
    let mut sim = reaction_simulator();
    sim.step("resource", "RefreshObservations", "{}").unwrap();
    let staged = sim
        .step("resource", "CollectionMeasured", &collection().to_string())
        .unwrap();
    assert_eq!(
        staged["fields"]["observed_sequence"], 0,
        "collection staging cannot project facts early"
    );
    assert_eq!(staged["fields"]["observed_at_ms"], 0);
    sim.assert_status("observation", "Measured");
    assert_eq!(sim.last_results().len(), 2);
    assert!(
        sim.last_results().iter().all(|result| result.success),
        "{:?}",
        sim.last_results()
    );
    let state = sim
        .step("resource", "Observe", &observation(1, 3000).to_string())
        .unwrap();
    assert_eq!(state["fields"]["observed_sequence"], 2);
    assert_eq!(state["fields"]["intended_configuration"], "approved-config");
}

#[test]
fn successful_collection_clears_the_previous_failure() {
    let mut sim = reaction_simulator();
    sim.step("resource", "RefreshObservations", "{}").unwrap();
    sim.step(
        "resource",
        "CollectionFailed",
        &json!({"expected_refresh_sequence":1,"error_message":"previous read failed"}).to_string(),
    )
    .unwrap();
    sim.step("resource", "RefreshObservations", "{}").unwrap();
    let mut params = collection();
    params["expected_refresh_sequence"] = json!(2);
    params["error_message"] = json!("");
    let result = sim
        .step("resource", "CollectionMeasured", &params.to_string())
        .unwrap();
    assert_eq!(result["fields"]["error_message"], "");
}

#[test]
fn stale_projection_retains_immutable_evidence_and_the_newer_resource_facts() {
    let mut sim = reaction_simulator();
    sim.step("resource", "Observe", &observation(0, 2500).to_string())
        .unwrap();
    sim.step("resource", "RefreshObservations", "{}").unwrap();
    let staged = sim
        .step("resource", "CollectionMeasured", &collection().to_string())
        .unwrap();
    assert_eq!(staged["fields"]["observed_at_ms"], 2500);
    assert_eq!(staged["fields"]["observed_revision"], "old-revision");
    sim.assert_status("observation", "Measured");
    assert!(sim.last_results()[0].success);
    assert!(!sim.last_results()[1].success);
    let state = sim
        .step("resource", "Observe", &observation(1, 3000).to_string())
        .unwrap();
    assert_eq!(state["fields"]["observed_sequence"], 2);
}

#[test]
fn model_sync_does_not_duplicate_resource_scheduling() {
    let ioa = temper_spec::automaton::parse_automaton(&source("model_sync")).unwrap();
    assert!(ioa.actions.iter().all(|action| {
        !action
            .triggers
            .iter()
            .any(|trigger| trigger.target_action.as_deref() == Some("RefreshObservations"))
    }));
}

#[test]
fn known_provider_execution_is_not_repeated_under_scheduler_faults() {
    for seed in 1..=32 {
        let ioa = source("railway_service_instance");
        let handler = EntityActorHandler::new(
            "DsfRailwayServiceInstance",
            "subject",
            Arc::new(TransitionTable::from_ioa_source(&ioa)),
        )
        .with_ioa_invariants(&ioa);
        let mut sim = SimActorSystem::new(SimActorSystemConfig {
            seed,
            max_ticks: 150,
            max_actions_per_actor: 40,
            faults: FaultConfig::heavy(),
        });
        sim.register_actor("subject", Box::new(handler));
        step(
            &mut sim,
            "Register",
            json!({"project_id":"project-1","service_id":"service-1","environment_id":"production","config_ref":"file-1","config_sha256":"hash-1"}),
        );
        executing(&mut sim, 0);
        step(&mut sim, "DeployExecutionSucceeded", provider(0));
        let result = sim.run_random();
        assert!(
            result.all_invariants_held,
            "seed={seed}: {:?}",
            sim.violations()
        );
        let events = sim.events_json("subject");
        let writes = events
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["action"] == "DeployExecute")
            .count();
        assert_eq!(writes, 1, "seed={seed}: {events}");
    }
}

#[test]
fn guest_failures_require_original_sequence_while_host_failures_use_state_timers() {
    let mut sim = registered(10);
    executing(&mut sim, 0);
    for input in [
        json!({"error_message":"trap"}),
        json!({"operation_key":"operation-0","expected_operation_sequence":0,"error_message":"old trap"}),
    ] {
        assert!(
            sim.step("subject", "DeployExecutionUncertain", &input.to_string())
                .is_err()
        );
    }
    step(&mut sim, "DeployExecutingTimedOut", json!({}));
    sim.assert_status("subject", "DeployUnknown");
    for (file, _) in ENTITIES.iter().take(6) {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        for action in ioa.actions {
            for trigger in action.triggers {
                assert!(
                    trigger.on_failure.is_none(),
                    "unfenced host failure: {file}.{}",
                    action.name
                );
            }
        }
    }
}

#[tokio::test]
async fn resource_timer_reuse_cancels_the_previous_operation_generation() {
    use temper_runtime::{ActorSystem, tenant::TenantId};
    use temper_server::{
        registry::SpecRegistry,
        request_context::AgentContext,
        state::{DispatchCommand, ServerState},
    };
    let xml = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/dsf-twin/specs/model.csdl.xml"),
    )
    .unwrap();
    // Only wall-clock duration is shortened. Actions, guards and provider triggers
    // are the installed contract, including the deliberately missing WASM module.
    let ioa =
        source("railway_service_instance").replace("after_seconds = 300", "after_seconds = 1");
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("DsfRailwayServiceInstance", ioa.as_str())],
    );
    let state = ServerState::from_registry(ActorSystem::new("typed-resource-timers"), registry);
    let tenant = TenantId::from("default".to_owned());
    let ctx = AgentContext::for_service("typed-resource-timer-test");
    state
        .get_or_create_tenant_entity(&tenant, "DsfRailwayServiceInstance", "subject", json!({}))
        .await
        .unwrap();
    let dispatch = |action, params| {
        state.dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "DsfRailwayServiceInstance",
            entity_id: "subject",
            action,
            params,
            agent_ctx: &ctx,
            await_integration: false,
            await_reactions: true,
        })
    };
    assert!(dispatch("Register",json!({"project_id":"project-1","service_id":"service-1","environment_id":"production","config_ref":"file-1","config_sha256":"hash-1"})).await.unwrap().success);
    let _ = dispatch("Deploy", request(0)).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(dispatch("DeployValidationFailed",json!({"operation_key":"operation-0","expected_operation_sequence":1,"error_message":"validated refusal"})).await.unwrap().success);
    assert!(dispatch("DeployAcknowledgeFailure",json!({"operation_key":"operation-0","expected_operation_sequence":1,"failure_evidence_ref":"failed-before-write"})).await.unwrap().success);
    let _ = dispatch("Deploy", request(1)).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let before = state
        .get_tenant_entity_state(&tenant, "DsfRailwayServiceInstance", "subject")
        .await
        .unwrap();
    assert_eq!(
        before.state.status, "DeployValidating",
        "old generation must not time out the second operation"
    );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let after = state
        .get_tenant_entity_state(&tenant, "DsfRailwayServiceInstance", "subject")
        .await
        .unwrap();
    assert_eq!(
        after.state.status, "DeployFailed",
        "the second operation still receives its own timeout"
    );
}

#[test]
fn abandoning_reconciliation_records_failure_without_asserting_absence() {
    let mut sim = registered(32);
    executing(&mut sim, 0);
    step(&mut sim, "DeployExecutionSucceeded", provider(0));
    step(&mut sim, "DeployVerify", json!({}));
    step(&mut sim, "DeployVerificationSucceeded", verification(0));
    executing(&mut sim, 1);
    let params = json!({"operation_key":"operation-1","expected_operation_sequence":2,
        "error_message":"Old receipt was read before any provider write",
        "failure_evidence_ref":"evidence:pre-write-failure"});
    assert!(
        sim.step(
            "subject",
            "DeployAbandonReconciliation",
            &params.to_string()
        )
        .is_err()
    );
    let unknown = step(
        &mut sim,
        "DeployExecutionUncertain",
        json!({"operation_key":"operation-1",
        "expected_operation_sequence":2,"error_message":"Revision differs"}),
    );
    for (key, value) in [
        ("operation_key", json!("stale")),
        ("expected_operation_sequence", json!(0)),
        ("error_message", json!("")),
        ("failure_evidence_ref", json!("")),
    ] {
        let mut invalid = params.clone();
        invalid[key] = value;
        assert!(
            sim.step(
                "subject",
                "DeployAbandonReconciliation",
                &invalid.to_string()
            )
            .is_err()
        );
    }
    let failed = step(&mut sim, "DeployAbandonReconciliation", params.clone());
    assert_eq!(failed["status"], "DeployFailed");
    assert_eq!(failed["fields"]["execution_attempts"], 1);
    assert_eq!(failed["fields"]["reconciliation_attempts"], 0);
    assert_eq!(failed["fields"]["operation_sequence"], 2);
    assert_eq!(failed["fields"]["provider_execution_id"], "deployment-1");
    assert_ne!(failed["fields"]["provider_known"], true);
    assert_ne!(failed["fields"]["operation_verified"], true);
    assert_ne!(failed["fields"]["deploy_verified"], true);
    for name in [
        "absence_evidence_ref",
        "provider_execution_id",
        "provider_evidence_ref",
    ] {
        assert_eq!(failed["fields"][name], unknown["fields"][name], "{name}");
    }
    assert!(
        sim.step(
            "subject",
            "DeployAbandonReconciliation",
            &params.to_string()
        )
        .is_err()
    );
    let mut acknowledgement = params;
    acknowledgement
        .as_object_mut()
        .unwrap()
        .remove("error_message");
    let active = step(&mut sim, "DeployAcknowledgeFailure", acknowledgement);
    assert_eq!(active["status"], "Active");
    assert_ne!(active["fields"]["operation_verified"], true);
    assert!(!sim.has_violations());
}

#[test]
fn exhausted_verification_can_be_stopped_without_claiming_success() {
    let mut sim = registered(31);
    executing(&mut sim, 0);
    step(&mut sim, "DeployExecutionSucceeded", provider(0));
    let params = json!({"operation_key":"operation-0","expected_operation_sequence":1,
        "error_message":"Exact revision has no request correlation tags",
        "failure_evidence_ref":"evidence:missing-tags"});
    assert!(
        sim.step(
            "subject",
            "DeployStopExhaustedVerification",
            &params.to_string()
        )
        .is_err()
    );
    for _ in 0..40 {
        step(&mut sim, "DeployVerify", json!({}));
        step(&mut sim, "DeployVerifyingTimedOut", json!({}));
    }
    for (key, value) in [
        ("operation_key", json!("stale")),
        ("expected_operation_sequence", json!(0)),
        ("error_message", json!("")),
        ("failure_evidence_ref", json!("")),
    ] {
        let mut invalid = params.clone();
        invalid[key] = value;
        assert!(
            sim.step(
                "subject",
                "DeployStopExhaustedVerification",
                &invalid.to_string()
            )
            .is_err()
        );
    }
    let failed = step(&mut sim, "DeployStopExhaustedVerification", params.clone());
    assert_eq!(failed["status"], "DeployFailed");
    assert_eq!(failed["fields"]["verification_attempts"], 40);
    assert_eq!(failed["fields"]["execution_attempts"], 1);
    assert_eq!(failed["fields"]["operation_sequence"], 1);
    assert_eq!(failed["fields"]["provider_known"], true);
    assert_ne!(failed["fields"]["operation_verified"], true);
    assert_ne!(failed["fields"]["deploy_verified"], true);
    assert!(
        sim.step(
            "subject",
            "DeployStopExhaustedVerification",
            &params.to_string()
        )
        .is_err()
    );
    let mut acknowledgement = params;
    acknowledgement
        .as_object_mut()
        .unwrap()
        .remove("error_message");
    let active = step(&mut sim, "DeployAcknowledgeFailure", acknowledgement);
    assert_eq!(active["status"], "Active");
    assert_ne!(active["fields"]["operation_verified"], true);
    executing(&mut sim, 1);
    assert!(!sim.has_violations());
}

#[test]
fn explicit_resume_restores_only_the_exhausted_read_budget() {
    let mut sim = registered(11);
    executing(&mut sim, 0);
    step(&mut sim, "DeployExecutingTimedOut", json!({}));
    for _ in 0..20 {
        step(&mut sim, "DeployReconcile", json!({}));
        step(&mut sim, "DeployReconcilingTimedOut", json!({}));
    }
    assert!(sim.step("subject", "DeployReconcile", "{}").is_err());
    let correlation = json!({"operation_key":"operation-0","expected_operation_sequence":1});
    assert!(
        sim.step(
            "subject",
            "DeployResumeReconciliation",
            &json!({"operation_key":"old","expected_operation_sequence":1}).to_string()
        )
        .is_err()
    );
    let resumed = step(&mut sim, "DeployResumeReconciliation", correlation.clone());
    assert_eq!(resumed["fields"]["reconciliation_attempts"], 0);
    assert_eq!(resumed["fields"]["execution_attempts"], 1);
    assert_eq!(resumed["fields"]["operation_sequence"], 1);
    step(&mut sim, "DeployReconcile", json!({}));
    step(&mut sim, "DeployProviderFound", provider(0));
    for _ in 0..40 {
        step(&mut sim, "DeployVerify", json!({}));
        step(&mut sim, "DeployVerifyingTimedOut", json!({}));
    }
    assert!(sim.step("subject", "DeployVerify", "{}").is_err());
    let resumed = step(&mut sim, "DeployResumeVerification", correlation);
    assert_eq!(resumed["fields"]["verification_attempts"], 0);
    assert_eq!(resumed["fields"]["execution_attempts"], 1);
    step(&mut sim, "DeployVerify", json!({}));
    step(&mut sim, "DeployVerificationSucceeded", verification(0));
    assert!(!sim.has_violations());
}

#[test]
fn generated_csdl_and_module_manifest_are_current() {
    let generator =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin/specs/generate.py");
    let result = std::process::Command::new("python3")
        .arg(generator)
        .arg("--check")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn every_provider_action_validates_and_verifies_its_exact_configuration_or_revision() {
    for (file, entity) in ENTITIES.iter().take(6) {
        let ioa = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        let register = ioa
            .actions
            .iter()
            .find(|action| action.name == "Register")
            .unwrap();
        let registration: serde_json::Map<String, Value> = register
            .params
            .iter()
            .map(|parameter| {
                (
                    parameter.name().to_owned(),
                    json!(format!("bound-{}", parameter.name())),
                )
            })
            .collect();
        for name in [
            "Deploy",
            "ApplyConfiguration",
            "Rollback",
            "SetAlias",
            "RetrySelected",
        ] {
            let Some(action) = ioa.actions.iter().find(|action| action.name == name) else {
                continue;
            };
            let mut sim = simulator(file, entity, 467);
            step(&mut sim, "Register", Value::Object(registration.clone()));
            let mut accepted: serde_json::Map<String, Value> = action
                .params
                .iter()
                .map(|parameter| {
                    (
                        parameter.name().to_owned(),
                        json!(format!("accepted-{}", parameter.name())),
                    )
                })
                .collect();
            accepted.insert("expected_operation_sequence".into(), json!(0));
            step(&mut sim, name, Value::Object(accepted.clone()));
            let configuration = matches!(name, "ApplyConfiguration" | "SetAlias");
            let intended = if configuration {
                "intended_configuration"
            } else {
                "intended_revision"
            };
            let verified = if configuration {
                "verified_configuration"
            } else {
                "verified_revision"
            };
            let requested = if configuration {
                "request_configuration"
            } else {
                "request_revision"
            };
            let mut valid = json!({"operation_key":accepted["operation_key"],"expected_operation_sequence":1,"validation_evidence_ref":"valid-proof"});
            valid[intended] = accepted[requested].clone();
            step(&mut sim, &format!("{name}ValidationSucceeded"), valid);
            step(&mut sim, &format!("{name}Execute"), json!({}));
            step(
                &mut sim,
                &format!("{name}ExecutionSucceeded"),
                json!({"operation_key":accepted["operation_key"],"expected_operation_sequence":1,"provider_execution_id":"actual-write","provider_evidence_ref":"actual-read"}),
            );
            step(&mut sim, &format!("{name}Verify"), json!({}));
            let mut proof = json!({"operation_key":accepted["operation_key"],"expected_operation_sequence":1,"verified_resource_id":"subject","provider_evidence_ref":"actual-read","flow_evidence_ref":"actual-probe","telemetry_evidence_ref":"actual-telemetry"});
            proof[verified] = json!("wrong-target-value");
            assert!(
                sim.step(
                    "subject",
                    &format!("{name}VerificationSucceeded"),
                    &proof.to_string()
                )
                .is_err()
            );
            proof[verified] = accepted[requested].clone();
            step(&mut sim, &format!("{name}VerificationSucceeded"), proof);
            sim.assert_status("subject", "Active");
            assert!(!sim.has_violations(), "{entity}.{name}");
        }
    }
}

#[test]
fn agent_action_manifest_matches_ioa_and_has_no_retired_resource_routes() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin/specs");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(directory.join("module-contracts.json")).unwrap())
            .unwrap();
    for entry in fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|s| s == "toml") {
            let text = fs::read_to_string(path).unwrap();
            assert!(!text.contains("DsfResource"));
            assert!(!text.contains("DsfOperation"));
        }
    }
    for (file, entity) in &ENTITIES[..6] {
        let document = temper_spec::automaton::parse_automaton(&source(file)).unwrap();
        let resource = &manifest["resources"][entity];
        assert_eq!(resource["entity_set"], format!("{entity}s"));
        assert_eq!(resource["properties"]["Id"], "Edm.String");
        for action in &document.actions {
            let name = action.name.as_str();
            if let Some(operation) = name.strip_suffix("VerificationSucceeded") {
                let flag = resource["verification_flags"][operation].as_str().unwrap();
                assert!(action.effect.iter().any(|effect| matches!(effect,
                    temper_spec::automaton::Effect::Assign {
                        var,
                        op: temper_spec::predicate::AssignOp::Set,
                        value: temper_spec::predicate::Arg::Lit(temper_spec::predicate::Literal::Bool(true)),
                    } if var == flag)));
                let request = document
                    .actions
                    .iter()
                    .find(|action| action.name == operation)
                    .unwrap();
                for variable in document
                    .state
                    .iter()
                    .filter(|variable| variable.name.ends_with("_verified"))
                {
                    assert!(request.effect.iter().any(|effect|matches!(effect,
                        temper_spec::automaton::Effect::Assign {
                            var,
                            op: temper_spec::predicate::AssignOp::Set,
                            value: temper_spec::predicate::Arg::Lit(temper_spec::predicate::Literal::Bool(false)),
                        } if var == &variable.name)));
                }
            }
            let selected = [
                "RefreshObservations",
                "Deploy",
                "ApplyConfiguration",
                "Rollback",
                "SetAlias",
                "RetrySelected",
            ]
            .contains(&name)
                || name.ends_with("ResumeReconciliation")
                || name.ends_with("ResumeVerification")
                || name.ends_with("AcknowledgeFailure")
                || name.ends_with("StopExhaustedVerification")
                || name.ends_with("AbandonReconciliation");
            if selected {
                let actual = &resource["human_actions"][name];
                assert_eq!(
                    actual["params"],
                    serde_json::to_value(&action.params).unwrap(),
                    "{entity}.{name}"
                );
                assert_eq!(actual["from"], serde_json::to_value(&action.from).unwrap());
                let nonempty: Vec<_> = action
                    .constraints
                    .iter()
                    .filter_map(|constraint| match constraint {
                        temper_spec::automaton::ActionConstraint::ParamNonempty { param } => {
                            Some(param.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(actual["param_nonempty"], json!(nonempty));
            }
        }
    }
}

#[test]
fn real_collector_callback_commits_evidence_before_resource_cas() {
    use dsf_resource_collect::{Error, Host, Request, Response, Runtime};
    use sha2::{Digest, Sha256};
    struct RecordedHost(std::collections::VecDeque<Response>);
    impl Host for RecordedHost {
        fn request(&mut self, _: &Request) -> Result<Response, Error> {
            Ok(self.0.pop_front().expect("bounded collector request"))
        }
        fn secret(&mut self, _: &str) -> Result<String, Error> {
            Ok("test-token".into())
        }
    }
    let config = json!({"version":3,"resource_id":"subject","target":{"project_id":"project-1","service_id":"service-1","environment_id":"production","token_secret":"railway_token"},"verification":{"application":{"kind":"unbound"},"flow":{"kind":"provider_configuration"},"datadog":{"site":"datadoghq.com","service":"backend","environment":"production","api_key_secret":"dd_api","app_key_secret":"dd_app"}}});
    let hash = format!("{:x}", Sha256::digest(config.to_string().as_bytes()));
    for stale in [false, true] {
        let mut sim = reaction_simulator_with_registration(
            json!({"project_id":"project-1","service_id":"service-1","environment_id":"production","config_ref":"file-1","config_sha256":hash,"intended_configuration":"approved-config"}),
        );
        sim.step("resource", "RefreshObservations", "{}").unwrap();
        let captured = json!({"status":"Refreshing","refresh_sequence":1,"observed_sequence":0,"project_id":"project-1","service_id":"service-1","environment_id":"production","config_ref":"file-1","config_sha256":hash});
        let deployment = json!({"id":"actual-deployment","status":"SUCCESS","createdAt":"2026-09-06T00:00:00Z","meta":{"commitHash":"a".repeat(40)}});
        let provider = json!({"data":{"service":{"id":"service-1","projectId":"project-1"},"serviceInstance":{"id":"instance-1","serviceId":"service-1","environmentId":"production","latestDeployment":deployment,"activeDeployments":[deployment]}}});
        let mut host = RecordedHost(
            [config.clone(), provider]
                .into_iter()
                .map(|value| Response {
                    status: 200,
                    body: value.to_string(),
                })
                .collect(),
        );
        let callback = dsf_resource_collect::collect::<dsf_resource_collect::Railway>(
            &mut Runtime {
                host: &mut host,
                base: "https://temper.invalid",
                tenant: "default",
                now_ms: 2000,
            },
            "subject",
            &captured,
        )
        .unwrap();
        assert!(host.0.is_empty());
        let observation_id = callback.params["collected_observation_id"]
            .as_str()
            .unwrap();
        let ioa = source("observation");
        sim.register_actor(
            "actual-observation",
            "DsfObservation",
            observation_id,
            Box::new(
                EntityActorHandler::new(
                    "DsfObservation",
                    observation_id,
                    Arc::new(TransitionTable::from_ioa_source(&ioa)),
                )
                .with_ioa_invariants(&ioa),
            ),
        );
        if stale {
            sim.step("resource", "Observe", &observation(0, 2500).to_string())
                .unwrap();
        }
        sim.step("resource", &callback.action, &callback.params.to_string())
            .unwrap();
        sim.assert_status("actual-observation", "Measured");
        assert_eq!(sim.last_results().len(), 2);
        assert!(sim.last_results()[0].success);
        assert_eq!(sim.last_results()[1].success, !stale);
        let after = sim
            .step("resource", "Observe", &observation(1, 3000).to_string())
            .unwrap();
        assert_eq!(after["fields"]["observed_sequence"], 2);
        assert_eq!(after["fields"]["intended_configuration"], "approved-config");
    }
}

#[test]
fn model_sync_failure_callback_cannot_fail_a_newer_collection() {
    let mut sim = simulator("model_sync", "DsfModelSync", 467);
    step(
        &mut sim,
        "Configure",
        json!({"subject_type":"DsfFlow", "source_kind":"github", "source_id":"repo", "resource_id":"flow-1", "source_config_ref":"file-1", "computer_id":"computer-1"}),
    );
    step(&mut sim, "Refresh", json!({}));
    for params in [
        json!({"error_message":"host error"}),
        json!({"expected_sequence":0,"error_message":"old result"}),
    ] {
        assert!(
            sim.step("subject", "CollectionFailed", &params.to_string())
                .is_err()
        );
        sim.assert_status("subject", "Collecting");
    }
    step(
        &mut sim,
        "CollectionFailed",
        json!({"expected_sequence":1,"error_message":"current result"}),
    );
    step(&mut sim, "Refresh", json!({}));
    assert!(
        sim.step(
            "subject",
            "CollectionFailed",
            &json!({"expected_sequence":1,"error_message":"replay"}).to_string()
        )
        .is_err()
    );
    sim.assert_status("subject", "Collecting");
    step(&mut sim, "CollectionTimedOut", json!({}));
    sim.assert_status("subject", "Failed");
}

#[test]
fn only_current_verified_operation_can_satisfy_the_effort() {
    let mut sim = registered(468);
    let first = step(&mut sim, "Deploy", request(0));
    assert_eq!(first["fields"]["operation_verified"], false);
    step(&mut sim, "DeployValidationSucceeded", validation(0));
    step(&mut sim, "DeployExecute", json!({}));
    step(&mut sim, "DeployExecutionSucceeded", provider(0));
    step(&mut sim, "DeployVerify", json!({}));
    let verified = step(&mut sim, "DeployVerificationSucceeded", verification(0));
    assert_eq!(verified["fields"]["operation_verified"], true);
    assert_eq!(verified["fields"]["deploy_verified"], true);
    assert_eq!(verified["fields"]["rollback_verified"], false);
    let next = step(&mut sim, "Deploy", request(1));
    assert_eq!(next["fields"]["operation_verified"], false);
    step(
        &mut sim,
        "DeployValidationFailed",
        json!({"operation_key":"operation-1","expected_operation_sequence":2,"error_message":"validation refused"}),
    );
    let acknowledged = step(
        &mut sim,
        "DeployAcknowledgeFailure",
        json!({"operation_key":"operation-1","expected_operation_sequence":2,"failure_evidence_ref":"refusal-evidence"}),
    );
    sim.assert_status("subject", "Active");
    assert_eq!(acknowledged["fields"]["operation_verified"], false);
    assert_eq!(acknowledged["fields"]["deploy_verified"], false);
    assert_eq!(acknowledged["fields"]["verified_revision"], "revision-1");
}

#[test]
fn model_sync_schedules_due_checks_and_fences_deferred_results() {
    let ioa = temper_spec::automaton::parse_automaton(&source("model_sync")).unwrap();
    for state in ["Idle", "Ready"] {
        assert!(ioa.state_timeouts.iter().any(|timer| timer.state == state
            && timer.on_timeout == "RefreshIfDue"
            && timer.after_seconds == 60));
    }
    assert!(
        !ioa.state_timeouts
            .iter()
            .any(|timer| timer.state == "Paused")
    );
    let mut sim = simulator("model_sync", "DsfModelSync", 467);
    step(
        &mut sim,
        "Configure",
        json!({"subject_type":"DsfFlow", "source_kind":"github", "source_id":"repo", "resource_id":"flow-1", "source_config_ref":"file-1", "computer_id":"computer-1"}),
    );
    for sequence in 1..=3 {
        let row = step(&mut sim, "RefreshIfDue", json!({}));
        assert_eq!(row["fields"]["scheduled_refresh"], true);
        assert!(
            sim.step(
                "subject",
                "CollectionDeferred",
                &json!({"expected_sequence":sequence-1}).to_string()
            )
            .is_err()
        );
        step(
            &mut sim,
            "CollectionDeferred",
            json!({"expected_sequence":sequence}),
        );
        sim.assert_status("subject", "Ready");
    }
    step(&mut sim, "Pause", json!({}));
    assert!(sim.step("subject", "RefreshIfDue", "{}").is_err());
    step(&mut sim, "Resume", json!({}));
    let row = step(&mut sim, "Refresh", json!({}));
    assert_eq!(row["fields"]["scheduled_refresh"], false);
}

#[tokio::test]
async fn model_sync_timer_rearms_after_deferral_and_stops_on_pause() {
    use temper_runtime::{ActorSystem, tenant::TenantId};
    use temper_server::{
        registry::SpecRegistry,
        request_context::AgentContext,
        state::{DispatchCommand, ServerState},
    };
    let xml = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/dsf-twin/specs/model.csdl.xml"),
    )
    .unwrap();
    // Shorten only the timer. No provider module is installed in this fixture;
    // callbacks below stand in for its deferred result, not a provider success.
    let ioa = source("model_sync").replace("after_seconds = 60", "after_seconds = 1");
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("DsfModelSync", ioa.as_str())],
    );
    let state = ServerState::from_registry(ActorSystem::new("model-sync-timers"), registry);
    let tenant = TenantId::from("default".to_owned());
    let ctx = AgentContext::for_service("model-sync-timer-test");
    state
        .get_or_create_tenant_entity(&tenant, "DsfModelSync", "subject", json!({}))
        .await
        .unwrap();
    let dispatch = |action, params| {
        state.dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "DsfModelSync",
            entity_id: "subject",
            action,
            params,
            agent_ctx: &ctx,
            await_integration: false,
            await_reactions: true,
        })
    };
    assert!(dispatch("Configure", json!({"subject_type":"DsfFlow", "source_kind":"github", "source_id":"repo", "resource_id":"flow-1", "source_config_ref":"file-1", "computer_id":"computer-1"})).await.unwrap().success);
    for sequence in 1..=2 {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let row = state
            .get_tenant_entity_state(&tenant, "DsfModelSync", "subject")
            .await
            .unwrap();
        assert_eq!(row.state.status, "Collecting");
        assert!(
            dispatch("CollectionDeferred", json!({"expected_sequence":sequence}))
                .await
                .unwrap()
                .success
        );
    }
    assert!(dispatch("Pause", json!({})).await.unwrap().success);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let row = state
        .get_tenant_entity_state(&tenant, "DsfModelSync", "subject")
        .await
        .unwrap();
    assert_eq!(row.state.status, "Paused");
}
