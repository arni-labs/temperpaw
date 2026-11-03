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
