//! release_run_lifecycle — side effects for the ReleaseRun automaton, run on
//! the named Computer's sandbox.
//!
//! One trigger, one side effect, one named callback. The module never
//! dispatches actions on other entities and never loops; the automaton and
//! the kernel `state_timeout` own the orchestration (see release_run.ioa.toml).
//!
//! | trigger action | side effect                                              | reports |
//! |----------------|----------------------------------------------------------|---------|
//! | Request        | preflight + merge via GitHub API (App install token, else `github_token`) | MergeSucceeded |
//! | Check          | one `curl` of health_url on the computer                 | CheckHealthy / CheckPending / CheckUnhealthy |
//! | CheckUnhealthy | `git revert -m 1` + push on the computer (same token)    | RollbackPushed |
//!
//! Any error surfaces through `set_error_result`, which the spec routes to
//! `Fail` (on_failure) so nothing fails silently.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;
use wasm_helpers::sandbox::{self, SandboxHandle, normalize_sandbox_provider};
use wasm_helpers::{bounded_reads, entity_field_str, odata_headers, resolve_temper_api_url};

mod github_app;

/// Marker separating the probe body from the HTTP status on the last line.
const HTTP_STATUS_MARKER: &str = "__HTTP_STATUS";

/// Hard ceiling on the watch budget: 240 checks × 30s = 2 hours. Bounds the
/// `max_checks` config so a caller cannot request an effectively permanent
/// watch that defeats automatic rollback.
const MAX_CHECKS_CEILING: u64 = 240;

/// Consecutive degraded-on-new-commit probes required before rollback. A
/// single degraded 30s window right after a swap (cache warmup) must not
/// revert a healthy release; three in a row is a real regression.
const DEGRADED_STREAK_THRESHOLD: u64 = 3;

/// The only base branch a release may target. Enforced at merge time so the
/// rollback (which reverts on this branch) can never push to the wrong one.
const RELEASE_BASE_BRANCH: &str = "main";

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let release = ReleaseFields::from_state(&ctx, &fields)?;

        ctx.log(
            "info",
            &format!(
                "release_run_lifecycle: {} on {} (repo {} pr {} computer {})",
                ctx.trigger_action, ctx.entity_id, release.repo, release.pr_number, release.computer_id
            ),
        );

        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let computer = fetch_computer(&ctx, &temper_api_url, &fields, &release.computer_id)?;
        let handle = sandbox_handle_from_computer(&computer)
            .map_err(|e| format!("release_run_lifecycle: computer {}: {e}", release.computer_id))?;

        let (action, params) = match ctx.trigger_action.as_str() {
            "Request" => merge(&ctx, &release, &temper_api_url, &fields)?,
            "Check" => check(&ctx, &handle, &release)?,
            "CheckUnhealthy" => rollback(&ctx, &handle, &release)?,
            other => return Err(format!("release_run_lifecycle: unsupported trigger {other}")),
        };
        set_success_result(action, &params);
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

/// The ReleaseRun fields every step needs, read from trigger params first
/// (the Request action carries them) and entity state otherwise.
struct ReleaseFields {
    repo: String,
    pr_number: String,
    computer_id: String,
    health_url: String,
    max_checks: u64,
    merge_sha: String,
    check_count: u64,
    degraded_streak: u64,
    /// Optional reviewed PR head to bind the merge to (empty = unbound).
    expected_head_sha: String,
}

impl ReleaseFields {
    fn from_state(ctx: &Context, fields: &Value) -> Result<Self, String> {
        let required = |key: &str| {
            param_or_field(ctx, fields, key)
                .ok_or_else(|| format!("release_run_lifecycle: missing {key}"))
        };
        let max_checks_raw = param_or_field(ctx, fields, "max_checks")
            .ok_or_else(|| "release_run_lifecycle: missing max_checks".to_string())?;
        Ok(Self {
            repo: required("repo")?,
            pr_number: required("pr_number")?,
            computer_id: required("computer_id")?,
            health_url: param_or_field(ctx, fields, "health_url").unwrap_or_default(),
            max_checks: parse_max_checks(&max_checks_raw)?,
            merge_sha: param_or_field(ctx, fields, "merge_sha").unwrap_or_default(),
            check_count: counter_field(fields, "check_count"),
            degraded_streak: counter_field(fields, "degraded_streak"),
            expected_head_sha: param_or_field(ctx, fields, "expected_head_sha").unwrap_or_default(),
        })
    }
}

/// Parse and bound the watch budget. Rejects non-numeric input and 0 (which
/// would make the first check exceed budget → instant rollback) and clamps to
/// [`MAX_CHECKS_CEILING`] so a huge value cannot create a permanent watch.
fn parse_max_checks(raw: &str) -> Result<u64, String> {
    let n: u64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("release_run_lifecycle: max_checks must be a number, got {raw:?}"))?;
    if n == 0 {
        return Err("release_run_lifecycle: max_checks must be at least 1".to_string());
    }
    if n > MAX_CHECKS_CEILING {
        return Err(format!(
            "release_run_lifecycle: max_checks {n} exceeds the ceiling {MAX_CHECKS_CEILING} (2h)"
        ));
    }
    Ok(n)
}

// -- Step 1: merge -------------------------------------------------------------

fn merge(
    ctx: &Context,
    release: &ReleaseFields,
    temper_api_url: &str,
    fields: &Value,
) -> Result<(&'static str, Value), String> {
    validate_repo(&release.repo)?;
    validate_number(&release.pr_number, "pr_number")?;
    // Validate the watch target BEFORE merging: a bad health_url must abort
    // the release rather than leave a merged-but-unwatchable rollout.
    validate_url(&release.health_url)?;

    // ARN-397: per-repo serialization. Refuse to start a merge while another
    // ReleaseRun for the same repo is already active — two concurrent releases
    // on one repo interleave merges/reverts and corrupt each other's watch.
    // (This is a read + our own Fail; no cross-entity transition dispatch.)
    if let Some(other) = active_release_conflict(ctx, temper_api_url, fields, &release.repo)? {
        return Err(format!(
            "release_run_lifecycle: a release for {} is already in flight ({other}); refusing to run a second concurrently (ARN-397)",
            release.repo
        ));
    }

    // Preflight: read the PR's base branch, head commit, and merge state.
    let pr = read_pr(ctx, &release.repo, &release.pr_number)?;
    if pr.base_ref != RELEASE_BASE_BRANCH {
        return Err(format!(
            "release_run_lifecycle: PR {}#{} targets base {:?}, only {RELEASE_BASE_BRANCH} is releasable",
            release.repo, release.pr_number, pr.base_ref
        ));
    }
    // head_sha is interpolated into the merge command and compared for binding;
    // require a real 40-hex sha so a malformed GitHub response can neither inject
    // nor produce an empty `"sha":""` PUT.
    validate_sha(&pr.head_sha)?;
    // Commit-binding: if bound to a reviewed head, refuse unless it still matches.
    if !head_binding_ok(&release.expected_head_sha, &pr.head_sha) {
        return Err(format!(
            "release_run_lifecycle: PR {}#{} head is {:?}, expected reviewed head {:?} — refusing to merge an unreviewed commit",
            release.repo, release.pr_number, pr.head_sha, release.expected_head_sha
        ));
    }
    if pr.merged {
        // Already merged before we acted (idempotent replay or out-of-band).
        // No head was pinned by us here, so bind only to the (optional) reviewed
        // head; base is re-validated in the gate.
        ctx.log("info", &format!("release_run_lifecycle: {}#{} already merged", release.repo, release.pr_number));
        return confirm_merged_pr(&pr, release, None);
    }

    // Do the merge from the host with a GitHub App installation token (same
    // Apps as the factory door). Tenant github_token is fallback only. The
    // computer's ~/.git-credentials is not the token.
    let merge_diag = put_merge(ctx, &release.repo, &release.pr_number, &pr.head_sha).err();

    // Authoritative post-merge confirm: re-read the PR and require it is NOW
    // merged into `main` at the head we pinned, with a valid merge sha. GitHub's
    // merge PUT pins only the head, not the base — a maintainer can retarget the
    // PR between our preflight read and the PUT, so the base MUST be re-validated
    // after the merge (base-branch TOCTOU). The re-read is retried a few times so
    // a transient GET blip or GitHub read-after-write lag does not strand an
    // actually-merged release as Failed (Fable R5 P3-2); it also reconciles an
    // ambiguous PUT whose connection dropped after the merge landed.
    let final_pr = read_pr_confirm(ctx, &release.repo, &release.pr_number, CONFIRM_READ_ATTEMPTS)?;
    if !final_pr.merged {
        let diag = merge_diag.unwrap_or_else(|| "PR is still open after the merge attempt".to_string());
        return Err(format!(
            "release_run_lifecycle: merge of {}#{} did not complete: {diag}",
            release.repo, release.pr_number
        ));
    }
    ctx.log("info", &format!("release_run_lifecycle: merged {}#{} (confirmed on re-read)", release.repo, release.pr_number));
    // Bind to the exact head we pinned in the PUT: if the PUT was refused (head
    // moved) and someone merged a DIFFERENT head out-of-band, the merged head
    // won't match and we refuse to watch an unintended commit (reconcile
    // head-bypass, Codex-2 R5).
    confirm_merged_pr(&final_pr, release, Some(&pr.head_sha))
}

