#!/usr/bin/env bash
# Build and package the required WASM modules for paw-media.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in openai_codex_image_generate openrouter_image_generate; do
    echo "Building $module (wasm32-unknown-unknown)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

echo "All paw-media WASM modules built and packaged."
