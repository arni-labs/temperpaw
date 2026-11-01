//! Project dashboard access and the paw-pm policy/metadata contract.
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::PathBuf,
};
use temper_authz::{AuthzEngine, Principal, PrincipalKind, SecurityContext};
use temper_spec::{automaton::parse_automaton, csdl::parse_csdl};

fn app_dir(app: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../os-apps")
        .join(app)
}

fn policies(app: &str) -> AuthzEngine {
    let mut files: Vec<_> = fs::read_dir(app_dir(app).join("policies"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "cedar"))
        .collect();
    files.sort();
    let text = files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n\n");
    AuthzEngine::new(&text).expect("installed app policies parse")
}

fn dashboard_admin() -> SecurityContext {
    SecurityContext {
        principal: Principal {
            id: "dashboard-admin".into(),
            kind: PrincipalKind::Admin,
            role: None,
            acting_for: None,
            agent_type: None,
            attributes: HashMap::new(),
        },
        context_attrs: HashMap::new(),
        correlation_id: "project-pm-contract".into(),
        cause: None,
        credential: None,
    }
}

fn allowed(engine: &AuthzEngine, ctx: &SecurityContext, entity: &str, action: &str) -> bool {
    engine
        .authorize(ctx, action, entity, &HashMap::new())
        .is_allowed()
}

#[test]
fn dashboard_admin_and_project_operators_can_complete_projects() {
    let engine = policies("paw-agent");
    for ctx in [
        dashboard_admin(),
        SecurityContext::from_resolved_identity("person", "human", None),
        SecurityContext::from_resolved_identity("lead", "supervisor", None),
    ] {
        for action in ["Complete", "Pause", "Resume", "Archive"] {
            assert!(
                allowed(&engine, &ctx, "Project", action),
                "{:?}: Project.{action}",
                ctx.principal
            );
        }
    }
    let agent = SecurityContext::from_resolved_identity("worker", "agent", None);
    assert!(allowed(&engine, &agent, "Project", "Update"));
    assert!(!allowed(&engine, &agent, "Project", "Complete"));
}

#[test]
fn pm_contributors_and_lifecycle_operators_keep_the_agreed_boundary() {
    let engine = policies("paw-pm");
    let agent = SecurityContext::from_resolved_identity("worker", "agent", None);
    for (entity, actions) in [
        (
            "Comment",
            &["create", "read", "list", "Create", "Edit", "React"][..],
        ),
        (
            "Cycle",
            &[
                "create",
                "read",
                "list",
                "SetProject",
                "AddIssueToCycle",
                "MarkIssueComplete",
                "RemoveIssueFromCycle",
            ][..],
        ),
        (
            "Label",
            &["create", "read", "list", "Create", "IncrementUsage"][..],
        ),
    ] {
        for action in actions {
            assert!(
                allowed(&engine, &agent, entity, action),
                "contributor: {entity}.{action}"
            );
        }
    }
    let operators = [
        dashboard_admin(),
        SecurityContext::from_resolved_identity("person", "human", None),
        SecurityContext::from_resolved_identity("lead", "supervisor", None),
    ];
    for (entity, actions) in [
        ("Comment", &["Delete"][..]),
        ("Cycle", &["Start", "Complete"][..]),
        ("Label", &["Archive"][..]),
    ] {
        for action in actions {
            assert!(
                !allowed(&engine, &agent, entity, action),
                "contributor: {entity}.{action}"
            );
            for ctx in &operators {
                assert!(
                    allowed(&engine, ctx, entity, action),
                    "operator: {entity}.{action}"
                );
            }
        }
    }
}

#[test]
fn pm_metadata_exposes_the_spec_actions_and_parameters() {
    let app = app_dir("paw-pm");
    let csdl = parse_csdl(&fs::read_to_string(app.join("specs/model.csdl.xml")).unwrap()).unwrap();
    for entity in ["Comment", "Cycle", "Label"] {
        let spec = parse_automaton(
            &fs::read_to_string(app.join(format!("specs/{}.ioa.toml", entity.to_lowercase())))
                .unwrap(),
        )
        .unwrap();
        let advertised: BTreeSet<_> = csdl
            .schemas
            .iter()
            .flat_map(|schema| &schema.actions)
            .filter(|action| {
                action.is_bound
                    && action
                        .parameters
                        .first()
                        .is_some_and(|param| param.type_name == format!("Paw.PM.{entity}"))
            })
            .map(|action| {
                (
                    action.name.clone(),
                    action
                        .parameters
                        .iter()
                        .skip(1)
                        .map(|param| param.name.clone())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect();
        let declared: BTreeSet<_> = spec
            .actions
            .iter()
            .map(|action| {
                (
                    action.name.clone(),
                    action
                        .params
                        .iter()
                        .map(|param| param.name().to_string())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect();
        assert_eq!(
            advertised, declared,
            "{entity}: public metadata must match installed actions"
        );
    }
}
