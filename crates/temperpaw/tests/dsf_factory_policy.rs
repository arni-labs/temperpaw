//! Authorization uses the installed Cedar policy and actual platform identities.
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::PathBuf};
use temper_authz::{AuthzDecision, AuthzEngine, SecurityContext};
use temper_server::request_context::AgentContext;
fn app() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin")
}
fn policy(extra: &str) -> AuthzEngine {
    AuthzEngine::new(&format!(
        "{}\n{extra}",
        fs::read_to_string(app().join("policies/factory.cedar")).expect("generated factory policy")
    ))
    .unwrap()
}
fn allowed(engine: &AuthzEngine, ctx: &SecurityContext, entity: &str, action: &str) -> bool {
    matches!(
        engine.authorize(
            ctx,
            action,
            entity,
            &HashMap::from([("id".into(), json!("resource-1"))])
        ),
        AuthzDecision::Allow { .. }
    )
}
fn service(name: &str) -> SecurityContext {
    AgentContext::for_service(name).security_ctx.unwrap()
}
#[test]
fn exhausted_verification_recovery_is_an_operator_command_not_a_callback() {
    let engine = policy("");
    for kind in ["human", "dsf-factory"] {
        let member = SecurityContext::from_resolved_identity("member", kind, None);
        assert!(allowed(
            &engine,
            &member,
            "DsfRailwayServiceInstance",
            "DeployStopExhaustedVerification"
        ));
        assert!(allowed(
            &engine,
            &member,
            "DsfRailwayServiceInstance",
            "DeployAbandonReconciliation"
        ));
        assert!(!allowed(
            &engine,
            &member,
            "DsfRailwayServiceInstance",
            "DeployVerificationFailed"
        ));
        assert!(!allowed(
            &engine,
            &member,
            "DsfRailwayServiceInstance",
            "DeployVerificationSucceeded"
        ));
    }
    let other = SecurityContext::from_resolved_identity("other", "other-agent", None);
    assert!(!allowed(
        &engine,
        &other,
        "DsfRailwayServiceInstance",
        "DeployStopExhaustedVerification"
    ));
}

#[test]
fn resident_agents_can_raise_questions_but_cannot_answer_them() {
    let text = fs::read_to_string(app().join("policies/resident_asks.cedar")).unwrap();
    let engine = AuthzEngine::new(&text).unwrap();
    let agent = SecurityContext::from_resolved_identity("factory", "dsf-factory", None);
    for action in ["create", "Raise", "RaiseBlocking", "RecordFyi", "Withdraw"] {
        assert!(allowed(&engine, &agent, "Ask", action), "{action}");
    }
    assert!(!allowed(&engine, &agent, "Ask", "Answer"));
    let ambient =
        AuthzEngine::new(&format!("{text}\npermit(principal, action, resource);")).unwrap();
    assert!(!allowed(&ambient, &agent, "Ask", "Answer"));
    let spoof =
        SecurityContext::anonymous().with_agent_context(Some("spoof"), None, Some("dsf-factory"));
    assert!(!allowed(&engine, &spoof, "Ask", "create"));
}