/// Number of times the post-merge confirmation re-read is attempted before we
/// give up and Fail. Each attempt is a fresh idempotent GET, so this absorbs a
/// transient GET failure or GitHub read-after-write lag without stranding a
/// merged release.
const CONFIRM_READ_ATTEMPTS: u32 = 3;

/// Re-read the PR for post-merge confirmation, retrying up to `attempts` times.
/// Returns as soon as a read reports `merged` (the state we expect after a
/// successful PUT); otherwise returns the last read (unmerged) or the last error
/// so the caller Fails on a genuinely-unmerged PR.
fn read_pr_confirm(
    ctx: &Context,
    repo: &str,
    pr_number: &str,
    attempts: u32,
) -> Result<PrInfo, String> {
    let mut last: Result<PrInfo, String> =
        Err("release_run_lifecycle: no confirmation read attempted".to_string());
    for _ in 0..attempts.max(1) {
        match read_pr(ctx, repo, pr_number) {
            Ok(pr) if pr.merged => return Ok(pr),
            other => last = other,
        }
    }
    last
}

/// Gate a merged PR before emitting MergeSucceeded: it must be merged into
/// `main` (base re-validated post-merge — the PUT pins only the head, so a
/// retargeted base is caught here, not silently watched/reverted), at the head
/// we pinned in the PUT (`pinned_head`, when we did the merge) AND at the
/// reviewed head (optional commit-binding), with a valid 40-hex merge sha to
/// watch and revert.
fn confirm_merged_pr(
    pr: &PrInfo,
    release: &ReleaseFields,
    pinned_head: Option<&str>,
) -> Result<(&'static str, Value), String> {
    if pr.base_ref != RELEASE_BASE_BRANCH {
        return Err(format!(
            "release_run_lifecycle: PR {}#{} merged into base {:?}, only {RELEASE_BASE_BRANCH} is releasable (base retargeted after preflight)",
            release.repo, release.pr_number, pr.base_ref
        ));
    }
    if let Some(pinned) = pinned_head {
        if pr.head_sha != pinned {
            return Err(format!(
                "release_run_lifecycle: PR {}#{} merged at head {:?}, but we pinned {:?} in the merge — refusing to watch a commit we did not merge (out-of-band merge of a moved head)",
                release.repo, release.pr_number, pr.head_sha, pinned
            ));
        }
    }
    if !head_binding_ok(&release.expected_head_sha, &pr.head_sha) {
        return Err(format!(
            "release_run_lifecycle: PR {}#{} merged at head {:?}, expected reviewed head {:?} — refusing to watch an unreviewed release",
            release.repo, release.pr_number, pr.head_sha, release.expected_head_sha
        ));
    }
    let sha = pr.merge_commit_sha.clone().ok_or_else(|| {
        format!("release_run_lifecycle: PR {}#{} reports merged but carries no sha", release.repo, release.pr_number)
    })?;
    validate_sha(&sha)?;
    Ok(merge_succeeded(sha, pr.base_ref.clone(), pr.head_sha.clone()))
}

/// Commit-binding predicate: an empty expected head is unbound (any head ok);
/// a set expected head must equal the PR's current head, else the merge is
/// refused (an unreviewed commit was pushed after the target was bound).
fn head_binding_ok(expected: &str, actual: &str) -> bool {
    expected.is_empty() || expected == actual
}

/// Build the MergeSucceeded callback (merge_sha to watch, base_branch for the
/// rollback, head_sha for audit).
fn merge_succeeded(merge_sha: String, base_branch: String, head_sha: String) -> (&'static str, Value) {
    (
        "MergeSucceeded",
        json!({ "merge_sha": merge_sha, "base_branch": base_branch, "head_sha": head_sha }),
    )
}

/// What we need to know about a PR before/after merging.
#[derive(Debug, Clone, PartialEq)]
struct PrInfo {
    base_ref: String,
    /// The PR's current head commit sha (what a merge would land).
    head_sha: String,
    merged: bool,
    merge_commit_sha: Option<String>,
}

/// GET the PR and parse base branch + merge state.
fn read_pr(ctx: &Context, repo: &str, pr_number: &str) -> Result<PrInfo, String> {
    let url = github_pr_url(repo, pr_number);
    let body = github_json(ctx, repo, "GET", &url, "")?;
    parse_pr(&body.to_string()).map_err(|e| {
        format!("release_run_lifecycle: could not read PR {repo}#{pr_number}: {e}")
    })
}

