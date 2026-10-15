use std::fs;
use std::path::Path;

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("{} should be readable: {err}", path.as_ref().display()))
}

#[test]
fn gitignore_excludes_local_worktree_and_extraction_noise() {
    let ignore = read(repo_root().join(".gitignore"));

    for pattern in [
        ".claude/worktrees/",
        ".claude/scheduled_tasks.lock",
        ".worktrees/",
        ".extracted/",
        "docs/plans/",
        "**/node_modules/",
        "**/.next/",
        "**/dist/",
    ] {
        assert!(
            ignore.contains(pattern),
            ".gitignore should exclude local/generated noise pattern {pattern}"
        );
    }
}

#[test]
fn agents_guide_requires_adrs_for_material_architecture_changes() {
    let guide = read(repo_root().join("AGENTS.md"));

    for needle in [
        "Architecture Decision Records",
        "material architecture",
        "os-apps/<app>/adrs/",
        "docs/adrs/",
    ] {
        assert!(
            guide.contains(needle),
            "AGENTS.md should codify ADR guidance with {needle}"
        );
    }
}

#[test]
fn paw_skills_app_declares_native_install_surface() {
    let root = repo_root();
    let manifest = read(root.join("os-apps/paw-skills/app.toml"));
    let install_spec = read(root.join("os-apps/paw-skills/specs/skill_install.ioa.toml"));
    let package_spec = read(root.join("os-apps/paw-skills/specs/skill_package.ioa.toml"));
    let binding_spec = read(root.join("os-apps/paw-skills/specs/skill_binding.ioa.toml"));
    let csdl = read(root.join("os-apps/paw-skills/specs/model.csdl.xml"));
    let policy = read(root.join("os-apps/paw-skills/policies/skills.cedar"));
    let adr = read(root.join("os-apps/paw-skills/adrs/001-native-skill-package-installation.md"));

    for needle in [
        "name = \"paw-skills\"",
        "startup_install = \"core\"",
        "dependencies = [\"paw-agent\", \"paw-fs\"]",
        "name = \"skill_installer\"",
    ] {
        assert!(
            manifest.contains(needle),
            "paw-skills app.toml should contain {needle}"
        );
    }

    for needle in [
        "name = \"SkillInstall\"",
        "states = [\"Requested\", \"Installing\", \"Installed\", \"Rejected\", \"Failed\", \"Archived\"]",
        "name = \"Approve\"",
        "module = \"skill_installer\"",
        "name = \"InstallComplete\"",
        "name = \"InstallFailed\"",
        "source_url",
        "target_scope_type",
        "target_scope_id",
    ] {
        assert!(
            install_spec.contains(needle),
            "SkillInstall spec should contain {needle}"
        );
    }

    for needle in [
        "name = \"SkillPackage\"",
        "content_digest",
        "source_url",
        "main_file_path",
    ] {
        assert!(
            package_spec.contains(needle),
            "SkillPackage spec should contain {needle}"
        );
    }

    for needle in [
        "name = \"SkillBinding\"",
        "scope_type",
        "scope_id",
        "skill_path",
        "file_id",
    ] {
        assert!(
            binding_spec.contains(needle),
            "SkillBinding spec should contain {needle}"
        );
    }

    for needle in [
        "resource is SkillInstall",
        "Action::\"Approve\"",
        "Action::\"InstallComplete\"",
        "context.module == \"skill_installer\"",
    ] {
        assert!(
            policy.contains(needle),
            "paw-skills Cedar policy should contain {needle}"
        );
    }

    for needle in [
        "EntityType Name=\"SkillInstall\"",
        "EntityType Name=\"SkillPackage\"",
        "EntityType Name=\"SkillBinding\"",
        "EntitySet Name=\"SkillInstalls\"",
        "Action Name=\"Approve\"",
        "Action Name=\"InstallComplete\"",
    ] {
        assert!(
            csdl.contains(needle),
            "paw-skills CSDL should expose native install OData surface with {needle}"
        );
    }

    for needle in [
        "external skill source",
        "TemperFS",
        "No bundled taste or anti-slop skill content",
    ] {
        assert!(
            adr.contains(needle),
            "paw-skills ADR should contain {needle}"
        );
    }
}

#[test]
fn native_skill_runtime_preserves_path_scoped_contract_without_agent_install_tool() {
    let root = repo_root();
    let installer = read(root.join("os-apps/paw-skills/wasm/skill_installer/src/lib.rs"));
    let context_preparer = read(root.join("os-apps/paw-agent/wasm/context_preparer/src/lib.rs"));
    let monty = read(root.join("os-apps/paw-agent/wasm/monty_repl/src/dispatch.rs"));
    let catalog = read(root.join("os-apps/paw-agent/wasm/tool-catalog/src/lib.rs"));

    for needle in [
        "github_tree_skill_raw_url",
        "https://raw.githubusercontent.com/",
        "extract_skill_frontmatter",
        "skill_temperfs_path",
        "/system/skills/",
        "/projects/{scope_id}/skills/",
        "/agents/{scope_id}/skills/",
        "SkillPackages",
        "SkillBindings",
        "InstallComplete",
    ] {
        assert!(
            installer.contains(needle),
            "skill_installer should contain {needle}"
        );
    }

    for needle in [
        "/system/skills/",
        "/projects/{project_id}/skills/",
        "/agents/{agent_id}/skills/",
    ] {
        assert!(
            context_preparer.contains(needle),
            "context_preparer should keep path-scoped skill discovery for {needle}"
        );
    }

    assert!(
        !monty.contains("fn temper_install_skill"),
        "Monty should not expose skill install as a normal agent tool"
    );
    assert!(
        !catalog.contains("method: \"install_skill\""),
        "tool catalog should not advertise temper.install_skill"
    );
}

#[test]
fn paw_skills_app_does_not_vendor_external_taste_skill_content() {
    let root = repo_root().join("os-apps/paw-skills");
    if !root.exists() {
        return;
    }

    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).unwrap_or_else(|err| {
            panic!(
                "{} should be listable while checking vendored taste content: {err}",
                path.display()
            )
        }) {
            let entry = entry.expect("directory entry should be readable");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }

            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !["md", "rs", "toml", "cedar", "sh"].contains(&ext) {
                continue;
            }

            let content = read(&path);
            assert!(
                !content.contains("Leonxlnx/taste-skill") || content.contains("E2E testing"),
                "{} should reference the external taste repo only as a test source, not as vendored seed content",
                path.display()
            );
            assert!(
                !content.contains("THE LILA BAN"),
                "{} appears to vendor the external taste skill body",
                path.display()
            );
        }
    }
}
