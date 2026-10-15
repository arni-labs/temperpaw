#!/usr/bin/env bash
set -euo pipefail
module_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$module_dir/../../wasm-build-env.sh"
python3 "$module_dir/../specs/generate.py" --check
python3 "$module_dir/../policies/generate.py" --check
python3 "$module_dir/generate_modules.py" --check --build "$@"
