#!/usr/bin/env bash
# Builds lq-wasm into site/pkg for the landing page.
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen at the version in Cargo.lock:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version> --locked
# wasm-opt (from binaryen) shrinks the output if it's on PATH, and is skipped otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."

want=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }' || true)
if [[ "$have" != "$want" ]]; then
  echo "error: need wasm-bindgen $want, found ${have:-none}." >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

cargo build --release --locked --target wasm32-unknown-unknown -p lq-wasm
rm -rf site/pkg
wasm-bindgen target/wasm32-unknown-unknown/release/lq_wasm.wasm \
  --target web --no-typescript --out-dir site/pkg

if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals site/pkg/lq_wasm_bg.wasm -o site/pkg/lq_wasm_bg.wasm
else
  echo "note: wasm-opt not found, so the wasm isn't size-optimized." >&2
fi

ls -l site/pkg
