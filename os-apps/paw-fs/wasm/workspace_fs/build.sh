#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../../wasm-build-env.sh"

src="$(temperpaw_build_wasm "$SCRIPT_DIR" wasm32-unknown-unknown)"
cp "$src" "$SCRIPT_DIR/workspace_fs.wasm"
cp "$src" "$SCRIPT_DIR/../workspace_fs.wasm"
echo "Built and packaged: $SCRIPT_DIR/workspace_fs.wasm and $SCRIPT_DIR/../workspace_fs.wasm"
