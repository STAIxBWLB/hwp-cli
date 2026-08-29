#!/bin/sh
# hwp 설치 스크립트 (Homebrew 없이 — macOS·Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --tag v0.2.0 --dir ~/bin
#
# GitHub 릴리스의 사전 빌드 아카이브를 받아(sha256 대조) 설치 디렉터리에 `hwp`를 놓는다.
# Rust 툴체인이 필요 없다. 설치 후에는 `hwp update`로 자체 갱신된다(같은 자산·같은 규칙).
# Windows는 릴리스 페이지의 .zip을 받아 PATH에 두면 된다(이 스크립트는 POSIX 셸 전용).
#
# 환경변수: HWP_INSTALL_DIR(설치 위치, 기본 ~/.local/bin), HWP_TAG(설치 버전)
set -eu

REPO="STAIxBWLB/hwp-cli"
BIN="hwp"
DIR="${HWP_INSTALL_DIR:-$HOME/.local/bin}"
TAG="${HWP_TAG:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    -t|--tag) TAG="${2:-}"; shift 2 ;;
    -d|--dir) DIR="${2:-}"; shift 2 ;;
    -h|--help)
      echo "사용법: install.sh [--tag vX.Y.Z] [--dir <설치경로>]"
      exit 0 ;;
    *) echo "알 수 없는 인자: $1" >&2; exit 2 ;;
  esac
done

die() { echo "오류: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 이(가) 필요합니다"; }

need curl
need tar

# 타깃 트리플 — release.yml 의 upload-assets 매트릭스와 대칭(하나라도 어긋나면 404).
os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)  target="aarch64-apple-darwin" ;;
  Darwin/x86_64) target="x86_64-apple-darwin" ;;
  Linux/x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64) target="aarch64-unknown-linux-gnu" ;;
  *) die "사전 빌드 바이너리가 없는 플랫폼입니다: $os/$arch
  소스에서 설치하세요: cargo install --git https://github.com/$REPO hwp-cli" ;;
esac

# 버전 결정: --tag 없으면 최신 릴리스로 리다이렉트되는 URL에서 태그를 뽑는다
# (GitHub API 미사용 — 비인증 호출 한도에 걸리지 않게).
if [ -z "$TAG" ]; then
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/$REPO/releases/latest")" \
    || die "최신 릴리스를 조회하지 못했습니다"
  TAG="${latest_url##*/}"
fi
case "$TAG" in
  v[0-9]*.[0-9]*.[0-9]*) : ;;
  *) die "릴리스 태그 형식이 아닙니다: '$TAG' (예: v0.3.0)" ;;
esac

asset="$BIN-$TAG-$target.tar.gz"
base="https://github.com/$REPO/releases/download/$TAG"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "hwp $TAG ($target) 내려받는 중..."
curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset" "$base/$asset" \
  || die "내려받기 실패: $base/$asset"
curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset.sha256" "$base/$BIN-$TAG-$target.sha256" \
  || die "체크섬 자산을 받지 못했습니다"

# 체크섬 대조 — 전송 손상·잘린 파일을 여기서 걸러낸다.
want="$(awk '{print $1; exit}' "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  got="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  die "sha256sum 또는 shasum 이(가) 필요합니다"
fi
[ "$want" = "$got" ] || die "체크섬 불일치 (기대 $want / 실제 $got)"

tar -xf "$tmp/$asset" -C "$tmp" || die "압축 해제 실패"
[ -f "$tmp/$BIN" ] || die "아카이브에 $BIN 실행 파일이 없습니다"

mkdir -p "$DIR" || die "설치 디렉터리를 만들지 못했습니다: $DIR"
# 실행 중인 바이너리를 덮어쓸 수 있게 임시 파일 → mv 로 교체한다(같은 파일시스템).
install_tmp="$DIR/.$BIN.install.$$"
cp "$tmp/$BIN" "$install_tmp" || die "설치 실패(권한 확인): $DIR"
chmod 755 "$install_tmp"
mv -f "$install_tmp" "$DIR/$BIN" || { rm -f "$install_tmp"; die "설치 실패: $DIR/$BIN"; }

echo "설치 완료: $DIR/$BIN ($("$DIR/$BIN" --version))"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo
     echo "PATH에 없습니다. 셸 설정에 추가하세요:"
     echo "  export PATH=\"$DIR:\$PATH\"" ;;
esac
echo "이후 업데이트: hwp update"
