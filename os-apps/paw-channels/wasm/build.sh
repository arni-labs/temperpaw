#!/usr/bin/env bash
# Build and package the required WASM modules for paw-channels.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in channel_connect send_reply transport_reconcile; do
    echo "Building $module (wasm32-unknown-unknown)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

module="route_message"
echo "Building $module (wasm32-wasip1)..."
src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-wasip1)"
cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"

echo "All paw-channels WASM modules built and packaged."
