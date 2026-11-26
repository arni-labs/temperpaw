#!/usr/bin/env bash
# /opt/ci/route.sh on ci-temperpaw: the only way into this box's gate, and the only code here that reads keys/. Root-owned,
# installed by ci/box/provision.sh from the checkout that provisions the box, never read from a commit at run time:
# no code from a commit runs as a user that can read a key (ci-migration, Forum r_17qbqd5ah0xf7940). As root:
#   route.sh <commit> <base> [<hook url>]   the Factory's route: sudo -n /opt/ci/route.sh {sha} {base} {hook}
#   route.sh --bundle <commit> <base>       ci/box/box.sh run: the commits come in /var/lib/ci/in/bundle; no hook
# It holds box.lock, fetches the two commits into root's /var/lib/ci/repo.git (origin over keys/deploy-key, or the
# bundle) and, while keys/ holds anything, takes only a commit on origin's main (r_sv0er8qphcswhr6k). Then ci-test (no
# sudo, no read on keys/, its own caches under /var/lib/ci/work) copies them into its own work/test.git and runs the
# commit's ci/box/gate.sh, and so every check. Every ci-test process is stopped before and after. Root never writes
# into work/ and reads from it only through ci-test (runuser ... cat), so nothing planted there reaches a root path.
# A passed route gate is published: the image the gate saved to work/runs/<commit>/publish/image.tar goes to R2 bucket
# $IMAGES with keys/images-write, for the release box to push to ghcr (stack ci/release/push-image.sh).
# The verdict goes to the hook signed with keys/hook-secret; a hook that never takes it fails the run.
# Exit: the gate's (0 passed, 1 failed), 2 on a usage error or a commit this box does not take.
set -uo pipefail
ORIGIN=https://github.com/arni-labs/temperpaw.git DEPTH=--depth=200 LABEL="ci-temperpaw box gate" BOX_ID=tbskhr4v3zlm1gy0m7m6m
IMAGES=temperpaw-images IMAGE=ghcr.io/arni-labs/temperpaw
CI=/var/lib/ci K=/var/lib/ci/keys W=/var/lib/ci/work T=ci-test
[ "$(id -u)" = 0 ] || { echo "route: runs as root (sudo -n /opt/ci/route.sh ...)"; exit 2; }
mode=route; [ "${1:-}" = --bundle ] && { mode=bundle; shift; }
commit=${1:-} base=${2:-} hook=${3:-}
[[ $commit =~ ^[0-9a-f]{40}$ && $base =~ ^[0-9a-f]{40}$ ]] || { echo "usage: route.sh [--bundle] <commit> <base> [<hook url>], full shas"; exit 2; }
[ -z "$hook" ] || [[ $hook =~ ^https://[A-Za-z0-9.-]+(/[A-Za-z0-9._~/?=\&%:+-]*)?$ ]] || { echo "route: the hook is not an https URL"; exit 2; }
[ $mode = route ] || [ -z "$hook" ] || { echo "route: a bundle run sends no verdict"; exit 2; }
mkdir -p "$CI/logs" && exec > >(tee -a "$CI/logs/gate-$commit.log") 2>&1
log=$!; trap 'exec >&- 2>&-; wait "$log"' EXIT # the log's last lines reach the caller before this exits
exec 9>"$CI/box.lock" && flock 9
t0=$SECONDS
uid=$(id -u "$T") || { echo "route: no user $T; run ci/box/provision.sh"; exit 2; }
# as_test <command>...: as ci-test, with an environment of its own, and without box.lock's fd.
as_test() { runuser -u "$T" -- env -i HOME="/home/$T" PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 XDG_RUNTIME_DIR="/run/user/$uid" "$@" 9>&-; }
sweep() { pkill -KILL -u "$T" 2>/dev/null; for _ in $(seq 50); do pgrep -u "$T" >/dev/null || return 0; sleep 0.2; done; echo "route: $T processes outlived kill -9"; }
sweep
install -d -o "$T" -g "$T" -m 755 "$W" && install -d -o "$T" -g "$T" -m 700 "/run/user/$uid"
# ci-test's rootless dockerd (rootlesskit's slirp4netns) opens /dev/net/tun; a tap still needs CAP_NET_ADMIN in its own
# namespace, so the usual 666 gives it nothing more.
[ ! -c /dev/net/tun ] || chmod 666 /dev/net/tun
publish=0; [ $mode = route ] && [ -s "$K/images-write" ] && publish=1

# --- the commits, into root's repo.git ------------------------------------------------------------------------------
R=$CI/repo.git
[ -d "$R" ] || git init -q --bare "$R"
git -C "$R" remote set-url origin "$ORIGIN" 2>/dev/null || git -C "$R" remote add origin "$ORIGIN"
[ -s "$K/deploy-key" ] && export GIT_SSH_COMMAND="ssh -i $K/deploy-key -o UserKnownHostsFile=$K/known_hosts -o IdentitiesOnly=yes"
if [ $mode = bundle ] && [ -f "$CI/in/bundle" ]; then
    git -C "$R" fetch -q "$CI/in/bundle" '+refs/ci/*:refs/ci/*' || { echo "route: the bundle did not fetch"; exit 2; }
    rm -f "$CI/in/bundle"
fi
for c in "$commit" "$base"; do
    git -C "$R" cat-file -e "$c^{commit}" 2>/dev/null || git -C "$R" fetch -q $DEPTH origin "$c" \
        || { echo "route: $c is neither on the box nor fetched from origin"; exit 2; }
done
if [ -n "$(ls -A "$K" 2>/dev/null)" ]; then
    git -C "$R" fetch -q $DEPTH origin main && git -C "$R" merge-base --is-ancestor "$commit" FETCH_HEAD || {
        echo "route: this box holds keys and ${commit:0:12} is not on origin's main, so it is not gated here; use a box with no keys"; exit 2; }
fi
git -C "$R" update-ref refs/ci/commit "$commit" && git -C "$R" update-ref refs/ci/base "$base"
git -C "$R" update-ref "refs/ci/tested/$commit" "$commit"
git -C "$R" for-each-ref --sort=-committerdate --format='%(refname)' refs/ci/tested | tail -n +51 | xargs -r -n1 git -C "$R" update-ref -d

# --- the commit's gate, as ci-test ----------------------------------------------------------------------------------
as_test bash -c 'set -e; cd "$1"; [ -d test.git ] || git init -q --bare test.git
    git -C test.git fetch -q --no-tags --update-shallow "$2" +refs/ci/commit:refs/ci/commit +refs/ci/base:refs/ci/base
    git -C test.git show "$3:ci/box/gate.sh" >"gate-$3.sh"' _ "$W" "$R" "$commit" || { echo "route: ci-test could not take the commit"; exit 2; }
echo "route: ${commit:0:12} against ${base:0:12} ($mode), handed to $T in $((SECONDS - t0)) s"
code=0
as_test CI_ALL="${CI_ALL:-0}" CI_PUBLISH=$publish bash "$W/gate-$commit.sh" "$commit" "$base" || code=$?
sweep
verdict=failed; [ $code = 0 ] && verdict=passed
said=$(as_test cat "$W/runs/$commit/summary.txt" 2>/dev/null | awk '$3 != 0 {printf "%s%s", sep, $1; sep=", "}' | tr -cd 'A-Za-z0-9 ,._-' | cut -c1-300)

# --- publish: images/<commit>/{image.tar,image.tar.sha256}, then COMMIT (the full sha) last, so one without COMMIT is
# unfinished. Every put is If-None-Match: *, so nothing published is ever replaced. A failed publish fails the run.
publish_image() (
    set -euo pipefail
    at=https://075a5c0a617de3bdc08a44f9794b6f2f.r2.cloudflarestorage.com/$IMAGES/images/$commit P=$CI/pub
    r2() { curl -sS --config <(printf 'user = "%s:%s"\n' "$(sed -n 1p "$K/images-write")" "$(sed -n 2p "$K/images-write")") \
               --aws-sigv4 aws:amz:auto:s3 "$@"; }
    put() { # put <file> <key>: 200 made, 412 already there (kept)
        local c; c=$(r2 -o /dev/null -w '%{http_code}' -H 'If-None-Match: *' \
            -H "x-amz-content-sha256: $(sha256sum "$1" | cut -c1-64)" -T "$1" "$at/$2")
        case $c in 200) ;; 412) echo "route: $2 was already there, kept" ;; *) echo "route: put $2 answered $c"; return 1 ;; esac; }
    [ "$(r2 -o /dev/null -w '%{http_code}' "$at/COMMIT")" != 200 ] || { echo "route: $commit is already published"; exit 0; }
    rm -rf "$P" && install -d -m 700 "$P" && trap 'rm -rf "$P"' EXIT
    # Read as ci-test, so a link planted in its place reads nothing ci-test could not.
    as_test cat "$W/runs/$commit/publish/image.tar" >"$P/image.tar"
    sha256sum "$P/image.tar" | cut -c1-64 >"$P/image.tar.sha256"
    put "$P/image.tar" image.tar && put "$P/image.tar.sha256" image.tar.sha256
    echo "$commit" >"$P/COMMIT" && put "$P/COMMIT" COMMIT
    echo "route: published $IMAGE:sha-${commit:0:7} to $at/, $(du -h "$P/image.tar" | cut -f1), sha256 $(cat "$P/image.tar.sha256")"
)
if [ $publish = 1 ] && [ $verdict = passed ]; then
    publish_image || { verdict=failed code=1 said="${said:+$said, }publish"; }
