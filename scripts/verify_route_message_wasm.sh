#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WASM_PATH="${1:-$ROOT/os-apps/paw-channels/wasm/route_message/route_message.wasm}"

if [ ! -s "$WASM_PATH" ]; then
    echo "route_message wasm verifier: missing artifact at $WASM_PATH" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    HASH="$(sha256sum "$WASM_PATH" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    HASH="$(shasum -a 256 "$WASM_PATH" | awk '{print $1}')"
else
    echo "route_message wasm verifier: need sha256sum or shasum" >&2
    exit 1
fi

if ! command -v strings >/dev/null 2>&1; then
    echo "route_message wasm verifier: need strings" >&2
    exit 1
fi

STRINGS_FILE="$(mktemp)"
trap 'rm -f "$STRINGS_FILE"' EXIT
strings "$WASM_PATH" > "$STRINGS_FILE"

if grep -Fq '$orderby' "$STRINGS_FILE"; then
    echo "route_message wasm verifier: forbidden OData orderby found in $WASM_PATH" >&2
    grep -F '$orderby' "$STRINGS_FILE" >&2 || true
    exit 1
fi

if grep -Fq 'Sequence desc' "$STRINGS_FILE"; then
    echo "route_message wasm verifier: forbidden Sequence desc lookup found in $WASM_PATH" >&2
    grep -F 'Sequence desc' "$STRINGS_FILE" >&2 || true
    exit 1
fi

if ! grep -Fq 'SessionEntries' "$STRINGS_FILE"; then
    echo "route_message wasm verifier: expected SessionEntries lookup strings in $WASM_PATH" >&2
    exit 1
fi

SIZE="$(wc -c < "$WASM_PATH" | tr -d ' ')"
echo "route_message wasm verifier: hash=$HASH size_bytes=$SIZE path=$WASM_PATH"
