#!/usr/bin/env bash
# Build and package the required WASM modules for paw-foresight.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in seed_world sample_endpoints decompose_endpoint spawn_repairers spawn_adversaries aggregate_costs evidence_ingest register_forecasts render_artifacts consistency_gate grade_hindcast animate_dwellers adjudicate_nodes; do
    echo "Building $module (wasm32-unknown-unknown)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

echo "All paw-foresight WASM modules built and packaged."
