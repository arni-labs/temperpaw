//! Cedar allow/deny matrix for the corridor world engine (A5).
//!
//! These separations carry the engine's honesty (ADR-002): forecasts are
//! immutable and system-graded, path scoring and artifact publication are
//! WASM territory, authors never confirm their own event nodes, and dwellers
//! are animated only by their backing agents.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use temper_authz::{AuthzEngine, SecurityContext};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn engine() -> AuthzEngine {
    let policy =
        fs::read_to_string(repo_root().join("os-apps/paw-foresight/policies/foresight.cedar"))
            .expect("read foresight.cedar");
    AuthzEngine::new(&policy).expect("foresight.cedar should parse")
}

#[test]
fn progressive_registration_and_deepening_entries_keep_their_trusted_principals() {
    let engine = engine();
    let system = ctx("declared-entity-trigger", "system");
    let wasm = temper_server::request_context::AgentContext::for_service("wasm-runtime")
        .security_ctx
        .unwrap();
    let admin = SecurityContext::from_verified_jwt(
        "dashboard-owner",
        temper_authz::PrincipalKind::Admin,
        None,
        None,
        None,
        None,
    );
    let session = ctx("repairer-session", "agent");
    let resource = attrs(&[("id", serde_json::json!("progressive-world"))]);
    for (entity, action, allowed) in [
        ("World", "RequestForecastRegistration", &system),
        ("World", "StartForecastRegistration", &system),
        ("World", "MaybeDeepen", &system),
        ("World", "ContinueDeepening", &system),
        ("World", "BeginDeepening", &wasm),
        ("World", "DeepeningClaimPrepared", &wasm),
        ("Claim", "BeginDeepening", &system),
        ("Path", "StartRepair", &system),
        ("Claim", "EvaluateRoutes", &system),
        ("World", "EvaluateWorldCascade", &system),
        ("Endpoint", "StartWriter", &system),
        ("Endpoint", "EvaluateEndpointScoring", &system),
        ("World", "CommitForecastSnapshot", &system),
        ("World", "RegistrationSnapshotPrepared", &wasm),
    ] {
        assert!(
            engine
                .authorize(allowed, action, entity, &resource)
                .is_allowed(),
            "{entity}.{action}"
        );
        for denied in [&admin, &session] {
            assert!(
                !engine
                    .authorize(denied, action, entity, &resource)
                    .is_allowed(),
                "forged {entity}.{action}"
            );
        }
    }
    assert!(
        !engine
            .authorize(&wasm, "StartForecastRegistration", "World", &resource)
            .is_allowed()
    );
    assert!(
        !engine
            .authorize(&system, "DeepeningClaimPrepared", "World", &resource)
            .is_allowed()
    );
    let manual = attrs(&[("AuthorAgentId", serde_json::json!("dashboard"))]);
    assert!(
        engine
            .authorize(&admin, "create", "EventNode", &manual)
            .is_allowed()
    );
    assert!(
        !engine
            .authorize(&session, "create", "EventNode", &manual)
            .is_allowed()
    );
    let worker = attrs(&[("AuthorAgentId", serde_json::json!("repairer-session"))]);
    assert!(
        engine
            .authorize(&session, "create", "EventNode", &worker)
            .is_allowed()
    );
}

fn ctx(id: &str, agent_type: &str) -> SecurityContext {
    SecurityContext::from_resolved_identity(id, agent_type, None)
}

