#!/usr/bin/env bash
# The line's stage, check, live and back for kind main_temper: the job .github/workflows/railway-redeploy.yml ran.
# Deploys one CI-built image (ghcr.io/arni-labs/temperpaw:sha-*) to the main Temper in one Railway environment, checks it
# against what ran just before, and goes back to that image on any miss (scripts/railway_release.sh).
#
#   STEP=stage   deploy to staging           (workflow step "stage")
#   STEP=check   check staging, deploy nothing  (workflow step "check_staging")
#   STEP=live    deploy to production        (workflow step "go_live")
#   STEP=back    deploy REF to production    (workflow step "roll_back"; REF is the image to go back to, required)
#
# Runner env: SRC, COMMIT, STEP, SCRATCH. Optional inputs: REF (image tag sha-*; default sha-<first 7 of COMMIT>),
# ROLLBACK_TAG, DRILL (staging only), RUN_ARTIFACT_BATCH_E2E (true|false), POLL_ATTEMPTS, POLL_SECONDS, SOAK_SECONDS.
# Logins (the first set one of each wins):
#   LOGIN_RAILWAY_TOKEN_STAGING (staging only), LOGIN_RAILWAY_TOKEN   -> RAILWAY_API_TOKEN (secret RAILWAY_TOKEN)
#   LOGIN_TEMPER_API_KEY_STAGING (staging only), LOGIN_TEMPER_API_KEY -> TEMPER_API_KEY (the environment's operator key)
#   LOGIN_DATADOG_API_KEY, LOGIN_DATADOG_APP_KEY, LOGIN_DATADOG_SITE (optional) -> DD_API_KEY, DD_APP_KEY, DD_SITE
#   LOGIN_GITHUB_READ (JSON {"arni-labs": token}, or a bare token; else GH_TOKEN already set) -> GH_TOKEN, for commit compare and the kernel pin
set -euo pipefail

: "${SRC:?}" "${COMMIT:?}" "${STEP:?}" "${SCRATCH:?}"
cd "$SRC"

# --- what the step means: the workflow's Factory step, the environment, whether it deploys
case "$STEP" in
  stage) DEPLOY_ENV=staging;    MODE=deploy ;;
  check) DEPLOY_ENV=staging;    MODE=check ;;
  live)  DEPLOY_ENV=production; MODE=deploy ;;
  back)  DEPLOY_ENV=production; MODE=deploy ;;
  *) echo "unknown STEP ${STEP}" >&2; echo '{"conclusion":"failure","what":"unknown step"}'; exit 64 ;;
esac

# --- the environment's non-secret settings (gh variable list, repository and the GitHub environment of the same name)
RAILWAY_PROJECT_ID=ad7f8977-cf48-43ef-b129-ba1e17896ae4
case "$DEPLOY_ENV" in
  staging)
    RAILWAY_ENVIRONMENT_ID=ac56df71-3cca-42ca-8b78-5d5190483926
    RAILWAY_SERVICE_ID=889efca2-13f4-4b1b-a144-dbd9e877001f
    BASE_URL=https://openpaw-staging-katagami-staging.up.railway.app
    MIGRATION_ROLLBACK=auto
    POSTGRES_SERVICE_ID=dc9eeb28-8236-4def-a7e1-e8fd71312b66
    GUEST_CHECK_URL=""
    DD_ENV=""
    ;;
  production)
    RAILWAY_ENVIRONMENT_ID=5c01a9d9-589d-427d-bd83-89264391fb9c
    RAILWAY_SERVICE_ID=4a8dedaa-8a2e-4cdd-945b-e06c781bb3f0
    BASE_URL=https://openpaw-production.up.railway.app
    MIGRATION_ROLLBACK=""
    POSTGRES_SERVICE_ID=""
    GUEST_CHECK_URL="https://unikon.dev/api/apps/a6m1yf28pxya?records=1"
    DD_ENV=prod
    ;;
esac
PROBE_ROUTES='POST anon /katagami/webhooks/tensorlake
POST anon /katagami/webhooks/datadog
POST anon /katagami/webhooks/stripe
POST operator /katagami/judge
POST operator /katagami/pass/guest
POST operator /katagami/pass/files
GET anon /paw/setup/status
GET forged-customer /paw/setup/status
GET forged-admin /paw/setup/status
GET bogus /paw/setup/status
GET operator /paw/setup/status'
MEMORY_LIMIT_GB=""

