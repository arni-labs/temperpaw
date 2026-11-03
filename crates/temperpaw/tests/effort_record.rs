//! Exercise the task record through the real entity actor; completion has no deployment side effects.
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};
use temper_jit::table::TransitionTable;
use temper_runtime::scheduler::{FaultConfig, SimActorSystem, SimActorSystemConfig};
use temper_server::entity_actor::sim_handler::EntityActorHandler;

fn source() -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-patrol/specs/effort.ioa.toml"),
    )
    .unwrap()
}
fn record(initial: &str) -> SimActorSystem {
    // Old persisted states use the same transition table as new records.
    let ioa = source().replace("initial = \"Open\"", &format!("initial = \"{initial}\""));
    let handler = EntityActorHandler::new(
        "Effort",
        "effort",
        Arc::new(TransitionTable::from_ioa_source(&ioa)),
    )
    .with_ioa_invariants(&ioa);
    let mut sim = SimActorSystem::new(SimActorSystemConfig {
        seed: 467,
        faults: FaultConfig::none(),
        ..Default::default()
    });
    sim.register_actor("effort", Box::new(handler));
    sim
}
fn step(sim: &mut SimActorSystem, name: &str, params: Value) -> Value {
    sim.step("effort", name, &params.to_string())
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}
#[test]
fn ordinary_task_records_multiple_prs_and_finishes_without_process_gates() {
    let mut sim = record("Open");
    step(
        &mut sim,
        "Update",
        json!({"task_summary":"Fix gallery", "task_detail":"Images load after navigation", "repo":"owner/repo", "branch":"fix", "intent_ref":""}),
    );
    for url in [
        "https://github.com/owner/repo/pull/1",
        "https://github.com/owner/repo/pull/2",
    ] {
        step(
            &mut sim,
            "AttachPullRequest",
            json!({"pull_request_urls":url}),
        );
    }
    step(
        &mut sim,
        "RecordEvidence",
        json!({"evidence":"Opened gallery and verified loaded images"}),
    );
    let result = step(
        &mut sim,
        "Complete",
        json!({"result_summary":"Fixed and exercised gallery"}),
    );
    sim.assert_status("effort", "Done");
    assert_eq!(
        result["lists"]["pull_request_urls"],
        json!([
            "https://github.com/owner/repo/pull/1",
            "https://github.com/owner/repo/pull/2"
        ])
    );
    assert_eq!(
        result["lists"]["evidence"],
        json!(["Opened gallery and verified loaded images"])
    );
    assert_eq!(
        result["fields"]["result_summary"],
        "Fixed and exercised gallery"
    );
    let parsed: toml::Value = toml::from_str(&source()).unwrap();
    for action in parsed["action"].as_array().unwrap() {
        assert!(
            action.get("triggers").is_none(),
            "record actions must not start external operations"
        );
        assert!(
            action.get("guard").is_none(),
            "record updates must not depend on ceremony"
        );
    }
    assert!(sim.step("effort", "Merge", "{}").is_err());
}
#[test]
fn legacy_records_can_finish_or_resume_without_walking_old_states() {
    for status in [
        "Intended",
        "Specified",
        "Planned",
        "Building",
        "InReview",
        "Proving",
        "Merged",
        "Deploying",
        "ResourceMergeChecking",
        "ResourceVerifying",
        "Verified",
        "Stalled",
        "Abandoned",
    ] {
        let mut sim = record(status);
        step(
            &mut sim,
            "Complete",
            json!({"result_summary":"Existing task delivered"}),
        );
        sim.assert_status("effort", "Done");
        step(&mut sim, "Reopen", json!({}));
        sim.assert_status("effort", "Open");
        step(
            &mut sim,
            "Cancel",
            json!({"result_summary":"No longer needed"}),
        );
        sim.assert_status("effort", "Cancelled");
    }
}
