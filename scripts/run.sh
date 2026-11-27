#!/usr/bin/env bash
# 브라우저 껍데기를 빌드하고 서버를 띄웁니다. Ctrl+C로 끝냅니다.
#   bash scripts/run.sh                      → http://127.0.0.1:8080/
#   bash scripts/run.sh --lag-ms 150 --jitter-ms 30   (주소에 지연을 지정하지 않은 연결의 기본 지연)
# 나머지 옵션은 서버의 --help를 보세요: bash scripts/run.sh --help
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

if [ "${1:-}" != "--help" ] && [ "${1:-}" != "-h" ]; then
  bash scripts/build-web.sh
fi
cargo build -q -p server
exec target/debug/server "$@"
