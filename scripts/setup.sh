#!/usr/bin/env bash
# 개발 환경 준비. 여러 번 실행해도 됩니다.
#   bash scripts/setup.sh
#
# 하는 일
#   1. Cargo의 target을 WSL 홈 아래에 만들고 저장소의 target을 그쪽으로 잇는 심볼릭 링크로 둡니다.
#      /mnt/e 같은 Windows 드라이브는 파일 쓰기가 매우 느려서 산출물을 거기에 쌓으면 빌드가 멈춘 것처럼 됩니다.
#   2. rust-toolchain.toml의 툴체인, 타깃, 구성요소를 설치합니다.
#   3. Cargo.lock의 wasm-bindgen과 같은 버전의 wasm-bindgen-cli를 설치합니다.
#   4. web/의 npm 패키지를 설치합니다.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_HOME="${UR_TARGET_DIR:-$HOME/.cache/undying-rite/target}"
cd "$ROOT"

step() { printf '\n== %s\n' "$*"; }

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
command -v rustup >/dev/null || { echo "rustup이 없습니다. https://rustup.rs 로 설치한 뒤 다시 실행하세요." >&2; exit 1; }

step "target 심볼릭 링크"
mkdir -p "$TARGET_HOME"
if [ -L target ]; then
  ln -sfn "$TARGET_HOME" target
elif [ -e target ]; then
  echo "저장소에 실제 target 디렉터리가 있습니다. 지우고 다시 실행하세요: rm -rf \"$ROOT/target\"" >&2
  exit 1
else
  ln -s "$TARGET_HOME" target
fi
REAL="$(readlink -f target)"
FSTYPE="$(findmnt -no FSTYPE -T "$REAL")"
echo "target -> $REAL ($FSTYPE)"
case "$FSTYPE" in
  9p|v9fs|drvfs|fuseblk|ntfs*) echo "target이 Windows 쪽 파일시스템($FSTYPE)에 있습니다. UR_TARGET_DIR을 리눅스 쪽 경로로 지정하세요." >&2; exit 1 ;;
esac

step "Rust 툴체인 (rust-toolchain.toml)"
rustup show active-toolchain || rustup toolchain install
rustup show | sed -n '/installed targets/,$p'

step "wasm-bindgen-cli"
WB_VERSION="$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/version = |"/, ""); print; exit}' Cargo.lock)"
if [ -z "$WB_VERSION" ]; then
  echo "Cargo.lock에서 wasm-bindgen 버전을 찾지 못했습니다." >&2; exit 1
fi
if command -v wasm-bindgen >/dev/null && [ "$(wasm-bindgen --version | awk '{print $2}')" = "$WB_VERSION" ]; then
  echo "wasm-bindgen $WB_VERSION 이미 설치됨"
else
  echo "wasm-bindgen-cli $WB_VERSION 설치 (처음에는 몇 분 걸립니다)"
  # cargo install의 임시 빌드 디렉터리도 리눅스 쪽에 둡니다.
  CARGO_TARGET_DIR="$HOME/.cache/undying-rite/cargo-install" cargo install wasm-bindgen-cli --version "$WB_VERSION" --locked
  rm -rf "$HOME/.cache/undying-rite/cargo-install"
fi

step "web/ npm 패키지 (처음에는 몇 분 걸립니다)"
(cd web && npm ci --no-audit --no-fund)

step "완료"
echo "전체 검사: bash scripts/check.sh"
