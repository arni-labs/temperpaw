#[test]
fn github_patrol_parser_extracts_issue_and_pr_agent_findings() {
    let output = r##"
Codex investigated GitHub with authenticated repo tools.
GITHUB_PATROL_RESULT_JSON_BEGIN
{
  "summary": "Two repository items need Patrol attention.",
  "evidence_scope": [
    {"surface":"open issues","query":"reviewed open issues by updated time and labels","result_summary":"issue #12 is actionable and unassigned","github_url":"https://github.com/nerdsane/temperpaw/issues/12"},
    {"surface":"open pull requests","query":"reviewed open PRs, checks, and review state","result_summary":"PR #34 has failing CI after review","github_url":"https://github.com/nerdsane/temperpaw/pull/34"}
  ],
  "findings": [
    {
      "title": "Issue #12 needs implementation triage",
      "severity": "warn",
      "risk_lane": "L1",
      "source_url": "https://github.com/nerdsane/temperpaw/issues/12",
      "source_kind": "issue",
      "fingerprint": "github:issue:12",
      "affected_refs": ["#12"],
      "evidence_json": {"facts":["unassigned actionable issue"]},
      "work_summary": "Triage and implement issue #12 if safe",
      "work_detail": "Read issue #12, decide the smallest safe Temper-native fix, test it, and open/update a PR.",
      "requires_human_approval": false
    },
    {
      "title": "PR #34 failing CI after review",
      "severity": "error",
      "risk_lane": "L2",
      "source_url": "https://github.com/nerdsane/temperpaw/pull/34",
      "source_kind": "pull_request",
      "fingerprint": "github:pr:34:ci",
      "affected_refs": ["#34"],
      "evidence_json": {"facts":["CI failed after requested changes"]},
      "work_summary": "Investigate PR #34 CI failure",
      "work_detail": "Inspect PR #34 and CI logs, then decide whether to request changes or create follow-up work.",
      "requires_human_approval": true
    }
  ],
  "residual_risks": ["GitHub search was point-in-time."],
  "recommended_next_queries": ["Re-check PR #34 checks before starting work."]
}
GITHUB_PATROL_RESULT_JSON_END
"##;

    let investigation =
        parse_github_patrol_investigation_output(output).expect("parse GitHub patrol output");

    assert_eq!(
        investigation.summary,
        "Two repository items need Patrol attention."
    );
    assert_eq!(investigation.evidence_scope.len(), 2);
    assert_eq!(investigation.findings.len(), 2);
    assert_eq!(investigation.findings[0].source_kind, "issue");
    assert_eq!(investigation.findings[1].source_kind, "pull_request");
    assert!(investigation.findings[1].requires_human_approval);
}

#[test]
fn github_patrol_classifier_ignores_followup_and_rework_prompts() {
    let patrol_task = "You are the local Codex GitHub Patrol agent for TemperPaw paw-patrol.\n\nPatrolRun: en-patrol\nPatrolKind: github_repository";
    let implementer_task = "You are the local Codex implementer for a Paw Patrol GitHub repository finding.\n\nPatrolRun: en-patrol\nPatrol kind: github_repository\nFinding: Issue #12 needs implementation triage";
    let rework_task = "You are the local Codex implementer for reviewer-requested rework.\n\nOriginal task:\nYou are the local Codex GitHub Patrol agent for TemperPaw paw-patrol.\n\nPatrolRun: en-patrol\nPatrolKind: github_repository";

    assert_eq!(
        extract_github_patrol_run_id(patrol_task).as_deref(),
        Some("en-patrol")
    );
    assert_eq!(extract_github_patrol_run_id(implementer_task), None);
    assert_eq!(extract_github_patrol_run_id(rework_task), None);
}
