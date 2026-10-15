//! Native PatrolRun/WorkerRun investigation reactions, without a WorkCycle.
use serde_json::json;
use std::{fs, path::PathBuf};
use temper_authz::{AuthzDecision, SecurityContext};
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::SpecRegistry,
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};

#[tokio::test]
async fn native_model_investigation_creates_one_worker_and_reports_correlated_evidence() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps");
    let xml = fs::read_to_string(root.join("paw-patrol/specs/model.csdl.xml")).unwrap();
    let patrol = fs::read_to_string(root.join("paw-patrol/specs/patrol_run.ioa.toml")).unwrap();
    let worker = fs::read_to_string(root.join("paw-patrol/specs/worker_run.ioa.toml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("PatrolRun", &patrol), ("WorkerRun", &worker)],
    );
    let state = ServerState::from_registry(ActorSystem::new("model-investigation"), registry);
    let policy = format!(
        "{}\n{}",
        fs::read_to_string(root.join("paw-patrol/policies/patrol.cedar")).unwrap(),
        fs::read_to_string(root.join("dsf-twin/policies/model_investigation.cedar")).unwrap()
    );
    state
        .authz
        .reload_tenant_policies("default", &policy)
        .unwrap();
    let tenant = TenantId::default();
    let ctx = AgentContext {
        security_ctx: Some(SecurityContext::from_resolved_identity(
            "worker-1", "worker", None,
        )),
        agent_id: Some("worker-1".into()),
        agent_type: Some("worker".into()),
        ..Default::default()
    };
    let dispatch = |entity, id, action, params| {
        state.dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: entity,
            entity_id: id,
            action,
            params,
            agent_ctx: &ctx,
            await_integration: false,
            await_reactions: true,
        })
    };
    state
        .get_or_create_tenant_entity(&tenant, "PatrolRun", "investigation", json!({}))
        .await
        .unwrap();
    let params = json!({"investigation_key":"investigation","observation_id":"observation-1","source_evidence":"{\"outcome\":\"different\"}","worker_run_id":"worker-run-1","task":"model task","branch_name":"codex/model","worktree_path":"/tmp/model","allowed_worker_id":"worker-1","provider_id":"local-codex","runner_kind":"local_codex","requested_by":"worker-1"});
    let response = dispatch(
        "PatrolRun",
        "investigation",
        "RequestModelInvestigation",
        params.clone(),
    )
    .await
    .unwrap();
    assert!(response.success, "{:?}", response.error);
    // Simulate a lost native reaction after the source transition committed.
    assert!(state.list_entity_ids(&tenant, "WorkerRun").is_empty());
    state.rebuild_reaction_dispatcher();
    assert!(
        dispatch(
            "PatrolRun",
            "investigation",
            "ReconcileModelWorker",
            json!({})
        )
        .await
        .unwrap()
        .success
    );
    assert_eq!(
        state.list_entity_ids(&tenant, "WorkerRun"),
        vec!["worker-run-1"]
    );
    assert!(state.list_entity_ids(&tenant, "WorkCycle").is_empty());
    assert!(
        !dispatch(
            "PatrolRun",
            "investigation",
            "RequestModelInvestigation",
            params
        )
        .await
        .unwrap()
        .success
    );
    let row = state
        .get_tenant_entity_state(&tenant, "WorkerRun", "worker-run-1")
        .await
        .unwrap();
    assert_eq!(row.state.fields["patrol_run_id"], "investigation");
    assert_eq!(row.state.fields["model_investigation"], true);
    assert!(
        dispatch(
            "WorkerRun",
            "worker-run-1",
            "Claim",
            json!({"worker_id":"worker-1"})
        )
        .await
        .unwrap()
        .success
    );
    // The existing StartLocal module has no provider call for a run without a FactoryCase.
    let module =
        fs::read(root.join("paw-patrol/wasm/worker_run_lifecycle/worker_run_lifecycle.wasm"))
            .unwrap();
    let hash = state.wasm_engine.compile_and_cache(&module).unwrap();
    state
        .wasm_module_registry
        .write()
        .unwrap()
        .register(&tenant, "worker_run_lifecycle", &hash);
    assert!(
        dispatch("WorkerRun", "worker-run-1", "StartLocal", json!({}))
            .await
            .unwrap()
            .success
    );
    assert!(
        !dispatch(
            "PatrolRun",
            "investigation",
            "StartModelInvestigation",
            json!({"expected_worker_run_id":"other"})
        )
        .await
        .unwrap()
        .success
    );
    assert!(
        dispatch(
            "PatrolRun",
            "investigation",
            "StartModelInvestigation",
            json!({"expected_worker_run_id":"worker-run-1"})
        )
        .await
        .unwrap()
        .success
    );
    assert!(dispatch("WorkerRun","worker-run-1","ReportInvestigation",json!({"result_summary":"model updated","evidence_json":"{\"observation_id\":\"observation-1\"}"})).await.unwrap().success);
    let actual = state
        .get_tenant_entity_state(&tenant, "PatrolRun", "investigation")
        .await
        .unwrap();
    assert_eq!(actual.state.status, "Complete");
    assert_eq!(actual.state.fields["summary"], "model updated");
    assert_eq!(state.list_entity_ids(&tenant, "WorkerRun").len(), 1);
    let factory = SecurityContext::from_resolved_identity("agent", "dsf-factory", None);
    assert!(matches!(
        state.authz.authorize_for_tenant(
            "default",
            &factory,
            "CompleteModelInvestigation",
            "PatrolRun",
            &Default::default()
        ),
        AuthzDecision::Deny(_)
    ));
}