fn put_merge(ctx: &Context, repo: &str, pr_number: &str, head_sha: &str) -> Result<String, String> {
    let url = github_merge_url(repo, pr_number);
    let body = format!(r#"{{"sha":"{head_sha}","merge_method":"merge"}}"#);
    let resp = github_json(ctx, repo, "PUT", &url, &body)?;
    parse_merge_sha(&resp.to_string())
}

fn github_pr_url(repo: &str, pr_number: &str) -> String {
    format!("https://api.github.com/repos/{repo}/pulls/{pr_number}")
}

fn github_merge_url(repo: &str, pr_number: &str) -> String {
    format!("https://api.github.com/repos/{repo}/pulls/{pr_number}/merge")
}

fn github_token(ctx: &Context, repo: &str) -> Result<String, String> {
    github_app::github_bearer(ctx, repo)
}

/// Charset + length check before the token is interpolated into `TOK='…'` on
/// the rollback command. Merge HTTP does not use this. GitHub App installation
/// tokens since the 2026 stateless rollout are `ghs_<appid>_<jwt>` (~520 chars,
/// two dots). Classic opaque `ghs_` / `ghp_` tokens stay accepted.
fn validate_github_token(token: &str) -> Result<(), String> {
    const MIN: usize = 8;
    const MAX: usize = 1024;
    if token.is_empty() {
        return Err(
            "release_run_lifecycle: github token is empty (App install or tenant github_token)"
                .to_string(),
        );
    }
    if token.len() < MIN || token.len() > MAX {
        return Err("release_run_lifecycle: github token length is not a token".to_string());
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("release_run_lifecycle: github token has unexpected characters".to_string());
    }
    Ok(())
}

fn github_json(
    ctx: &Context,
    repo: &str,
    method: &str,
    url: &str,
    body: &str,
) -> Result<Value, String> {
    let token = github_token(ctx, repo)?;
    let headers = vec![
        ("authorization".to_string(), format!("Bearer {token}")),
        ("accept".to_string(), "application/vnd.github+json".to_string()),
        (
            "user-agent".to_string(),
            "temperpaw-release-run-lifecycle".to_string(),
        ),
    ];
    let resp = ctx.http_call(method, url, &headers, body)?;
    if resp.status == 401 || resp.status == 403 {
        return Err(format!(
            "release_run_lifecycle: GitHub {method} HTTP {} (token rejected)",
            resp.status
        ));
    }
    if resp.status >= 400 {
        return Err(format!(
            "release_run_lifecycle: GitHub {method} HTTP {}",
            resp.status
        ));
    }
    serde_json::from_str(resp.body.trim()).map_err(|_| {
        format!(
            "release_run_lifecycle: unexpected GitHub response: {}",
            excerpt(&resp.body)
        )
    })
}

fn parse_pr(stdout: &str) -> Result<PrInfo, String> {
    let body: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| format!("unexpected GitHub response: {}", excerpt(stdout)))?;
    if let Some(msg) = body.get("message").and_then(Value::as_str) {
        // GitHub returns {"message": "..."} on error (404, bad creds, rate limit).
        if body.get("base").is_none() {
            return Err(format!("GitHub error: {msg}"));
        }
    }
    let base_ref = body
        .get("base")
        .and_then(|b| b.get("ref"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "PR response carried no base.ref".to_string())?;
    let head_sha = body
        .get("head")
        .and_then(|h| h.get("sha"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(PrInfo {
        base_ref,
        head_sha,
        merged: body.get("merged").and_then(Value::as_bool).unwrap_or(false),
        merge_commit_sha: body
            .get("merge_commit_sha")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// Non-terminal ReleaseRun / DsfDeploy states — a run in any of these is
/// "in flight" for per-repo serialization (ARN-397).
const ACTIVE_RELEASE_STATES: &[&str] = &["Requested", "Merging", "Watching", "Unhealthy"];

/// Both live merge types. A DsfDeploy-only or ReleaseRun-only scan lets the
/// other type merge the same repo (ARN-397). Config still names this row's set
/// so a typo fails closed; the conflict read always covers both.
const CONCURRENT_SCAN_SETS: &[&str] = &["ReleaseRuns", "DsfDeploys"];

/// Which OData set this row belongs to. ReleaseRun stays the default so
/// WorkCycle.Complete is unchanged. DsfDeploy sets DsfDeploys.
fn concurrent_entity_set(ctx: &Context) -> Result<&'static str, String> {
    concurrent_entity_set_name(ctx.config.get("concurrent_entity_set").map(String::as_str))
}

fn concurrent_entity_set_name(raw: Option<&str>) -> Result<&'static str, String> {
    match raw.unwrap_or("ReleaseRuns") {
        "" | "ReleaseRuns" => Ok("ReleaseRuns"),
        "DsfDeploys" => Ok("DsfDeploys"),
        other => Err(format!(
            "release_run_lifecycle: concurrent_entity_set must be ReleaseRuns or DsfDeploys, got {other:?}"
        )),
    }
}

/// Query other ReleaseRuns for `repo` and return the id of one that is still
/// active (excluding self), if any. A loopback read; failure surfaces as an
/// error so the check fails closed (we would rather refuse a release than run a
/// second concurrently on an unverifiable state).
///
/// TOCTOU residual: two Requests can both pass this read before either commits
/// its Merging state. A fully-atomic guard is a per-repo lane entity — tracked
/// as the stronger form of ARN-397; this read-reject closes the common case.
fn active_release_conflict(
    ctx: &Context,
    temper_api_url: &str,
    fields: &Value,
    repo: &str,
) -> Result<Option<String>, String> {
    let headers = odata_headers(ctx, &ctx.tenant, fields);
    // Filter on the active-status predicate ONLY, and match the repo
    // case-insensitively in Rust (below). We deliberately do NOT push
    // `repo eq …` into the OData filter: the kernel's `eq` is case-sensitive, so
    // an active `Owner/Repo` row would be filtered out server-side before the
    // case-insensitive compare could catch a new `owner/repo` run — GitHub
    // owner/repo is case-insensitive, so that is the SAME target and the guard
    // must not miss it. The active set across ALL repos is tiny (ARN-397 keeps
    // ≤1 active per repo), so status-only is a small, page-safe query. A bare
    // `repo eq …` also returned hundreds of terminal rows and paged the in-flight
    // one onto page 2 (Fable F1) — status-only avoids that too.
    let status_clause = ACTIVE_RELEASE_STATES
        .iter()
        .map(|s| format!("status eq '{s}'"))
        .collect::<Vec<_>>()
        .join(" or ");
    let _ = concurrent_entity_set(ctx)?;
    let mut runs = Vec::new();
    for set in CONCURRENT_SCAN_SETS {
        let path = format!("/tdata/{set}?$filter={status_clause}");
        let resp = bounded_reads::get_json(ctx, temper_api_url, &path, &headers, "release_run_lifecycle")
            .map_err(|e| {
                format!("release_run_lifecycle: could not check {set} for concurrent releases of {repo}: {e}")
            })?;
        // Fail CLOSED if the result is paginated: a truncated page could hide the
        // conflicting run, so refuse rather than risk running two releases at once.
        if resp.get("@odata.nextLink").is_some() {
            return Err(format!(
                "release_run_lifecycle: too many active {set} rows to check safely (paginated); refusing to start (ARN-397)"
            ));
        }
        runs.extend(parse_release_runs(&resp)?);
    }
    Ok(conflicting_active_release(&runs, &ctx.entity_id, repo).map(str::to_string))
}

/// Extract (id, repo, status) from an OData ReleaseRuns list response. Fails
/// CLOSED: a 200 with no `value` array, or any row missing an id, is an error
/// (not an empty list) — otherwise a malformed/unexpected response would let a
/// concurrent release through (Codex-1 R4 P2).
fn parse_release_runs(resp: &Value) -> Result<Vec<(String, String, String)>, String> {
    let arr = resp
        .get("value")
        .and_then(Value::as_array)
        .ok_or_else(|| "release_run_lifecycle: ReleaseRuns response carried no `value` array".to_string())?;
    arr.iter()
        .map(|e| {
            // Match bounded_reads::entity_id's key order — a row that missed
            // field-stamping must fail the check, not be silently dropped (which
            // would fail OPEN on the conflict guard).
            let id = entity_field_str(e, &["entity_id", "Id", "id"])
                .ok_or_else(|| "release_run_lifecycle: a ReleaseRun row carried no id (cannot verify concurrent releases)".to_string())?
                .to_string();
            let repo = entity_field_str(e, &["repo", "Repo"]).unwrap_or("").to_string();
            let status = entity_field_str(e, &["Status", "status"]).unwrap_or("").to_string();
            Ok((id, repo, status))
        })
        .collect()
}

/// Pure: given other runs and this run's id+repo, return a conflicting active
/// release id for the same repo (excluding self), if any.
fn conflicting_active_release<'a>(
    runs: &'a [(String, String, String)],
    self_id: &str,
    repo: &str,
) -> Option<&'a str> {
    // GitHub owner/repo is case-insensitive, so "Owner/Repo" and "owner/repo"
    // are the same target — compare case-insensitively so a casing mismatch
    // cannot slip a second concurrent release past the guard.
    let repo_lc = repo.to_ascii_lowercase();
    runs.iter()
        .find(|(id, r, status)| {
            id != self_id
                && r.to_ascii_lowercase() == repo_lc
                && ACTIVE_RELEASE_STATES.contains(&status.as_str())
        })
        .map(|(id, _, _)| id.as_str())
}

/// Read `sha` out of the GitHub merge response; a non-merge response
/// (`merged: false`, an error `message`) becomes an error carrying that text.
fn parse_merge_sha(stdout: &str) -> Result<String, String> {
    let body: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| format!("unexpected GitHub response: {}", excerpt(stdout)))?;
    if body.get("merged").and_then(Value::as_bool) == Some(true) {
        return body
            .get("sha")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "merge response carried no sha".to_string());
    }
    Err(body
        .get("message")
        .and_then(Value::as_str)
        .map(|m| format!("GitHub refused the merge: {m}"))
        .unwrap_or_else(|| format!("GitHub did not merge: {}", excerpt(stdout))))
}

// -- Step 2: one health probe ----------------------------------------------------

fn check(
    ctx: &Context,
    handle: &SandboxHandle,
    release: &ReleaseFields,
) -> Result<(&'static str, Value), String> {
    validate_url(&release.health_url)?;
    if release.merge_sha.is_empty() {
        return Err("release_run_lifecycle: no merge_sha to watch for".to_string());
    }
    let command = probe_command(&release.health_url);
    let result = sandbox::sandbox_exec(ctx, handle, &command, "/")?;
    let probe = parse_probe(&result.stdout);
    let verdict = evaluate_probe(
        &probe,
        &release.merge_sha,
        release.check_count,
        release.max_checks,
        release.degraded_streak,
    );
    ctx.log(
        "info",
        &format!(
            "release_run_lifecycle: check {}/{} streak {} -> http {} status {:?} sha {:?}: {:?}",
            release.check_count,
            release.max_checks,
            release.degraded_streak,
            probe.http_status,
            probe.status,
            probe.git_sha,
            verdict
        ),
    );
    let observed = probe.git_sha.clone().unwrap_or_default();
    Ok(match verdict {
        Verdict::Healthy => ("CheckHealthy", json!({ "observed_sha": observed })),
        Verdict::Pending { degraded_streak } => (
            "CheckPending",
            // Emit the streak as a JSON NUMBER: the spec's `set_counter_from_param`
            // effect (kernel `SetCounterFromParam`) only accepts numbers and
            // silently drops a string, which would pin the streak at 0 and make
            // the 3-strike rollback never fire (ARN-394 re-review).
            json!({ "observed_sha": observed, "degraded_streak": degraded_streak }),
        ),
        Verdict::Unhealthy(reason) => (
            "CheckUnhealthy",
            json!({ "reason": reason, "observed_sha": observed }),
        ),
    })
}

