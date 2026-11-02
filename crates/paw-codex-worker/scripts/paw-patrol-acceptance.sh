#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
MODE="${1:-quick}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
PROOF_DIR="${PROOF_DIR:-/tmp/paw-patrol-acceptance-${STAMP}-$$}"
ACCEPTANCE_LOG="${PROOF_DIR}/acceptance.log"
STEPS_FILE="${PROOF_DIR}/steps.tsv"
GIT_HEAD="$(git -C "$ROOT" rev-parse HEAD)"
GIT_BRANCH="$(git -C "$ROOT" branch --show-current)"
if [[ -z "$GIT_BRANCH" ]]; then
  GIT_BRANCH="(detached)"
fi
GIT_STATUS_SHORT="$(git -C "$ROOT" status --short)"
if [[ -z "$GIT_STATUS_SHORT" ]]; then
  GIT_CLEAN="true"
  GIT_STATUS_LABEL="clean"
else
  GIT_CLEAN="false"
  GIT_STATUS_LABEL="dirty"
fi

log() {
  printf '[paw-patrol-acceptance] %s\n' "$*" | tee -a "$ACCEPTANCE_LOG"
}

fail() {
  log "$*"
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

run_step() {
  local name="$1"
  shift
  local step_log="${PROOF_DIR}/${name}.log"

  log "running ${name}: $*"
  if "$@" >"$step_log" 2>&1; then
    printf '%s\tpassed\t%s\n' "$name" "$step_log" >>"$STEPS_FILE"
    log "passed ${name}"
  else
    local status="$?"
    printf '%s\tfailed\t%s\n' "$name" "$step_log" >>"$STEPS_FILE"
    log "failed ${name}; tailing ${step_log}"
    tail -120 "$step_log" | tee -a "$ACCEPTANCE_LOG" || true
    exit "$status"
  fi
}

write_artifact_link() {
  local label="$1"
  local path="$2"
  local href="${path#${PROOF_DIR}/}"

  if [[ -s "$path" ]]; then
    cat >>"${PROOF_DIR}/index.html" <<EOF
          <p><a href="${href}">${label}</a></p>
EOF
  else
    cat >>"${PROOF_DIR}/index.html" <<EOF
          <p class="muted">${label} not generated in ${MODE} mode</p>
EOF
  fi
}

write_visual_card() {
  local title="$1"
  local path="$2"
  local href="${path#${PROOF_DIR}/}"

  if [[ -s "$path" ]]; then
    cat >>"${PROOF_DIR}/index.html" <<EOF
      <section class="visual-card">
        <h2>${title}</h2>
        <a href="${href}"><img src="${href}" alt="${title} visual proof"></a>
      </section>
EOF
  fi
}

write_html_index() {
  local steps_rows
  steps_rows="$(awk -F '\t' '
    {
      status_class = $2 == "passed" ? "passed" : "failed";
      log_path = $3;
      sub("^.*/", "", log_path);
      printf "          <tr><td>%s</td><td class=\"%s\">%s</td><td><a href=\"%s\">log</a></td></tr>\n", $1, status_class, $2, log_path
    }
  ' "$STEPS_FILE")"

  cat >"${PROOF_DIR}/index.html" <<EOF
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Paw Patrol Acceptance Proof</title>
    <style>
      :root { color-scheme: light dark; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      body { margin: 0; background: #f7f5ef; color: #202124; }
      main { max-width: 1120px; margin: 0 auto; padding: 32px 20px 48px; }
      header { display: grid; gap: 10px; border-bottom: 2px solid #2f6f73; padding-bottom: 20px; }
      h1 { margin: 0; font-size: 28px; }
      h2 { margin: 0 0 12px; font-size: 18px; }
      a { color: #0b5d69; }
      .meta { display: flex; flex-wrap: wrap; gap: 8px; }
      .pill { border: 1px solid #8aa6a5; border-radius: 999px; padding: 4px 10px; background: #ffffff; font-size: 13px; }
      .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; margin-top: 22px; }
      .card, .visual-card { background: #ffffff; border: 1px solid #d5d2c6; border-radius: 8px; padding: 16px; }
      .visual-card img { display: block; width: 100%; height: auto; border: 1px solid #d5d2c6; border-radius: 6px; background: #fff; }
      .muted { color: #64615a; }
      table { width: 100%; border-collapse: collapse; margin-top: 12px; background: #ffffff; border: 1px solid #d5d2c6; }
      th, td { padding: 8px 10px; border-bottom: 1px solid #e5e1d8; text-align: left; font-size: 14px; }
      .passed { color: #137333; font-weight: 700; }
      .failed { color: #a50e0e; font-weight: 700; }
      @media (prefers-color-scheme: dark) {
        body { background: #171918; color: #f1f3f4; }
        .pill, .card, .visual-card, table { background: #202423; border-color: #43504e; }
        th, td { border-bottom-color: #394341; }
        a { color: #8fd7df; }
      }
    </style>
  </head>
  <body>
    <main>
      <header>
        <h1>Paw Patrol Acceptance Proof</h1>
        <div class="meta">
          <span class="pill">mode: ${MODE}</span>
          <span class="pill">status: passed</span>
          <span class="pill">git: ${GIT_BRANCH} @ ${GIT_HEAD}</span>
          <span class="pill">worktree: ${GIT_STATUS_LABEL}</span>
          <span class="pill">proof bundle: ${PROOF_DIR}</span>
        </div>
      </header>

      <section class="grid">
        <article class="card">
          <h2>Core Files</h2>
EOF

  write_artifact_link "proof.md" "${PROOF_DIR}/proof.md"
  write_artifact_link "summary.json" "${PROOF_DIR}/summary.json"
  write_artifact_link "acceptance.log" "${PROOF_DIR}/acceptance.log"
  write_artifact_link "steps.tsv" "${PROOF_DIR}/steps.tsv"

  cat >>"${PROOF_DIR}/index.html" <<EOF
        </article>
        <article class="card">
          <h2>Live Proof Bundles</h2>
EOF

  write_artifact_link "deterministic-smoke/proof.md" "${PROOF_DIR}/deterministic-smoke/proof.md"
  write_artifact_link "datadog-patrol-smoke/proof.md" "${PROOF_DIR}/datadog-patrol-smoke/proof.md"
  write_artifact_link "datadog-patrol-smoke/summary.json" "${PROOF_DIR}/datadog-patrol-smoke/summary.json"
  write_artifact_link "datadog-patrol-smoke/datadog-patrol.svg" "${PROOF_DIR}/datadog-patrol-smoke/datadog-patrol.svg"
  write_artifact_link "webhook-intake-smoke/proof.md" "${PROOF_DIR}/webhook-intake-smoke/proof.md"
  write_artifact_link "repo-sweep-brief-smoke/proof.md" "${PROOF_DIR}/repo-sweep-brief-smoke/proof.md"
  write_artifact_link "repo-sweep-brief-smoke/patrol-schedule.json" "${PROOF_DIR}/repo-sweep-brief-smoke/patrol-schedule.json"
  write_artifact_link "production-preflight/proof.md" "${PROOF_DIR}/production-preflight/proof.md"
  write_artifact_link "production-preflight/operator-handoff.md" "${PROOF_DIR}/production-preflight/operator-handoff.md"
  write_artifact_link "production-preflight/summary.json" "${PROOF_DIR}/production-preflight/summary.json"
  write_artifact_link "production-preflight/preflight.svg" "${PROOF_DIR}/production-preflight/preflight.svg"
  write_artifact_link "production-preflight-railway-discovery-smoke/proof.md" "${PROOF_DIR}/production-preflight-railway-discovery-smoke/proof.md"
  write_artifact_link "production-preflight-railway-discovery-smoke/operator-handoff.md" "${PROOF_DIR}/production-preflight-railway-discovery-smoke/operator-handoff.md"
  write_artifact_link "production-preflight-railway-discovery-smoke/summary.json" "${PROOF_DIR}/production-preflight-railway-discovery-smoke/summary.json"
  write_artifact_link "production-preflight-railway-discovery-smoke/railway-candidates.json" "${PROOF_DIR}/production-preflight-railway-discovery-smoke/railway-candidates.json"
  write_artifact_link "production-preflight-diff-smoke/proof.md" "${PROOF_DIR}/production-preflight-diff-smoke/proof.md"
  write_artifact_link "production-preflight-diff-smoke/summary.json" "${PROOF_DIR}/production-preflight-diff-smoke/summary.json"
  write_artifact_link "production-preflight-diff-smoke/preflight-diff.svg" "${PROOF_DIR}/production-preflight-diff-smoke/preflight-diff.svg"
  write_artifact_link "production-preflight-github-smoke/proof.md" "${PROOF_DIR}/production-preflight-github-smoke/proof.md"
  write_artifact_link "production-preflight-github-smoke/summary-without-confirm.json" "${PROOF_DIR}/production-preflight-github-smoke/summary-without-confirm.json"
  write_artifact_link "production-preflight-github-smoke/summary-with-confirm.json" "${PROOF_DIR}/production-preflight-github-smoke/summary-with-confirm.json"
  write_artifact_link "production-observe-only/proof.md" "${PROOF_DIR}/production-observe-only/proof.md"
  write_artifact_link "production-observe-only/summary.json" "${PROOF_DIR}/production-observe-only/summary.json"
  write_artifact_link "production-observe-only/observe-only.svg" "${PROOF_DIR}/production-observe-only/observe-only.svg"
  write_artifact_link "production-readiness-smoke/proof.md" "${PROOF_DIR}/production-readiness-smoke/proof.md"

  cat >>"${PROOF_DIR}/index.html" <<EOF
        </article>
      </section>

      <section class="card" style="margin-top: 16px;">
        <h2>Gate Results</h2>
        <table>
          <thead><tr><th>Gate</th><th>Status</th><th>Evidence</th></tr></thead>
          <tbody>
${steps_rows}
          </tbody>
        </table>
      </section>

      <section class="grid">
EOF

  write_visual_card "Deterministic WorkerRun Proof" "${PROOF_DIR}/deterministic-smoke/proof.svg"
  write_visual_card "Datadog MCP Patrol" "${PROOF_DIR}/datadog-patrol-smoke/datadog-patrol.svg"
  write_visual_card "Webhook Intake Proof" "${PROOF_DIR}/webhook-intake-smoke/webhook-intake.svg"
  write_visual_card "Repo Sweep ProofPacket" "${PROOF_DIR}/repo-sweep-brief-smoke/proof.svg"
  write_visual_card "Daily Brief" "${PROOF_DIR}/repo-sweep-brief-smoke/daily-brief.svg"
  write_visual_card "Production Preflight" "${PROOF_DIR}/production-preflight/preflight.svg"
  write_visual_card "Railway Discovery Preflight" "${PROOF_DIR}/production-preflight-railway-discovery-smoke/preflight.svg"
  write_visual_card "Preflight Diff" "${PROOF_DIR}/production-preflight-diff-smoke/preflight-diff.svg"
  write_visual_card "Production Observe-Only" "${PROOF_DIR}/production-observe-only/observe-only.svg"

  cat >>"${PROOF_DIR}/index.html" <<EOF
      </section>
    </main>
  </body>
</html>
EOF
}

write_summary_and_proof() {
  local steps_json
  steps_json="$(jq -R -s '
    split("\n")
    | map(select(length > 0))
    | map(split("\t") | {name: .[0], status: .[1], log: .[2]})
  ' <"$STEPS_FILE")"

  local summary_json
  summary_json="$(jq -n \
    --arg mode "$MODE" \
    --arg proof_dir "$PROOF_DIR" \
    --arg acceptance_log "$ACCEPTANCE_LOG" \
    --arg git_head "$GIT_HEAD" \
    --arg git_branch "$GIT_BRANCH" \
    --arg git_status_short "$GIT_STATUS_SHORT" \
    --arg git_clean "$GIT_CLEAN" \
    --arg deterministic "${PROOF_DIR}/deterministic-smoke" \
    --arg datadog "${PROOF_DIR}/datadog-patrol-smoke" \
    --arg webhook "${PROOF_DIR}/webhook-intake-smoke" \
    --arg repo "${PROOF_DIR}/repo-sweep-brief-smoke" \
    --arg preflight "${PROOF_DIR}/production-preflight" \
    --arg preflight_discovery "${PROOF_DIR}/production-preflight-railway-discovery-smoke" \
    --arg preflight_diff "${PROOF_DIR}/production-preflight-diff-smoke" \
    --arg preflight_github "${PROOF_DIR}/production-preflight-github-smoke" \
    --arg observe "${PROOF_DIR}/production-observe-only" \
    --arg production "${PROOF_DIR}/production-readiness-smoke" \
    --argjson steps "$steps_json" \
    '{
      status: "passed",
      mode: $mode,
      proof_dir: $proof_dir,
      acceptance_log: $acceptance_log,
      git_head: $git_head,
      git_branch: $git_branch,
      git_status_short: $git_status_short,
      git_clean: ($git_clean == "true"),
      steps: $steps,
      live_proof_bundles: {
        deterministic_smoke: $deterministic,
        datadog_patrol_smoke: $datadog,
        webhook_intake_smoke: $webhook,
        repo_sweep_brief_smoke: $repo,
        production_preflight: $preflight,
        production_preflight_railway_discovery_smoke: $preflight_discovery,
        production_preflight_diff_smoke: $preflight_diff,
        production_preflight_github_smoke: $preflight_github,
        production_observe_only: $observe,
        production_readiness_smoke: $production
      }
    }')"

  printf '%s\n' "$summary_json" >"${PROOF_DIR}/summary.json"

  # The generated human proof includes a ```mermaid state diagram.
  cat >"${PROOF_DIR}/proof.md" <<EOF
# Paw Patrol Acceptance Proof

Mode: \`${MODE}\`
Git head: \`${GIT_HEAD}\`
Git branch: \`${GIT_BRANCH}\`
Git status: \`${GIT_STATUS_LABEL}\`

## Flow

\`\`\`mermaid
flowchart TD
    A["Static gates"] --> B["Foundation and worker tests"]
    B --> C{"Mode"}
    C -->|"quick"| D["Acceptance summary"]
    C -->|"live"| E["Deterministic implementation smoke"]
    E --> F["Datadog MCP Patrol smoke"]
    F --> G["Webhook intake smoke"]
    G --> H["Repo sweep and daily brief smoke"]
    H --> I["Production readiness smoke"]
    I --> D
\`\`\`

## Evidence

- Summary JSON: ${PROOF_DIR}/summary.json
- Visual index: ${PROOF_DIR}/index.html
- Acceptance log: ${ACCEPTANCE_LOG}
- Deterministic proof bundle: ${PROOF_DIR}/deterministic-smoke
- Datadog MCP Patrol proof bundle: ${PROOF_DIR}/datadog-patrol-smoke
  - Covers PatrolRun -> WorkerRun -> Codex result envelope -> RecordEvidence -> Signal/ObservabilityFinding/FactoryCase/WorkCycle/ProofPacket fanout.
- Webhook proof bundle: ${PROOF_DIR}/webhook-intake-smoke
  - Covers patrol-request, patrol-datadog, patrol-github, and patrol-discord.
  - Includes github-webhook-event.json, github-signal.json, and GitHub Signal state evidence.
- Repo sweep proof bundle: ${PROOF_DIR}/repo-sweep-brief-smoke
  - Default PatrolSchedule evidence: ${PROOF_DIR}/repo-sweep-brief-smoke/patrol-schedule.json
- Production preflight proof bundle: ${PROOF_DIR}/production-preflight
  - Visual summary: ${PROOF_DIR}/production-preflight/preflight.svg
  - Operator handoff: ${PROOF_DIR}/production-preflight/operator-handoff.md
- Railway discovery preflight proof bundle: ${PROOF_DIR}/production-preflight-railway-discovery-smoke
  - Candidate list: ${PROOF_DIR}/production-preflight-railway-discovery-smoke/railway-candidates.json
  - Operator handoff: ${PROOF_DIR}/production-preflight-railway-discovery-smoke/operator-handoff.md
- Preflight diff proof bundle: ${PROOF_DIR}/production-preflight-diff-smoke
  - Visual summary: ${PROOF_DIR}/production-preflight-diff-smoke/preflight-diff.svg
- GitHub preflight proof bundle: ${PROOF_DIR}/production-preflight-github-smoke
  - Without confirmation: ${PROOF_DIR}/production-preflight-github-smoke/summary-without-confirm.json
  - With confirmation: ${PROOF_DIR}/production-preflight-github-smoke/summary-with-confirm.json
- Production observe-only proof bundle: ${PROOF_DIR}/production-observe-only
  - Visual summary: ${PROOF_DIR}/production-observe-only/observe-only.svg
- Production readiness proof bundle: ${PROOF_DIR}/production-readiness-smoke

## Machine Summary

\`\`\`json
${summary_json}
\`\`\`
EOF

  write_html_index
  printf '%s\n' "$summary_json"
  log "proof bundle: ${PROOF_DIR}"
}

case "$MODE" in
  quick | live)
    ;;
  *)
    fail "usage: $0 [quick|live]"
    ;;
esac

mkdir -p "$PROOF_DIR"
: >"$ACCEPTANCE_LOG"
: >"$STEPS_FILE"

require_cmd cargo
require_cmd git
require_cmd jq

log "repo root: ${ROOT}"
log "mode: ${MODE}"
log "proof dir: ${PROOF_DIR}"

run_step syntax-deterministic bash -n "${ROOT}/crates/paw-codex-worker/scripts/deterministic-smoke.sh"
run_step syntax-datadog-patrol bash -n "${ROOT}/crates/paw-codex-worker/scripts/datadog-patrol-smoke.sh"
run_step syntax-webhook bash -n "${ROOT}/crates/paw-codex-worker/scripts/webhook-intake-smoke.sh"
run_step syntax-repo-sweep bash -n "${ROOT}/crates/paw-codex-worker/scripts/repo-sweep-brief-smoke.sh"
run_step syntax-production-readiness bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-readiness.sh"
run_step syntax-production-preflight bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-preflight.sh"
run_step syntax-production-preflight-diff bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-diff.sh"
run_step syntax-production-preflight-diff-smoke bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-diff-smoke.sh"
run_step syntax-production-preflight-railway-discovery-smoke bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-railway-discovery-smoke.sh"
run_step syntax-production-preflight-github-smoke bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-github-smoke.sh"
run_step syntax-production-observe bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-observe-only.sh"
run_step syntax-production-observe-smoke bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-observe-only-smoke.sh"
run_step syntax-production-smoke bash -n "${ROOT}/crates/paw-codex-worker/scripts/production-readiness-smoke.sh"
run_step syntax-acceptance bash -n "${ROOT}/crates/paw-codex-worker/scripts/paw-patrol-acceptance.sh"
run_step fmt cargo fmt --check --all
run_step diff-check git diff --check
run_step cargo-check cargo check --locked -p temperpaw -p paw-codex-worker
run_step foundation cargo test --locked -p temperpaw --test paw_patrol_foundation -- --nocapture
run_step worker-tests cargo test --locked -p paw-codex-worker --quiet
run_step production-preflight env \
  PROOF_DIR="${PROOF_DIR}/production-preflight" \
  CHECK_RAILWAY=0 \
  CHECK_GITHUB=0 \
  "${ROOT}/crates/paw-codex-worker/scripts/production-preflight.sh"
run_step production-preflight-railway-discovery-smoke env \
  PROOF_DIR="${PROOF_DIR}/production-preflight-railway-discovery-smoke" \
  "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-railway-discovery-smoke.sh"
run_step production-preflight-diff-smoke env \
  PROOF_DIR="${PROOF_DIR}/production-preflight-diff-smoke" \
  "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-diff-smoke.sh"
run_step production-preflight-github-smoke env \
  PROOF_DIR="${PROOF_DIR}/production-preflight-github-smoke" \
  "${ROOT}/crates/paw-codex-worker/scripts/production-preflight-github-smoke.sh"

if [[ "$MODE" == "live" ]]; then
  run_step deterministic-smoke env \
    PROOF_DIR="${PROOF_DIR}/deterministic-smoke" \
    "${ROOT}/crates/paw-codex-worker/scripts/deterministic-smoke.sh"

  run_step datadog-patrol-smoke env \
    PROOF_DIR="${PROOF_DIR}/datadog-patrol-smoke" \
    "${ROOT}/crates/paw-codex-worker/scripts/datadog-patrol-smoke.sh"

  run_step webhook-intake-smoke env \
    PROOF_DIR="${PROOF_DIR}/webhook-intake-smoke" \
    "${ROOT}/crates/paw-codex-worker/scripts/webhook-intake-smoke.sh"

  run_step repo-sweep-brief-smoke env \
    PROOF_DIR="${PROOF_DIR}/repo-sweep-brief-smoke" \
    "${ROOT}/crates/paw-codex-worker/scripts/repo-sweep-brief-smoke.sh"

  run_step production-readiness-smoke env \
    PROOF_DIR="${PROOF_DIR}/production-readiness-smoke" \
    "${ROOT}/crates/paw-codex-worker/scripts/production-readiness-smoke.sh"

  run_step production-observe-only env \
    PROOF_DIR="${PROOF_DIR}/production-observe-only" \
    "${ROOT}/crates/paw-codex-worker/scripts/production-observe-only-smoke.sh"
fi

write_summary_and_proof
