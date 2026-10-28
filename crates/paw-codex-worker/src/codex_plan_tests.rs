#[test]
fn codex_plan_args_use_read_only_sandbox_without_bypass() {
    let args = codex_plan_args(Path::new("/tmp/paw-worktree"), "Plan the fix");
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        args,
        vec![
            "exec",
            "--ignore-user-config",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--cd",
            "/tmp/paw-worktree",
            "--skip-git-repo-check",
            "Plan the fix"
        ]
    );
    assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
}
