use super::*;
use dsf_resource_common::{Request, Response};
use std::collections::BTreeMap;

fn fixture() -> (Invocation, Value, String) {
    let manifest = json!({"version":1,"experiment_id":"variant-a","effort_id":"effort-1","computer_id":"arni-big",
        "branch":"codex/arn467-variant-a","source_revision":"a".repeat(40),"runner_sha256":"a".repeat(64),"database_id":"dsf_variant_a",
        "media_bucket":"dsf-variant-a","media_namespace":"experiments/variant-a/","permitted_external_calls":[],
        "cors_origin":"https://variant-a.invalid","production_database_id":"production-project","production_media_bucket":"production-media"});
    let raw = manifest.to_string();
    let mut state = manifest.clone();
    state["permitted_external_calls"] = json!("[]");
    state["manifest_ref"] = json!("manifest-1");
    state["manifest_sha256"] = json!(digest(raw.as_bytes()));
    state["operation_sequence"] = json!(1);
    state["status"] = json!("ValidationPreparing");
    (
        Invocation::parse("variant-a", &state, Phase::Validate).unwrap(),
        manifest,
        raw,
    )
}

#[test]
fn exact_manifest_and_production_binding_checks() {
    let (inv, manifest, raw) = fixture();
    assert!(validate_manifest(&inv, &raw).is_ok());
    assert!(validate_manifest(&inv, &(raw.clone() + " ")).is_err());
    for name in [
        "effort_id",
        "branch",
        "source_revision",
        "database_id",
        "media_bucket",
        "computer_id",
    ] {
        let mut changed = manifest.clone();
        changed[name] = json!("another");
        let mut bound = Invocation::parse(&inv.id, &inv.state, Phase::Validate).unwrap();
        let raw = changed.to_string();
        bound.state["manifest_sha256"] = json!(digest(raw.as_bytes()));
        assert!(validate_manifest(&bound, &raw).is_err(), "{name}");
    }
}

#[test]
fn retry_keeps_exec_identity_but_other_sequence_and_variant_cannot_share_it() {
    let (mut inv, _, _) = fixture();
    let original = execution_id(&inv, Phase::Validate);
    assert_eq!(original, execution_id(&inv, Phase::Validate));
    assert_ne!(original, execution_id(&inv, Phase::Run));
    inv.sequence += 1;
    assert_ne!(original, execution_id(&inv, Phase::Validate));
    inv.sequence -= 1;
    inv.id = "variant-b".into();
    assert_ne!(original, execution_id(&inv, Phase::Validate));
}

#[test]
fn unpinned_runner_is_refused_and_shell_content_stays_encoded() {
    assert!(command(Phase::Run, "{}", "main").is_err());
    let command = command(Phase::Run, "'$(touch /bad)'", &"a".repeat(64)).unwrap();
    assert!(!command.contains("touch /bad"));
    assert!(command.contains("unshare --net -- env -i"));
    assert!(command.contains("hashlib.sha256"));
}

#[test]
fn receipts_cannot_cross_phase_or_invocation_and_need_actual_isolation() {
    let (inv, manifest, _) = fixture();
    let mut proof = manifest.clone();
    for (key,value) in json!({"manifest_sha256":digest(manifest.to_string().as_bytes()),"phase":"validate","outcome":"passed",
        "database_system_identifier":"998221","database_oid":"16442","pgvector_version":"0.6.0","network_interfaces":["lo"],"external_routes":[],"external_calls":[]}).as_object().unwrap() { proof[key]=value.clone(); }
    let callback = receipt(
        &inv,
        Phase::Validate,
        &manifest,
        &proof.to_string(),
        "exec-1",
    )
    .unwrap();
    assert_eq!(callback.action, "IsolationSucceeded");
    assert_eq!(callback.params["expected_sequence"], 1);
    assert!(receipt(&inv, Phase::Run, &manifest, &proof.to_string(), "exec-1").is_err());
    for (key, value) in [
        ("experiment_id", json!("variant-b")),
        ("network_interfaces", json!(["lo", "eth0"])),
        ("database_oid", json!("")),
        ("source_revision", json!("b".repeat(40))),
    ] {
        let mut altered = proof.clone();
        altered[key] = value;
        assert!(
            receipt(
                &inv,
                Phase::Validate,
                &manifest,
                &altered.to_string(),
                "exec-1"
            )
            .is_err(),
            "{key}"
        );
    }
}

struct HostFixture(BTreeMap<String, Value>);
impl Host for HostFixture {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        assert_eq!(
            request.method, "GET",
            "experiment integration never writes or dispatches"
        );
        Ok(Response {
            status: 200,
            body: self.0.get(&request.url).expect(&request.url).to_string(),
        })
    }
    fn secret(&mut self, _: &str) -> Result<String, Error> {
        panic!("selection needs no provider credential")
    }
}

