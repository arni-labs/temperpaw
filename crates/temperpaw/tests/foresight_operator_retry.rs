use temper_authz::{AuthzEngine, SecurityContext};
#[test]
fn operator_retry_is_verified_and_machine_callbacks_stay_closed() {
    let policy = include_str!("../../../os-apps/paw-foresight/policies/foresight.cedar");
    let engine = AuthzEngine::new(policy).unwrap();
    let operator = SecurityContext::from_resolved_identity("operator", "operator", None);
    let ordinary = SecurityContext::from_resolved_identity("session", "agent", None);
    let mut unverified = operator.clone();
    unverified
        .principal
        .attributes
        .insert("agentTypeVerified".into(), false.into());
    let attrs = std::collections::HashMap::new();
    for action in ["create", "Start"] {
        assert!(
            engine
                .authorize(&operator, action, "SemanticRun", &attrs)
                .is_allowed(),
            "verified {action}"
        );
        for principal in [&ordinary, &unverified] {
            assert!(
                !engine
                    .authorize(principal, action, "SemanticRun", &attrs)
                    .is_allowed(),
                "unverified {action}"
            );
        }
    }
    for action in [
        "Cancel",
        "Fail",
        "Complete",
        "Recorded",
        "Evaluate",
        "CheckReasoning",
        "SpawnReasoning",
        "ReasoningComplete",
        "ReasoningRetry",
        "ResumePrepared",
        "Expanded",
    ] {
        assert!(
            !engine
                .authorize(&operator, action, "SemanticRun", &attrs)
                .is_allowed(),
            "must remain denied {action}"
        );
    }
    assert!(
        !engine
            .authorize(&operator, "StartSeed", "World", &attrs)
            .is_allowed()
    );
}
