#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/repo/os-apps"
cp "$ROOT/os-apps/wasm-build-env.sh" "$TMP/repo/os-apps/"
while IFS= read -r file; do
    relative="${file#"$ROOT/"}"
    mkdir -p "$TMP/repo/$(dirname "$relative")"
    if [[ "$file" == */build.sh ]]; then cp "$file" "$TMP/repo/$relative"; fi
done < <(find "$ROOT/os-apps" \( -name Cargo.toml -o -name build.sh \) -type f)
cat > "$TMP/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
target_dir="${CARGO_TARGET_DIR:-target}"
target=""
while (($#)); do
    case "$1" in
        --target-dir) target_dir="$2"; shift;;
        --target) target="$2"; shift;;
    esac
    shift
done
module="${PWD##*/}"
case "$target_dir" in
    /*) ;;
    *) target_dir="$PWD/$target_dir";;
esac
artifact="$target_dir/$target/release/${module//-/_}.wasm"
if [[ "${TEST_CARGO_MODE:-}" == fail ]]; then exit 17; fi
if [[ "${TEST_CARGO_MODE:-}" != missing ]]; then
    mkdir -p "$(dirname "$artifact")"
    printf 'fresh %s wasi_snapshot_preview1\n' "$module" > "$artifact"
fi
printf '%s\t%s\n' "$artifact" "$PWD/$module.wasm" >> "$TEST_ARTIFACT_LOG"
if [[ "$PWD" == */paw-fs/wasm/* ]]; then
    printf '%s\t%s\n' "$artifact" "$PWD/../$module.wasm" >> "$TEST_ARTIFACT_LOG"
fi
CARGO
cat > "$TMP/bin/wasm-tools" <<'WASM'
#!/usr/bin/env bash
cat "$2"
WASM
chmod +x "$TMP/bin/cargo" "$TMP/bin/wasm-tools"
export PATH="$TMP/bin:$PATH"
export TEST_ARTIFACT_LOG="$TMP/artifacts"
# Exercise the shell builders migrated to temperpaw_build_wasm. dsf-twin has
# its own generator and explicit Cargo output directory, outside this fixture.
builders=(
    paw-agent/wasm/build.sh
    paw-channels/wasm/build.sh
    paw-compute/wasm/build.sh
    paw-foresight/wasm/build.sh
    paw-fs/wasm/artifact_batch_apply/build.sh
    paw-fs/wasm/blob_adapter/build.sh
    paw-fs/wasm/workspace_fs/build.sh
    paw-ingest/wasm/build.sh
    paw-managed-agents/wasm/build.sh
    paw-media/wasm/build.sh
    paw-patrol/wasm/build.sh
    paw-research/wasm/build.sh
    paw-skills/wasm/build.sh
)
for index in "${!builders[@]}"; do
    builders[$index]="$TMP/repo/os-apps/${builders[$index]}"
    test -f "${builders[$index]}"
done
for mode in default absolute relative missing fail; do
    for builder in "${builders[@]}"; do
        find "$TMP/repo" -name '*.wasm' -delete
        rm -rf "$TMP/shared target"
        while IFS= read -r manifest; do
            relative="${manifest#"$ROOT/"}"
            directory="$TMP/repo/$(dirname "$relative")"
            module="${directory##*/}"
            for target in wasm32-unknown-unknown wasm32-wasip1; do
                mkdir -p "$directory/target/$target/release"
                printf 'stale wasi_snapshot_preview1\n' > "$directory/target/$target/release/${module//-/_}.wasm"
            done
        done < <(find "$ROOT/os-apps" -name Cargo.toml -type f)
        unset CARGO_TARGET_DIR TEST_CARGO_MODE
        case "$mode" in
            absolute|missing|fail) export CARGO_TARGET_DIR="$TMP/shared target";;
            relative) export CARGO_TARGET_DIR="redirected target";;
        esac
        case "$mode" in missing|fail) export TEST_CARGO_MODE="$mode";; esac
        : > "$TEST_ARTIFACT_LOG"
        if bash "$builder" > "$TMP/log" 2>&1; then result=0; else result=$?; fi
        if [[ "$mode" == missing || "$mode" == fail ]]; then
            if [[ "$result" == 0 ]]; then
                echo "FAIL: $builder succeeded with $mode output" >&2
                cat "$TMP/log" >&2
                exit 1
            fi
        else
            if [[ "$result" != 0 ]]; then cat "$TMP/log" >&2; exit 1; fi
            while IFS=$'\t' read -r artifact package; do
                if ! cmp -s "$artifact" "$package"; then
                    echo "FAIL: $builder packaged bytes differ from $artifact: $package ($mode)" >&2
                    exit 1
                fi
            done < "$TEST_ARTIFACT_LOG"
            test -s "$TEST_ARTIFACT_LOG"
        fi
    done
    echo "PASS: $mode (${#builders[@]} builders)"
done

# Exercise the release entry points too: a working builder cannot package a
# required module when the Docker or CI list never invokes it.
unset TEST_CARGO_MODE
export CARGO_TARGET_DIR="$TMP/pipeline target"
python3 - "$ROOT" "$TMP/repo" <<'PYTHON'
import re
import subprocess
import sys
from pathlib import Path

import tomllib

root, fixture = map(Path, sys.argv[1:])
pipelines = {
    "Dockerfile": re.findall(
        r"cd (?:/app/)?(os-apps/[^\s]+) && bash build\.sh",
        (root / "Dockerfile").read_text(),
    ),
    "CI": [
        str(Path(script).parent)
        for script in re.findall(
            r"^\s+(os-apps/[^\s]+/build\.sh)",
            (root / ".github/workflows/ci.yml").read_text(),
            re.MULTILINE,
        )
    ],
}
# dsf-twin's generated modules are outside this shell-builder fixture.
apps = {path.parent.parent for path in root.glob("os-apps/*/wasm/build.sh")}
apps.discard(root / "os-apps/dsf-twin")
apps.add(root / "os-apps/paw-fs")
for pipeline, directories in pipelines.items():
    assert directories, f"{pipeline}: no WASM builders found"
    for artifact in fixture.rglob("*.wasm"):
        artifact.unlink()
    for directory in directories:
        if Path(directory).parts[1] == "dsf-twin":
            continue
        subprocess.run(
            ["bash", "build.sh"],
            cwd=fixture / directory,
            stdout=subprocess.DEVNULL,
            check=True,
        )
    for app in sorted(apps):
        manifest = tomllib.loads((app / "app.toml").read_text())
        packaged = fixture / app.relative_to(root) / "wasm"
        for module in manifest.get("wasm_modules", []):
            if module.get("criticality") != "app-required":
                continue
            name = module["name"]
            artifacts = list(packaged.rglob(f"{name}.wasm"))
            assert artifacts and all(
                path.read_bytes().startswith(b"fresh ") for path in artifacts
            ), f"{pipeline}: required module {app.name}/{name} was not packaged"
    print(f"PASS: {pipeline} packages required modules")
PYTHON
