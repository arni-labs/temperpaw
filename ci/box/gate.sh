#!/usr/bin/env bash
# Runs on ci-temperpaw, as tl-user: tests one commit as .github/workflows/ci.yml did on GitHub's runners for a push to
# main (jobs checks, tests-root, tests-others, full, by the same steps and the commit's own scripts), and builds
# docker.yml's image from the Dockerfile with a warm BuildKit cache, tagged as docker.yml tagged a push to main:
#   gate.sh <commit> <base>        CI_ALL=1 builds every os-app module again, as the nightly did (no --reuse)
# ci/box/box.sh uploads this file and the git bundle (/var/lib/ci/in.bundle) before it calls it. Warm by design: one
# worktree at one path, so a checkout keeps unchanged files' times and cargo rebuilds only what the commit changed;
# target/, dashboard/node_modules and the packaged os-app modules (os-apps/**/*.wasm, with their stamps in
# ~/.cache/temperpaw-os-app-modules, as the Actions cache held them) survive the clean between runs. Docker runs for
# the gate's length only; its build cache stays on disk. Answers go to /var/lib/ci/runs/<commit>/ (<job>.txt ending
# `exit <code>`; summary.txt; image-name.txt naming the built image; verdict.txt last).
# Left out on purpose: pushing the image (ci/box/publish.sh does that, with a credential the box does not keep).
# Exit 0 when every job passed, 1 when one failed, 2 on a usage error. Prints no key.
set -uo pipefail
# The Factory's command holds /var/lib/ci/box.lock on fd 9 for as long as this runs. Closed here, so no daemon this
# starts (a server, Postgres, Redis, dockerd) inherits it and keeps the box locked after a run that was killed.
exec 9>&-
commit=${1:?usage: gate.sh <commit> <base>} base=${2:?usage: gate.sh <commit> <base>}
CI=/var/lib/ci repo=/var/lib/ci/repo.git tree=/var/lib/ci/tree
. /opt/ci/env.sh
[ "$(id -u)" != 0 ] || { echo "gate: run as a user that is not root, as GitHub's runner is"; exit 2; }
t0=$SECONDS
if [ -s "$CI/in.bundle" ]; then
    git -C "$repo" fetch -q "$CI/in.bundle" '+refs/ci/*:refs/ci/*' || { echo "gate: the bundle did not fetch"; exit 2; }
    rm -f "$CI/in.bundle"
fi
# A commit the box lacks comes from origin (https://github.com/arni-labs/temperpaw.git), as the Factory's route runs a gate: no bundle, only the
# two commits. ci/box/box.sh's bundle carries them instead when it drives the box.
keys=${CI_KEYS:-$CI/keys}
git -C "$repo" remote get-url origin >/dev/null 2>&1 || git -C "$repo" remote add origin https://github.com/arni-labs/temperpaw.git
for c in "$commit" "$base"; do
    git -C "$repo" cat-file -e "$c^{commit}" 2>/dev/null || git -C "$repo" fetch -q --depth=200 origin "$c" \
        || { echo "gate: $c is neither on the box nor fetched from origin"; exit 2; }
done
commit=$(git -C "$repo" rev-parse --verify "$commit^{commit}") || exit 2
base=$(git -C "$repo" rev-parse --verify "$base^{commit}") || exit 2
git -C "$repo" update-ref "refs/ci/tested/$commit" "$commit"
git -C "$repo" for-each-ref --sort=-committerdate --format='%(refname)' refs/ci/tested | tail -n +51 | xargs -r -n1 git -C "$repo" update-ref -d
if git -C "$tree" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "$tree" reset -q --hard && git -C "$tree" checkout -q --force --detach "$commit" \
      && git -C "$tree" clean -q -ffdx -e target/ -e node_modules/ -e '/os-apps/**/*.wasm' || exit 2
else
    rm -rf "$tree"; git -C "$repo" worktree prune; git -C "$repo" worktree add -q --force --detach "$tree" "$commit" || exit 2
fi
out=$CI/runs/$commit && rm -rf "$out" && mkdir -p "$out" && touch "$out/summary.txt"
ulimit -n "$(ulimit -Hn)"
cd "$tree" || exit 2
exec 3>&1
echo "gate: $commit against $base on $(nproc) cores; checkout in $((SECONDS - t0)) s"

