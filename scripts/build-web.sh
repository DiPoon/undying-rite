#!/usr/bin/env bash
# 브라우저 껍데기를 빌드합니다 (WebAssembly 포함). 결과는 web/dist/이고 서버가 이것을 제공합니다.
#   bash scripts/build-web.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

bash scripts/build-wasm.sh
npm --prefix web run --silent build -- --logLevel warn
echo "web/dist/ 갱신"
