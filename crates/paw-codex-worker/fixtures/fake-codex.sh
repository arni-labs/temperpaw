#!/bin/sh
set -eu

if [ "${1:-}" = "--version" ]; then
  echo "fake-codex 0.1.0"
  exit 0
fi

if [ "${1:-}" != "exec" ]; then
  echo "fake-codex only supports --version and exec" >&2
  exit 2
fi

shift
while [ "$#" -gt 0 ]; do
  case "${1:-}" in
    --dangerously-bypass-approvals-and-sandbox|--ignore-user-config|--ephemeral)
      shift
      ;;
    --sandbox)
      shift
      if [ "$#" -eq 0 ]; then
        echo "--sandbox requires a value" >&2
        exit 2
      fi
      shift
      ;;
    --cd)
      shift
      if [ "$#" -eq 0 ]; then
        echo "--cd requires a directory" >&2
        exit 2
      fi
      cd "$1"
      shift
      ;;
    --skip-git-repo-check)
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "fake-codex does not support option: $1" >&2
      exit 2
      ;;
    *)
      break
      ;;
  esac
done
prompt="${1:-}"

case "$prompt" in
  "PAW_FAKE_CODEX_HANG:"*)
    sleep "${PAW_FAKE_CODEX_SLEEP_SECS:-5}"
    echo "fake codex unexpectedly woke up"
    ;;
  "PAW_FAKE_CODEX_ORPHAN:"*)
    marker="${prompt#PAW_FAKE_CODEX_ORPHAN:}"
    (
      trap "" HUP
      sleep 1
      printf '%s\n' "fake orphan survived" > "$marker"
    ) &
    sleep "${PAW_FAKE_CODEX_SLEEP_SECS:-5}"
    echo "fake codex unexpectedly woke up"
    ;;
  "PAW_CODEX_DOCTOR_EXEC_SMOKE:"*)
    echo "PAW_CODEX_DOCTOR_EXEC_OK"
    ;;
  "PAW_FAKE_CODEX_PRINT_OBSERVE_ENV:"*)
    printf 'TEMPER_OBSERVE_METADATA=%s\n' "${TEMPER_OBSERVE_METADATA:-}"
    printf 'DD_TAGS=%s\n' "${DD_TAGS:-}"
    ;;
  "You are the independent reviewer"* | "You are the independent repo-health Patrol scan reviewer"*)
    echo "SUMMARY: Fake reviewer approved the agent-led worker E2E output."
    echo "LIVE_E2E: Confirmed the fake implementer marker exists in the assigned worktree."
    echo "VERDICT: approve"
    ;;
  *"Datadog MCP Risk Patrol agent"* | *"Datadog MCP Patrol agent"*)
    cat <<'JSON'
Fake Codex used its Datadog MCP fixture.
DATADOG_PATROL_RESULT_JSON_BEGIN
{
  "summary": "Fake Datadog MCP patrol found one actionable Discord-facing issue.",
  "evidence_scope": [
    {"surface":"monitors","query":"fixture monitor search","result_summary":"one monitor-like signal reviewed","datadog_url":""},
    {"surface":"logs","query":"fixture production error search","result_summary":"one Discord trace leak sample reviewed","datadog_url":""},
    {"surface":"traces","query":"fixture APM trace search","result_summary":"Discord request trace checked","datadog_url":""},
    {"surface":"metrics","query":"fixture error-rate metric search","result_summary":"error-rate metric checked","datadog_url":""},
    {"surface":"incidents","query":"fixture incident search","result_summary":"no open incident in fixture","datadog_url":""},
    {"surface":"dashboards","query":"fixture dashboard review","result_summary":"runtime dashboard reviewed","datadog_url":""}
  ],
  "findings": [
    {
      "title": "Fixture Discord trace leak",
      "severity": "error",
      "risk_lane": "L2",
      "source_url": "",
      "datadog_monitor_id": "",
      "fingerprint": "datadog:mcp:fixture-discord-trace-leak",
      "affected_services": ["temperpaw-production"],
      "evidence_json": {"surface":"logs","sample_count":1},
      "work_summary": "Sanitize Discord trace output",
      "work_detail": "Add regression coverage and live Discord-facing proof for sanitized errors.",
      "requires_human_approval": true
    }
  ],
  "residual_risks": ["fixture only"],
  "recommended_next_queries": ["fixture follow-up query"]
}
DATADOG_PATROL_RESULT_JSON_END
JSON
    ;;
  *"local Codex DailyBrief agent"*)
    cat <<'JSON'
