#!/usr/bin/env bash
# 전체 검사. 작업을 마쳤다고 보고하기 전에 반드시 통과시킵니다.
#   bash scripts/check.sh            전체 (화면 없는 시험 60초 포함, 3~4분)
#   bash scripts/check.sh --quick    화면 없는 시험을 10초로 줄임 (작업 중 확인용. 보고에는 전체 결과를 씁니다)
# 항목 하나가 실패해도 끝까지 돌리고, 마지막에 항목별 결과를 요약합니다. 하나라도 실패하면 종료 코드 1.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

SOAK_SECS=60
[ "${1:-}" = "--quick" ] && SOAK_SECS=10
DET_SEED=20261127
DET_TICKS=3000

RESULTS=()
FAILED=0
run() {
  local name="$1"; shift
  printf '\n== %s\n' "$name"
  local start=$SECONDS
  if "$@"; then
    RESULTS+=("[통과] $name ($((SECONDS - start))초)")
  else
    RESULTS+=("[실패] $name ($((SECONDS - start))초)")
    FAILED=1
  fi
}

target_on_linux_fs() {
  [ -L target ] || { echo "target이 심볼릭 링크가 아닙니다. bash scripts/setup.sh 를 실행하세요."; return 1; }
  local real fstype
  real="$(readlink -f target)"
  fstype="$(findmnt -no FSTYPE -T "$real")"
  echo "target -> $real (파일시스템: $fstype)"
  case "$fstype" in
    9p|v9fs|drvfs|fuseblk|ntfs*) echo "빌드 산출물이 Windows 쪽 파일시스템에 쌓이고 있습니다."; return 1 ;;
  esac
  case "$real" in
    /mnt/*) echo "빌드 산출물이 /mnt 아래에 있습니다."; return 1 ;;
  esac
}

clippy_wasm() {
  cargo clippy -q -p shared -p client-core -p client-wasm --target wasm32-unknown-unknown -- -D warnings
}

determinism() {
  local native wasm
  native="$(cargo run -q -p tools -- determinism --seed $DET_SEED --ticks $DET_TICKS)" || return 1
  wasm="$(node scripts/wasm-determinism.mjs $DET_SEED $DET_TICKS)" || return 1
  echo "네이티브:      $native"
  echo "WebAssembly:  $wasm"
  [ "$native" = "$wasm" ] || { echo "결과가 다릅니다."; return 1; }
}

# web/의 TypeScript에는 이동, 충돌, 시야, 판정, 타이머 계산이 없어야 합니다.
# 수학 함수를 부르는 줄이 생기면 규칙이 두 벌이 되기 시작한 신호이므로 막습니다. (wasm-bindgen 출력은 제외)
web_boundary() {
  local hits
  hits="$(rg -n --glob '!wasm/**' -e 'Math\.' -e '\b(atan2|sqrt|hypot|sin|cos|tan)\s*\(' -e 'setInterval|setTimeout' web/src || true)"
  if [ -n "$hits" ]; then
    echo "web/src에 계산이나 타이머가 있습니다. 이 계산은 Rust(client-core)로 옮겨야 합니다:"
    echo "$hits"
    return 1
  fi
  echo "web/src에 Math.*, 삼각함수, 타이머 호출 없음"
}

# 실제 서버 실행 파일을 빈 포트에 띄우고, 브라우저와 같은 WebAssembly 빌드로 두 클라이언트를 접속시킵니다.
browser_path_smoke() {
  cargo build -q -p server || return 1
  local log pid url rc
  log="$(mktemp)"
  target/debug/server --bind 127.0.0.1:0 --static web/dist >"$log" 2>&1 &
  pid=$!
  for _ in $(seq 1 50); do
    url="$(sed -n 's/^서버 시작: \(http:\/\/[^/]*\)\/.*/\1/p' "$log")"
    [ -n "$url" ] && break
    sleep 0.1
  done
  if [ -z "$url" ]; then
    echo "서버가 뜨지 않았습니다:"; cat "$log"; kill "$pid" 2>/dev/null; rm -f "$log"; return 1
  fi
  echo "서버 $url (pid $pid)"
  node scripts/wasm-smoke.mjs "$url"
  rc=$?
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  echo "서버 로그:"; sed 's/^/  /' "$log"
  rm -f "$log"
  return $rc
}

run "빌드 산출물 위치" target_on_linux_fs
run "포맷 (cargo fmt --check)" cargo fmt --all --check
run "clippy 네이티브 (경고를 오류로)" cargo clippy -q --workspace --all-targets -- -D warnings
run "clippy wasm32 (shared, client-core, client-wasm)" clippy_wasm
run "워크스페이스 테스트" cargo test -q --workspace
run "WebAssembly 빌드" bash scripts/build-wasm.sh
run "네이티브와 WebAssembly의 해시 일치" determinism
run "web 타입 검사" npm --prefix web run --silent typecheck
run "web 빌드" npm --prefix web run --silent build -- --logLevel warn
run "web 경계 (계산 없음)" web_boundary
run "브라우저 경로 연기 시험 (서버 실행 파일 + wasm 클라이언트 2개)" browser_path_smoke
run "화면 없는 시험 (테스트 클라이언트 4개, 150±30ms, ${SOAK_SECS}초)" cargo run -q -p tools -- soak --clients 4 --lag-ms 150 --jitter-ms 30 --secs $SOAK_SECS

printf '\n== 요약\n'
printf '%s\n' "${RESULTS[@]}"
exit $FAILED