#[test]
fn installed_bundle_includes_investigation_policy_and_retains_provider_guards() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps");
    temper_platform::os_apps::add_os_apps_dir_preferred(root);
    let bundle = temper_platform::os_apps::get_os_app("dsf-twin").unwrap();
    assert!(
        bundle
            .cedar_policy_sources
            .iter()
            .any(|source| source.relative_path == "policies/model_investigation.cedar")
    );
    assert!(
        bundle
            .cedar_policy_sources
            .iter()
            .any(|source| source.relative_path == "policies/factory.cedar"
                && source.text.contains("access_secret"))
    );
}

#[test]
fn investigation_policy_refuses_forged_results_and_reassignment_even_with_ambient_permit() {
    use std::collections::HashMap;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin");
    let policy = fs::read_to_string(root.join("policies/model_investigation.cedar")).unwrap();
    let engine =
        temper_authz::AuthzEngine::new(&format!("{policy}\npermit(principal,action,resource);"))
            .unwrap();
    let fields = HashMap::from([
        ("worker_id".into(), json!("owner")),
        ("model_investigation".into(), json!(true)),
    ]);
    for identity in [
        SecurityContext::from_resolved_identity("agent", "dsf-factory", None),
        SecurityContext::from_resolved_identity("other", "worker", None),
    ] {
        for action in [
            "ReportInvestigation",
            "FailInvestigation",
            "ReplayInvestigationResult",
            "ReplayInvestigationFailure",
            "Configure",
        ] {
            assert!(
                matches!(
                    engine.authorize(&identity, action, "WorkerRun", &fields),
                    AuthzDecision::Deny(_)
                ),
                "{action}"
            );
        }
    }
    let owner = SecurityContext::from_resolved_identity("owner", "worker", None);
    assert!(matches!(
        engine.authorize(&owner, "ReportInvestigation", "WorkerRun", &fields),
        AuthzDecision::Allow { .. }
    ));
    assert!(matches!(
        engine.authorize(&owner, "Configure", "WorkerRun", &fields),
        AuthzDecision::Deny(_)
    ));
}

#[test]
fn investigation_actor_preserves_assignment_across_reordered_and_replayed_callbacks() {
    use std::sync::Arc;
    use temper_jit::table::TransitionTable;
    use temper_runtime::scheduler::{FaultConfig, SimActorSystem, SimActorSystemConfig};
    use temper_server::entity_actor::sim_handler::EntityActorHandler;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-patrol/specs");
    let ioa = fs::read_to_string(root.join("patrol_run.ioa.toml")).unwrap();
    for seed in 467..499 {
        let mut sim = SimActorSystem::new(SimActorSystemConfig {
            seed,
            faults: FaultConfig::none(),
            ..Default::default()
        });
        sim.register_actor(
            "subject",
            Box::new(
                EntityActorHandler::new(
                    "PatrolRun",
                    "subject",
                    Arc::new(TransitionTable::from_ioa_source(&ioa)),
                )
                .with_ioa_invariants(&ioa),
            ),
        );
        let mut step = |action: &str, params: serde_json::Value| {
            sim.step("subject", action, &params.to_string())
        };
        let requested = step(
            "RequestModelInvestigation",
            json!({"investigation_key":"subject","observation_id":"obs","source_evidence":"{}","worker_run_id":"worker-one"}),
        );
        assert_eq!(requested.unwrap()["status"], "Queued");
        for index in 0..8 {
            let action = if (index + seed) % 2 == 0 {
                "StartModelInvestigation"
            } else {
                "CompleteModelInvestigation"
            };
            let refused = step(
                action,
                json!({"expected_worker_run_id":"different","summary":"forged","evidence_json":"{}"}),
            );
            assert!(refused.is_err());
        }
        let duplicate = step(
            "RequestModelInvestigation",
            json!({"investigation_key":"subject","observation_id":"changed","source_evidence":"changed","worker_run_id":"other"}),
        );
        assert!(duplicate.is_err());
        let unchanged = step("ReconcileModelWorker", json!({})).unwrap();
        assert_eq!(unchanged["fields"]["worker_run_id"], "worker-one");
        assert_eq!(unchanged["fields"]["observation_id"], "obs");
        assert_eq!(
            step(
                "StartModelInvestigation",
                json!({"expected_worker_run_id":"worker-one"})
            )
            .unwrap()["status"],
            "Running"
        );
        assert_eq!(
            step(
                "CompleteModelInvestigation",
                json!({"expected_worker_run_id":"worker-one","summary":"checked","evidence_json":"{}"})
            ).unwrap()["status"],
            "Complete"
        );
        let replay = step(
            "CompleteModelInvestigation",
            json!({"expected_worker_run_id":"worker-one","summary":"overwritten","evidence_json":"changed"}),
        );
        assert!(replay.is_err());
        sim.assert_status("subject", "Complete");
        assert!(!sim.has_violations());
    }
}
