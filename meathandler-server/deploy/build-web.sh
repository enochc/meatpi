#!/usr/bin/env bash
# Build the Slint dashboard for the browser into meathandler-server/web-dist.
# Needs: rustup target add wasm32-unknown-unknown, and a wasm-bindgen CLI whose
# version matches the wasm-bindgen crate in Cargo.lock.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/meathandler-server/web-dist"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
WASM_BINDGEN="${WASM_BINDGEN:-wasm-bindgen}"

cd "$ROOT"
cargo build -p meatpi-app --lib --target wasm32-unknown-unknown --profile web
# Overwrite in place: the same three files are produced every time.
mkdir -p "$OUT"
"$WASM_BINDGEN" --target web --no-typescript --out-dir "$OUT" \
  "$TARGET_DIR/wasm32-unknown-unknown/web/meatpi_app.wasm"
cp "$ROOT/meathandler-server/web/index.html" "$OUT/"
ls -la "$OUT"
