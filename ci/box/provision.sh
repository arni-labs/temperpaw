#!/usr/bin/env bash
# Installs on ci-temperpaw (as root) every tool .github/workflows/ci.yml's jobs set up on GitHub's runners, and Docker
# with BuildKit for docker.yml's image. Idempotent: run again (ci/box/box.sh provision) after a version below changes.
# The box's users (ci-migration, Forum r_17qbqd5ah0xf7940: no code from a commit runs as a user that can read a key):
#   root     /opt/ci/route.sh, the fixed wrapper (from /tmp/ci-route.sh, which box.sh provision uploads), keys/,
#            repo.git, box.lock, logs/; /opt/ci (node, the wasmtime venv) is root's and read-only to the others.
#            in/ is tl-user's, for box.sh's uploads.
#   ci-test  runs the commit's gate.sh and every job: no sudo, no read on keys/. Its work, rustup and cargo homes are
#            in /var/lib/ci/work, and Docker is its own rootless dockerd (data in ~ci-test/.local/share/docker).
#   tl-user  the route's entry: its sudo is `sudo -n /opt/ci/route.sh` and nothing else
set -euo pipefail
NODE_MAJOR=22                 # actions/setup-node node-version: 22
WASMTIME_PY=38.0.0            # ci.yml's "Channel continuation regression" pip install wasmtime==38.0.0
MOBY=fbf3ed25f893e6ce21336f1101590e40a13934f4 # docker-v29.1.3, the docker.io here: contrib/dockerd-rootless.sh
ROOTLESS_SHA256=8eb00d61b5e78ffc0dcd628f056389b5859cc61d6fe3ce17b4a3a4b08143c8a5
CI=/var/lib/ci T=ci-test W=/var/lib/ci/work
# Waits for box.lock, the lock route.sh holds for a gate, before touching dockerd, /opt/ci or the work: provision run
# during a gate would kill its rootless dockerd and swap tools under it. flock -o holds the lock in flock itself, so no
# daemon provision starts (apt's postinst) inherits the lock and keeps it after provision ends.
if [ -z "${CI_PROVISION_LOCKED:-}" ]; then
    mkdir -p "$CI"
    flock -n "$CI/box.lock" true || echo "provision: a gate holds box.lock; waiting for it"
    exec env CI_PROVISION_LOCKED=1 flock -o "$CI/box.lock" bash "$0" "$@"
fi
export DEBIAN_FRONTEND=noninteractive
step() { echo "provision: $*"; }
step apt
apt-get update -q >/dev/null
apt-get install -yq --no-install-recommends build-essential pkg-config libssl-dev clang libclang-dev cmake libz3-dev \
    git jq curl ca-certificates xz-utils openssl python3 python3-venv procps psmisc docker.io docker-buildx \
    rootlesskit uidmap slirp4netns >/dev/null
