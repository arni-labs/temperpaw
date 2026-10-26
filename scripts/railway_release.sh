#!/usr/bin/env bash
# Deploy a TemperPaw image to one Railway environment, check it against what ran before, and go back on any miss.
# .github/workflows/railway-redeploy.yml runs it; a person can run it the same way with the same environment.
#
#   scripts/railway_release.sh snapshot          what runs now: its deployment, image, routes and probe answers
#   scripts/railway_release.sh deploy <sha-tag>  point the service at ghcr.io/arni-labs/temperpaw:<tag>, wait for SUCCESS
#   scripts/railway_release.sh check             the new deployment against the snapshot
#   scripts/railway_release.sh rollback          back to the snapshot's image, checked against the snapshot
#
# Environment: RAILWAY_API_TOKEN (a project token for that environment), RAILWAY_PROJECT_ID, RAILWAY_ENVIRONMENT_ID,
# RAILWAY_SERVICE_ID, BASE_URL, TEMPER_API_KEY (the operator key), STATE_DIR.
# Optional:
#   PROBE_ROUTES     one "<GET|POST> <anon|operator|forged-customer|forged-admin|bogus> <path>" per line; each answer
#                    after the deploy must equal the answer before it
#   GUEST_CHECK_URL  a guest read through the site; after the deploy it must answer 200 ok with the same counts
#   ROLLBACK_TAG     the image to go back to when Railway's record names none and the service has no IMAGE_TAG
#   EXPECTED_SHA     the full commit /paw/version must report
#   SOAK_SECONDS     how long the new deployment runs before memory is read (default 300)
#   MEMORY_LIMIT_GB  a limit on Railway's memory figure; without it memory is reported, not gated
#   DD_API_KEY, DD_APP_KEY, DD_SITE, DD_ENV  read this build's memory gauges from Datadog
#   POLL_ATTEMPTS, POLL_SECONDS
#   DRILL            any value: the check reports a deliberate miss, to prove the way back
#   MIGRATION_ROLLBACK, POSTGRES_SERVICE_ID  "auto" with the environment's Postgres service: going back to a kernel
#                    that knows fewer storage migrations deletes the newer ones' _sqlx_migrations rows, when every one
#                    is listed in scripts/additive_migrations.txt (through a TCP proxy opened for that and removed).
#                    Otherwise the way back stops and says which rows a person must remove.
#   GH_TOKEN         reads the kernel pin and its migrations from GitHub
# It prints statuses, counts and ids, never a key.
set -euo pipefail
shopt -s inherit_errexit 2>/dev/null || true

GQL="${RAILWAY_GRAPHQL_URL:-https://backboard.railway.com/graphql/v2}"
IMAGE_REPO="ghcr.io/arni-labs/temperpaw"
POLL_ATTEMPTS="${POLL_ATTEMPTS:-60}"
POLL_SECONDS="${POLL_SECONDS:-15}"
SOAK_SECONDS="${SOAK_SECONDS:-300}"
BASE_URL="${BASE_URL%/}"
ROUTE_MESSAGE="HttpEndpoint route table rebuilt"
ASK_FAILED_MESSAGE="HttpEndpoint actor ask failed during rebuild"

for v in RAILWAY_API_TOKEN RAILWAY_PROJECT_ID RAILWAY_ENVIRONMENT_ID RAILWAY_SERVICE_ID BASE_URL TEMPER_API_KEY STATE_DIR; do
  [ -n "${!v:-}" ] || { echo "missing $v" >&2; exit 64; }
done
mkdir -p "$STATE_DIR"

