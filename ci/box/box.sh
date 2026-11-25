#!/usr/bin/env bash
# Drives ci-temperpaw, the one warm Tensorlake box that runs temperpaw's checks in place of GitHub Actions (ci.yml, docker.yml).
# Run from any checkout of arni-labs/temperpaw on a machine holding the Tensorlake project key:
#   ci/box/box.sh run [<commit> [<base>]]   gate <commit> (default HEAD) against <base> (default <commit>^), as a push
#                                           to main from <base> is tested; exit 0 passed, 1 failed
#   ci/box/box.sh provision                 install or update the box's tools (ci/box/provision.sh), idempotent
#   ci/box/box.sh status | suspend | exec <bash>
# The box is named, keeps its disk across sleeps (git clone, cargo targets, node_modules, toolchains), wakes on the
# first call (auto_resume) and sleeps when idle: this script suspends it after each run, and Tensorlake does after 600 s
# with no ingress if anything is left awake. Commits reach it as a git bundle of what it lacks (the first run seeds a
# shallow clone), so it holds no GitHub credential. Each run uploads this checkout's ci/box/gate.sh, which then runs the
# commit's own scripts. With CI_STATUS=1 the verdict is posted to the commit as the GitHub status `ci-temperpaw` (gh).
# The key is read from TENSORLAKE_API_KEY or ~/.config/tensorlake/env and only ever handed to curl through a pipe, never
# in an argument (ps shows every argv); nothing here prints it.
set -euo pipefail
BOX=${CI_BOX:-ci-temperpaw}
API=https://api.tensorlake.ai
here=$(cd "$(dirname "$0")" && pwd)
if [ -z "${TENSORLAKE_API_KEY:-}" ]; then set -a; . ~/.config/tensorlake/env; set +a; fi
: "${TENSORLAKE_API_KEY:?no TENSORLAKE_API_KEY}"
tl() { curl -sS --fail-with-body --config <(printf 'header = "Authorization: Bearer %s"\n' "$TENSORLAKE_API_KEY") "$@"; }

# The API's own calls are retried (it answers 502 now and then); a process run never is, since it would run twice.
tl_api() { tl --retry 4 --retry-all-errors --retry-delay 2 "$@"; }
box_json() { tl_api "$API/sandboxes" | python3 -c '
import json, sys
d = json.load(sys.stdin); boxes = d.get("sandboxes", d) if isinstance(d, dict) else d
m = [b for b in boxes if b.get("name") == sys.argv[1]]
print(json.dumps(m[0]) if m else "")' "$BOX"; }
box_field() { python3 -c 'import json,sys; b=json.loads(sys.argv[1]); print(b.get(sys.argv[2]) or (b.get("id") if sys.argv[2] == "sandbox_id" else "") or "")' "$1" "$2"; }

# wake: resume the box if it sleeps and wait until it runs; prints its id.
wake() {
    local info id st waited=0
    info=$(box_json); [ -n "$info" ] || { echo "box: no sandbox named $BOX" >&2; return 1; }
    id=$(box_field "$info" sandbox_id)
    while :; do
        st=$(box_field "$(box_json)" status)
        case $st in
            running) break ;;
            suspended) tl_api -o /dev/null -X POST "$API/sandboxes/$id/resume" || true ;;
            terminated|archived|resume_failed) echo "box: $BOX is $st" >&2; return 1 ;;
        esac
        sleep 2; waited=$((waited + 2)); [ $waited -lt 300 ] || { echo "box: $BOX still $st after 300 s" >&2; return 1; }
    done
    echo "$id"
}

put() { tl -o /dev/null -X PUT --data-binary @"$2" "https://$1.sandbox.tensorlake.ai/api/v1/files?path=$3"; }

# run_on <id> <timeout s> <user> <bash script>: runs it to its end, streaming its output; exits with its exit code.
# Work inside a box is not traffic, so an authenticated call every 60 s keeps the box awake while the run lasts.
run_on() {
    local id=$1 timeout=$2 user=$3 script=$4 body beat
    body=$(python3 -c 'import json,sys; print(json.dumps({"command":"bash","args":["-lc",sys.argv[1]],"timeout":int(sys.argv[2]),"user":sys.argv[3]}))' "$script" "$timeout" "$user")
    ( while sleep 60; do tl -o /dev/null "https://$id.sandbox.tensorlake.ai/api/v1/processes" || true; done ) >/dev/null 2>&1 &
    beat=$!
    local code=0
    tl -N --max-time $((timeout + 60)) -X POST -H 'content-type: application/json' \
        "https://$id.sandbox.tensorlake.ai/api/v1/processes/run" -d "$body" | python3 -u -c '
import json, sys
code = 1
for line in sys.stdin:
    line = line.strip().removeprefix("data:").strip()
    if not line or line.startswith(":"):  # an event-stream keep-alive
        continue
    try:
        event = json.loads(line)
    except ValueError:
        print(line); continue
    if "line" in event:
        print(event["line"], file=sys.stderr if event.get("stream") == "stderr" else sys.stdout, flush=True)
    if event.get("exit_code") is not None:
        code = event["exit_code"]
sys.exit(code)' || code=$?
    { kill "$beat"; wait "$beat"; } 2>/dev/null || true
    return $code
}