/// `curl` the health endpoint from the computer; the HTTP status is appended
/// on its own marker line so a body that is not JSON still yields a verdict.
/// `health_url` is single-quoted AND validated by [`validate_url`] first.
fn probe_command(health_url: &str) -> String {
    // `-g` (globoff): a `[` or `]` in the URL must be a literal, not a curl glob
    // range — otherwise curl multi-fetches or exits 3 and every probe reads as
    // status 0 (Pending), burning the whole budget before a real rollback
    // (ARN-394 re-review).
    format!("curl -g -sS -m 15 -w '\\n{HTTP_STATUS_MARKER} %{{http_code}}' '{health_url}'")
}

/// What one probe observed.
#[derive(Debug, Default, PartialEq)]
struct Probe {
    http_status: u16,
    status: Option<String>,
    git_sha: Option<String>,
}

fn parse_probe(stdout: &str) -> Probe {
    let (body, status_line) = match stdout.rfind(HTTP_STATUS_MARKER) {
        Some(idx) => (&stdout[..idx], &stdout[idx + HTTP_STATUS_MARKER.len()..]),
        None => (stdout, ""),
    };
    let http_status = status_line.trim().parse::<u16>().unwrap_or(0);
    let json: Value = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
    Probe {
        http_status,
        status: json.get("status").and_then(Value::as_str).map(str::to_string),
        git_sha: json.get("git_sha").and_then(Value::as_str).map(str::to_string),
    }
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Healthy,
    /// Keep watching; carries the degraded-streak to persist for the next check
    /// (0 unless the new commit is serving degraded).
    Pending { degraded_streak: u64 },
    Unhealthy(String),
}

/// Decide the outcome of one probe.
///
/// - Healthy: HTTP 2xx, `status == healthy`, and the served `git_sha` is the
///   merge commit — the rollout landed and is serving cleanly.
/// - Unhealthy: the new commit is being served but reports non-2xx / not-healthy
///   for [`DEGRADED_STREAK_THRESHOLD`] consecutive probes (a real regression,
///   not a one-window warmup blip), OR the probe budget is spent without the
///   new commit becoming healthy. Both trigger the rollback.
/// - Pending: anything else while budget remains — old build still serving,
///   deploy in progress, transient non-2xx, or a not-yet-confirmed degraded
///   streak on the new commit (streak carried forward).
fn evaluate_probe(
    probe: &Probe,
    merge_sha: &str,
    check_count: u64,
    max_checks: u64,
    degraded_streak: u64,
) -> Verdict {
    let serving_new = probe.git_sha.as_deref() == Some(merge_sha);
    let ok = (200..300).contains(&probe.http_status);
    let healthy = probe.status.as_deref() == Some("healthy");

    if serving_new && ok && healthy {
        return Verdict::Healthy;
    }

    // The new commit is serving but not cleanly (non-2xx or status != healthy).
    // Require a consecutive streak before reverting, so one warmup window does
    // not roll back a healthy release.
    if serving_new {
        let streak = degraded_streak + 1;
        // Roll back on a confirmed degraded streak OR when the probe budget is
        // spent — so max_checks is a true upper bound even on the degraded path
        // (with a small max_checks the streak alone might never be reached).
        if streak >= DEGRADED_STREAK_THRESHOLD || check_count >= max_checks {
            return Verdict::Unhealthy(format!(
                "new commit {merge_sha} served degraded for {streak} consecutive checks by check {check_count}/{max_checks} (last: http {}, status {})",
                probe.http_status,
                probe.status.as_deref().unwrap_or("unknown")
            ));
        }
        return Verdict::Pending { degraded_streak: streak };
    }

    // Old commit still serving (or health has no sha yet). Reset the streak.
    if check_count >= max_checks {
        return Verdict::Unhealthy(format!(
            "rollout of {merge_sha} not healthy after {check_count} checks (last: http {}, status {}, sha {})",
            probe.http_status,
            probe.status.as_deref().unwrap_or("none"),
            probe.git_sha.as_deref().unwrap_or("none")
        ));
    }
    Verdict::Pending { degraded_streak: 0 }
}

// -- Step 3: rollback ------------------------------------------------------------

fn rollback(
    ctx: &Context,
    handle: &SandboxHandle,
    release: &ReleaseFields,
) -> Result<(&'static str, Value), String> {
    validate_repo(&release.repo)?;
    validate_sha(&release.merge_sha)?;
    let token = github_token(ctx, &release.repo)?;
    validate_github_token(&token)?;
    let command = rollback_command(&release.repo, &release.merge_sha, &token);
    let result = sandbox::sandbox_exec(ctx, handle, &command, "/")?;
    if result.exit_code != 0 {
        return Err(format!(
            "release_run_lifecycle: revert of {} in {} failed (exit {}): {}",
            release.merge_sha,
            release.repo,
            result.exit_code,
            excerpt(&result.stderr)
        ));
    }
    let revert_sha = parse_revert_sha(&result.stdout)
        .ok_or_else(|| format!("release_run_lifecycle: revert pushed but no sha printed: {}", excerpt(&result.stdout)))?;
    ctx.log(
        "info",
        &format!("release_run_lifecycle: reverted {} as {revert_sha} and pushed {RELEASE_BASE_BRANCH}", release.merge_sha),
    );
    Ok(("RollbackPushed", json!({ "revert_sha": revert_sha })))
}

/// Revert the merge commit on the base branch and push, from a FRESH, ambient-
/// config-free checkout. Hardening (ARN-394 re-review P0-4 — the deterministic
/// reused dir was code-execution-capable shared state):
/// - `mktemp -d` per attempt (never reuses a checkout a prior sandbox user
///   could have poisoned), removed on exit via `trap`;
/// - `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null` neutralize any
///   attacker-planted global/system git config (credential.helper, url
///   rewrites, `core.hooksPath`, `gpg.program`); `-c core.hooksPath=/dev/null
///   -c commit.gpgsign=false` disable repo hooks and signing on clone/revert;
/// - because the ambient credential helper is now off, the token is supplied
///   per-invocation via an `http.extraHeader` Authorization header (`$AUTH`, a
///   runtime var, not a literal in the stored command) — it is NEVER written
///   into the remote URL or `$DIR/.git/config`;
/// - the revert only auto-runs for a TRUE merge commit (>=2 parents → `-m 1`,
///   which reverts the whole PR merge). A single-parent tip means the PR was
///   merged out-of-band via squash/rebase: a squash tip is one commit but a
///   rebase tip is only the LAST of N, and the two are indistinguishable from
///   the tip alone — a plain revert of a rebase tip would silently leave the
///   earlier commits deployed while reporting success (Fable R5 P2). We refuse
///   (exit 3 → Fail) and escalate to a human rather than push a partial revert.
///   Our own workflow always merges via `merge_method=merge` (a 2-parent merge
///   commit), so the normal release path is always fully rollbackable;
/// - `GIT_TERMINAL_PROMPT=0` and a `timeout` around every network git call;
/// - a configured commit identity so `git revert` (which commits) never stalls;
/// - idempotent at the TIP only: re-print HEAD if HEAD already reverts this
///   merge, else revert and push.
/// Prints the resulting head sha on the last line. `repo`/`merge_sha` are
/// pre-validated (owner/name, 40-hex) so they are safe to interpolate.
fn rollback_command(repo: &str, merge_sha: &str, token: &str) -> String {
    let b = RELEASE_BASE_BRANCH;
    // Token is an App installation token or the tenant github_token (charset-
    // validated before interpolation). Assigned into $TOK for this command
    // only — never written into the remote URL or $DIR/.git/config.
    format!(
        "TOK='{token}'; \
         set -e; export GIT_TERMINAL_PROMPT=0 GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null; \
         AUTH=$(printf 'x-access-token:%s' \"$TOK\" | base64 | tr -d '\\n'); \
         HDR=\"http.extraHeader=Authorization: Basic $AUTH\"; \
         URL=\"https://github.com/{repo}.git\"; \
         DIR=$(mktemp -d); trap 'rm -rf \"$DIR\"' EXIT; \
         timeout 120 git -c core.hooksPath=/dev/null -c \"$HDR\" clone -q --branch {b} \"$URL\" \"$DIR\"; \
         cd \"$DIR\"; \
         git config core.hooksPath /dev/null; git config commit.gpgsign false; \
         git config user.name 'temperpaw-release'; git config user.email 'release@temperpaw.local'; \
         if git log -1 --format=%B HEAD | grep -qF \"This reverts commit {merge_sha}\"; then \
           git rev-parse HEAD; \
         else \
           NP=$(git rev-list --parents -n 1 {merge_sha} | wc -w); \
           if [ \"$NP\" -ge 3 ]; then \
             git -c core.hooksPath=/dev/null revert -m 1 --no-edit {merge_sha} >/dev/null; \
             timeout 60 git -c \"$HDR\" push -q \"$URL\" HEAD:{b}; git rev-parse HEAD; \
           else \
             echo 'release_run_lifecycle: {merge_sha} is not a merge commit (out-of-band squash/rebase merge) — a partial revert could leave part of the release deployed; refusing to auto-roll-back, escalate to a human' >&2; \
             exit 3; \
           fi; \
         fi"
    )
}

