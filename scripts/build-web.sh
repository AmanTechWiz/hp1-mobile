#!/usr/bin/env sh
# Builds the browser version of openhp1-game into a static site directory.
#
# Usage: scripts/build-web.sh [--release] [output directory]
#
# The output contains no original game files; players import their own
# installation in the browser. Serve the directory over HTTPS (or localhost),
# because browsers only expose WebGPU to secure contexts.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=dev
profile_dir=debug
if [ "${1:-}" = "--release" ]; then
    profile=release
    profile_dir=release
    shift
fi
output=${1:-"$root/target/web"}
target_dir=${CARGO_TARGET_DIR:-"$root/target"}

bindgen_version=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' "$root/Cargo.lock")
if ! command -v wasm-bindgen >/dev/null 2>&1 ||
    [ "$(wasm-bindgen --version | cut -d' ' -f2)" != "$bindgen_version" ]; then
    echo "wasm-bindgen $bindgen_version is required:" >&2
    echo "  cargo install wasm-bindgen-cli --version $bindgen_version --locked" >&2
    exit 1
fi

cargo build --manifest-path "$root/Cargo.toml" --target wasm32-unknown-unknown \
    --profile "$profile" -p openhp1-game

mkdir -p "$output"
wasm-bindgen --target web --no-typescript --out-dir "$output" --out-name openhp1 \
    "$target_dir/wasm32-unknown-unknown/$profile_dir/openhp1-game.wasm"
cp "$root/web/index.html" "$root/web/host.js" "$root/web/manifest.webmanifest" "$output/"

if command -v wasm-opt >/dev/null 2>&1 && [ "$profile" = release ]; then
    wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int \
        "$output/openhp1_bg.wasm" -o "$output/openhp1_bg.wasm"
fi

echo "Built $output"
