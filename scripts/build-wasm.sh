#!/usr/bin/env bash
# client-wasm을 WebAssembly로 빌드하고 wasm-bindgen으로 web/src/wasm/에 내보냅니다.
#   bash scripts/build-wasm.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

cargo build -q -p client-wasm --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/client_wasm.wasm \
  --target web --out-dir web/src/wasm
echo "web/src/wasm/ 갱신 ($(du -h web/src/wasm/client_wasm_bg.wasm | cut -f1))"