fi
echo "route: verdict $verdict (gate exit $code) in $((SECONDS - t0)) s"

# --- the verdict, to the Factory's gate hook: {type, id, tests_url}, HMAC-SHA256 of the body under keys/hook-secret ----
if [ -n "$hook" ]; then
    [ -s "$K/hook-secret" ] || { echo "route: no hook-secret in $K, so the verdict went to no hook"; exit 1; }
    body=$(python3 -c 'import json,sys; print(json.dumps({"type": sys.argv[1], "id": sys.argv[2], "tests_url": sys.argv[3]}))' \
        "$verdict" "$commit" "$LABEL $verdict${said:+: $said}; $W/runs/$commit on $BOX_ID")
    signature=$(printf '%s' "$body" | python3 -c 'import hmac,sys; print(hmac.new(open(sys.argv[1],"rb").read().strip(), sys.stdin.buffer.read(), "sha256").hexdigest())' "$K/hook-secret")
    took=no
    for try in 1 2 3 4 5; do
        curl -sS --fail-with-body --max-time 30 -X POST -H 'content-type: application/json' \
            -H "x-katagami-signature: sha256=$signature" --data "$body" "$hook" && { echo; took=yes; break; }
        sleep $((try * 3))
    done
    [ $took = yes ] && echo "route: the gate hook took the verdict" || { echo "route: the Factory's gate hook did not take the verdict"; exit 1; }
fi
exit $code