#[test]
fn verified_operator_can_answer_asks_without_other_factory_powers() {
    let text = fs::read_to_string(app().join("../paw-patrol/policies/patrol.cedar")).unwrap();
    let resident = fs::read_to_string(app().join("policies/resident_asks.cedar")).unwrap();
    let engine = AuthzEngine::new(&format!("{text}\n{resident}")).unwrap();
    let operator = SecurityContext::from_resolved_identity("operator", "operator", None);
    assert!(allowed(&engine, &operator, "Ask", "Answer"));
    for (entity, action) in [
        ("Ask", "create"),
        ("Ask", "RaiseBlocking"),
        ("Ask", "Withdraw"),
        ("Ask", "update"),
        ("Ask", "delete"),
        ("Effort", "MarkDeployVerified"),
    ] {
        assert!(
            !allowed(&engine, &operator, entity, action),
            "{entity}.{action}"
        );
    }
    for ctx in [
        SecurityContext::anonymous(),
        SecurityContext::anonymous().with_agent_context(Some("operator"), None, Some("operator")),
        SecurityContext::from_resolved_identity("other", "operator", None),
        SecurityContext::from_resolved_identity("factory", "dsf-factory", None),
    ] {
        assert!(!allowed(&engine, &ctx, "Ask", "Answer"));
    }
}
#[test]
fn registered_members_can_request_every_declared_command_but_not_forge_callbacks() {
    let engine = policy("");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(app().join("specs/module-contracts.json")).unwrap(),
    )
    .unwrap();
    for name in ["dsf-factory", "human"] {
        let ctx = SecurityContext::from_resolved_identity("member", name, None);
        for (entity, r) in manifest["resources"].as_object().unwrap() {
            for action in r["human_actions"]
                .as_object()
                .unwrap()
                .keys()
                .chain(["Register".to_string(), "Retire".to_string()].iter())
            {
                assert!(
                    allowed(&engine, &ctx, entity, action),
                    "{name} {entity}.{action}"
                );
            }
            for action in [
                "Observe",
                "CollectionSucceeded",
                "DeployVerificationSucceeded",
            ] {
                assert!(
                    !allowed(&engine, &ctx, entity, action),
                    "{name} forged {entity}.{action}"
                );
            }
        }
        assert!(!allowed(&engine, &ctx, "DsfObservation", "create"));
        assert!(!allowed(&engine, &ctx, "DsfObservation", "RecordMeasured"));
        assert!(!allowed(&engine, &ctx, "Exec", "Run"));
    }
}
#[test]
fn unregistered_headers_other_agents_and_ambient_permits_cannot_forge_evidence() {
    let engine = policy("permit(principal, action, resource);");
    for ctx in [
        SecurityContext::anonymous().with_agent_context(Some("spoof"), None, Some("wasm-runtime")),
        SecurityContext::from_resolved_identity("other", "other-agent", None),
        SecurityContext::from_resolved_identity("member", "dsf-factory", None),
    ] {
        for (entity, action) in [
            ("DsfObservation", "RecordMeasured"),
            ("DsfObservation", "create"),
            ("DsfVercelProject", "DeployVerificationSucceeded"),
            ("DsfModelSync", "CollectionSucceeded"),
        ] {
            assert!(!allowed(&engine, &ctx, entity, action), "{entity}.{action}");
        }
    }
    let engine = policy("");
    let spoof =
        SecurityContext::anonymous().with_agent_context(Some("spoof"), None, Some("dsf-factory"));
    assert!(!allowed(&engine, &spoof, "DsfVercelProject", "Deploy"));
}
#[test]
fn runtime_callbacks_and_timers_are_allowed_but_exec_requires_declared_experiment_trigger() {
    let engine = policy("");
    for ctx in [service("wasm-runtime"), service("dsf-factory-runtime")] {
        assert!(allowed(
            &engine,
            &ctx,
            "DsfVercelProject",
            "DeployVerificationSucceeded"
        ));
        assert!(allowed(&engine, &ctx, "DsfObservation", "RecordMeasured"));
    }
    assert!(allowed(
        &engine,
        &service("timeout-scheduler"),
        "DsfVercelProject",
        "RefreshObservations"
    ));
    let mut ctx = service("dsf-factory-runtime");
    assert!(!allowed(&engine, &ctx, "Exec", "Run"));
    ctx.context_attrs
        .insert("triggerDeclaredPrincipal".into(), json!(true));
    ctx.context_attrs
        .insert("triggerSourceEntityType".into(), json!("DsfExperiment"));
    assert!(allowed(&engine, &ctx, "Exec", "Run"));
    assert!(allowed(&engine, &ctx, "Exec", "create"));
    assert!(!allowed(&engine, &ctx, "Exec", "Delete"));
}
#[test]
fn policy_generator_matches_the_current_declared_actions() {
    assert!(
        std::process::Command::new("python3")
            .arg(app().join("policies/generate.py"))
            .arg("--check")
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn internal_record_authority_is_not_a_guest_secret_or_spec_template() {
    let engine = policy("permit(principal, action, resource);");
    let mut ctx = SecurityContext::from_resolved_identity("member", "dsf-factory", None);
    ctx.principal.role = Some("wasm_module".into());
    let manifest = fs::read_to_string(app().join("app.toml")).unwrap();
    let mut modules = 0;
    for line in manifest.lines() {
        let Some(module) = line.trim().strip_prefix("name = \"dsf_") else {
            continue;
        };
        modules += 1;
        let module = format!("dsf_{}", module.trim_end_matches('"'));
        ctx.context_attrs.insert("module".into(), json!(module));
        for key in ["temper_api_key", "temper_api_url"] {
            assert!(
                matches!(
                    engine.authorize(
                        &ctx,
                        "access_secret",
                        "Secret",
                        &HashMap::from([("id".into(), json!(key))])
                    ),
                    AuthzDecision::Deny(_)
                ),
                "{module}: {key}"
            );
        }
    }
    assert!(modules >= 55, "all packaged DSF modules must be checked");
    for entry in fs::read_dir(app().join("specs")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            let source = fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("{secret:temper_api_"),
                "{}",
                path.display()
            );
        }
    }
}
#[test]
fn compiled_modules_receive_only_their_named_secrets_and_callers_receive_none() {
    let engine = policy("");
    let mut ctx = SecurityContext::from_resolved_identity("member", "dsf-factory", None);
    let secret = |ctx: &SecurityContext, name: &str| {
        matches!(
            engine.authorize(
                ctx,
                "access_secret",
                "Secret",
                &HashMap::from([("id".into(), json!(name))])
            ),
            AuthzDecision::Allow { .. }
        )
    };
    assert!(!secret(&ctx, "dsf_railway_token"));
    ctx.principal.role = Some("wasm_module".into());
    ctx.context_attrs
        .insert("module".into(), json!("dsf_railway_deploy_verify"));
    assert!(secret(&ctx, "dsf_railway_token"));
    assert!(secret(&ctx, "dsf_datadog_api_key"));
    assert!(!secret(&ctx, "temper_api_key"));
    assert!(!secret(&ctx, "github_token"));
    assert!(!secret(&ctx, "dsf_vercel_token"));
    assert!(!secret(&ctx, "unrelated_production_password"));
    ctx.context_attrs
        .insert("module".into(), json!("dsf_railway_collect"));
    assert!(!secret(&ctx, "dsf_datadog_api_key"));
    ctx.context_attrs
        .insert("module".into(), json!("dsf_model_collect"));
    assert!(secret(&ctx, "dsf_github_token"));
    assert!(!secret(&ctx, "dsf_railway_token"));
    for provider in ["supabase", "r2", "datadog"] {
        ctx.context_attrs.insert(
            "module".into(),
            json!(format!("dsf_{provider}_configuration_verify")),
        );
        assert!(secret(&ctx, "dsf_railway_token"));
        for phase in ["validate", "execute", "reconcile"] {
            ctx.context_attrs.insert(
                "module".into(),
                json!(format!("dsf_{provider}_configuration_{phase}")),
            );
            assert!(!secret(&ctx, "dsf_railway_token"));
        }
    }
}