#[test]
fn resume_reads_existing_execution_and_deadline_becomes_uncertain() {
    let (mut inv, manifest, raw) = fixture();
    let id = execution_id(&inv, Phase::Validate);
    let archive = "a".repeat(64);
    let expected_command = command(Phase::Validate, &raw, &archive).unwrap();
    let mut host = HostFixture(BTreeMap::from([
        (
            "https://temper.invalid/tdata/DsfExperiments('variant-a')".into(),
            inv.state.clone(),
        ),
        (
            "https://temper.invalid/tdata/Files('manifest-1')/$value".into(),
            manifest,
        ),
        (
            format!("https://temper.invalid/tdata/Execs('{id}')"),
            json!({"Status":"Running","ComputerId":"arni-big","Command":expected_command}),
        ),
    ]));
    let result = execute(
        &mut Runtime {
            host: &mut host,
            base: "https://temper.invalid",
            tenant: "default",
            now_ms: 100,
        },
        &inv,
        Phase::Validate,
    )
    .unwrap();
    assert_eq!(result.action, "ValidationReconciled");
    assert_eq!(result.params["exec_id"], id);
    inv.state["status"] = json!("Validating");
    inv.state["exec_id"] = json!(id);
    inv.state["phase_deadline_ms"] = json!("100");
    host.0.insert(
        "https://temper.invalid/tdata/DsfExperiments('variant-a')".into(),
        inv.state.clone(),
    );
    let result = execute(
        &mut Runtime {
            host: &mut host,
            base: "https://temper.invalid",
            tenant: "default",
            now_ms: 100,
        },
        &inv,
        Phase::Validate,
    );
    assert!(matches!(result, Err(Error::Pending(_))));
}
#[test]
fn the_selection_phase_is_fenced_by_its_own_counter() {
    let (inv, _, _) = fixture();
    let mut state = inv.state.clone();
    state["status"] = json!("Selecting");
    state["selection_ask_id"] = json!("ask-1");
    state["delivery_effort_id"] = json!("delivery-1");
    // operation_sequence is 1 from the fixture; the Exec phases already spent it.
    state["selection_sequence"] = json!(3);
    let selecting = Invocation::parse("variant-a", &state, Phase::Select).unwrap();
    assert_eq!(selecting.sequence, 3);
    assert_eq!(
        selecting.callback("SelectionSucceeded", json!({})).params["expected_sequence"],
        json!(3)
    );
    // The Exec phases keep reading the shared counter.
    assert_eq!(
        Invocation::parse("variant-a", &state, Phase::Run)
            .unwrap()
            .sequence,
        1
    );
    // A selection that has not started yet carries no fence and cannot bind.
    let mut unstarted = state.clone();
    unstarted["selection_sequence"] = json!(0);
    assert!(Invocation::parse("variant-a", &unstarted, Phase::Select).is_err());
}

#[test]
fn selection_requires_answered_same_effort_choice_and_accepted_delivery() {
    let (mut inv, _, _) = fixture();
    inv.state["status"] = json!("Selecting");
    inv.state["selection_ask_id"] = json!("ask-1");
    inv.state["delivery_effort_id"] = json!("delivery-1");
    for fault in [
        "none",
        "open",
        "other_effort",
        "other_choice",
        "unaccepted",
        "other_delivery",
    ] {
        let mut ask = json!({"Status":"Answered","EffortId":"effort-1","Chose":"variant-a"});
        let mut intent = json!({"Status":"Accepted","EffortId":"delivery-1"});
        match fault {
            "open" => ask["Status"] = json!("Open"),
            "other_effort" => ask["EffortId"] = json!("other"),
            "other_choice" => ask["Chose"] = json!("variant-b"),
            "unaccepted" => intent["Status"] = json!("Open"),
            "other_delivery" => intent["EffortId"] = json!("other"),
            _ => {}
        }
        let mut host = HostFixture(BTreeMap::from([
            ("https://temper.invalid/tdata/Asks('ask-1')".into(), ask),
            (
                "https://temper.invalid/tdata/Efforts('delivery-1')".into(),
                json!({"IntentId":"intent-1"}),
            ),
            (
                "https://temper.invalid/tdata/Intents('intent-1')".into(),
                intent,
            ),
        ]));
        let result = select(
            &mut Runtime {
                host: &mut host,
                base: "https://temper.invalid",
                tenant: "default",
                now_ms: 0,
            },
            &inv,
        );
        assert_eq!(result.is_ok(), fault == "none", "{fault}");
    }
}
