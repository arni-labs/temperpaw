#!/usr/bin/env bash
# Build and package the required WASM modules for paw-compute.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

# Temper supplies WASI preview1; unknown-unknown can pull in unsupported
# wasm-bindgen imports (the July 2026 production failure). Verify before copy.
verify_blob() {
    local wasm="$1"
    if command -v wasm-tools >/dev/null 2>&1; then
        local dump; dump="$(wasm-tools print "$wasm" 2>/dev/null)"
        local wasi wbind
        wasi="$(printf '%s' "$dump" | grep -c 'wasi_snapshot_preview1' || true)"
        wbind="$(printf '%s' "$dump" | grep -c 'wbindgen' || true)"
    else
        local wasi wbind
        wasi="$(strings "$wasm" | grep -c 'wasi_snapshot_preview1' || true)"
        wbind="$(strings "$wasm" | grep -c 'wbindgen' || true)"
    fi
    if [ "${wasi:-0}" -lt 1 ] || [ "${wbind:-1}" -ne 0 ]; then
        echo "  !! BAD BLOB: wasi_imports=$wasi wbindgen=$wbind (need wasi>=1, wbindgen==0)" >&2
        exit 1
    fi
    echo "  -> blob ok: wasi_imports=$wasi wbindgen=0"
}

for module in computer_exec computer_exec_start computer_exec_poll computer_copy_start computer_copy_poll computer_terminate computer_sleep computer_wake; do
    echo "Building $module (wasm32-wasip1)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-wasip1)"
    verify_blob "$src"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

echo "All paw-compute WASM modules built and packaged."
