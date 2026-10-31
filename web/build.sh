#!/usr/bin/env bash
# Build the web app into web/dist, ready for the Worker to serve.
#
#   ./build.sh                 the pinned Blackline from git (Cargo.toml)
#   BLACKLINE=../../northstar ./build.sh
#                              a local checkout of the desktop repo instead
#
# Needs: rustup target add wasm32-unknown-unknown, and the wasm-bindgen CLI
# at exactly the version Cargo.lock names (the script says which).
# SHRINK=1 also runs wasm-opt (binaryen) for a smaller download; test the
# result in Firefox before deploying it.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\033[1;31m::\033[0m %s\n' "$*"; }

patch=()
# NORTHSTAR is the name it had before; it still works
local_app="${BLACKLINE:-${NORTHSTAR:-}}"
if [ -n "$local_app" ]; then
  path="$(cd "$local_app" && pwd)"
  patch=(--config "patch.\"https://github.com/exiledddev/northstar\".blackline.path=\"$path\"")
  say "Using Blackline from $path"
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
"$bindgen" --target web --no-typescript --out-dir dist --out-name blackline_web \
  target/wasm32-unknown-unknown/release/blackline_web.wasm

# Shrinking with wasm-opt is off unless asked for (SHRINK=1): a shrink by the
# binaryen some systems ship has stopped the app starting in Firefox
# ("failed to grow table"), and the unshrunk app is the same app, only a
# larger download.
opt="${WASM_OPT:-wasm-opt}"
if [ "${SHRINK:-0}" = "1" ] && command -v "$opt" >/dev/null 2>&1; then
  say "Shrinking with $("$opt" --version 2>/dev/null | head -1)"
  "$opt" -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-reference-types \
    --enable-sign-ext --enable-mutable-globals \
    dist/blackline_web_bg.wasm -o dist/blackline_web_bg.wasm
fi

cp static/* dist/
size="$(du -h dist/blackline_web_bg.wasm | cut -f1)"
gz="$(gzip -9 -c dist/blackline_web_bg.wasm | wc -c | awk '{printf "%.1f MB", $1/1048576}')"
say "Built web/dist — the app is $size ($gz compressed)"