gql() { # $1: the request body; prints the response, fails on an error
  local response
  # A project token goes in Project-Access-Token: sent as a Bearer token, Railway answers "Not Authorized".
  response="$(curl -fsS --max-time 30 -H "Project-Access-Token: ${RAILWAY_API_TOKEN}" -H "Content-Type: application/json" \
    --data "$1" "$GQL")" || { echo "Railway API did not answer" | tee -a "$STATE_DIR/errors.txt" >&2; return 1; }
  if jq -e '.errors' >/dev/null <<<"$response"; then
    # Which call failed: the first field the request names (deployments, variableUpsert, ...), never its variables.
    echo "Railway API, $(jq -r '.query | capture("{ *(?<f>[A-Za-z]+)").f // "request"' <<<"$1"): $(jq -r '.errors[0].message // "error"' <<<"$response")" |
      tee -a "$STATE_DIR/errors.txt" >&2
    return 1
  fi
  printf '%s' "$response"
}

# The deployment serving now: the newest SUCCESS in this environment, with the image Railway deployed it from (empty
# for a `railway up` upload).
live_deployment() {
  gql "$(jq -n --arg s "$RAILWAY_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg p "$RAILWAY_PROJECT_ID" \
    '{query: "query($s: String!, $e: String!, $p: String!) { deployments(first: 20, input: {projectId: $p, serviceId: $s, environmentId: $e}) { edges { node { id status createdAt meta } } } }",
      variables: {s: $s, e: $e, p: $p}}')" |
    jq -c '[.data.deployments.edges[].node | select(.status == "SUCCESS")][0] // empty | {id, createdAt, image: (.meta.image // "")}'
}

deployment_status() {
  gql "$(jq -n --arg d "$1" '{query: "query($d: String!) { deployment(id: $d) { id status meta instances { status } } }", variables: {d: $d}}')" |
    jq -c '.data.deployment | {id, status, image: (.meta.image // ""), instances: [.instances[]?.status]}'
}

# The service's IMAGE_TAG variable, which Dockerfile.deploy builds from when someone deploys by hand with `railway up`.
# Empty when the service has none. Only that one value leaves the response.
image_tag_var() {
  gql "$(jq -n --arg p "$RAILWAY_PROJECT_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg s "$RAILWAY_SERVICE_ID" \
    '{query: "query($p: String!, $e: String!, $s: String) { variables(projectId: $p, environmentId: $e, serviceId: $s) }", variables: {p: $p, e: $e, s: $s}}')" |
    jq -r '.data.variables.IMAGE_TAG // empty'
}

# Log lines of one deployment that match a Railway filter, as objects of their structured fields.
log_fields() {
  gql "$(jq -n --arg d "$1" --arg f "$2" \
    '{query: "query($d: String!, $f: String) { deploymentLogs(deploymentId: $d, filter: $f, limit: 500) { timestamp attributes { key value } } }",
      variables: {d: $d, f: $f}}')" |
    jq -c '.data.deploymentLogs[]
      | ((.attributes // []) | map({key, value: (.value | fromjson? // .)}) | from_entries) as $a
      | ($a.fields | if type == "string" then (fromjson? // {}) elif type == "object" then . else {} end) as $nested
      | ($a | with_entries(select(.key | startswith("fields."))) | with_entries(.key |= ltrimstr("fields."))) as $flat
      | {timestamp} + $nested + $flat'
}

# The default tenant's route count, from the last rebuild a deployment logged (its boot rebuild for a new one).
route_count() {
  log_fields "$1" "@fields.message:\"${ROUTE_MESSAGE}\"" | jq -s -r '[.[] | select(.tenant == "default")] | sort_by(.timestamp) | last | .route_count // empty'
}

ask_failures() {
  log_fields "$1" "@fields.message:\"${ASK_FAILED_MESSAGE}\"" | jq -s 'length'
}

# Each PROBE_ROUTES request once, as {"<method> <auth> <path>": "<status>"}. A dropped route answers with the API's
# fallback instead of its module, and a broken sign-in answers a forged or made-up caller differently.
probe() {
  local method auth path code out="{}" headers
  while read -r method auth path; do
    [ -n "${path:-}" ] || continue
    headers=(-H "Content-Type: application/json")
    case "$auth" in
      operator) headers+=(-H "Authorization: Bearer ${TEMPER_API_KEY}" -H "X-Tenant-Id: default") ;;
      forged-customer) headers+=(-H "x-temper-principal-id: someone" -H "x-temper-principal-kind: customer") ;;
      forged-admin) headers+=(-H "x-temper-principal-id: someone" -H "x-temper-principal-kind: admin") ;;
      bogus) headers+=(-H "Authorization: Bearer not-a-real-token") ;;
    esac
    if [ "$method" = POST ]; then
      code="$(curl -s -o /dev/null -m 30 -w '%{http_code}' -X POST "${headers[@]}" -d '{}' "${BASE_URL}${path}" || true)"
    else
      code="$(curl -s -o /dev/null -m 30 -w '%{http_code}' "${headers[@]}" "${BASE_URL}${path}" || true)"
    fi
    out="$(jq -c --arg k "$method $auth $path" --arg c "${code:-000}" '. + {($k): $c}' <<<"$out")"
  done <<<"${PROBE_ROUTES:-}"
  printf '%s' "$out"
}

