#!/usr/bin/env bash
# Build and package the required WASM modules for paw-skills.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

module="skill_installer"
echo "Building $module (wasm32-unknown-unknown)..."
src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"

echo "All paw-skills WASM modules built and packaged."