# --- the jobs: one function each, named as ci.yml names them --------------------------------------------------------
job_checks() { # in a target of its own, so clippy never waits on the tests' build lock
    export CARGO_TARGET_DIR=$tree/target/lint
    echo "== WASM artifact packaging regression"; bash scripts/test-wasm-build-artifacts.sh || return
    echo "== Channel continuation regression"
    ( source os-apps/wasm-build-env.sh && wasm_path="$(temperpaw_build_wasm "$PWD/os-apps/paw-channels/wasm/route_message" wasm32-wasip1)" \
      && /opt/ci/rmt/bin/python scripts/test-route-message-continuation.py "$wasm_path" ) || return
    echo "== Datadog monitors file"; python3 scripts/check_monitors.py || return
    echo "== cargo fmt"; cargo fmt --all -- --check || return
    echo "== cargo clippy"; cargo clippy --locked -p temperpaw -p paw-codex-worker --all-targets -- -D warnings || return
    echo "== Worker script syntax"; local f; for f in crates/paw-codex-worker/scripts/*.sh; do bash -n "$f" || return; done
    echo "== WASM rsa crates on wasm32-unknown-unknown"
    bash scripts/check-wasm-rsa-getrandom.sh \
      && cargo check --manifest-path os-apps/paw-patrol/wasm/release_run_lifecycle/Cargo.toml --target wasm32-unknown-unknown
}
job_tests_root() { # cargo test (root workspace)
    echo "== Every os-app module, built and packaged as a release builds it"
    if [ "${CI_ALL:-0}" = 1 ]; then python3 scripts/ci/build_os_app_modules.py; else python3 scripts/ci/build_os_app_modules.py --reuse; fi || return
    bash scripts/verify_route_message_wasm.sh || return
    echo "== cargo test, the root workspace"; python3 scripts/ci/rust_tests.py root
}
job_tests_others() { python3 scripts/ci/rust_tests.py others 1/1; } # both of GitHub's parts, one machine
job_full() {
    cd dashboard || return
    local sum; sum=$(sha256sum package-lock.json | cut -c1-64)
    [ "$(cat node_modules/.ci-lock 2>/dev/null)" = "$sum" ] || { npm install --no-audit --no-fund && echo "$sum" >node_modules/.ci-lock; } || return
    cd .. && echo "== Smoke scripts" && crates/paw-codex-worker/scripts/production-git-ancestry-guard-smoke.sh || return
    echo "== Dashboard build"; cd dashboard && npm run build
}
# docker.yml's image of a push to main: tags edge and sha-<7>, build args BUILD_VERSION and BUILD_SHA.
job_image() {
    local image=ghcr.io/arni-labs/temperpaw
    DOCKER_HOST=unix://$CI/docker.sock docker buildx build --load --progress=plain \
      -t "$image:sha-${commit:0:7}" -t "$image:edge" \
      --build-arg "BUILD_VERSION=sha-${commit:0:7}" --build-arg "BUILD_SHA=$commit" \
      --label org.opencontainers.image.title=TemperPaw \
      --label "org.opencontainers.image.description=TemperPaw - Agent daemon built on Temper platform" \
      --label org.opencontainers.image.source=https://github.com/arni-labs/temperpaw \
      --label org.opencontainers.image.revision="$commit" . || return
    printf '%s\n' "$image:sha-${commit:0:7}" "$image:edge" >"$out/image-name.txt"
    DOCKER_HOST=unix://$CI/docker.sock docker image inspect --format '{{.Id}} {{.Size}}' "$image:sha-${commit:0:7}"
}

run_job() {
    local name=$1 s=$SECONDS c
    ( set -o pipefail; eval "job_${1//-/_}" ) >"$out/$name.txt" 2>&1 </dev/null; c=$?
    echo "exit $c" >>"$out/$name.txt"
    printf '%-24s exit %-3s %5s s\n' "$name" "$c" "$((SECONDS - s))" | tee -a "$out/summary.txt" >&3
}
# Fail fast (ci-migration, 25 Nov): once a job fails the verdict is red, so the lanes still running stop then instead of
# running to their end; each failed job's tail still goes in the log below. set -m gives each lane its own process
# group, so the kill reaches every cargo, node and docker client it started. /proc, since a finished lane can linger
# as a zombie until it is waited for.
lanes_wait() {
    local p alive
    while :; do
        alive=(); for p in "$@"; do [ -r "/proc/$p/status" ] && ! grep -q '^State:[[:space:]]*Z' "/proc/$p/status" && alive+=("$p"); done
        [ ${#alive[@]} -gt 0 ] || break
        if awk '$3 != 0 {f = 1} END {exit !f}' "$out/summary.txt"; then
            echo "gate: a job failed, so the lanes still running stop" >&3
            for p in "${alive[@]}"; do kill -TERM -- "-$p" 2>/dev/null; done; break
        fi
        sleep 2
    done
    wait "$@" 2>/dev/null
}
# Docker for the gate's length (a process the run started dies with it); its data and build cache stay in /var/lib/docker.
docker_up() {
    sudo -n dockerd --group "$(id -gn)" -H "unix://$CI/docker.sock" --pidfile "$CI/docker.pid" >"$CI/dockerd.log" 2>&1 &
    for _ in $(seq 60); do DOCKER_HOST=unix://$CI/docker.sock docker info >/dev/null 2>&1 && return; sleep 1; done
    tail -20 "$CI/dockerd.log"; return 1
}
docker_down() { [ -s "$CI/docker.pid" ] && sudo -n kill "$(cat "$CI/docker.pid")" 2>/dev/null; sleep 2; true; }
trap docker_down EXIT
docker_up || { echo "gate: dockerd did not start"; exit 1; }
( peak=0; while sleep 2; do
    used=$(awk '/MemTotal/ {t=$2} /MemAvailable/ {a=$2} END {print int((t - a) / 1024)}' /proc/meminfo)
    [ "$used" -le "$peak" ] || { peak=$used; echo "$peak" >"$out/peak-mem-mb.txt"; }
done ) >/dev/null 2>&1 &
sampler=$!
# Each lane in a cargo target of its own; the ones that only compile yield the cores (nice 10) to the tests.
set -m
( renice -n 10 "$BASHPID" >/dev/null; run_job checks ) & l1=$!
run_job tests-root & l2=$!
( renice -n 10 "$BASHPID" >/dev/null; run_job tests-others; run_job full ) & l3=$!
( renice -n 10 "$BASHPID" >/dev/null; run_job image ) & l4=$!
set +m
lanes_wait $l1 $l2 $l3 $l4
kill "$sampler" 2>/dev/null; wait "$sampler" 2>/dev/null
echo "gate: peak memory in use $(cat "$out/peak-mem-mb.txt" 2>/dev/null || echo 0) MB of $(awk '/MemTotal/ {print int($2 / 1024)}' /proc/meminfo) MB"
verdict=passed
for f in $(awk '$3 != 0 {print $1}' "$out/summary.txt"); do verdict=failed; echo "== $f (last 40 lines)"; tail -40 "$out/$f.txt"; done
echo "gate: $(wc -l <"$out/summary.txt") jobs, verdict $verdict in $((SECONDS - t0)) s"
said=$(awk '$3 != 0 {printf "%s%s", sep, $1; sep=", "}' "$out/summary.txt")
# A passed push is published when CI_PUBLISH=1: its image to ghcr (the tags image-name.txt lists), with keys/ghcr-token
# (a token that may write packages), as docker.yml pushed it. A push that fails fails the verdict: the Factory's
# railway target deploys only the tag it names.
if [ $verdict = passed ] && [ "${CI_PUBLISH:-0}" = 1 ]; then
    s=$SECONDS
    ( set -e; [ -s "$keys/ghcr-token" ] || { echo "no ghcr-token in $keys"; exit 1; }
      export DOCKER_HOST=unix://$CI/docker.sock DOCKER_CONFIG; DOCKER_CONFIG=$(mktemp -d)
      trap 'rm -rf "$DOCKER_CONFIG"' EXIT
      tr -d '\n' <"$keys/ghcr-token" | docker login ghcr.io -u ci-box --password-stdin
      while read -r tag; do docker push -q "$tag"; done <"$out/image-name.txt" ) >"$out/publish.txt" 2>&1; c=$?
    echo "exit $c" >>"$out/publish.txt"
    printf '%-24s exit %-3s %5s s\n' publish "$c" "$((SECONDS - s))" | tee -a "$out/summary.txt"
    [ $c = 0 ] || { verdict=failed said="publish"; tail -30 "$out/publish.txt"; }
fi

echo "$verdict" >"$out/verdict.txt"
# The verdict to the Factory's gate hook, as ci-temper-v2 and ci-katagami send it: {type, id, tests_url}, signed with
# HMAC-SHA256 of the body under keys/hook-secret.
if [ -n "${GATE_HOOK:-}" ]; then
    if [ -s "$keys/hook-secret" ]; then
        body=$(python3 -c 'import json,sys; print(json.dumps({"type": sys.argv[1], "id": sys.argv[2], "tests_url": sys.argv[3]}))' \
            "$verdict" "$commit" "ci-temperpaw box gate $verdict${said:+: $said}; $out on tbskhr4v3zlm1gy0m7m6m")
        signature=$(printf '%s' "$body" | python3 -c 'import hmac,sys; print(hmac.new(open(sys.argv[1],"rb").read().strip(), sys.stdin.buffer.read(), "sha256").hexdigest())' "$keys/hook-secret")
        curl -sS --fail-with-body --max-time 30 -X POST -H 'content-type: application/json' \
            -H "x-katagami-signature: sha256=$signature" --data "$body" "$GATE_HOOK" && echo "gate: the gate hook took the verdict" ||
            echo "gate: the Factory's gate hook did not take the verdict"
    else echo "gate: no hook-secret in $keys, so the verdict went to no hook"; fi
fi
[ $verdict = passed ]