fn parse_revert_sha(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| is_full_sha(l))
        .map(str::to_string)
}

// -- Helpers -----------------------------------------------------------------------

/// Read a value from the trigger params, falling back to entity state.
fn param_or_field(ctx: &Context, fields: &Value, key: &str) -> Option<String> {
    ctx.trigger_params
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            entity_field_str(fields, &[key])
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

/// Counters arrive as numbers or numeric strings depending on the read path.
fn counter_field(fields: &Value, key: &str) -> u64 {
    match fields.get(key) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Fetch the Computer row by id via the Temper API loopback.
fn fetch_computer(
    ctx: &Context,
    temper_api_url: &str,
    fields: &Value,
    computer_id: &str,
) -> Result<Value, String> {
    let headers = odata_headers(ctx, &ctx.tenant, fields);
    let path = format!("/tdata/Computers('{}')", bounded_reads::odata_escape(computer_id));
    bounded_reads::get_json(ctx, temper_api_url, &path, &headers, "release_run_lifecycle")
}

/// Build a SandboxHandle from a Computer row's recorded fields. The computer
/// must be explicitly `Ready` with a sandbox_url — an empty/absent status is
/// NOT treated as ready (no fail-open on the gate that decides whether to run
/// commands on a sandbox).
fn sandbox_handle_from_computer(computer: &Value) -> Result<SandboxHandle, String> {
    let status = entity_field_str(computer, &["Status", "status"]).unwrap_or("");
    if status != "Ready" {
        return Err(format!("computer status is {status:?}, must be \"Ready\""));
    }
    let sandbox_url = entity_field_str(computer, &["SandboxUrl", "sandbox_url"])
        .map(str::trim)
        .unwrap_or("");
    if sandbox_url.is_empty() {
        return Err("no sandbox_url recorded — provision the computer first".to_string());
    }
    let sandbox_id = entity_field_str(computer, &["MachineId", "machine_id"])
        .filter(|s| !s.is_empty())
        .or_else(|| entity_field_str(computer, &["Name", "name"]).filter(|s| !s.is_empty()))
        .unwrap_or("computer-sandbox");
    let provider = entity_field_str(computer, &["Provider", "provider"])
        .filter(|s| !s.is_empty())
        .map(normalize_sandbox_provider)
        .unwrap_or_else(|| "tensorlake".to_string());
    Ok(SandboxHandle {
        sandbox_url: sandbox_url.to_string(),
        sandbox_id: sandbox_id.to_string(),
        provider,
    })
}

/// `owner/name` only — interpolated into shell commands, so anything else is
/// refused rather than quoted. Each part is a GitHub-legal segment: ASCII
/// alphanumerics plus `-_.`, not equal to `.`/`..`, and not starting with `-`
/// or `.` (which would let `owner/..` climb out of the workspace dir or a
/// leading `-` be read as a flag).
fn validate_repo(repo: &str) -> Result<(), String> {
    fn part_ok(p: &str) -> bool {
        !p.is_empty()
            && p != "."
            && p != ".."
            && !p.starts_with('-')
            && !p.starts_with('.')
            && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    }
    let mut parts = repo.split('/');
    let ok = matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(o), Some(n), None) if part_ok(o) && part_ok(n)
    );
    if ok {
        Ok(())
    } else {
        Err(format!("release_run_lifecycle: repo must be owner/name, got {repo:?}"))
    }
}

fn validate_number(value: &str, what: &str) -> Result<(), String> {
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        Ok(())
    } else {
        Err(format!("release_run_lifecycle: {what} must be numeric, got {value:?}"))
    }
}

/// Require a full 40-hex git sha. Used on every success arm (PR head, merge
/// commit) and before interpolating a sha into a shell command — GitHub always
/// returns 40-hex object shas, so a shorter/malformed value is a bad response we
/// refuse rather than watch/revert an unusable identity.
fn validate_sha(sha: &str) -> Result<(), String> {
    if is_full_sha(sha) {
        Ok(())
    } else {
        Err(format!("release_run_lifecycle: expected a 40-hex git sha, got {sha:?}"))
    }
}

fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Strictly validate the health URL before it is spliced into a shell command.
/// Must be `https://…` and contain only characters that are safe inside a
/// single-quoted shell string AND legal in a URL — no quotes, backslash,
/// whitespace, or shell metacharacters. The single-quoting in
/// [`probe_command`] is defense-in-depth; this allowlist is the real guard.
fn validate_url(url: &str) -> Result<(), String> {
    const DANGEROUS: &[char] = &[
        '\'', '"', '`', '\\', ';', '$', '<', '>', '|', '(', ')', '{', '}', '*', '!', ' ', '\t',
        '\n', '\r',
    ];
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '&' | '+' | ',' | '=' | '%'
            )
    };
    if !url.starts_with("https://") {
        return Err(format!("release_run_lifecycle: health_url must be https://, got {url:?}"));
    }
    if url.len() > 2048 {
        return Err("release_run_lifecycle: health_url is too long".to_string());
    }
    if let Some(bad) = url.chars().find(|c| DANGEROUS.contains(c) || !allowed(*c)) {
        return Err(format!(
            "release_run_lifecycle: health_url contains a disallowed character {bad:?}"
        ));
    }
    // SSRF guard: the probe is curled from the credentialed computer sandbox, so
    // the host must be an external service — never loopback, a private range, or
    // the cloud metadata endpoint (Greptile R6 P1). Syntax validation alone lets
    // an internal URL through; this pins the host to a public FQDN.
    validate_url_host(url)?;
    Ok(())
}

/// Reject a health_url whose host is loopback, a private/link-local range, the
/// 169.254 metadata range, or a bare (non-FQDN) internal name — the probe runs
/// from a sandbox with repo credentials, so it must only reach public endpoints.
fn validate_url_host(url: &str) -> Result<(), String> {
    let rest = &url["https://".len()..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Drop any userinfo (user@host); take the host, stripping the port. IPv6
    // literals are bracketed ([::1]:443) — take what's inside the brackets.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, is_ipv6) = if let Some(after) = host_port.strip_prefix('[') {
        (after.split(']').next().unwrap_or(""), true)
    } else {
        (host_port.split(':').next().unwrap_or(""), false)
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return Err("release_run_lifecycle: health_url has no host".to_string());
    }
    let private = if is_ipv6 {
        host == "::1"            // loopback
            || host.starts_with("fe80:")   // link-local
            || host.starts_with("fc")      // unique-local
            || host.starts_with("fd")
    } else {
        host == "localhost"
            || host.ends_with(".localhost")
            || host.starts_with("127.")    // loopback
            || host.starts_with("10.")     // private
            || host.starts_with("192.168.")
            || host.starts_with("169.254.") // link-local + cloud metadata
            || host.starts_with("0.")
            || is_172_private(&host)
            // A public health endpoint is always an FQDN or IP; a bare
            // single-label name (e.g. "internal") resolves only inside the
            // sandbox's network, so refuse it.
            || !host.contains('.')
    };
    if private {
        return Err(format!(
            "release_run_lifecycle: health_url host {host:?} is not a public endpoint (loopback/private/link-local/internal)"
        ));
    }
    Ok(())
}

