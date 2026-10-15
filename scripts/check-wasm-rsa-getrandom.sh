#!/usr/bin/env bash
# rsa 0.9 pulls getrandom 0.2, which compile_error!s on wasm32-unknown-unknown
# unless features = ["custom"]. The "js" feature is forbidden (wasm-bindgen).
# Do not put getrandom on wasm-helpers (ARN-443). Each rsa crate must declare
# the custom backend itself, under the wasm32-unknown-unknown target table.
# PR checks run this so a new rsa crate cannot skip the stub the way
# chain_github_ready did on #500/#501.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

while IFS= read -r toml; do
    if awk '
        /^\[/ {
            in_deps = ($0 ~ /^\[dependencies\]/ || $0 ~ /^\[target\.[^]]+\.dependencies\]/)
        }
        in_deps && $0 ~ /^rsa[[:space:]]*=/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$toml"; then
        if ! awk '
            /^\[/ {
                in_wasm = ($0 ~ /^\[target\.wasm32-unknown-unknown\.dependencies\]/)
            }
            in_wasm && $0 ~ /^getrandom[[:space:]]*=/ {
                has_getrandom = 1
                if ($0 ~ /features = \["custom"\]/) has_custom = 1
                if ($0 ~ /"js"/) has_js = 1
            }
            END {
                if (has_js) exit 1
                exit (has_getrandom && has_custom) ? 0 : 1
            }
        ' "$toml"; then
            echo "FAIL: $toml depends on rsa but does not enable getrandom 0.2 custom (and only custom) under [target.wasm32-unknown-unknown.dependencies]" >&2
            fail=1
        fi
    fi
done < <(find "$root/os-apps" -name Cargo.toml -print | sort)

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "ok: every os-app rsa crate enables getrandom custom on wasm32-unknown-unknown"