# --- the logins
if [ "$DEPLOY_ENV" = staging ]; then
  RAILWAY_API_TOKEN="${LOGIN_RAILWAY_TOKEN_STAGING:-${LOGIN_RAILWAY_TOKEN:-}}"
  TEMPER_API_KEY="${LOGIN_TEMPER_API_KEY_STAGING:-${LOGIN_TEMPER_API_KEY:-}}"
else
  RAILWAY_API_TOKEN="${LOGIN_RAILWAY_TOKEN:-}"
  TEMPER_API_KEY="${LOGIN_TEMPER_API_KEY:-}"
fi
DD_API_KEY="${LOGIN_DATADOG_API_KEY:-}"
DD_APP_KEY="${LOGIN_DATADOG_APP_KEY:-}"
DD_SITE="${LOGIN_DATADOG_SITE:-}"
# LOGIN_GITHUB_READ is a JSON object keyed by GitHub account, {"arni-labs": "<token>"}; a bare token is taken as it is.
if [ -n "${LOGIN_GITHUB_READ:-}" ]; then
  GH_TOKEN="$(jq -r 'if type == "object" then .["arni-labs"] // empty else . end' <<<"$LOGIN_GITHUB_READ" 2>/dev/null || printf '%s' "$LOGIN_GITHUB_READ")"
fi
GH_TOKEN="${GH_TOKEN:-}"
export RAILWAY_API_TOKEN TEMPER_API_KEY DD_API_KEY DD_APP_KEY GH_TOKEN
[ -n "$DD_SITE" ] && export DD_SITE

# The sandbox has curl but not gh; scripts/railway_release.sh and this script only use `gh api <path> [-H h] [--jq expr]`.
if ! command -v gh >/dev/null 2>&1; then
  mkdir -p "$SCRATCH/bin"
  cat > "$SCRATCH/bin/gh" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
[ "${1:-}" = api ] || { echo "gh shim: only 'gh api' is supported" >&2; exit 2; }
shift
path="$1"; shift
accept="application/vnd.github+json"; jqexpr=""
while [ $# -gt 0 ]; do
  case "$1" in
    -H) case "$2" in [Aa]ccept:*) accept="${2#*: }" ;; esac; shift 2 ;;
    --jq) jqexpr="$2"; shift 2 ;;
    *) echo "gh shim: unsupported argument $1" >&2; exit 2 ;;
  esac
done
body="$(curl -fsS -H "Authorization: Bearer ${GH_TOKEN:?}" -H "Accept: $accept" -H "X-GitHub-Api-Version: 2022-11-28" "https://api.github.com/${path#/}")"
if [ -n "$jqexpr" ]; then jq -r "$jqexpr" <<<"$body"; else printf '%s\n' "$body"; fi
SHIM
  chmod +x "$SCRATCH/bin/gh"
  export PATH="$SCRATCH/bin:$PATH"
fi

IMAGE_TAG="${REF:-sha-${COMMIT:0:7}}"
EXPECTED_SHA_INPUT="$COMMIT"
ROLLBACK_TAG="${ROLLBACK_TAG:-}"
DRILL="${DRILL:-}"
RUN_ARTIFACT_BATCH_E2E="${RUN_ARTIFACT_BATCH_E2E:-false}"
POLL_ATTEMPTS="${POLL_ATTEMPTS:-60}"
POLL_SECONDS="${POLL_SECONDS:-15}"
SOAK_SECONDS="${SOAK_SECONDS:-300}"
STATE_DIR="$SCRATCH/release"
rm -rf "$STATE_DIR"; mkdir -p "$STATE_DIR"
export RAILWAY_PROJECT_ID RAILWAY_ENVIRONMENT_ID RAILWAY_SERVICE_ID BASE_URL PROBE_ROUTES GUEST_CHECK_URL MEMORY_LIMIT_GB \
  DD_ENV MIGRATION_ROLLBACK POSTGRES_SERVICE_ID POLL_ATTEMPTS POLL_SECONDS SOAK_SECONDS ROLLBACK_TAG DEPLOY_ENV STATE_DIR

stop() { # code, why: the step ends here with no deploy
  echo "$2" >&2
  jq -nc --arg w "$2" --arg step "$STEP" '{conclusion: "failure", what: $w, step: $step, result: "failed"}'
  exit "$1"
}

