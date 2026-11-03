#!/usr/bin/env bash
# Build and package the required WASM modules for paw-agent.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../../wasm-build-env.sh"

for module in context_preparer provider_auth_gate provider_caller provider_response_applier sandbox_provisioner workspace_provisioner context_compactor steering_checker session_link_monitor coding_agent_runner cron_compute_next workspace_restorer agent_reply emit_ots_trajectory session_recoverer request_approval request_plan_review plan_approval_handler plan_review_feedback_handler openai_codex_auth; do
    echo "Building $module (wasm32-unknown-unknown)..."
    src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-unknown-unknown)"
    cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
    echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"
done

module="monty_repl"
echo "Building $module (wasm32-wasip1)..."
src="$(temperpaw_build_wasm "$SCRIPT_DIR/$module" wasm32-wasip1)"
cp "$src" "$SCRIPT_DIR/$module/$module.wasm"
echo "  -> packaged $SCRIPT_DIR/$module/$module.wasm"

echo "All paw-agent WASM modules built and packaged."
