use super::*;

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
