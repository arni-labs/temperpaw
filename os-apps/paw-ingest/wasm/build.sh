#!/usr/bin/env bash
# Build and package the required WASM modules for paw-ingest.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in validate_webhook route_webhook process_webhook; do
    echo "Building $module (wasm32-unknown-unknown)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

echo "All paw-ingest WASM modules built and packaged."
