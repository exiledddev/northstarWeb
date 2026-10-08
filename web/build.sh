#!/usr/bin/env bash
# Build the web app into web/dist, ready for the Worker to serve.
#
#   ./build.sh                 the pinned Northstar from git (Cargo.toml)
#   NORTHSTAR=../../northstar ./build.sh
#                              a local checkout of the desktop repo instead
#
# Needs: rustup target add wasm32-unknown-unknown, and the wasm-bindgen CLI
# at exactly the version Cargo.lock names (the script says which). wasm-opt
# (from binaryen) is used when found, to make the download smaller.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\033[1;31m::\033[0m %s\n' "$*"; }

patch=()
if [ -n "${NORTHSTAR:-}" ]; then
  path="$(cd "$NORTHSTAR" && pwd)"
  patch=(--config "patch.\"https://github.com/exiledddev/northstar\".northstar.path=\"$path\"")
  say "Using Northstar from $path"
fi

say "Compiling for the browser"
cargo build --release "${patch[@]}"

want="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
bindgen="${WASM_BINDGEN:-wasm-bindgen}"
have="$("$bindgen" --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$have" != "$want" ]; then
  say "wasm-bindgen $want is needed (found: ${have:-none}). Install it with:"
  echo "    cargo install wasm-bindgen-cli --version $want"
  echo "or point WASM_BINDGEN at a wasm-bindgen $want binary."
  exit 1
fi

say "Binding"
# emptied in place rather than deleted, so a running `wrangler dev` keeps
# serving it
mkdir -p dist
find dist -mindepth 1 -delete
"$bindgen" --target web --no-typescript --out-dir dist --out-name northstar_web \
  target/wasm32-unknown-unknown/release/northstar_web.wasm

opt="${WASM_OPT:-wasm-opt}"
if command -v "$opt" >/dev/null 2>&1; then
  say "Shrinking"
  "$opt" -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-reference-types \
    --enable-sign-ext --enable-mutable-globals \
    dist/northstar_web_bg.wasm -o dist/northstar_web_bg.wasm
fi

cp static/* dist/
size="$(du -h dist/northstar_web_bg.wasm | cut -f1)"
gz="$(gzip -9 -c dist/northstar_web_bg.wasm | wc -c | awk '{printf "%.1f MB", $1/1048576}')"
say "Built web/dist — the app is $size ($gz compressed)"