# --- validate inputs and secrets
if [ "$STEP" = back ] && [ -z "${REF:-}" ]; then stop 64 "back needs REF, the image tag to go back to (sha-<7..40 hex>)"; fi
[[ "$IMAGE_TAG" =~ ^sha-[0-9a-f]{7,40}$ ]] || stop 64 "image tag must be sha-<7..40 hex>, got ${IMAGE_TAG}"
short="${IMAGE_TAG#sha-}"
[[ "$EXPECTED_SHA_INPUT" =~ ^[0-9a-f]{40}$ ]] || stop 64 "COMMIT must be a full 40-character lowercase git SHA"
if [ "$STEP" != back ] && [ "${EXPECTED_SHA_INPUT:0:${#short}}" != "$short" ]; then
  stop 64 "image tag ${IMAGE_TAG} does not match the change's commit ${EXPECTED_SHA_INPUT}"
fi
# The full commit /paw/version must report; the tag's short sha must name exactly one commit.
if [ "$STEP" = back ]; then
  full="$(gh api "repos/arni-labs/temperpaw/commits/${short}" --jq .sha 2>/dev/null || true)"
else
  full="$COMMIT"
fi
[[ "$full" =~ ^[0-9a-f]{40}$ ]] || stop 64 "expected_sha cannot be verified for ${IMAGE_TAG}: no such commit"
EXPECTED_SHA="$full"; export EXPECTED_SHA
[[ -z "$ROLLBACK_TAG" || "$ROLLBACK_TAG" =~ ^sha-[0-9a-f]{7,40}$ ]] || stop 64 "ROLLBACK_TAG must be sha-<7..40 hex>"
if [ -n "$DRILL" ] && [ "$DEPLOY_ENV" != staging ]; then stop 64 "a drill runs on staging only"; fi
[ -z "$DRILL" ] || export DRILL
case "$RUN_ARTIFACT_BATCH_E2E" in true|false) ;; *) stop 64 "RUN_ARTIFACT_BATCH_E2E must be true or false" ;; esac
for v in RAILWAY_API_TOKEN TEMPER_API_KEY; do
  [ -n "${!v}" ] || stop 65 "${v} is required (environment ${DEPLOY_ENV}): its login is missing"
done
for v in POLL_ATTEMPTS POLL_SECONDS SOAK_SECONDS; do
  case "${!v}" in ''|*[!0-9]*) stop 64 "${v} must be an integer" ;; esac
done
echo "${MODE} ${IMAGE_TAG} (${full}) on ${DEPLOY_ENV} at ${BASE_URL}"

# step <var> <command...>: run it, keep its outcome in <var> (success|failure) without stopping the script
step() { local var="$1"; shift; set +e; ( set -euo pipefail; "$@" ); local rc=$?; set -e; if [ "$rc" -eq 0 ]; then printf -v "$var" success; else printf -v "$var" failure; fi; }

deploy_outcome=skipped; check_outcome=skipped; package_outcome=skipped; e2e_outcome=skipped; rollback_outcome=skipped
already=""

# --- what runs now (a failure here ends the run: the workflow's step failed before any later one ran)
echo "::group::What runs now"
if ! bash scripts/railway_release.sh snapshot; then
  echo "::endgroup::"
  stop 1 "the snapshot of what runs now failed: no deploy"
fi
echo "::endgroup::"

# --- never backwards: a release the serving commit already contains is live already (stage and live only)
if [ "$STEP" = stage ] || [ "$STEP" = live ]; then
  echo "::group::Is it live already"
  fwd="$SCRATCH/forward.out"
  if ! bash scripts/railway_release.sh forward | tee "$fwd"; then
    echo "::endgroup::"
    stop 1 "$(tail -1 "$STATE_DIR/errors.txt" 2>/dev/null || echo 'the release does not go on top of what serves')"
  fi
  echo "::endgroup::"
  if grep -qx 'already=true' "$fwd"; then already=true; fi
fi

