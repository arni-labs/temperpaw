use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.as_ref().display()))
}

#[test]
fn paw_agent_no_longer_exposes_capability_request_install_surface() {
    let root = repo_root();

    for removed in [
        "os-apps/paw-agent/specs/capability_request.ioa.toml",
        "os-apps/paw-agent/policies/capability_request.cedar",
        "os-apps/paw-agent/wasm/capability_installer/Cargo.toml",
        "os-apps/paw-agent/wasm/capability_installer/src/lib.rs",
    ] {
        assert!(
            !root.join(removed).exists(),
            "{removed} should not exist in the active paw-agent app surface"
        );
    }

    for active_file in [
        "os-apps/paw-agent/APP.md",
        "os-apps/paw-agent/specs/model.csdl.xml",
        "os-apps/paw-agent/policies/session.cedar",
        "os-apps/paw-agent/wasm/build.sh",
        "os-apps/paw-agent/system/skills/platform-awareness/SKILL.md",
        "os-apps/paw-agent/system/skills/temper-app-creation/SKILL.md",
        "os-apps/paw-agent/agents/paw/skills/temperpaw-agent/SKILL.md",
    ] {
        let content = read(root.join(active_file));
        assert!(
            !content.contains("CapabilityRequest"),
            "{active_file} should not mention the removed app-install request entity"
        );
        assert!(
            !content.contains("capability_installer"),
            "{active_file} should not reference the removed installer WASM"
        );
    }
}

#[test]
fn agent_guidance_makes_genesis_repair_the_default_app_workflow() {
    let root = repo_root();
    let platform = read(root.join("os-apps/paw-agent/system/skills/platform-awareness/SKILL.md"));
    let app_creation =
        read(root.join("os-apps/paw-agent/system/skills/temper-app-creation/SKILL.md"));
    let paw_agent = read(root.join("os-apps/paw-agent/agents/paw/skills/temperpaw-agent/SKILL.md"));
    let combined = format!("{platform}\n{app_creation}\n{paw_agent}");

    for needle in [
        "repair that app and publish the next Genesis version",
        "temper.search_apps",
        "temper.update_app",
        "temper.publish_app",
        "temper.install_app",
        "owner/name@hash",
        "old ref, new ref, and smoke result",
        "not a fork or lineage change",
    ] {
        assert!(
            combined.contains(needle),
            "Genesis repair guidance should contain {needle}"
        );
    }

    assert!(
        combined.contains("Never install a Temper app by local catalog name"),
        "skills should reject local app-name installs as the normal path"
    );
    assert!(
        combined.contains("Genesis pinned refs are the app install path"),
        "skills should make Genesis pinned refs the only normal install path"
    );
}

#[test]
fn katagami_is_not_a_tracked_local_temperpaw_app_source() {
    let root = repo_root();

    for forbidden in ["os-apps/katagami-commons", "os-apps/katagami-curation"] {
        assert!(
            !root.join(forbidden).exists(),
            "{forbidden} must be installed from Genesis, not tracked as a local app source"
        );
    }
}

#[test]
fn paw_agent_publishes_apps_through_canonical_genesis_actions() {
    let root = repo_root();
    let repl = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));
    let worker = read(root.join("crates/paw-codex-worker/src/directed_evolution/workdir.rs"));
    let combined = format!("{repl}\n{worker}");

    for required in [
        "Temper.Git.RegisterNewApp",
        "Temper.Git.PublishNewVersion",
        "verify_genesis_latest_hash",
        "\"verified_latest\": true",
        "http.{}/.extraHeader",
        "X-Tenant-Id: {registry_tenant}",
        "git config protocol.version 0",
        "genesis_read -c {}={} -c protocol.version=0 clone",
        // A repository that is not public answers a clone only with a credential: the read token
        // reaches the clone's environment from a sandbox file, never the command text.
        "ctx.get_secret(\"genesis_read_token\")",
        "GIT_CONFIG_KEY_0={} GIT_CONFIG_VALUE_0=\\\"$read_header\\\" git",
        "rm -f \\\"$read_header_file\\\"",
        "tar --exclude=.git --exclude=target",
        "--force-with-lease=refs/heads/main:$existing_hash",
    ] {
        assert!(
            combined.contains(required),
            "Paw app publish/update path must contain canonical Genesis publish proof `{required}`"
        );
    }

    for forbidden in [
        "Temper.RegisterNewApp",
        "Temper.PublishNewVersion",
        "App.PublishNewVersion",
    ] {
        assert!(
            !combined.contains(forbidden),
            "Paw app publish/update path must not use legacy action route `{forbidden}`"
        );
    }
}

#[test]
fn deployment_docs_preserve_production_databases() {
    let root = repo_root();
    let deployment = read(root.join("docs/deployment.md"));
    let adr = read(root.join("docs/adrs/0051-genesis-app-repair-without-capability-requests.md"));
    let combined = format!("{deployment}\n{adr}");

    for needle in ["Do not reset", "wipe", "replace", "production database"] {
        assert!(
            combined.contains(needle),
            "deployment safety docs should explicitly include {needle}"
        );
    }
}

#[test]
fn paw_channels_policy_allows_system_reconcile_without_broad_local_install_escape_hatch() {
    let root = repo_root();
    let policy = read(root.join("os-apps/paw-channels/policies/channels.cedar"));

    for required in [
        "transport_reconcile",
        "transport-reconcile",
        "action == Action::\"http_call\"",
        "resource is HttpEndpoint",
        "principal.agent_type == \"system\"",
        "action in [Action::\"create\", Action::\"Register\", Action::\"Update\", Action::\"Disable\", Action::\"Enable\"]",
        "resource is AgentRoute",
    ] {
        assert!(
            policy.contains(required),
            "paw-channels policy must contain {required}"
        );
    }

    assert!(
        !policy.contains("CapabilityRequest"),
        "transport policy must not reintroduce capability request install UX"
    );
}