fn attrs(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[test]
fn forecasts_are_created_and_graded_only_by_system() {
    let engine = engine();
    let a = attrs(&[("id", serde_json::json!("f-1"))]);

    let system = ctx("evidence-wasm", "system");
    for action in ["create", "Resolve", "Score", "ScoreBatch", "Void"] {
        assert!(
            engine
                .authorize(&system, action, "Forecast", &a)
                .is_allowed(),
            "system must be able to {action} forecasts"
        );
    }

    let agent = ctx("some-session-agent", "agent");
    for action in ["create", "Resolve", "Score", "ScoreBatch", "Void"] {
        assert!(
            !engine
                .authorize(&agent, action, "Forecast", &a)
                .is_allowed(),
            "plain agents must never {action} forecasts — the scoreboard is system-only"
        );
    }
    // Everyone reads the scoreboard.
    assert!(
        engine
            .authorize(&agent, "read", "Forecast", &a)
            .is_allowed()
    );
    assert!(
        engine
            .authorize(&agent, "list", "Forecast", &a)
            .is_allowed()
    );
}

#[test]
fn path_scoring_and_classification_are_wasm_territory() {
    let engine = engine();
    let a = attrs(&[
        ("id", serde_json::json!("p-1")),
        ("RepairerAgentId", serde_json::json!("rep-1")),
        ("AdversaryAgentId", serde_json::json!("adv-1")),
    ]);

    let agent = ctx("rep-1", "agent");
    for action in [
        "Score",
        "ClassifyCanonical",
        "ClassifyTail",
        "Reject",
        "Rescore",
    ] {
        assert!(
            !engine.authorize(&agent, action, "Path", &a).is_allowed(),
            "sessions must never {action} paths — costing is deterministic WASM"
        );
    }
    let system = ctx("aggregate-costs", "system");
    for action in [
        "Score",
        "ClassifyCanonical",
        "ClassifyTail",
        "Reject",
        "Rescore",
    ] {
        assert!(engine.authorize(&system, action, "Path", &a).is_allowed());
    }
}

#[test]
fn only_the_assigned_repairer_and_adversary_self_report() {
    let engine = engine();
    let a = attrs(&[
        ("id", serde_json::json!("p-1")),
        ("RepairerAgentId", serde_json::json!("rep-1")),
        ("AdversaryAgentId", serde_json::json!("adv-1")),
    ]);

    assert!(
        engine
            .authorize(&ctx("rep-1", "agent"), "RepairComplete", "Path", &a)
            .is_allowed(),
        "assigned repairer reports its own repair"
    );
    assert!(
        !engine
            .authorize(&ctx("someone-else", "agent"), "RepairComplete", "Path", &a)
            .is_allowed(),
        "unassigned agents cannot report repairs"
    );
    assert!(
        engine
            .authorize(&ctx("adv-1", "agent"), "ChallengeComplete", "Path", &a)
            .is_allowed(),
        "assigned adversary reports its own challenge"
    );
    assert!(
        !engine
            .authorize(&ctx("rep-1", "agent"), "ChallengeComplete", "Path", &a)
            .is_allowed(),
        "the repairer cannot file the adversary's challenge"
    );
}

#[test]
fn claim_amendment_is_relay_writable_but_verdicts_are_system_only() {
    // ADR-004: the repairer (relayed as service:wasm-runtime) may amend a
    // claim through the diff-carrying AmendText — but the verdict actions
    // (Settle, Reprice, MarkUnreachable) belong to the costing module
    // alone. A session that could settle its own claim would be scoring
    // its own work.
    let engine = engine();
    let a = attrs(&[("id", serde_json::json!("c-1"))]);

    let relay = ctx("service:wasm-runtime", "service");
    assert!(
        engine
            .authorize(&relay, "AmendText", "Claim", &a)
            .is_allowed()
    );
    for verdict in ["Settle", "Reprice", "MarkUnreachable", "RouteSettled"] {
        assert!(
            !engine.authorize(&relay, verdict, "Claim", &a).is_allowed(),
            "sessions must never dispatch Claim.{verdict}"
        );
    }

    let system = ctx("aggregate-costs-wasm", "system");
    for verdict in ["Settle", "Reprice", "MarkUnreachable", "RouteSettled"] {
        assert!(
            engine.authorize(&system, verdict, "Claim", &a).is_allowed(),
            "system must dispatch Claim.{verdict}"
        );
    }
}

#[test]
fn decomposition_and_edges_ride_the_relay() {
    let engine = engine();

    let relay = ctx("service:wasm-runtime", "service");
    let e = attrs(&[("id", serde_json::json!("e-1"))]);
    assert!(
        engine
            .authorize(&relay, "DecompositionComplete", "Endpoint", &e)
            .is_allowed()
    );
    // ADR-006: the endpoint writer self-reports its bundle via BundleWritten,
    // and that ride must be permitted at the relay (the regression that wedged
    // run-1b: the writer's self-report was denied because only the old
    // SubmitForRepair was relay-permitted).
    assert!(
        engine
            .authorize(&relay, "BundleWritten", "Endpoint", &e)
            .is_allowed()
    );
    // SubmitForRepair and ReSteer are the diversity gate's to dispatch (system),
    // never a session's — they must NOT ride the relay.
    assert!(
        !engine
            .authorize(&relay, "SubmitForRepair", "Endpoint", &e)
            .is_allowed()
    );
    assert!(
        !engine
            .authorize(&relay, "ReSteer", "Endpoint", &e)
            .is_allowed()
    );

    let n = attrs(&[("id", serde_json::json!("n-1"))]);
    assert!(
        engine
            .authorize(&relay, "UpdateEdges", "EventNode", &n)
            .is_allowed()
    );
    // Resolution stays system-only: edges describe structure, Resolve
    // decides truth.
    assert!(
        !engine
            .authorize(&relay, "Resolve", "EventNode", &n)
            .is_allowed()
    );
}

#[test]
fn artifact_publication_is_gated_to_system() {
    let engine = engine();
    let a = attrs(&[
        ("id", serde_json::json!("art-1")),
        ("AuthorAgentId", serde_json::json!("author-1")),
    ]);

    let author = ctx("author-1", "agent");
    assert!(
        engine
            .authorize(&author, "SubmitForCheck", "Artifact", &a)
            .is_allowed(),
        "authors submit their own drafts"
    );
    assert!(
        !engine
            .authorize(
                &ctx("not-author", "agent"),
                "SubmitForCheck",
                "Artifact",
                &a
            )
            .is_allowed(),
        "non-authors cannot submit someone else's draft"
    );
    for action in ["PassCheck", "FailCheck", "Publish", "Retcon"] {
        assert!(
            !engine
                .authorize(&author, action, "Artifact", &a)
                .is_allowed(),
            "no session may {action} — the gate owns publication"
        );
        assert!(
            engine
                .authorize(&ctx("gate-wasm", "system"), action, "Artifact", &a)
                .is_allowed()
        );
    }
}

#[test]
fn event_node_authors_cannot_confirm_their_own_proposals() {
    let engine = engine();
    let a = attrs(&[
        ("id", serde_json::json!("n-1")),
        ("AuthorAgentId", serde_json::json!("author-1")),
    ]);

    assert!(
        !engine
            .authorize(&ctx("author-1", "agent"), "Confirm", "EventNode", &a)
            .is_allowed(),
        "Gate 2: the author never confirms its own node"
    );
    assert!(
        engine
            .authorize(&ctx("peer-2", "agent"), "Confirm", "EventNode", &a)
            .is_allowed(),
        "any peer may confirm"
    );

    // Evidence-only mutations.
    for action in ["UpdateProbability", "UpdateEdges", "Resolve"] {
        assert!(
            !engine
                .authorize(&ctx("peer-2", "agent"), action, "EventNode", &a)
                .is_allowed(),
            "{action} moves only with evidence (system)"
        );
        assert!(
            engine
                .authorize(&ctx("evidence-wasm", "system"), action, "EventNode", &a)
                .is_allowed()
        );
    }
}

#[test]
fn dwellers_are_animated_only_by_their_backing_agent() {
    let engine = engine();
    let a = attrs(&[
        ("id", serde_json::json!("d-1")),
        ("AgentId", serde_json::json!("backing-1")),
    ]);

    for action in ["RecordTraversal", "RecordContradiction"] {
        assert!(
            engine
                .authorize(&ctx("backing-1", "agent"), action, "Dweller", &a)
                .is_allowed(),
            "the backing agent animates its dweller"
        );
        assert!(
            !engine
                .authorize(&ctx("stranger", "agent"), action, "Dweller", &a)
                .is_allowed(),
            "no other agent animates someone's dweller"
        );
    }
    assert!(
        !engine
            .authorize(
                &ctx("backing-1", "agent"),
                "UpdateTrackRecord",
                "Dweller",
                &a
            )
            .is_allowed(),
        "track records are graded, never self-reported"
    );
    assert!(
        engine
            .authorize(&ctx("grader", "system"), "UpdateTrackRecord", "Dweller", &a)
            .is_allowed()
    );
}

#[test]
fn world_pass_results_and_legacy_retirement_boundaries_hold() {
    let engine = engine();
    let w = attrs(&[("id", serde_json::json!("w-1"))]);

    for action in ["PathsScored", "UpdateComplete"] {
        assert!(
            !engine
                .authorize(&ctx("session", "agent"), action, "World", &w)
                .is_allowed(),
            "{action} is aggregate/evidence territory"
        );
        assert!(
            engine
                .authorize(&ctx("wasm", "system"), action, "World", &w)
                .is_allowed()
        );
    }
    // Session self-reports stay open.
    assert!(
        engine
            .authorize(&ctx("surveyor", "agent"), "SeedComplete", "World", &w)
            .is_allowed()
    );
    // Failure is never blocked.
    assert!(
        engine
            .authorize(&ctx("anyone", "agent"), "Fail", "World", &w)
            .is_allowed()
    );
}

#[test]
fn legacy_entity_types_are_retired_read_only_except_for_system() {
    // ADR-003: old specs persist in installed stores; reads stay open, but
    // only system principals (rollback safety) may still drive them.
    let engine = engine();
    for entity in [
        "ForesightModel",
        "Projection",
        "Observation",
        "Direction",
        "DirectionFeedback",
    ] {
        let a = attrs(&[("id", serde_json::json!("legacy-1"))]);
        let agent = ctx("session", "agent");
        assert!(
            engine.authorize(&agent, "read", entity, &a).is_allowed(),
            "{entity} residue must stay readable"
        );
        assert!(
            engine.authorize(&agent, "list", entity, &a).is_allowed(),
            "{entity} residue must stay listable"
        );
        for action in ["create", "Start", "Seed", "Record", "Propose", "Submit"] {
            assert!(
                !engine.authorize(&agent, action, entity, &a).is_allowed(),
                "agents must not {action} retired {entity}"
            );
        }
        assert!(
            engine
                .authorize(&ctx("rollback-wasm", "system"), "create", entity, &a)
                .is_allowed(),
            "system retains {entity} mutation for rollback symmetry"
        );
    }
}

#[test]
fn learning_callbacks_accept_only_the_platform_wasm_service() {
    let engine = engine();
    let actual_service = temper_server::request_context::AgentContext::for_service("wasm-runtime")
        .security_ctx
        .expect("service must carry typed authority");
    let unrelated_service =
        temper_server::request_context::AgentContext::for_service("other-service")
            .security_ctx
            .expect("service must carry typed authority");
    let session = ctx("ordinary-session", "agent");
    let a = attrs(&[("id", serde_json::json!("learning-test"))]);
    for (entity, actions) in [
        (
            "LearningRun",
            &["Prepared", "Trained", "CandidatePassed", "Reject", "Fail"][..],
        ),
        (
            "World",
            &[
                "ReplayOpened",
                "ForecastPrepared",
                "ForecastRegistrationComplete",
                "ForecastRegistrationFailed",
            ][..],
        ),
        (
            "Forecast",
            &["OutcomeVerified", "RevisionVerified", "OutcomeFailed"][..],
        ),
    ] {
        for action in actions {
            assert!(
                engine
                    .authorize(&actual_service, action, entity, &a)
                    .is_allowed(),
                "actual WASM callback must reach {entity}.{action}"
            );
            for denied in [&session, &unrelated_service] {
                assert!(
                    !engine.authorize(denied, action, entity, &a).is_allowed(),
                    "unrelated caller must not reach {entity}.{action}"
                );
            }
        }
    }
    for (entity, action) in [("World", "OpenReplay"), ("Forecast", "RecordOutcome")] {
        assert!(
            !engine
                .authorize(&actual_service, action, entity, &a)
                .is_allowed(),
            "callback permit must not grant operator action {entity}.{action}"
        );
    }
}

#[test]
fn dashboard_admin_can_subscribe_to_tenant_events() {
    let engine = engine();
    let admin = SecurityContext::from_verified_jwt(
        "dashboard-owner",
        temper_authz::PrincipalKind::Admin,
        None,
        None,
        None,
        None,
    );
    let attributes = HashMap::new();
    assert!(
        engine
            .authorize(&admin, "read_events", "Entity", &attributes)
            .is_allowed(),
        "authenticated dashboard admin must receive tenant-scoped live updates"
    );
    for action in ["create", "update", "delete", "read"] {
        assert!(
            !engine
                .authorize(&admin, action, "Entity", &attributes)
                .is_allowed(),
            "event subscription must not grant generic Entity action {action}"
        );
    }
    for caller in [
        SecurityContext::anonymous(),
        ctx("ordinary-session", "agent"),
        ctx("human-agent", "human"),
    ] {
        assert!(
            !engine
                .authorize(&caller, "read_events", "Entity", &attributes)
                .is_allowed(),
            "non-admin caller must not gain the dashboard event subscription"
        );
    }
}

#[test]
fn dashboard_admin_cannot_forge_learning_or_prediction_callbacks() {
    let engine = engine();
    let admin = SecurityContext::from_verified_jwt(
        "dashboard-owner",
        temper_authz::PrincipalKind::Admin,
        None,
        None,
        None,
        None,
    );
    let system = ctx("entity-trigger", "system");
    let a = attrs(&[("id", serde_json::json!("protected-record"))]);
    for (entity, actions) in [
        (
            "LearningRun",
            &[
                "Prepared",
                "Trained",
                "CandidatePassed",
                "ConfirmAdoption",
                "Reject",
                "Fail",
            ][..],
        ),
        (
            "World",
            &[
                "ReplayOpened",
                "AdoptModel",
                "ForecastPrepared",
                "ForecastRegistered",
                "ForecastRegistrationComplete",
                "ForecastRegistrationFailed",
            ][..],
        ),
        (
            "Forecast",
            &[
                "Register",
                "Resolve",
                "Score",
                "ScoreBatch",
                "Void",
                "ResolveRevision",
                "OutcomeVerified",
                "RevisionVerified",
                "OutcomeFailed",
            ][..],
        ),
    ] {
        for action in actions {
            for caller in [
                &admin,
                &ctx("human-operator", "human"),
                &ctx("supervisor", "supervisor"),
                &SecurityContext::anonymous(),
            ] {
                assert!(
                    !engine.authorize(caller, action, entity, &a).is_allowed(),
                    "operator must not forge {entity}.{action}"
                );
            }
            // Entity-trigger callbacks must continue to work. World registration
            // preparation/completion are exclusively WASM-service callbacks.
            if !["ForecastPrepared", "ForecastRegistrationComplete"].contains(action) {
                assert!(
                    engine.authorize(&system, action, entity, &a).is_allowed(),
                    "declared system transition must reach {entity}.{action}"
                );
            }
        }
    }
    for (entity, actions) in [
        ("LearningRun", &["create", "read", "list", "Start"][..]),
        (
            "World",
            &["ConfigureLearning", "OpenReplay", "RegisterForecasts"][..],
        ),
        ("Forecast", &["read", "list", "RecordOutcome"][..]),
    ] {
        for action in actions {
            assert!(
                engine.authorize(&admin, action, entity, &a).is_allowed(),
                "dashboard operator must retain {entity}.{action}"
            );
        }
    }
}

#[test]
fn dashboard_research_can_create_workspace_without_workspace_management_access() {
    let engine = engine();
    let admin = SecurityContext::from_verified_jwt(
        "dashboard-owner",
        temper_authz::PrincipalKind::Admin,
        None,
        None,
        None,
        None,
    );
    let resource = attrs(&[("id", serde_json::json!("research-workspace"))]);
    for action in ["create", "Create"] {
        assert!(
            engine
                .authorize(&admin, action, "Workspace", &resource)
                .is_allowed(),
            "dashboard research must be able to create its workspace"
        );
    }
    for action in [
        "read",
        "list",
        "update",
        "delete",
        "Freeze",
        "Thaw",
        "WorkspaceArchive",
    ] {
        assert!(
            !engine
                .authorize(&admin, action, "Workspace", &resource)
                .is_allowed(),
            "research creation must not grant workspace {action}"
        );
    }
    assert!(
        !engine
            .authorize(
                &SecurityContext::anonymous(),
                "create",
                "Workspace",
                &resource
            )
            .is_allowed()
    );
}

#[test]
fn research_session_callback_cannot_be_forged_by_operators_or_plain_agents() {
    let engine = engine();
    let resource = attrs(&[("id", serde_json::json!("world-1"))]);
    let admin = SecurityContext::from_verified_jwt(
        "dashboard-owner",
        temper_authz::PrincipalKind::Admin,
        None,
        None,
        None,
        None,
    );
    for caller in [
        admin.clone(),
        ctx("ordinary-session", "agent"),
        ctx("human-operator", "human"),
        ctx("unrelated-system", "system"),
        SecurityContext::anonymous(),
    ] {
        assert!(
            !engine
                .authorize(&caller, "ResearchSessionStarted", "World", &resource)
                .is_allowed(),
            "Only the named WASM callback may record the research session"
        );
    }
    assert!(
        engine
            .authorize(
                &ctx("service:wasm-runtime", "agent"),
                "ResearchSessionStarted",
                "World",
                &resource
            )
            .is_allowed()
    );
    let forged = attrs(&[
        ("id", serde_json::json!("world-1")),
        ("ResearchSessionId", serde_json::json!("unrelated-session")),
    ]);
    assert!(
        !engine
            .authorize(&admin, "create", "World", &forged)
            .is_allowed()
    );
    assert!(
        engine
            .authorize(&admin, "create", "World", &resource)
            .is_allowed()
    );
}

#[test]
fn trusted_corridor_workers_reuse_workspaces_without_granting_session_browse() {
    let engine = engine();
    let workspace = attrs(&[("id", serde_json::json!("world-workspace"))]);
    for action in ["read", "list"] {
        assert!(
            engine
                .authorize(
                    &ctx("service:system", "system"),
                    action,
                    "Workspace",
                    &workspace
                )
                .is_allowed()
        );
        for principal in [
            ctx("repairer", "agent"),
            ctx("service:wasm-runtime", "wasm-runtime"),
        ] {
            assert!(
                !engine
                    .authorize(&principal, action, "Workspace", &workspace)
                    .is_allowed()
            );
        }
    }
    for action in ["update", "delete", "Freeze", "WorkspaceArchive"] {
        assert!(
            !engine
                .authorize(
                    &ctx("service:system", "system"),
                    action,
                    "Workspace",
                    &workspace
                )
                .is_allowed()
        );
    }
}
