use std::fs;
use std::path::Path;

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|err| panic!("{} should be readable: {err}", path.as_ref().display()))
}

// Since 311e80496 (Stack guidance sync) an ADR records durable architectural
// decisions when useful and is not a completion gate.
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
