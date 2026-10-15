#!/usr/bin/env bash
# Build and package the required WASM modules for paw-patrol.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in patrol_request_router signal_router worker_run_lifecycle work_cycle_lifecycle finding_lifecycle review_gate_lifecycle repo_sweep_lifecycle daily_brief_lifecycle patrol_schedule_lifecycle patrol_run_lifecycle release_run_lifecycle record_ingest chain_file_ready temper_deploy_lifecycle contract_probe; do
    echo "Building $module (wasm32-unknown-unknown)..."
    if [[ "$module" == patrol_request_router ]]; then
        src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown --bin patrol_request_router)"
    else
        src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    fi
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

echo "All paw-patrol WASM modules built and packaged."