# The guest read: "<status> <ok> <thing count, ...>".
guest() {
  [ -n "${GUEST_CHECK_URL:-}" ] || return 0
  local body code
  body="$(curl -s -m 60 -w '\n%{http_code}' "$GUEST_CHECK_URL" || true)"
  code="$(tail -n1 <<<"$body")"
  sed '$d' <<<"$body" | jq -r --arg c "$code" \
    '"\($c) \(.ok // false) \([(.data.records // [])[] | select(type == "object") | "\(.thing) \((.records // []) | length)"] | join(", "))"' \
    2>/dev/null || echo "$code false not JSON"
}

wait_healthy() {
  local attempt healthz readyz
  for attempt in $(seq 1 "$POLL_ATTEMPTS"); do
    healthz="$(curl -s -o /dev/null -m 20 -w '%{http_code}' "${BASE_URL}/healthz" || true)"
    readyz="$(curl -s -o /dev/null -m 20 -w '%{http_code}' "${BASE_URL}/readyz" || true)"
    if [ "$healthz" = 200 ] && [ "$readyz" = 200 ]; then
      echo "healthz 200, readyz 200 after ${attempt} attempt(s)"
      return 0
    fi
    echo "attempt ${attempt}: healthz ${healthz:-000}, readyz ${readyz:-000}"
    sleep "$POLL_SECONDS"
  done
  return 1
}

live_sha() {
  curl -s -m 20 -H "Authorization: Bearer ${TEMPER_API_KEY}" "${BASE_URL}/paw/version" | jq -r '.sha // empty' 2>/dev/null || true
}

# Point the service's source at one image, with the deploy settings `railway up` carried in railway.toml. Starts
# nothing: Railway stages it until a deployment starts. IMAGE_TAG follows it where the service has one.
set_image() {
  local tag="$1"
  printf '%s' "$tag" >"$STATE_DIR/source-set"
  if [ -n "$(image_tag_var)" ]; then
    gql "$(jq -n --arg p "$RAILWAY_PROJECT_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg s "$RAILWAY_SERVICE_ID" --arg v "$tag" \
      '{query: "mutation($input: VariableUpsertInput!) { variableUpsert(input: $input) }",
        variables: {input: {projectId: $p, environmentId: $e, serviceId: $s, name: "IMAGE_TAG", value: $v, skipDeploys: true}}}')" >/dev/null
  fi
  gql "$(jq -n --arg s "$RAILWAY_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg image "${IMAGE_REPO}:${tag}" \
    '{query: "mutation($s: String!, $e: String!, $input: ServiceInstanceUpdateInput!) { serviceInstanceUpdate(serviceId: $s, environmentId: $e, input: $input) }",
      variables: {s: $s, e: $e, input: {source: {image: $image}, healthcheckPath: "/readyz", healthcheckTimeout: 3600,
        overlapSeconds: 0, drainingSeconds: 0, restartPolicyType: "ON_FAILURE", restartPolicyMaxRetries: 3}}}')" >/dev/null
}

