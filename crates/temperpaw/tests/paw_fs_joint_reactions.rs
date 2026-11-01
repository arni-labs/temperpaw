//! Every reaction paw-fs's specs send lands on a move its target can take.
//!
//! The kernel's joint walk (ADR-0150, the check `temper verify` gates on)
//! found two that were dropped: File.StreamUpdated superseding a version that
//! two close writes had already superseded, and FileVersion.Create recording
//! itself on a File that was still Created. A dropped reaction is lost work,
//! so this walks paw-fs as the kernel does and names any that is dropped.
use std::fs;
use std::path::Path;

use temper_verify::composite::{Bound, CompositeOutcome, Progress, verify_all};

#[test]
fn paw_fs_reactions_are_never_dropped() {
    let specs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-fs/specs");
    let mut automata = Vec::new();
    for entry in fs::read_dir(&specs).expect("paw-fs specs directory") {
        let path = entry.expect("spec entry").path();
        if path.to_string_lossy().ends_with(".ioa.toml") {
            let source = fs::read_to_string(&path).expect("spec text");
            automata.push(
                temper_spec::automaton::parse_automaton(&source)
                    .unwrap_or_else(|err| panic!("{} does not parse: {err}", path.display())),
            );
        }
    }
    assert!(
        automata.len() >= 6,
        "paw-fs has File, FileVersion, Directory, Workspace, ..."
    );

    let progress = Progress::default();
    let bound = Bound {
        room: &temper_server::host_room::HOST_ROOM,
        progress: &progress,
    };
    let refs: Vec<_> = automata.iter().collect();
    let results = verify_all(&refs, bound);

    let mut problems = Vec::new();
    for result in &results {
        match result.outcome {
            CompositeOutcome::Verified => {}
            CompositeOutcome::Violated => {
                for drop in &result.dropped_reactions {
                    problems.push(format!(
                        "{}.{} sends '{}' to {}.{}, which {} in '{}' cannot take",
                        drop.source_entity,
                        drop.source_action,
                        drop.trigger_name,
                        drop.target_entity,
                        drop.target_action,
                        drop.target_entity,
                        drop.target_state,
                    ));
                }
            }
            CompositeOutcome::Incomplete => problems.push(format!(
                "the walk from {} did not finish, so it proves nothing",
                result.seed
            )),
        }
    }
    assert!(
        problems.is_empty(),
        "dropped reactions:\n{}",
        problems.join("\n")
    );
}