#[test]
fn actual_wasm_authorization_adapter_uses_method_context_and_secret_id() {
    use std::sync::Arc;
    use temper_server::authz::CedarWasmAuthzGate;
    use temper_wasm::{WasmAuthzContext, WasmAuthzDecision, WasmAuthzGate};
    let paw = fs::read_to_string(app().join("../paw-patrol/policies/patrol.cedar")).unwrap();
    let text = format!(
        "{}\n{paw}",
        fs::read_to_string(app().join("policies/factory.cedar")).unwrap()
    );
    let engine = Arc::new(AuthzEngine::new("").unwrap());
    engine.reload_tenant_policies("default", &text).unwrap();
    let gate = CedarWasmAuthzGate::new(engine);
    let ctx = WasmAuthzContext {
        tenant: "default".into(),
        module_name: "dsf_media_retry_selected_verify".into(),
        agent_id: Some("member".into()),
        session_id: None,
        entity_type: "DsfMediaPipeline".into(),
        trigger_action: "VerifyRetry".into(),
    };
    for name in [
        "dsf_admin_api_key",
        "dsf_railway_token",
        "dsf_cloudflare_token",
        "dsf_datadog_api_key",
    ] {
        assert!(
            matches!(
                gate.authorize_secret_access(name, &ctx),
                WasmAuthzDecision::Allow
            ),
            "{name}"
        );
    }
    assert!(matches!(
        gate.authorize_secret_access("github_token", &ctx),
        WasmAuthzDecision::Deny(_)
    ));
    assert!(matches!(
        gate.authorize_secret_access("dsf_vercel_token", &ctx),
        WasmAuthzDecision::Deny(_)
    ));
}

#[test]
fn model_due_checks_and_deferred_callbacks_are_runtime_only() {
    let engine = policy("permit(principal, action, resource);");
    let agent = SecurityContext::from_resolved_identity("factory", "dsf-factory", None);
    for action in ["RefreshIfDue", "CollectionDeferred"] {
        assert!(!allowed(&engine, &agent, "DsfModelSync", action));
        assert!(allowed(
            &engine,
            &service("wasm-runtime"),
            "DsfModelSync",
            action
        ));
        assert!(allowed(
            &engine,
            &service("timeout-scheduler"),
            "DsfModelSync",
            action
        ));
    }
}
