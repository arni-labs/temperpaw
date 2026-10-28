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

/// Every terminal path that finishes a session has to emit a trajectory —
/// otherwise the training set silently loses whole classes of run.
#[test]
fn every_terminal_action_emits_a_trajectory() {
    let spec = session_spec();
    let emit = "[[action.triggers]]\nname = \"emit_ots_trajectory\"\n";
    for action in [
        "FinalizeResult",
        "FinalizeResultNoReply",
        "RecordResult",
        "RecordResultNoReply",
        "RecordResultInlineReply",
        "Fail",
        "Cancel",
        "TimeoutFail",
    ] {
        let marker = format!("name = \"{action}\"\n");
        let start = spec
            .find(&marker)
            .unwrap_or_else(|| panic!("{action} must exist in session.ioa.toml"));
        let block = &spec[start..];
        let block = &block[..block.find("\n[[action]]").unwrap_or(block.len())];
        assert!(
            block.contains(emit),
            "{action} must trigger emit_ots_trajectory"
        );
    }
}

/// The JCS contract fields ride natively on the pinned kernel structs, so a
/// consumer that deserializes a stored row and writes it back keeps them. The
/// carriers that stood in for them before the pin bump are gone and must stay
/// gone — a mirror that outlives its reason is a second source of truth.
/// Degradation markers are the exception: the kernel models no field for those,
/// so `metadata.tags` still carries them.
#[test]
fn emitter_carries_unmodeled_signal_in_kernel_modeled_fields() {
    let emitter = emitter_source();
    assert!(
        emitter.contains("fn kernel_round_trip_keeps_the_jcs_contract_fields"),
        "the native round trip is what the carriers were removed against; it \
         has to be proven, not assumed"
    );
    assert!(
        emitter.contains("fn degradation_markers_survive_the_kernel_round_trip"),
        "a completeness marker that a re-serialization drops is worse than none"
    );
    for gone in [
        "TOKEN_SIGNAL_CARRIER_TYPE",
        "TOKEN_SIGNALS_TAG",
        "HARNESS_TAG_PREFIX",
        "SPEC_VERSION_TAG_PREFIX",
    ] {
        assert!(
            !emitter.contains(gone),
            "{gone} is a pre-bump carrier. The kernel models these fields now, \
             so mirroring them again would leave the row carrying the same fact \
             twice with nothing keeping the copies equal"
        );
    }

    // `-p temperpaw` does not reach the os-app WASM modules — they are their
    // own workspaces — so a gate living in one of them only fires if CI runs
    // that manifest. Asserting the source text of a test nothing executes
    // proves nothing.
    let ci = fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml should exist");
    for module in [
        "emit_ots_trajectory",
        "provider_response_applier",
        "wasm-helpers",
        "openai-chat-wire",
        "monty_repl",
    ] {
        assert!(
            ci.contains(&format!(
                "cargo test --manifest-path os-apps/paw-agent/wasm/{module}/Cargo.toml"
            )),
            "CI must run {module}'s tests; the OTS contract is asserted there"
        );
    }

    let manifest = fs::read_to_string(
        repo_root().join("os-apps/paw-agent/wasm/emit_ots_trajectory/Cargo.toml"),
    )
    .expect("emit_ots_trajectory Cargo.toml should exist");
    // Dependency lines only: the comments around them name these crates too,
    // and matching one of those would read as "no rev pinned".
    let pinned_rev = |crate_name: &str| -> String {
        manifest
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .find(|line| line.starts_with(crate_name) && line.contains("rev = \""))
            .and_then(|line| line.split("rev = \"").nth(1))
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("{crate_name} should pin a rev"))
            .to_string()
    };
    let sdk_rev = pinned_rev("temper-wasm-sdk");
    let ots_rev = pinned_rev("temper-ots");
    assert_eq!(
        sdk_rev, ots_rev,
        "the round trip only proves anything if it runs against the kernel this \
         module is built for"
    );
}