# Start a deployment of the image set_image named and wait for Railway's SUCCESS. Prints the deployment id. Until the
# new deployment answers /readyz, Railway keeps the old one serving.
deploy_image() {
  local tag="$1" image="${IMAGE_REPO}:$1" started attempt node status seen crashed=0
  set_image "$tag"
  started="$(gql "$(jq -n --arg s "$RAILWAY_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" \
    '{query: "mutation($s: String!, $e: String!) { serviceInstanceDeployV2(serviceId: $s, environmentId: $e) }", variables: {s: $s, e: $e}}')" |
    jq -r '.data.serviceInstanceDeployV2 // empty')"
  [[ "$started" =~ ^[0-9a-f-]{36}$ ]] || { echo "Railway started no deployment" >&2; return 1; }
  echo "deployment ${started} of ${image} started" >&2
  printf '%s' "$started" >"$STATE_DIR/last-started"
  for attempt in $(seq 1 "$POLL_ATTEMPTS"); do
    node="$(deployment_status "$started" 2>/dev/null || echo '{}')"
    status="$(jq -r '.status // empty' <<<"$node")"
    seen="$(jq -r '.image // empty' <<<"$node")"
    case "$status" in
      SUCCESS)
        [ "$seen" = "$image" ] || { echo "deployment ${started} runs ${seen:-an unknown image}, not ${image}" >&2; return 1; }
        echo "deployment ${started} of ${image} is SUCCESS" >&2
        printf '%s' "$started"
        return 0
        ;;
      FAILED | CRASHED | REMOVED | SKIPPED)
        echo "deployment ${started} ended ${status}" >&2
        return 1
        ;;
    esac
    # A process that exits at boot is restarted three times and then stays down, while the deployment still waits
    # out the hour-long health check: two polls in a row with every instance crashed is a failed boot.
    if jq -e '(.instances | length) > 0 and all(.instances[]; . == "CRASHED" or . == "EXITED")' <<<"$node" >/dev/null; then
      crashed=$((crashed + 1))
      [ "$crashed" -lt 2 ] || { echo "deployment ${started}: every instance crashed (${status})" >&2; return 1; }
    else
      crashed=0
    fi
    echo "attempt ${attempt}: deployment ${started} ${status:-not known yet} $(jq -c '.instances // []' <<<"$node")" >&2
    sleep "$POLL_SECONDS"
  done
  echo "deployment ${started} did not reach SUCCESS in time" >&2
  return 1
}

memory_gb() { # the service's highest memory over the last $1 seconds, from Railway
  gql "$(jq -n --arg s "$RAILWAY_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg start "$(jq -rn --argjson t "$(($(date -u +%s) - $1))" '$t | todate')" \
    '{query: "query($s: String, $e: String, $start: DateTime!) { metrics(serviceId: $s, environmentId: $e, startDate: $start, measurements: [MEMORY_USAGE_GB], sampleRateSeconds: 30) { measurement values { ts value } } }",
      variables: {s: $s, e: $e, start: $start}}')" |
    jq -r '[.data.metrics[].values[].value] | max // empty'
}

datadog_max() { # the highest value of one gauge for this build over the last $2 seconds
  local now
  now="$(date -u +%s)"
  curl -fsS -m 30 -G "https://api.${DD_SITE:-datadoghq.com}/api/v1/query" \
    -H "DD-API-KEY: ${DD_API_KEY}" -H "DD-APPLICATION-KEY: ${DD_APP_KEY}" \
    --data-urlencode "from=$((now - $2))" --data-urlencode "to=${now}" \
    --data-urlencode "query=max:$1{service:temperpaw,env:${DD_ENV},version:${EXPECTED_SHA}}" |
    jq -r '[.series[]?.pointlist[]?[1] | select(. != null)] | max // empty'
}

# Everything that must equal the snapshot, for a deployment that now serves. Appends misses to the array named misses.
against_before() { # $1 deployment id
  local want got failed after key was now guest_now
  wait_healthy || misses+=("healthz and readyz did not both answer 200")
  want="$(jq -r .route_count "$STATE_DIR/before.json")"
  got="$(route_count "$1")"
  echo "route_count: ${got:-none} (before: ${want:-none})"
  if [ -z "$got" ]; then
    misses+=("the deployment logged no route table for the default tenant")
  elif [ "$got" != "$want" ]; then
    misses+=("route_count ${got}, before ${want}")
  fi
  failed="$(ask_failures "$1")"
  [ "${failed:-0}" = 0 ] || misses+=("${failed} route actor ask(s) failed during a rebuild")
  after="$(probe)"
  while IFS= read -r key; do
    [ -n "$key" ] || continue
    was="$(jq -r --arg k "$key" '.probes[$k]' "$STATE_DIR/before.json")"
    now="$(jq -r --arg k "$key" '.[$k]' <<<"$after")"
    echo "${key}: ${now} (before: ${was})"
    [ "$now" = "$was" ] || misses+=("${key} answered ${now}, before ${was}")
  done < <(jq -r 'keys[]' <<<"$after")
  if [ -n "${GUEST_CHECK_URL:-}" ]; then
    guest_now="$(guest)"
    was="$(jq -r .guest "$STATE_DIR/before.json")"
    echo "guest read: ${guest_now} (before: ${was})"
    case "$guest_now" in "200 true "*) ;; *) misses+=("the guest read answered ${guest_now}") ;; esac
    [ "$guest_now" = "$was" ] || misses+=("the guest read changed: ${guest_now}, before ${was}")
  fi
  jq -n --arg id "$1" --arg routes "${got:-}" --argjson probes "$after" --arg guest "${guest_now:-}" \
    '{deployment_id: $id, route_count: $routes, probes: $probes, guest: $guest}' >"$STATE_DIR/checked.json"
}

