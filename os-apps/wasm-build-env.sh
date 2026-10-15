#!/usr/bin/env bash

# Temper WASM guests import host functions supplied by the runtime. Preserve
# those unresolved symbols as imports when building standalone .wasm artifacts.
temperpaw_configure_wasm_linker() {
    local allow_undefined="-C link-arg=--allow-undefined"

    case " ${RUSTFLAGS:-} " in
        *" ${allow_undefined} "*)
            ;;
        *)
            export RUSTFLAGS="${RUSTFLAGS:-}${RUSTFLAGS:+ }${allow_undefined}"
            ;;
    esac
}

temperpaw_configure_wasm_linker

# Keep Cargo's output and the packaged input on the same explicit directory.
# Relative CARGO_TARGET_DIR values are resolved from the module, just like Cargo.
# Without it, builders intentionally use the module-local target directory.
temperpaw_build_wasm() (
    set -euo pipefail
    local module_dir="$1"
    local target="$2"
    shift 2
    cd "$module_dir"
    local module="${PWD##*/}"
    local target_dir="${CARGO_TARGET_DIR:-target}"
    case "$target_dir" in
        /*) ;;
        *) target_dir="$PWD/$target_dir" ;;
    esac

    cargo build --target "$target" --release --target-dir "$target_dir" "$@" >&2 || exit $?
    local artifact="$target_dir/$target/release/${module//-/_}.wasm"
    if [[ ! -f "$artifact" ]]; then
        echo "Missing required WASM output for $module in $target_dir/$target/release" >&2
        exit 1
    fi
    printf '%s\n' "$artifact"
)