if [ "$already" != true ]; then
  if [ "$MODE" = deploy ]; then
    echo "::group::Deploy the image"
    step deploy_outcome bash scripts/railway_release.sh deploy "$IMAGE_TAG"
    echo "::endgroup::"
  fi
  echo "::group::Check what runs against what ran before"
  # A check-only run checks the deployment that runs now against itself.
  if [ "$MODE" = deploy ]; then
    step check_outcome bash scripts/railway_release.sh check
  else
    step check_outcome bash -c 'jq "{deployment_id, image_tag}" "$STATE_DIR/before.json" > "$STATE_DIR/new.json" && bash scripts/railway_release.sh check'
  fi
  echo "::endgroup::"

  if [ "$RUN_ARTIFACT_BATCH_E2E" = true ] && [ "$check_outcome" = success ]; then
    echo "::group::ArtifactBatch production E2E"
    step package_outcome bash -c 'command -v rustup >/dev/null && rustup target add wasm32-unknown-unknown || true; bash os-apps/paw-fs/wasm/artifact_batch_apply/build.sh'
    if [ "$package_outcome" = success ]; then
      step e2e_outcome env TEMPER_BASE_URL="$BASE_URL" TENANT_ID=default RUN_SUFFIX="line-${STEP_RUN:-$$}" \
        PACKAGED_WASM_PATH=os-apps/paw-fs/wasm/artifact_batch_apply/artifact_batch_apply.wasm bash scripts/production_artifact_batch_e2e.sh
    fi
    echo "::endgroup::"
  fi

  if [ "$MODE" = deploy ] && { [ "$deploy_outcome" = failure ] || [ "$check_outcome" = failure ] || [ "$package_outcome" = failure ] || [ "$e2e_outcome" = failure ]; }; then
    echo "::group::Go back to the image that ran before"
    step rollback_outcome bash scripts/railway_release.sh rollback
    echo "::endgroup::"
  fi
fi

# --- the release record
if [ "$MODE" = check ]; then
  if [ "$check_outcome" = success ] && [ "$package_outcome" != failure ] && [ "$e2e_outcome" != failure ]; then result=checked; else result=check-failed; fi
elif [ "$already" = true ]; then result=already_live
elif [ "$rollback_outcome" = success ]; then result=rolled_back
elif [ "$rollback_outcome" = failure ]; then result=rollback_failed
elif [ "$deploy_outcome" = success ] && [ "$check_outcome" = success ] && [ "$package_outcome" != failure ] && [ "$e2e_outcome" != failure ]; then result=verified
else result=failed; fi

read_json() { if [ -f "$STATE_DIR/$1.json" ]; then jq -c . "$STATE_DIR/$1.json"; else echo null; fi; }
misses="$(cat "$STATE_DIR/misses.txt" 2>/dev/null || cat "$STATE_DIR/errors.txt" 2>/dev/null || true)"
record="$(jq -n --arg result "$result" --arg environment "$DEPLOY_ENV" --arg image_tag "$IMAGE_TAG" \
  --arg commit "$EXPECTED_SHA" --arg run_url "${STEP_RUN:-}" --arg misses "$misses" \
  --argjson before "$(read_json before)" --argjson new "$(read_json new)" \
  --argjson checked "$(read_json checked)" --argjson rolled_back "$(read_json rolled-back)" \
  '{result: $result, environment: $environment, ref: (if $result == "already_live" then $before.image_tag else $image_tag end),
    previous_ref: ($before.image_tag // null), image_tag: $image_tag, previous_image_tag: ($before.image_tag // null),
    commit: $commit, run_url: $run_url, misses: ($misses | split("\n") | map(select(length > 0))),
    before: $before, new: $new, checked: $checked, rolled_back: $rolled_back}')"
{
  echo "### ${result}: ${IMAGE_TAG} on ${DEPLOY_ENV}"
  echo "Previous image: $(jq -r '.previous_image_tag // "unknown"' <<<"$record")."
  jq -r '.misses[] | "- miss: \(.)"' <<<"$record"
  echo '```json'; jq . <<<"$record"; echo '```'
}

case "$result" in verified|checked|already_live) conclusion=success ;; *) conclusion=failure ;; esac
what="$(jq -r 'if .result == "already_live" then "already live: \(.before.sha) serves on \(.environment), and it contains \(.commit)"
               elif .result == "verified" or .result == "checked" then "\(.ref) \(.result) on \(.environment)"
               else "\(.result): \(.misses[0] // "a step failed before the checks")" end' <<<"$record")"
jq -nc --arg c "$conclusion" --arg w "$what" --arg step "$STEP" --argjson record "$record" \
  '{conclusion: $c, what: $w, step: $step, result: $record.result, ref: $record.ref, previous_ref: $record.previous_ref,
    commit: $record.commit, record: $record}'
[ "$conclusion" = success ]
