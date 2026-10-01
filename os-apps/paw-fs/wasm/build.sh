#!/usr/bin/env bash
# Build every paw-fs module through its existing packaging entry point.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
for builder in "$SCRIPT_DIR"/*/build.sh; do
    bash "$builder"
done