/// 172.16.0.0 – 172.31.255.255 (RFC1918).
fn is_172_private(host: &str) -> bool {
    host.strip_prefix("172.")
        .and_then(|r| r.split('.').next())
        .and_then(|o| o.parse::<u8>().ok())
        .map(|n| (16..=31).contains(&n))
        .unwrap_or(false)
}

fn excerpt(text: &str) -> String {
    let t = text.trim();
    if t.len() <= 300 {
        t.to_string()
    } else {
        let mut end = 300;
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &t[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "3d9284a1b2c3d4e5f60718293a4b5c6d7e8f9012";

    // -- max_checks bounds ----------------------------------------------------

    #[test]
    fn max_checks_rejects_non_numeric_zero_and_huge() {
        assert_eq!(parse_max_checks("60").unwrap(), 60);
        assert_eq!(parse_max_checks(" 2 ").unwrap(), 2);
        assert!(parse_max_checks("banana").is_err());
        assert!(parse_max_checks("0").is_err());
        assert!(parse_max_checks("18446744073709551615").is_err());
        assert!(parse_max_checks(&(MAX_CHECKS_CEILING + 1).to_string()).is_err());
        assert_eq!(parse_max_checks(&MAX_CHECKS_CEILING.to_string()).unwrap(), MAX_CHECKS_CEILING);
    }

    // -- PR preflight / reconcile ---------------------------------------------

    #[test]
    fn github_urls_target_the_pr() {
        assert_eq!(
            github_pr_url("arni-labs/deep-sci-fi", "109"),
            "https://api.github.com/repos/arni-labs/deep-sci-fi/pulls/109"
        );
        assert_eq!(
            github_merge_url("arni-labs/deep-sci-fi", "106"),
            "https://api.github.com/repos/arni-labs/deep-sci-fi/pulls/106/merge"
        );
    }

    #[test]
    fn github_token_rejects_empty_and_shell_metacharacters() {
        assert!(validate_github_token("").is_err());
        assert!(validate_github_token("ghp_oktoken12").is_ok());
        assert!(validate_github_token("ghs_installtoken12").is_ok());
        assert!(validate_github_token("bad token").is_err());
        assert!(validate_github_token("x';curl").is_err());
        assert!(validate_github_token("{secret:github_token}").is_err());
    }

    #[test]
    fn github_token_accepts_stateless_app_installation_jwt() {
        // GitHub changelog 2026-04-24 / 2026-05-15: ghs_<appid>_<jwt>, ~520 chars, two dots.
        let token = format!(
            "ghs_123456_{}.{}.{}",
            "A".repeat(180),
            "B".repeat(180),
            "C".repeat(140)
        );
        assert!(token.len() > 256);
        assert!(token.contains('.'));
        assert!(validate_github_token(&token).is_ok());
        assert!(validate_github_token(&format!("ghs_{}", "x".repeat(1021))).is_err());
        assert!(validate_github_token("ghs_foo.bar;curl").is_err());
    }

    #[test]
    fn parse_pr_reads_base_head_and_merge_state() {
        let out = format!(
            r#"{{"base":{{"ref":"main"}},"head":{{"sha":"{SHA}"}},"merged":false,"merge_commit_sha":"{SHA}"}}"#
        );
        let pr = parse_pr(&out).unwrap();
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.head_sha, SHA);
        assert!(!pr.merged);
        assert_eq!(pr.merge_commit_sha.as_deref(), Some(SHA));
    }

    #[test]
    fn head_binding_allows_unbound_and_matching_only() {
        assert!(head_binding_ok("", "anything")); // unbound
        assert!(head_binding_ok(SHA, SHA)); // matches
        assert!(!head_binding_ok(SHA, "0000000")); // reviewed head moved
    }

    #[test]
    fn parse_pr_surfaces_github_error() {
        let err = parse_pr(r#"{"message":"Not Found"}"#).unwrap_err();
        assert!(err.contains("Not Found"), "{err}");
    }

    #[test]
    fn parse_pr_rejects_non_json() {
        assert!(parse_pr("curl: (7) failed").is_err());
    }

    // -- merge ----------------------------------------------------------------

    #[test]
    fn merge_body_pins_the_head_sha() {
        let body = format!(r#"{{"sha":"{SHA}","merge_method":"merge"}}"#);
        assert!(body.contains(&format!("\"sha\":\"{SHA}\"")));
        assert!(body.contains("\"merge_method\":\"merge\""));
    }

    #[test]
    fn per_repo_reject_finds_a_concurrent_active_release() {
        let runs = vec![
            ("self".into(), "a/b".into(), "Merging".into()),
            ("other-done".into(), "a/b".into(), "Healthy".into()), // terminal, ignore
            ("other-active".into(), "a/b".into(), "Watching".into()), // conflict
            ("elsewhere".into(), "c/d".into(), "Merging".into()),   // other repo
        ];
        assert_eq!(conflicting_active_release(&runs, "self", "a/b"), Some("other-active"));
        // No conflict when the only same-repo runs are self or terminal.
        let clean = vec![
            ("self".into(), "a/b".into(), "Merging".into()),
            ("done".into(), "a/b".into(), "RolledBack".into()),
        ];
        assert_eq!(conflicting_active_release(&clean, "self", "a/b"), None);
    }

    #[test]
    fn per_repo_reject_is_case_insensitive() {
        // GitHub owner/repo is case-insensitive: a differently-cased active run
        // for the same repo must still be caught.
        let runs = vec![("other".into(), "Arni-Labs/Deep-Sci-Fi".into(), "Watching".into())];
        assert_eq!(
            conflicting_active_release(&runs, "self", "arni-labs/deep-sci-fi"),
            Some("other")
        );
    }

    #[test]
    fn parse_release_runs_reads_kernel_row_shape() {
        // The kernel returns {entity_id, status, fields{...}} rows; a row that
        // missed field-stamping (only entity_id) must still parse, not be dropped.
        let resp = json!({"value":[
            {"entity_id":"r1","status":"Watching","fields":{"repo":"a/b","Status":"Watching"}},
            {"entity_id":"r2","status":"Failed","fields":{"repo":"a/b"}}
        ]});
        let runs = parse_release_runs(&resp).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().any(|(i, r, s)| i == "r1" && r == "a/b" && s == "Watching"));
        assert!(runs.iter().any(|(i, _, s)| i == "r2" && s == "Failed"));
    }

    #[test]
    fn parse_release_runs_fails_closed_on_malformed_response() {
        // A 200 with no `value` array must error (fail closed), not become an
        // empty list that lets a concurrent release through (Codex-1 R4 P2).
        assert!(parse_release_runs(&json!({"unexpected": true})).is_err());
        // A row missing every id key must error too, not be silently dropped.
        let no_id = json!({"value":[{"fields":{"repo":"a/b","Status":"Watching"}}]});
        assert!(parse_release_runs(&no_id).is_err());
    }

    #[test]
    fn merge_sha_is_read_from_a_merged_response() {
        let out = format!(r#"{{"sha":"{SHA}","merged":true,"message":"Pull Request successfully merged"}}"#);
        assert_eq!(parse_merge_sha(&out).unwrap(), SHA);
    }

    #[test]
    fn refused_merge_surfaces_githubs_message() {
        let out = r#"{"message":"Pull Request is not mergeable","documentation_url":"x"}"#;
        let err = parse_merge_sha(out).unwrap_err();
        assert!(err.contains("not mergeable"), "{err}");
    }

    #[test]
    fn non_json_merge_output_is_an_error_not_a_sha() {
        let err = parse_merge_sha("curl: (7) Failed to connect").unwrap_err();
        assert!(err.contains("unexpected GitHub response"), "{err}");
    }

    #[test]
    fn validators_refuse_shell_unsafe_inputs() {
        assert!(validate_repo("arni-labs/deep-sci-fi").is_ok());
        assert!(validate_repo("a/b; rm -rf /").is_err());
        assert!(validate_repo("no-slash").is_err());
        assert!(validate_repo("a/b/c").is_err());
        // `.`/`..`/leading-dot/leading-dash parts are refused (dir-climb, flag).
        assert!(validate_repo("owner/..").is_err());
        assert!(validate_repo("../etc").is_err());
        assert!(validate_repo("owner/.hidden").is_err());
        assert!(validate_repo("-owner/name").is_err());
        assert!(validate_number("106", "pr_number").is_ok());
        assert!(validate_number("106 || true", "pr_number").is_err());
        assert!(validate_number("", "pr_number").is_err());
    }

    // -- health_url validation (P0: shell injection) --------------------------

    #[test]
    fn validate_url_accepts_a_normal_https_health_url() {
        assert!(validate_url("https://deep-sci-fi-production.up.railway.app/health").is_ok());
        assert!(validate_url("https://x.example/health?full=1&k=v#frag").is_ok());
    }

    #[test]
    fn validate_url_rejects_injection_and_non_https() {
        // The exact exploit shape from the review.
        assert!(validate_url("https://x/'; cat ~/.git-credentials; '").is_err());
        assert!(validate_url("https://x/$(whoami)").is_err());
        assert!(validate_url("https://x/`id`").is_err());
        assert!(validate_url("https://x/a|b").is_err());
        assert!(validate_url("https://x/a b").is_err());
        assert!(validate_url("http://x/health").is_err());
        assert!(validate_url("").is_err());
        assert!(validate_url("https://x/\"q\"").is_err());
    }

    #[test]
    fn validate_url_rejects_internal_and_private_hosts() {
        // SSRF guard: the probe runs from a credentialed sandbox, so internal
        // targets are refused (Greptile R6 P1).
        assert!(validate_url("https://localhost/health").is_err());
        assert!(validate_url("https://127.0.0.1/health").is_err());
        assert!(validate_url("https://169.254.169.254/latest/meta-data").is_err()); // cloud metadata
        assert!(validate_url("https://10.0.0.5/health").is_err());
        assert!(validate_url("https://192.168.1.10/health").is_err());
        assert!(validate_url("https://172.16.0.9/health").is_err());
        assert!(validate_url("https://172.31.255.1/health").is_err());
        assert!(validate_url("https://[::1]/health").is_err());
        assert!(validate_url("https://internal/health").is_err()); // bare single-label name
        // A real public health endpoint still passes, incl. userinfo/port forms.
        assert!(validate_url("https://deep-sci-fi-production.up.railway.app/health").is_ok());
        assert!(validate_url("https://x.example:8443/health").is_ok());
        // 172.32+ is public, not RFC1918.
        assert!(validate_url("https://172.32.0.1/health").is_ok());
    }

    // -- probe ----------------------------------------------------------------

    #[test]
    fn probe_command_curls_health_and_appends_http_status() {
        let cmd = probe_command("https://deep-sci-fi-production.up.railway.app/health");
        assert!(cmd.starts_with("curl -g -sS -m 15"), "{cmd}");
        // -g/globoff so a `[` in the URL is literal, not a curl glob range.
        assert!(cmd.contains("curl -g "), "{cmd}");
        assert!(cmd.contains("__HTTP_STATUS %{http_code}"));
        assert!(cmd.ends_with("/health'"));
    }

    #[test]
    fn probe_parses_body_and_status_line() {
        let out = format!("{{\"status\":\"healthy\",\"git_sha\":\"{SHA}\"}}\n__HTTP_STATUS 200");
        let p = parse_probe(&out);
        assert_eq!(p.http_status, 200);
        assert_eq!(p.status.as_deref(), Some("healthy"));
        assert_eq!(p.git_sha.as_deref(), Some(SHA));
    }

    #[test]
    fn probe_without_json_body_still_yields_http_status() {
        let p = parse_probe("<html>502 Bad Gateway</html>\n__HTTP_STATUS 502");
        assert_eq!(p.http_status, 502);
        assert_eq!(p, Probe { http_status: 502, status: None, git_sha: None });
    }

    fn probe(http: u16, status: Option<&str>, sha: Option<&str>) -> Probe {
        Probe { http_status: http, status: status.map(String::from), git_sha: sha.map(String::from) }
    }

    #[test]
    fn healthy_when_new_commit_serves_healthy() {
        assert_eq!(evaluate_probe(&probe(200, Some("healthy"), Some(SHA)), SHA, 1, 20, 0), Verdict::Healthy);
    }

    #[test]
    fn pending_while_old_commit_still_serves_resets_streak() {
        assert_eq!(
            evaluate_probe(&probe(200, Some("healthy"), Some("oldsha00")), SHA, 1, 20, 2),
            Verdict::Pending { degraded_streak: 0 }
        );
    }

    #[test]
    fn pending_when_health_has_no_sha_yet() {
        assert_eq!(
            evaluate_probe(&probe(200, Some("healthy"), None), SHA, 3, 20, 0),
            Verdict::Pending { degraded_streak: 0 }
        );
    }

    #[test]
    fn pending_on_transient_502_while_old_commit_serves() {
        assert_eq!(
            evaluate_probe(&probe(502, None, None), SHA, 2, 20, 0),
            Verdict::Pending { degraded_streak: 0 }
        );
    }

    #[test]
    fn single_degraded_probe_on_new_commit_does_not_roll_back() {
        // First degraded window on the new sha: streak -> 1, keep watching.
        assert_eq!(
            evaluate_probe(&probe(200, Some("degraded"), Some(SHA)), SHA, 3, 20, 0),
            Verdict::Pending { degraded_streak: 1 }
        );
        // Second: streak -> 2, still watching.
        assert_eq!(
            evaluate_probe(&probe(503, None, Some(SHA)), SHA, 4, 20, 1),
            Verdict::Pending { degraded_streak: 2 }
        );
    }

    #[test]
    fn consecutive_degraded_streak_rolls_back() {
        match evaluate_probe(&probe(200, Some("degraded"), Some(SHA)), SHA, 5, 20, 2) {
            Verdict::Unhealthy(reason) => {
                assert!(reason.contains("3 consecutive"), "{reason}");
                assert!(reason.contains("degraded"), "{reason}");
            }
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }

    #[test]
    fn matching_sha_with_http_500_counts_toward_the_streak() {
        // Serving the new sha but HTTP 500 is degraded, not pending.
        assert_eq!(
            evaluate_probe(&probe(500, Some("healthy"), Some(SHA)), SHA, 5, 20, 0),
            Verdict::Pending { degraded_streak: 1 }
        );
    }

    #[test]
    fn unhealthy_once_the_check_budget_is_spent() {
        match evaluate_probe(&probe(200, Some("healthy"), Some("oldsha00")), SHA, 20, 20, 0) {
            Verdict::Unhealthy(reason) => {
                assert!(reason.contains("after 20 checks"), "{reason}");
                assert!(reason.contains("oldsha00"), "{reason}");
            }
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }

    #[test]
    fn budget_is_not_spent_one_check_early() {
        assert_eq!(
            evaluate_probe(&probe(200, Some("healthy"), Some("oldsha00")), SHA, 19, 20, 0),
            Verdict::Pending { degraded_streak: 0 }
        );
    }

    // -- rollback -------------------------------------------------------------

    #[test]
    fn rollback_is_freshly_isolated_and_idempotent() {
        let cmd = rollback_command("arni-labs/deep-sci-fi", SHA, "ghp_testdummytoken12");
        assert!(cmd.contains("GIT_TERMINAL_PROMPT=0"));
        // Fresh per-attempt checkout, cleaned up — no reuse of a poisonable dir.
        assert!(cmd.contains("mktemp -d"), "{cmd}");
        assert!(cmd.contains("trap 'rm -rf \"$DIR\"' EXIT"), "{cmd}");
        assert!(!cmd.contains("~/workspace/release-"), "{cmd}");
        // Ambient git config/hooks neutralized against a poisoned sandbox.
        assert!(cmd.contains("GIT_CONFIG_GLOBAL=/dev/null"), "{cmd}");
        assert!(cmd.contains("GIT_CONFIG_SYSTEM=/dev/null"), "{cmd}");
        assert!(cmd.contains("core.hooksPath=/dev/null"), "{cmd}");
        assert!(cmd.contains("commit.gpgsign false"), "{cmd}");
        // Token supplied via an Authorization header ($AUTH/$TOK runtime vars),
        // NOT baked into the remote URL — so it never lands in $DIR/.git/config.
        assert!(cmd.contains("TOK='ghp_testdummytoken12'"), "{cmd}");
        assert!(!cmd.contains("~/.git-credentials"), "{cmd}");
        assert!(cmd.contains("http.extraHeader=Authorization: Basic $AUTH"), "{cmd}");
        assert!(cmd.contains("URL=\"https://github.com/arni-labs/deep-sci-fi.git\""), "{cmd}");
        assert!(!cmd.contains("x-access-token:$TOK@github.com"), "no token in URL: {cmd}");
        // Configured identity so `git revert` can commit.
        assert!(cmd.contains("git config user.email"));
        // Bounded network calls.
        assert!(cmd.contains("timeout 120 git"));
        assert!(cmd.contains("timeout 60 git -c \"$HDR\" push"), "{cmd}");
        // Auto-revert ONLY a true merge commit (≥2 parents → `-m 1`, reverts the
        // whole PR merge). A single-parent tip (out-of-band squash/rebase) is
        // refused (exit 3 → Fail → escalate), never partially reverted while
        // reporting success (Fable R5 P2).
        assert!(cmd.contains("git rev-list --parents -n 1"), "{cmd}");
        assert!(cmd.contains("if [ \"$NP\" -ge 3 ]; then"), "{cmd}");
        assert!(cmd.contains(&format!("revert -m 1 --no-edit {SHA}")), "{cmd}");
        assert!(cmd.contains("exit 3"), "{cmd}");
        assert!(!cmd.contains("revert $MF"), "no blind plain revert of a single-parent tip: {cmd}");
        assert!(cmd.contains("HEAD:main"), "{cmd}");
        // Idempotency: skip ONLY if the revert of this merge is at the TIP
        // (HEAD's message), not anywhere in history (ARN-394 re-review).
        assert!(cmd.contains(&format!("This reverts commit {SHA}")));
        assert!(cmd.contains("git log -1 --format=%B HEAD"), "{cmd}");
        assert!(!cmd.contains("git log origin/main --grep"), "{cmd}");
    }

    #[test]
    fn revert_sha_is_the_last_sha_line() {
        let out = format!("Switched to branch 'main'\n{SHA}\n");
        assert_eq!(parse_revert_sha(&out).as_deref(), Some(SHA));
        assert_eq!(parse_revert_sha("nothing here"), None);
    }

    #[test]
    fn rollback_refuses_a_non_sha() {
        assert!(validate_sha(SHA).is_ok());
        assert!(validate_sha("main; rm -rf ~").is_err());
        assert!(validate_sha("").is_err());
        // Strict 40-hex: a short (7-hex) sha is refused on the success arms — a
        // real GitHub object sha is always 40 (Codex-1/Fable R4 P3).
        assert!(validate_sha("3d9284a").is_err());
    }

    fn release_fixture(expected_head_sha: &str) -> ReleaseFields {
        ReleaseFields {
            repo: "a/b".into(),
            pr_number: "1".into(),
            computer_id: "c1".into(),
            health_url: "https://h.example".into(),
            max_checks: 60,
            merge_sha: String::new(),
            check_count: 0,
            degraded_streak: 0,
            expected_head_sha: expected_head_sha.into(),
        }
    }

    fn pr_merged_into(base: &str, head: &str, merge_sha: &str) -> PrInfo {
        PrInfo {
            base_ref: base.into(),
            head_sha: head.into(),
            merged: true,
            merge_commit_sha: Some(merge_sha.into()),
        }
    }

    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MERGE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn confirm_merged_pr_refuses_a_retargeted_base() {
        // base-branch TOCTOU: a PR retargeted off main after preflight and
        // merged there must NOT be blessed / watched / reverted (all 3 R4).
        let r = release_fixture("");
        let pr = pr_merged_into("release/1.2", HEAD, MERGE);
        let err = confirm_merged_pr(&pr, &r, None).unwrap_err();
        assert!(err.contains("only main is releasable"), "{err}");
    }

    #[test]
    fn confirm_merged_pr_enforces_head_binding_and_valid_sha() {
        // Bound to a reviewed head that no longer matches → refused.
        let bound = release_fixture(HEAD);
        let moved = pr_merged_into("main", "cccccccccccccccccccccccccccccccccccccccc", MERGE);
        assert!(confirm_merged_pr(&moved, &bound, None).unwrap_err().contains("unreviewed"));
        // A short/malformed merge sha is refused (not watched).
        let bad_sha = pr_merged_into("main", HEAD, "3d9284a");
        assert!(confirm_merged_pr(&bad_sha, &release_fixture(""), None).is_err());
        // Unbound + main + valid sha → MergeSucceeded carrying the merge sha.
        let (action, payload) = confirm_merged_pr(&pr_merged_into("main", HEAD, MERGE), &release_fixture(""), None).unwrap();
        assert_eq!(action, "MergeSucceeded");
        assert_eq!(payload["merge_sha"], MERGE);
        assert_eq!(payload["base_branch"], "main");
    }

    #[test]
    fn confirm_merged_pr_binds_to_the_pinned_head() {
        // Post-PUT path: if the PUT was refused (head moved) and a DIFFERENT head
        // was merged out-of-band, the merged head won't match the head we pinned
        // → refuse, even when unbound by expected_head_sha (Codex-2 R5).
        let r = release_fixture(""); // unbound
        let other = pr_merged_into("main", "dddddddddddddddddddddddddddddddddddddddd", MERGE);
        let err = confirm_merged_pr(&other, &r, Some(HEAD)).unwrap_err();
        assert!(err.contains("we pinned") && err.contains("did not merge"), "{err}");
        // Merged at exactly the pinned head → allowed.
        let matched = pr_merged_into("main", HEAD, MERGE);
        assert_eq!(confirm_merged_pr(&matched, &r, Some(HEAD)).unwrap().0, "MergeSucceeded");
    }

    #[test]
    fn degraded_new_commit_rolls_back_when_budget_is_spent() {
        // serving the new commit degraded, streak below threshold, but the probe
        // budget is spent → Unhealthy (max_checks is a true upper bound even on
        // the degraded path, Codex-1 R4 P3).
        match evaluate_probe(&probe(503, Some("degraded"), Some(SHA)), SHA, 20, 20, 0) {
            Verdict::Unhealthy(_) => {}
            v => panic!("expected Unhealthy at budget on degraded path, got {v:?}"),
        }
    }

    // -- helpers --------------------------------------------------------------

    #[test]
    fn counter_reads_number_or_string() {
        assert_eq!(counter_field(&json!({"check_count": 3}), "check_count"), 3);
        assert_eq!(counter_field(&json!({"check_count": "7"}), "check_count"), 7);
        assert_eq!(counter_field(&json!({}), "check_count"), 0);
    }

    #[test]
    fn handle_requires_an_explicitly_ready_computer() {
        let ready = json!({"Status":"Ready","fields":{"sandbox_url":"https://s.example","machine_id":"m1","provider":"tl"}});
        let h = sandbox_handle_from_computer(&ready).unwrap();
        assert_eq!(h.sandbox_url, "https://s.example");
        assert_eq!(h.provider, "tensorlake");
        // Not Ready.
        let sleeping = json!({"Status":"Sleeping","fields":{"sandbox_url":"https://s.example"}});
        assert!(sandbox_handle_from_computer(&sleeping).err().unwrap().contains("must be"));
        // No fail-open on empty/absent status.
        let no_status = json!({"fields":{"sandbox_url":"https://s.example"}});
        assert!(sandbox_handle_from_computer(&no_status).is_err());
        let bare = json!({"Status":"Ready","fields":{"sandbox_url":""}});
        assert!(sandbox_handle_from_computer(&bare).err().unwrap().contains("no sandbox_url"));
    }

    #[test]
    fn concurrent_entity_set_accepts_only_known_sets() {
        assert_eq!(concurrent_entity_set_name(None).unwrap(), "ReleaseRuns");
        assert_eq!(concurrent_entity_set_name(Some("")).unwrap(), "ReleaseRuns");
        assert_eq!(
            concurrent_entity_set_name(Some("DsfDeploys")).unwrap(),
            "DsfDeploys"
        );
        assert!(concurrent_entity_set_name(Some("Deploys")).is_err());
    }

    #[test]
    fn concurrent_scan_covers_release_run_and_dsf_deploy() {
        assert_eq!(CONCURRENT_SCAN_SETS, ["ReleaseRuns", "DsfDeploys"]);
    }
}