systemctl disable --now docker.service docker.socket >/dev/null 2>&1 || true
pkill -x dockerd 2>/dev/null || true # a rootful one from before the wrapper
step "users"
getent passwd "$T" >/dev/null || useradd -m -s /bin/bash "$T"
grep -q "^$T:" /etc/subuid || echo "$T:200000:65536" >>/etc/subuid
grep -q "^$T:" /etc/subgid || echo "$T:200000:65536" >>/etc/subgid
s=$(mktemp); echo "tl-user ALL=(root) NOPASSWD: /opt/ci/route.sh" >"$s"
visudo -cqf "$s" && for f in $(grep -l '^tl-user' /etc/sudoers.d/* 2>/dev/null); do rm -f "$f"; done && install -m 440 "$s" /etc/sudoers.d/ci-route; rm -f "$s"
gpasswd -d tl-user sudo >/dev/null 2>&1 || true
step "rootless docker ($MOBY)"
curl -fsSL -o /tmp/dockerd-rootless.sh "https://raw.githubusercontent.com/moby/moby/$MOBY/contrib/dockerd-rootless.sh"
echo "$ROOTLESS_SHA256  /tmp/dockerd-rootless.sh" | sha256sum -c - >/dev/null && install -m 755 /tmp/dockerd-rootless.sh /usr/local/bin/dockerd-rootless.sh
step "node $NODE_MAJOR"
want=$(curl -fsSL https://nodejs.org/dist/index.json | python3 -c "import json,sys; print(next(r['version'] for r in json.load(sys.stdin) if r['version'].startswith('v$NODE_MAJOR.')))")
if [ "$(/opt/ci/node/bin/node --version 2>/dev/null)" != "$want" ]; then
    rm -rf /opt/ci/node.new && mkdir -p /opt/ci/node.new
    curl -fsSL "https://nodejs.org/dist/$want/node-$want-linux-x64.tar.xz" | tar -xJ -C /opt/ci/node.new --strip-components=1
    rm -rf /opt/ci/node && mv /opt/ci/node.new /opt/ci/node
fi
[ -x /opt/ci/rmt/bin/python ] || python3 -m venv /opt/ci/rmt
/opt/ci/rmt/bin/pip install -q wasmtime=="$WASMTIME_PY"
install -m 755 /tmp/ci-route.sh /opt/ci/route.sh
cat >/opt/ci/env.sh <<ENV
export RUSTUP_HOME=$W/rustup CARGO_HOME=$W/cargo
export PATH=/opt/ci/node/bin:$W/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export CARGO_TERM_COLOR=never
ENV
step "layout"
install -d -o root -g root -m 755 "$CI" "$CI/logs" && install -d -o root -g root -m 700 "$CI/keys"
chown -R root:root "$CI/keys" "$CI/logs" && find "$CI/keys" -type f ! -name '*.pub' -exec chmod 600 {} +
install -d -o "$T" -g "$T" -m 755 "$W" && install -d -o tl-user -g tl-user -m 700 "$CI/in"
# tl-user's warm state from before the wrapper moves to ci-test (a rename, so target/ and the registry stay):
# rustup and cargo homes, the tree and its clone (which becomes ci-test's test.git), the os-app module stamps.
for d in /opt/ci/rustup:rustup /opt/ci/cargo:cargo "$CI/repo.git:test.git" "$CI/tree:tree" "$CI/runs:runs"; do
    [ -e "${d%%:*}" ] && [ ! -e "$W/${d#*:}" ] && mv "${d%%:*}" "$W/${d#*:}"
done
if [ -d /home/tl-user/.cache/temperpaw-os-app-modules ] && [ ! -e "/home/$T/.cache/temperpaw-os-app-modules" ]; then
    install -d -o "$T" -g "$T" "/home/$T/.cache" && mv /home/tl-user/.cache/temperpaw-os-app-modules "/home/$T/.cache/"
fi
rm -rf "$CI/gate.sh" "$CI"/gate-*.sh "$CI"/gate-pub-*.sh "$CI/image.tar" "$CI/image.tar.sha256" "$CI/docker.pid" "$CI/docker.sock"
chown -R "$T:$T" "$W" "/home/$T"
[ ! -d "$CI/repo.git" ] || chown -R root:root "$CI/repo.git"
[ ! -e "$CI/box.lock" ] || chown root:root "$CI/box.lock"
chown -R root:root /opt/ci && chmod -R go-w /opt/ci
# ci-test fetches the commits from root's repo.git, which git otherwise refuses as another user's (safe.directory).
git config --system --get-all safe.directory | grep -qxF "$CI/repo.git" || git config --system --add safe.directory "$CI/repo.git"
step "rust stable (wasm32-unknown-unknown, wasm32-wasip1), as dtolnay/rust-toolchain@stable, as $T"
runuser -u "$T" -- env -i HOME="/home/$T" PATH=/usr/bin:/bin bash -c '
    set -euo pipefail; . /opt/ci/env.sh
    [ ! -d '"$W"'/test.git ] || git -C '"$W"'/test.git worktree repair '"$W"'/tree 2>/dev/null || true
    command -v rustup >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --no-modify-path --profile minimal --default-toolchain none
    rustup toolchain install stable --profile minimal -c clippy -c rustfmt -t wasm32-unknown-unknown -t wasm32-wasip1 >/dev/null
    rustup default stable >/dev/null
    rustc --version'
step "done: node $(/opt/ci/node/bin/node --version), $(docker --version), buildx $(docker buildx version | cut -d" " -f2)," \
    "route.sh $(sha256sum /opt/ci/route.sh | cut -c1-12), sudo for tl-user: $(sudo -l -U tl-user | sed -n '/may run/,$p' | tail -n +2 | xargs)," \
    "$T: $(runuser -u "$T" -- sh -c 'ls /var/lib/ci/keys >/dev/null 2>&1 && echo READS-KEYS || echo no-keys; sudo -n true 2>/dev/null && echo SUDO || echo no-sudo' | xargs)"
