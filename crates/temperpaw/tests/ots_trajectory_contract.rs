//! Contract tests for OTS trajectory emission (ARN-109).
//!
//! These assert the wiring that decides whether a stored trajectory is usable
//! training data: that tool spans are actually persisted, that the emitter is
//! told which spec governed the run, and that the emitted JSON uses the exact
//! field names the kernel's `temper-ots` structs deserialize.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn session_spec() -> String {
    fs::read_to_string(repo_root().join("os-apps/paw-agent/specs/session.ioa.toml"))
        .expect("session.ioa.toml should exist")
}

fn emitter_source() -> String {
    fs::read_to_string(
        repo_root().join("os-apps/paw-agent/wasm/emit_ots_trajectory/src/ots_build.rs"),
    )
    .expect("ots_build.rs should exist")
}

/// `metadata.spec_version` has to name the spec that actually ran, and the WASM
/// guest context exposes no spec hash — so the literal in the spec is the
/// identity, and it must track the app manifest.
#[test]
fn emitter_spec_version_matches_the_app_manifest() {
    let manifest = fs::read_to_string(repo_root().join("os-apps/paw-agent/app.toml"))
        .expect("paw-agent app.toml should exist");
    let version = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = "))
        .map(|value| value.trim().trim_matches('"').to_string())
        .expect("app.toml should declare a version");
    let expected = format!("spec_version = \"paw-agent@{version}\"");

    assert!(
        session_spec().contains(&expected),
        "emit_ots_trajectory config must declare {expected}; the emitter reports \
         it as OTSMetadata.spec_version and it may not drift from app.toml"
    );
}

/// The truncation marker is a two-sided contract kept as two independent
/// literals: `monty_repl` seals a full span document with it, and the emitter
/// recognizes it instead of turning it into a decision. Nothing tied them
/// together, so a rename on one side would leave the other reading a real tool
/// call named `_tool_spans_truncated` — a decision the agent never made,
/// attributed to it, on a record that no longer knows it is partial.
#[test]
fn the_span_truncation_marker_means_the_same_thing_on_both_sides() {
    const DECL: &str = "pub const TOOL_SPANS_TRUNCATED_MARKER: &str = \"";

    fn marker_literal(source: &str, whose: &str) -> String {
        let start = source
            .find(DECL)
            .unwrap_or_else(|| panic!("{whose} must declare TOOL_SPANS_TRUNCATED_MARKER"));
        let rest = &source[start + DECL.len()..];
        let end = rest
            .find('"')
            .unwrap_or_else(|| panic!("{whose}'s marker literal must terminate"));
        rest[..end].to_string()
    }

    let writer =
        fs::read_to_string(repo_root().join("os-apps/paw-agent/wasm/monty_repl/src/session.rs"))
            .expect("monty_repl session.rs should exist");
    let written = marker_literal(&writer, "the writer");
    let read = marker_literal(&emitter_source(), "the emitter");

    assert_eq!(
        written, read,
        "the writer seals with {written:?} and the emitter looks for {read:?}; a \
         document sealed by one would be invisible to the other"
    );

    // The line the writer appends has to carry that same value as its
    // `tool_name`, because that field — not a substring search — is what the
    // emitter matches on.
    assert!(
        writer.contains(&format!("\\\"tool_name\\\":\\\"{written}\\\"")),
        "the sealing line must carry the marker as its tool_name"
    );
}