suspend_box() { local id; id=$(box_field "$(box_json)" sandbox_id); tl_api -o /dev/null -X POST "$API/sandboxes/$id/suspend" && echo "box: $BOX suspended"; }

cmd=${1:-}; shift || true
case $cmd in
status)
    info=$(box_json); [ -n "$info" ] || { echo "no sandbox named $BOX"; exit 1; }
    python3 -c 'import json,sys; b=json.loads(sys.argv[1]); print(b.get("name"), b.get("sandbox_id") or b.get("id"), b.get("status"), b.get("resources"), "idle suspend", b.get("timeout_secs"), "s", b.get("suspend_reason") or "")' "$info" ;;
suspend) suspend_box ;;
exec)
    id=$(wake); run_on "$id" "${CI_TIMEOUT:-600}" tl-user "$*" ;;
provision)
    id=$(wake)
    put "$id" "$here/provision.sh" /tmp/ci-provision.sh
    run_on "$id" 3600 root 'bash /tmp/ci-provision.sh'
    suspend_box ;;
run)
    repo=$(git rev-parse --show-toplevel)
    commit=$(git rev-parse --verify "${1:-HEAD}^{commit}")
    base=$(git rev-parse --verify "${2:-$commit^}^{commit}")
    started=$(date +%s)
    id=$(wake)
    echo "box: $BOX ($id) awake in $(( $(date +%s) - started )) s"
    # What the box holds, so the bundle carries only what it lacks; a box with no clone gets a shallow one.
    have=$(run_on "$id" 120 tl-user 'git -C /var/lib/ci/repo.git for-each-ref --format="%(objectname)" 2>/dev/null | sort -u | head -200' 2>/dev/null || true)
    echo "box: asked what it holds in $(( $(date +%s) - started )) s"
    not=()
    for c in $have; do git -C "$repo" cat-file -e "$c^{commit}" 2>/dev/null && not+=("^$c"); done
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"; git -C "$repo" update-ref -d refs/ci/commit; git -C "$repo" update-ref -d refs/ci/base' EXIT
    git -C "$repo" update-ref "refs/ci/commit" "$commit"
    git -C "$repo" update-ref "refs/ci/base" "$base"
    if [ ${#not[@]} -eq 0 ]; then
        # A box with no clone gets a shallow one: the last 200 commits to <commit> and <base>, as a bare repository.
        git init -q --bare "$tmp/repo.git"
        git -C "$tmp/repo.git" fetch -q --depth=200 "file://$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)" \
            refs/ci/commit:refs/ci/commit refs/ci/base:refs/ci/base
        COPYFILE_DISABLE=1 tar --no-xattrs -czf "$tmp/repo.tgz" -C "$tmp" repo.git
        put "$id" "$tmp/repo.tgz" /var/lib/ci/repo.tgz
        run_on "$id" 300 tl-user 'cd /var/lib/ci && rm -rf repo.git && tar -xzf repo.tgz && rm repo.tgz && git -C repo.git config core.bare true'
        echo "box: seeded a shallow clone ($(du -h "$tmp/repo.tgz" | cut -f1))"
    else
        git -C "$repo" bundle create -q "$tmp/ci.bundle" refs/ci/commit refs/ci/base "${not[@]}" 2>/dev/null || true
    fi
    echo "box: git to send made in $(( $(date +%s) - started )) s"
    put "$id" "$here/gate.sh" /var/lib/ci/gate.sh
    if [ -s "$tmp/ci.bundle" ]; then put "$id" "$tmp/ci.bundle" /var/lib/ci/in.bundle; echo "box: sent $(du -h "$tmp/ci.bundle" | cut -f1) of git"; fi
    echo "box: ready to gate in $(( $(date +%s) - started )) s"
    code=0
    # The route's gates hold /var/lib/ci/box.lock while they run, so a hand run takes it too: one gate per box at a time.
    run_on "$id" 5400 tl-user "exec 9>/var/lib/ci/box.lock && flock 9 && cd /var/lib/ci && CI_ALL=${CI_ALL:-0} bash gate.sh $commit $base" || code=$?
    echo "box: verdict $([ $code = 0 ] && echo passed || echo failed) (exit $code) in $(( $(date +%s) - started )) s from wake"
    if [ "${CI_STATUS:-0}" = 1 ]; then
        gh api -X POST "repos/arni-labs/temperpaw/statuses/$commit" -f context=ci-temperpaw \
          -f state="$([ $code = 0 ] && echo success || echo failure)" \
          -f description="ci-temperpaw box gate against ${base:0:10}" >/dev/null && echo "box: status ci-temperpaw posted to $commit"
    fi
    # A route gate may be running, or waiting on the lock this run just let go: the box sleeps only when none holds it.
    if run_on "$id" 60 tl-user 'flock -n /var/lib/ci/box.lock true' >/dev/null 2>&1; then suspend_box
    else echo "box: $BOX left awake: a route gate holds box.lock"; fi
    exit $code ;;
*) sed -n '2,15p' "$0"; exit 2 ;;
esac
