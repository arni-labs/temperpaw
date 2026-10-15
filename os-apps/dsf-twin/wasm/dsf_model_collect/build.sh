#!/usr/bin/env bash
set -euo pipefail
module_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Keep WASM-only linker options inside this process; native tests use their own flags.
source "$module_dir/../../../wasm-build-env.sh"
cargo build --manifest-path "$module_dir/Cargo.toml" --target wasm32-unknown-unknown --release "$@"
cp "$module_dir/target/wasm32-unknown-unknown/release/dsf_model_collect.wasm" "$module_dir/dsf_model_collect.wasm"