Fake Codex rendered the DailyBrief fixture.
DAILY_BRIEF_RESULT_JSON_BEGIN
{
  "summary_markdown": "# Patrol Daily Brief\n\n```mermaid\nflowchart LR\n  Proofs[\"Ready ProofPackets\"] --> Brief[\"DailyBrief.Render\"]\n  Risks[\"Open risks\"] --> Brief\n```\n\nFake DailyBrief summarized the current Patrol proof and risk facts.",
  "visual_summary_url": "data:image/svg+xml,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20width%3D%271200%27%20height%3D%27720%27%20viewBox%3D%270%200%201200%20720%27%3E%3Crect%20width%3D%271200%27%20height%3D%27720%27%20fill%3D%27%23f8fafc%27%2F%3E%3Ctext%20x%3D%2780%27%20y%3D%27140%27%20font-family%3D%27Inter%2C%20Arial%2C%20sans-serif%27%20font-size%3D%2754%27%20font-weight%3D%27700%27%20fill%3D%27%230f172a%27%3EPatrol%20Daily%20Brief%3C%2Ftext%3E%3Ctext%20x%3D%2780%27%20y%3D%27220%27%20font-family%3D%27Inter%2C%20Arial%2C%20sans-serif%27%20font-size%3D%2728%27%20fill%3D%27%23334155%27%3EFake%20Codex%20brief%20fixture%3C%2Ftext%3E%3C%2Fsvg%3E",
  "proof_packet_ids": [],
  "open_risks": [],
  "done_items": [{"type":"DailyBrief","id":"fixture","summary":"Fake DailyBrief rendered"}],
  "residual_risks": ["fixture only"]
}
DAILY_BRIEF_RESULT_JSON_END
JSON
    ;;
  *"repo-health Patrol agent"*)
    cat <<'JSON'
Fake Codex investigated the repo health fixture.
REPO_HEALTH_PATROL_RESULT_JSON_BEGIN
{
  "summary_markdown": "# Repo Health Patrol\n\n```mermaid\nflowchart LR\n  Codex[\"Codex repo-health agent\"] --> Graph[\"Repo graph evidence\"]\n  Graph --> Findings[\"Quality + security findings\"]\n  Findings --> Temper[\"RepoGraphSnapshot.ScanComplete\"]\n```\n\nFake repo-health patrol found one actionable readability issue.",
  "evidence_scope": [
    {"surface":"codebase_graph","query_or_command":"rg --files","result_summary":"fixture file graph inspected"},
    {"surface":"wasm_modules","query_or_command":"rg os-apps --glob Cargo.toml","result_summary":"fixture WASM surface inspected"},
    {"surface":"specs_policies","query_or_command":"rg cedar ioa","result_summary":"fixture specs and policies inspected"},
    {"surface":"dependencies","query_or_command":"cargo metadata --no-deps","result_summary":"fixture dependency surface inspected"},
    {"surface":"tests_proofs","query_or_command":"rg \"#\\[test\\]|ProofPacket\"","result_summary":"fixture tests and proofs inspected"},
    {"surface":"security_readability","query_or_command":"rg \"TODO|HACK|tokio::spawn|sleep\"","result_summary":"fixture security/readability patterns inspected"}
  ],
  "quality_findings": [
    {
      "fingerprint": "quality:fixture-giant-module",
      "title": "Fixture giant module should be split",
      "severity": "medium",
      "evidence": "fixture/src/lib.rs mixes parsing, routing, and proof rendering concerns.",
      "affected_paths": ["fixture/src/lib.rs"]
    }
  ],
  "security_findings": [],
  "summary": {
    "scanned_files": 12,
    "scanned_lines": 1200,
    "giant_modules": 1,
    "todo_hack_hits": 0,
    "duplicate_logic_candidates": 0,
    "broad_cedar_policies": 0,
    "dependency_risk_hits": 0,
    "rust_orchestration_hits": 0,
    "polling_loop_hits": 0,
    "missing_test_coverage_hits": 0
  },
  "residual_risks": ["fixture only"],
  "recommended_next_actions": ["open a cleanup WorkCycle for fixture/src/lib.rs"]
}
REPO_HEALTH_PATROL_RESULT_JSON_END
JSON
    ;;
  *)
    printf '%s\n' "$prompt" > .paw-fake-codex-implementation
    echo "Fake implementation completed and wrote .paw-fake-codex-implementation."
    ;;
esac