# The highest Postgres migration the kernel of one TemperPaw commit knows: its temper pin from Cargo.lock, then that
# temper commit's crates/temper-store-postgres/migrations.
known_migration() { # $1 full TemperPaw commit
  local rev
  rev="$(gh api "repos/arni-labs/temperpaw/contents/Cargo.lock?ref=$1" -H "Accept: application/vnd.github.raw" |
    grep -A2 '^name = "temper-store-postgres"$' | grep -o 'rev=[0-9a-f]*' | head -n1)"
  [ -n "$rev" ] || return 1
  gh api "repos/arni-labs/temper/contents/crates/temper-store-postgres/migrations?ref=${rev#rev=}" \
    --jq '[.[].name | capture("^(?<n>[0-9]+)_").n | tonumber] | max'
}

# The environment's Postgres, through a TCP proxy this run opens when there is none, as psql variables.
db_open() {
  local proxies vars
  vars="$(gql "$(jq -n --arg p "$RAILWAY_PROJECT_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg s "$POSTGRES_SERVICE_ID" \
    '{query: "query($p: String!, $e: String!, $s: String) { variables(projectId: $p, environmentId: $e, serviceId: $s) }", variables: {p: $p, e: $e, s: $s}}')")"
  PGUSER="$(jq -r '.data.variables.PGUSER' <<<"$vars")"
  PGPASSWORD="$(jq -r '.data.variables.PGPASSWORD' <<<"$vars")"
  PGDATABASE="$(jq -r '.data.variables.PGDATABASE' <<<"$vars")"
  [ -z "${GITHUB_ACTIONS:-}" ] || echo "::add-mask::${PGPASSWORD}"
  export PGUSER PGPASSWORD PGDATABASE PGCONNECT_TIMEOUT=20
  # A Postgres with a public address already (DATABASE_PUBLIC_URL) is reached there; otherwise through a proxy this run
  # opens and db_close removes.
  if [ -n "$(jq -r '.data.variables.DATABASE_PUBLIC_URL // empty' <<<"$vars")" ]; then
    PGHOST="$(jq -r '.data.variables.DATABASE_PUBLIC_URL | capture("@(?<h>[^:/]+):(?<p>[0-9]+)/").h' <<<"$vars")"
    PGPORT="$(jq -r '.data.variables.DATABASE_PUBLIC_URL | capture("@(?<h>[^:/]+):(?<p>[0-9]+)/").p' <<<"$vars")"
    export PGHOST PGPORT
    return 0
  fi
  proxies="$(gql "$(jq -n --arg s "$POSTGRES_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" \
    '{query: "query($s: String!, $e: String!) { tcpProxies(serviceId: $s, environmentId: $e) { id domain proxyPort } }", variables: {s: $s, e: $e}}')")"
  if [ "$(jq '.data.tcpProxies | length' <<<"$proxies")" = 0 ]; then
    gql "$(jq -n --arg s "$POSTGRES_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" \
      '{query: "mutation($e: String!, $patch: EnvironmentConfig!) { environmentPatchCommit(environmentId: $e, patch: $patch, commitMessage: \"a TCP proxy for a migration rollback, removed after\", skipDeploys: true) }",
        variables: {e: $e, patch: {services: {($s): {networking: {tcpProxies: {"5432": {}}}}}}}}')" >/dev/null
    touch "$STATE_DIR/proxy-opened"
    for _ in $(seq 1 30); do
      proxies="$(gql "$(jq -n --arg s "$POSTGRES_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" \
        '{query: "query($s: String!, $e: String!) { tcpProxies(serviceId: $s, environmentId: $e) { id domain proxyPort } }", variables: {s: $s, e: $e}}')")"
      [ "$(jq '.data.tcpProxies | length' <<<"$proxies")" = 0 ] || break
      sleep 5
    done
  fi
  PGHOST="$(jq -r '.data.tcpProxies[0].domain' <<<"$proxies")"
  PGPORT="$(jq -r '.data.tcpProxies[0].proxyPort' <<<"$proxies")"
  export PGHOST PGPORT
}

db_close() { # removes only a proxy this run opened
  [ -f "$STATE_DIR/proxy-opened" ] || return 0
  local id proxies deleted
  proxies="$(gql "$(jq -n --arg s "$POSTGRES_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" \
    '{query: "query($s: String!, $e: String!) { tcpProxies(serviceId: $s, environmentId: $e) { id } }", variables: {s: $s, e: $e}}')")" || return 1
  # Missing or malformed data cannot prove that the proxy is absent.
  id="$(jq -er '.data.tcpProxies | if type == "array" then
    if length == 0 then "" else .[0].id | select(type == "string" and length > 0) end
    else error("tcpProxies must be an array") end' <<<"$proxies")" || return 1
  if [ -n "$id" ]; then
    deleted="$(gql "$(jq -n --arg i "$id" '{query: "mutation($i: String!) { tcpProxyDelete(id: $i) }", variables: {i: $i}}')")" || return 1
    jq -e '.data.tcpProxyDelete == true' >/dev/null <<<"$deleted" || return 1
  fi
  rm -f "$STATE_DIR/proxy-opened" || return 1
  echo "the TCP proxy is removed"
}

# Before going back to the build that ran before: the storage migrations the newer kernel added that the older one does
# not know. An older kernel refuses a database that recorded them ("migration N was previously applied but is missing"),
# so their _sqlx_migrations rows go, but only when every one is listed as additive. Fails with the reason otherwise.
migrations_back_to() { # $1 full TemperPaw commit that ran before
  local old new extra additive rows
  old="$(known_migration "$1")" || { echo "the migrations of ${1} could not be read" >&2; return 1; }
  new="$(known_migration "$EXPECTED_SHA")" || { echo "the migrations of ${EXPECTED_SHA} could not be read" >&2; return 1; }
  [ "$new" -gt "$old" ] || { echo "no storage migration between them (both know up to ${old})"; return 0; }
  extra="$(seq $((old + 1)) "$new" | tr '\n' ' ')"
  additive="$(awk '/^[0-9]+ / { print $1 }' "$(dirname "$0")/additive_migrations.txt")"
  for v in $extra; do
    grep -qx "$v" <<<"$additive" || {
      echo "can't roll back past migration ${v}: it is not listed as additive, and ${1:0:7} knows only up to ${old}" >&2
      return 1
    }
  done
  if [ "${MIGRATION_ROLLBACK:-}" != auto ] || [ -z "${POSTGRES_SERVICE_ID:-}" ]; then
    echo "going back needs a hand step first: DELETE FROM _sqlx_migrations WHERE version IN ($(tr ' ' ',' <<<"${extra% }")); (additive: ${extra% })" >&2
    return 1
  fi
  db_open
  rows="$(psql -v ON_ERROR_STOP=1 -Atc "DELETE FROM _sqlx_migrations WHERE version IN ($(tr ' ' ',' <<<"${extra% }")) RETURNING version" | tr '\n' ' ')" ||
    { db_close; echo "the migration rows could not be removed" >&2; return 1; }
  echo "removed the _sqlx_migrations rows ${rows:-none} (additive: ${extra% }); the next forward deploy applies them again"
  db_close
}

report_misses() { # exits 1 when there are any
  if [ "${#misses[@]}" -gt 0 ]; then
    printf 'MISS: %s\n' "${misses[@]}"
    printf '%s\n' "${misses[@]}" >>"$STATE_DIR/misses.txt"
    exit 1
  fi
}

case "${1:-}" in
  snapshot)
    live="$(live_deployment)"
    [ -n "$live" ] || { echo "no SUCCESS deployment in this environment" >&2; exit 1; }
    id="$(jq -r .id <<<"$live")"
    image="$(jq -r .image <<<"$live")"
    # The way back, in order: the image Railway deployed the running deployment from; for an upload, the IMAGE_TAG it
    # was built from; then the tag given by hand.
    tag=""
    [[ "$image" == "${IMAGE_REPO}:"* ]] && tag="${image#"${IMAGE_REPO}":}"
    [ -n "$tag" ] || tag="$(image_tag_var)"
    [ -n "$tag" ] || tag="${ROLLBACK_TAG:-}"
    [[ "$tag" =~ ^sha-[0-9a-f]{7,40}$ ]] || {
      echo "nothing names the image of the running deployment ${id} (no image, no IMAGE_TAG, no rollback_tag): no way back, so no deploy" >&2
      exit 1
    }
    # The way back must be the build that serves, and every baseline must have been read: without one, a check after
    # the deploy would have nothing to compare with.
    sha="$(live_sha)"
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "/paw/version names no commit, so the running build is unknown: no deploy" >&2; exit 1; }
    [ "${sha:0:$((${#tag} - 4))}" = "${tag#sha-}" ] || {
      echo "${tag} does not name the build that serves (${sha}): no way back, so no deploy" >&2
      exit 1
    }
    routes="$(route_count "$id")"
    [[ "$routes" =~ ^[0-9]+$ ]] || { echo "deployment ${id} logged no route table for the default tenant: no baseline, so no deploy" >&2; exit 1; }
    probes="$(probe)"
    guest_was="$(guest)"
    jq -n --arg id "$id" --arg tag "$tag" --arg routes "$routes" --arg sha "$sha" --argjson probes "$probes" --arg guest "$guest_was" \
      '{deployment_id: $id, image_tag: $tag, route_count: $routes, sha: $sha, probes: $probes, guest: $guest}' | tee "$STATE_DIR/before.json"
    ;;

  deploy)
    tag="${2:-}"
    [[ "$tag" =~ ^sha-[0-9a-f]{7,40}$ ]] || { echo "deploy takes a sha-<hex> tag" >&2; exit 64; }
    id="$(deploy_image "$tag")"
    jq -n --arg id "$id" --arg tag "$tag" '{deployment_id: $id, image_tag: $tag}' | tee "$STATE_DIR/new.json"
    ;;

  check)
    misses=()
    new_id="$(jq -r .deployment_id "$STATE_DIR/new.json")"
    if [ -n "${EXPECTED_SHA:-}" ]; then
      sha="$(live_sha)"
      [ "$sha" = "$EXPECTED_SHA" ] || misses+=("/paw/version reports ${sha:-nothing}, not ${EXPECTED_SHA}")
    fi
    against_before "$new_id"
    if [ "${#misses[@]}" = 0 ]; then
      echo "running ${SOAK_SECONDS}s, then everything is checked again and memory read"
      sleep "$SOAK_SECONDS"
      [ "$(live_deployment | jq -r '.id // empty')" = "$new_id" ] || misses+=("deployment ${new_id} no longer serves after the soak")
      if [ -n "${EXPECTED_SHA:-}" ]; then
        sha="$(live_sha)"
        [ "$sha" = "$EXPECTED_SHA" ] || misses+=("after the soak /paw/version reports ${sha:-nothing}, not ${EXPECTED_SHA}")
      fi
      against_before "$new_id"
      mem="$(memory_gb "$SOAK_SECONDS" || true)"
      echo "memory (Railway, highest over ${SOAK_SECONDS}s): ${mem:-none} GB${MEMORY_LIMIT_GB:+, limit ${MEMORY_LIMIT_GB} GB}"
      if [ -n "${MEMORY_LIMIT_GB:-}" ]; then
        if [ -z "$mem" ]; then
          misses+=("Railway reported no memory, so the ${MEMORY_LIMIT_GB} GB limit could not be checked")
        elif awk -v m="$mem" -v l="$MEMORY_LIMIT_GB" 'BEGIN { exit !(m > l) }'; then
          misses+=("memory ${mem} GB over ${MEMORY_LIMIT_GB} GB")
        fi
      fi
      if [ -n "${DD_API_KEY:-}" ] && [ -n "${DD_APP_KEY:-}" ] && [ -n "${DD_ENV:-}" ] && [ -n "${EXPECTED_SHA:-}" ]; then
        rss="$(datadog_max process_resident_memory_bytes "$SOAK_SECONDS" || true)"
        kept="$(datadog_max jemalloc_resident_bytes "$SOAK_SECONDS" || true)"
        held="$(datadog_max jemalloc_allocated_bytes "$SOAK_SECONDS" || true)"
        echo "memory gauges (Datadog, this build): resident ${rss:-none} B, jemalloc resident ${kept:-none} B, allocated ${held:-none} B"
        [ -n "$rss" ] && [ -n "$kept" ] && echo "resident beyond jemalloc: $(awk -v r="$rss" -v k="$kept" 'BEGIN { printf "%.0f MB", (r - k) / 1e6 }')"
      fi
      jq --arg mem "${mem:-}" --arg rss "${rss:-}" --arg kept "${kept:-}" --arg held "${held:-}" \
        '. + {memory_gb: $mem, resident_bytes: $rss, jemalloc_resident_bytes: $kept, jemalloc_allocated_bytes: $held}' \
        "$STATE_DIR/checked.json" >"$STATE_DIR/checked.json.tmp" && mv "$STATE_DIR/checked.json.tmp" "$STATE_DIR/checked.json"
    fi
    [ -z "${DRILL:-}" ] || misses+=("drill: a deliberate miss, to prove the way back")
    report_misses
    echo "every check passed"
    ;;

  rollback)
    trap db_close EXIT
    [ -f "$STATE_DIR/source-set" ] || { echo "the service was not changed, so nothing to roll back"; exit 0; }
    tag="$(jq -r .image_tag "$STATE_DIR/before.json")"
    before_id="$(jq -r .deployment_id "$STATE_DIR/before.json")"
    before_sha="$(jq -r .sha "$STATE_DIR/before.json")"
    started="$(cat "$STATE_DIR/last-started" 2>/dev/null || true)"
    misses=()
    if [ -z "$started" ] || { [ "$(jq -r '.status // empty' <<<"$(deployment_status "$started")")" != SUCCESS ] &&
      [ "$(live_deployment | jq -r '.id // empty')" = "$before_id" ]; }; then
      # Nothing took over, so the old deployment still serves: stop whatever started (a deployment Railway may have
      # begun without answering is the newest one after the old), point the source back, and restart nothing.
      [ -n "$started" ] || started="$(gql "$(jq -n --arg s "$RAILWAY_SERVICE_ID" --arg e "$RAILWAY_ENVIRONMENT_ID" --arg p "$RAILWAY_PROJECT_ID" \
        '{query: "query($s: String!, $e: String!, $p: String!) { deployments(first: 1, input: {projectId: $p, serviceId: $s, environmentId: $e}) { edges { node { id } } } }",
          variables: {s: $s, e: $e, p: $p}}')" | jq -r --arg old "$before_id" '.data.deployments.edges[0].node.id // empty | select(. != $old)')"
      if [ -n "$started" ]; then
        gql "$(jq -n --arg d "$started" '{query: "mutation($d: String!) { deploymentCancel(id: $d) }", variables: {d: $d}}')" >/dev/null ||
          gql "$(jq -n --arg d "$started" '{query: "mutation($d: String!) { deploymentRemove(id: $d) }", variables: {d: $d}}')" >/dev/null
        echo "deployment ${started} stopped"
      fi
      set_image "$tag"
      echo "${before_id} (${tag}) kept serving"
      # It serves, but a restart would boot its kernel against whatever the new one migrated.
      migrations_back_to "$before_sha" || misses+=("the old deployment serves but would not survive a restart: see above")
      id="$before_id"
    else
      migrations_back_to "$before_sha" || { misses+=("no way back to ${tag}: see above"); report_misses; }
      echo "going back to ${tag}"
      id="$(deploy_image "$tag")"
    fi
    jq -n --arg id "$id" --arg tag "$tag" '{deployment_id: $id, image_tag: $tag}' | tee "$STATE_DIR/rolled-back.json"
    sha="$(live_sha)"
    [ "$sha" = "$before_sha" ] || misses+=("/paw/version reports ${sha:-nothing}, not ${before_sha} that served before")
    against_before "$id"
    report_misses
    echo "back on ${tag}, and it answers as it did before"
    ;;

  *)
    echo "usage: railway_release.sh snapshot|deploy <sha-tag>|check|rollback" >&2
    exit 64
    ;;
esac
