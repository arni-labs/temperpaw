#!/usr/bin/env bash
# Runs on ci-temperpaw, as ci-test: tests one commit as .github/workflows/ci.yml did on GitHub's runners for a push to
# main (jobs checks, tests-root, tests-others, full, and replay.yml's replay, by the same steps and the commit's own scripts), and builds
# docker.yml's image from the Dockerfile with a warm BuildKit cache, tagged as docker.yml tagged a push to main:
#   gate.sh <commit> <base>        CI_ALL=1 builds every os-app module again, as the nightly did (no --reuse)
# /opt/ci/route.sh (the fixed wrapper) runs it as ci-test, which holds no key and has no sudo, after it put the two
# commits in /var/lib/ci/work/test.git (refs/ci/commit, refs/ci/base); it publishes and sends the verdict. Warm by design: one
# worktree at one path, so a checkout keeps unchanged files' times and cargo rebuilds only what the commit changed;
# target/, dashboard/node_modules and the packaged os-app modules (os-apps/**/*.wasm, with their stamps in
# ~/.cache/temperpaw-os-app-modules, as the Actions cache held them) survive the clean between runs. Docker runs for
# the gate's length only, rootless, as ci-test; its build cache stays on disk. Answers go to /var/lib/ci/work/runs/<commit>/ (<job>.txt ending
# `exit <code>`; summary.txt; image-name.txt naming the built image; verdict.txt last).
# With CI_PUBLISH=1 a passed push's image is saved to publish/image.tar there, for route.sh to put in R2.
# Exit 0 when every job passed, 1 when one failed, 2 on a usage error. Prints no key.
set -uo pipefail
commit=${1:?usage: gate.sh <commit> <base>} base=${2:?usage: gate.sh <commit> <base>}
CI=/var/lib/ci/work repo=/var/lib/ci/work/test.git tree=/var/lib/ci/work/tree
. /opt/ci/env.sh
[ "$(id -u)" != 0 ] || { echo "gate: runs as ci-test, through /opt/ci/route.sh"; exit 2; }
t0=$SECONDS
commit=$(git -C "$repo" rev-parse --verify "$commit^{commit}") || exit 2
base=$(git -C "$repo" rev-parse --verify "$base^{commit}") || exit 2
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
# replay.yml's check, which ran on pull requests: whether a spec change rewrites the records it already has
# (.github/ci/replay.py). On a push the intent lines ("Replay intended: ...") come from the commit messages since base,
# as katagami's gate reads them.
job_replay() {
    python3 -m unittest discover -s .github/ci -p 'replay_test.py' || return
    git log --format=%B "$base..$commit" >"$out/replay-messages.txt" && python3 .github/ci/replay.py "$base" "$commit" "$out/replay-messages.txt"
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
    docker buildx build --load --progress=plain \
      -t "$image:sha-${commit:0:7}" -t "$image:edge" \
      --build-arg "BUILD_VERSION=sha-${commit:0:7}" --build-arg "BUILD_SHA=$commit" \
      --label org.opencontainers.image.title=TemperPaw \
      --label "org.opencontainers.image.description=TemperPaw - Agent daemon built on Temper platform" \
      --label org.opencontainers.image.source=https://github.com/arni-labs/temperpaw \
      --label org.opencontainers.image.revision="$commit" . || return
    printf '%s\n' "$image:sha-${commit:0:7}" "$image:edge" >"$out/image-name.txt"
    docker image inspect --format '{{.Id}} {{.Size}}' "$image:sha-${commit:0:7}"
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
# Docker for the gate's length, rootless: ci-test's own dockerd (rootlesskit), so a build or a container is ci-test and
# no more. Its data and build cache stay in ~/.local/share/docker; route.sh stops every ci-test process after the gate.
export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock
docker_up() {
    dockerd-rootless.sh >"$CI/dockerd.log" 2>&1 & docker_pid=$!
    for _ in $(seq 60); do docker info >/dev/null 2>&1 && return; sleep 1; done
    tail -20 "$CI/dockerd.log"; return 1
}
docker_down() { [ -z "${docker_pid:-}" ] || kill "$docker_pid" 2>/dev/null; sleep 2; true; }
trap docker_down EXIT
docker_up || { echo "gate: dockerd did not start"; exit 1; }
( peak=0; while sleep 2; do
    used=$(awk '/MemTotal/ {t=$2} /MemAvailable/ {a=$2} END {print int((t - a) / 1024)}' /proc/meminfo)
    [ "$used" -le "$peak" ] || { peak=$used; echo "$peak" >"$out/peak-mem-mb.txt"; }
done ) >/dev/null 2>&1 &
sampler=$!
# Each lane in a cargo target of its own; the ones that only compile yield the cores (nice 10) to the tests.
set -m
( renice -n 10 "$BASHPID" >/dev/null; run_job replay; run_job checks ) & l1=$!
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
# A passed push to publish (CI_PUBLISH=1, from route.sh): its sha-<7> image as a `docker save` archive, which route.sh
# hashes and puts in R2 for the release box. A save that fails fails the verdict, since the line needs the image.
if [ $verdict = passed ] && [ "${CI_PUBLISH:-0}" = 1 ]; then
    mkdir -p "$out/publish" && docker save -o "$out/publish/image.tar" "ghcr.io/arni-labs/temperpaw:sha-${commit:0:7}" \
        || { verdict=failed; echo "gate: the image did not save"; }
fi
echo "$verdict" >"$out/verdict.txt"
[ $verdict = passed ]
