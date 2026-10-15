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
fn paw_fs_specs_use_inline_triggers_and_explicit_counter_assignment() {
    let root = repo_root();
    let file_spec = read(root.join("os-apps/paw-fs/specs/file.ioa.toml"));
    let file_version_spec = read(root.join("os-apps/paw-fs/specs/file_version.ioa.toml"));
    let workspace_spec = read(root.join("os-apps/paw-fs/specs/workspace.ioa.toml"));

    for needle in [
        "name = \"last_version_id\"",
        "\"size_bytes = params.size_bytes\"",
        "name = \"file_stream_updated_creates_version\"",
        "[action.triggers.args]",
        "file_id = \"Id\"",
        "version_number = \"version_count\"",
        "previous_version_id = \"previous_version_id\"",
    ] {
        assert!(
            file_spec.contains(needle),
            "file spec should contain {needle}"
        );
    }
    assert!(
        !file_spec.contains("spawn('FileVersion'"),
        "file spec should not create FileVersion via spawn after the inline trigger hard cut"
    );

    for needle in [
        "name = \"mime_type\"",
        "name = \"previous_version_id\"",
        "\"version_number = params.version_number\"",
        "\"size_bytes = params.size_bytes\"",
    ] {
        assert!(
            file_version_spec.contains(needle),
            "file_version spec should contain {needle}"
        );
    }

    let needle = "\"quota_limit = params.quota_limit\"";
    assert!(
        workspace_spec.contains(needle),
        "workspace spec should contain {needle}"
    );
}

#[test]
fn paw_fs_versioning_contract_uses_no_legacy_reactions_file() {
    let root = repo_root();
    let reactions_path = root.join("os-apps/paw-fs/reactions/reactions.toml");
    let file_version_spec = read(root.join("os-apps/paw-fs/specs/file_version.ioa.toml"));
    let csdl = read(root.join("os-apps/paw-fs/specs/model.csdl.xml"));

    assert!(
        !reactions_path.exists(),
        "legacy paw-fs reactions.toml should stay removed after the inline trigger hard cut"
    );
    for needle in [
        "name = \"record_newest_version_on_file\"",
        "target_entity = \"File\"",
        "target_action = \"RecordVersion\"",
        "field = \"file_id\"",
    ] {
        assert!(
            file_version_spec.contains(needle),
            "file_version spec should contain {needle}"
        );
    }

    for needle in [
        "<Property Name=\"LastVersionId\" Type=\"Edm.Guid\"/>",
        "<NavigationProperty Name=\"LastVersion\" Type=\"Paw.FS.FileVersion\">",
        "<Property Name=\"MimeType\" Type=\"Edm.String\" Nullable=\"false\"/>",
        "<Property Name=\"PreviousVersionId\" Type=\"Edm.Guid\"/>",
        "<NavigationProperty Name=\"PreviousVersion\" Type=\"Paw.FS.FileVersion\">",
        "<Parameter Name=\"version_number\" Type=\"Edm.Int32\" Nullable=\"false\"/>",
    ] {
        assert!(csdl.contains(needle), "CSDL should contain {needle}");
    }
}
