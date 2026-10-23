//! `scripts/railway_release.sh forward`, the step that keeps a deploy from moving the main Temper backwards, run
//! against a stand-in `gh` on a five-commit main (c1 to c5). Nothing reaches GitHub or Railway.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn commit(n: u8) -> String {
    n.to_string().repeat(40)
}

/// A commit that main's line never passes through, such as a branch build's.
const OFF_MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
    errors: String,
}

/// Runs `forward` with `served` in the snapshot and `release` as EXPECTED_SHA. The stand-in `gh` answers
/// `api repos/<repo>/compare/<base>...<head> --jq .status` from main's order, with main's head c5.
fn forward(served: &str, release: &str, deploy_env: &str) -> Run {
    let dir = std::env::temp_dir().join(format!("railway-forward-{}-{}-{}-{deploy_env}", std::process::id(), &served[..4], &release[..4]));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("bin")).unwrap();
    let state = dir.join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("before.json"), format!(r#"{{"deployment_id":"d1","image_tag":"sha-{}","sha":"{}"}}"#, &served[..7], served)).unwrap();
    let gh = dir.join("bin/gh");
    fs::write(
        &gh,
        r#"#!/usr/bin/env bash
# gh api repos/<o>/<r>/compare/<base>...<head> --jq .status
main="1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 3333333333333333333333333333333333333333 4444444444444444444444444444444444444444 5555555555555555555555555555555555555555"
spec="${2##*/compare/}"; base="${spec%%...*}"; head="${spec##*...}"
[ "$head" = main ] && head=5555555555555555555555555555555555555555
[ "$base" = "$head" ] && { echo identical; exit 0; }
i=0; bi=-1; hi=-1
for c in $main; do [ "$c" = "$base" ] && bi=$i; [ "$c" = "$head" ] && hi=$i; i=$((i + 1)); done
if [ $bi -lt 0 ] || [ $hi -lt 0 ]; then echo diverged; elif [ $bi -lt $hi ]; then echo ahead; else echo behind; fi
"#,
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
    let out = Command::new("bash")
        .arg(repo_root().join("scripts/railway_release.sh"))
        .arg("forward")
        .env("PATH", format!("{}:{}", dir.join("bin").display(), std::env::var("PATH").unwrap()))
        .env("RAILWAY_API_TOKEN", "test")
        .env("RAILWAY_PROJECT_ID", "test")
        .env("RAILWAY_ENVIRONMENT_ID", "test")
        .env("RAILWAY_SERVICE_ID", "test")
        .env("BASE_URL", "http://127.0.0.1:9")
        .env("TEMPER_API_KEY", "test")
        .env("STATE_DIR", &state)
        .env("EXPECTED_SHA", release)
        .env("DEPLOY_ENV", deploy_env)
        .output()
        .unwrap();
    let errors = fs::read_to_string(state.join("errors.txt")).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        errors,
    }
}

#[test]
fn a_release_what_serves_already_contains_deploys_nothing() {
    let run = forward(&commit(4), &commit(2), "production");
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "already=true", "an older release is live already, never deployed over a newer build");
    assert!(run.stderr.contains("already live"), "{}", run.stderr);
}

#[test]
fn the_build_that_serves_is_live_already() {
    let run = forward(&commit(4), &commit(4), "production");
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "already=true");
}

#[test]
fn a_release_ahead_of_what_serves_goes_on_top_of_it() {
    let run = forward(&commit(2), &commit(4), "production");
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "already=false");
}

#[test]
fn production_refuses_a_commit_main_does_not_contain() {
    let run = forward(&commit(2), OFF_MAIN, "production");
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(!run.stdout.contains("already="), "{}", run.stdout);
    assert!(run.errors.contains("main does not contain"), "the refusal is the outcome's reason: {}", run.errors);
}

#[test]
fn staging_takes_a_branch_build_that_does_not_go_backwards() {
    let run = forward(&commit(2), OFF_MAIN, "staging");
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "already=false");
}

#[test]
fn the_workflow_skips_the_deploy_and_its_checks_when_the_release_is_live_already() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/railway-redeploy.yml")).unwrap();
    assert!(workflow.contains("scripts/railway_release.sh forward"));
    assert!(workflow.contains("inputs.step != 'roll_back'"), "roll_back goes back on purpose and never runs forward");
    for step in ["id: deploy", "id: check"] {
        let at = workflow.find(step).unwrap();
        let next: String = workflow[at..].chars().take(400).collect();
        assert!(next.contains("steps.forward.outputs.already != 'true'"), "{step} waits on forward");
    }
    assert!(workflow.contains("result=already_live"), "an already-live release is its own result");
    assert!(workflow.contains(r#".result == "already_live" then "ok""#) || workflow.contains(r#"or .result == "already_live" then "ok""#));
}
