#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT_DIR="$ROOT_DIR/../public/wasm"
mkdir -p "$OUT_DIR"

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup not found; please install Rust toolchain" >&2
  exit 1
fi

rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

cargo build --release --target wasm32-unknown-unknown --manifest-path "$ROOT_DIR/Cargo.toml"

WASM_IN="$ROOT_DIR/target/wasm32-unknown-unknown/release/audio_processor.wasm"
WASM_OUT="$OUT_DIR/audio_processor.wasm"

if ! command -v wasm-opt >/dev/null 2>&1; then
  echo "wasm-opt not found; copying unoptimized wasm" >&2
  cp "$WASM_IN" "$WASM_OUT"
else
  wasm-opt -O3 --enable-simd "$WASM_IN" -o "$WASM_OUT"
fi

echo "Built: $WASM_OUT"


