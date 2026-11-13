#!/bin/sh
# agent-hwp installer without Homebrew, for macOS/Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --tag v1.6.0 --dir ~/bin
#
# Installs the ahwp release archive after verifying its SHA-256. 2.0 publishes no hwp
# compatibility name or hwp- asset (#510); an `hwp` an earlier installer left in the
# directory is not touched.
# No Rust toolchain is required. Windows users can use the release-page ZIP instead.
# Existing environment variables remain HWP_INSTALL_DIR and HWP_TAG.
set -eu

REPO="STAIxBWLB/agent-hwp"
BIN="ahwp"
DIR="${HWP_INSTALL_DIR:-$HOME/.local/bin}"
TAG="${HWP_TAG:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    -t|--tag) TAG="${2:-}"; shift 2 ;;
    -d|--dir) DIR="${2:-}"; shift 2 ;;
    -h|--help)
      echo "사용법: install.sh [--tag vX.Y.Z] [--dir <설치경로>]"
      exit 0 ;;
    *) echo "알 수 없는 인자: $1. --help로 허용 인자를 확인하세요" >&2; exit 2 ;;
  esac
done

die() { echo "오류: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 이(가) 필요합니다. 해당 도구를 설치한 뒤 다시 실행하세요"; }

need curl
need tar

# Keep the supported triples aligned with release.yml's upload-assets matrix.
os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)  target="aarch64-apple-darwin" ;;
  Darwin/x86_64) target="x86_64-apple-darwin" ;;
  Linux/x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64) target="aarch64-unknown-linux-gnu" ;;
  *) die "사전 빌드 바이너리가 없는 플랫폼입니다: $os/$arch
  소스에서 설치하세요: cargo install --git https://github.com/$REPO agent-hwp" ;;
esac

# Resolve the latest-release redirect without spending an unauthenticated API request.
if [ -z "$TAG" ]; then
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/$REPO/releases/latest")" \
    || die "최신 릴리스를 조회하지 못했습니다. 네트워크 상태를 확인하거나 --tag로 버전을 지정한 뒤 다시 실행하세요"
  TAG="${latest_url##*/}"
fi
case "$TAG" in
  v[0-9]*.[0-9]*.[0-9]*) : ;;
  *) die "릴리스 태그 형식이 아닙니다: '$TAG'. --tag v1.6.0처럼 버전을 지정하세요" ;;
esac

base="https://github.com/$REPO/releases/download/$TAG"
tmp="$(mktemp -d)" || die "임시 작업공간을 만들지 못했습니다. 임시 디렉터리 권한과 여유 공간을 확인한 뒤 다시 실행하세요"
primary_tmp=
cleanup() {
  [ -z "$primary_tmp" ] || rm -f "$primary_tmp"
  rm -rf "$tmp"
}
trap cleanup EXIT

# Every failure stops the install: an HTTP 404 means the release has no asset for this
# platform, and a transport failure or another status says nothing about the release.
asset_base="$BIN-$TAG-$target"
asset="$asset_base.tar.gz"
fetch_archive() {
  if http_status="$(curl -sSL --proto '=https' --tlsv1.2 \
    -o "$tmp/$asset" -w '%{http_code}' "$base/$asset")"; then
    :
  else
    die "내려받기 실패: $base/$asset. 네트워크 상태를 확인한 뒤 다시 실행하세요"
  fi
  case "$http_status" in
    200) return 0 ;;
    404) die "해당 버전의 ahwp 릴리스 자산이 없습니다. --tag와 지원 플랫폼을 확인한 뒤 다시 실행하세요" ;;
    *) die "릴리스 자산 요청이 HTTP $http_status 상태로 실패했습니다. 서버 상태와 릴리스 버전을 확인한 뒤 다시 실행하세요" ;;
  esac
}

echo "ahwp $TAG ($target) 내려받는 중..."
fetch_archive

# Checksum errors must stop here, without using unchecked bytes.
curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset.sha256" "$base/$asset_base.sha256" \
  || die "체크섬 자산을 받지 못했습니다. 네트워크 상태와 릴리스 체크섬 자산을 확인한 뒤 다시 실행하세요"
want="$(awk '{print $1; exit}' "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  got="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  got="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  die "sha256sum 또는 shasum 이(가) 필요합니다. 체크섬 도구를 설치한 뒤 다시 실행하세요"
fi
[ "$want" = "$got" ] || die "체크섬 불일치 (기대 $want / 실제 $got). 설치를 중단했습니다. 릴리스 자산과 네트워크 상태를 확인한 뒤 다시 실행하세요"

tar -xf "$tmp/$asset" -C "$tmp" || die "압축 해제 실패. 릴리스 아카이브를 확인한 뒤 다시 실행하세요"
if [ ! -f "$tmp/$BIN" ] || [ -L "$tmp/$BIN" ]; then
  die "아카이브에 $BIN 일반 실행 파일이 없습니다. 올바른 릴리스 자산을 확인한 뒤 다시 실행하세요"
fi

mkdir -p "$DIR" || die "설치 디렉터리를 만들지 못했습니다: $DIR. 디렉터리 권한과 여유 공간을 확인한 뒤 다시 실행하세요"
primary_tmp="$(mktemp "$DIR/.$BIN.install.XXXXXX")" \
  || die "ahwp 설치 후보를 만들지 못했습니다. 설치 디렉터리 권한과 여유 공간을 확인한 뒤 다시 실행하세요"

cp "$tmp/$BIN" "$primary_tmp" \
  || die "ahwp 설치 후보를 복사하지 못했습니다. 설치 디렉터리 권한과 여유 공간을 확인한 뒤 다시 실행하세요"
chmod 755 "$primary_tmp" \
  || die "ahwp 실행 권한을 설정하지 못했습니다. 설치 디렉터리 권한을 확인한 뒤 다시 실행하세요"

# The final name is replaced by a same-filesystem rename, never an in-place copy.
mv -f "$primary_tmp" "$DIR/$BIN" \
  || die "ahwp 설치 파일을 게시하지 못했습니다: $DIR/$BIN. 설치 디렉터리 권한과 여유 공간을 확인한 뒤 다시 실행하세요"
primary_tmp=

echo "설치 완료: $DIR/$BIN ($("$DIR/$BIN" --version))"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo
     echo "PATH에 없습니다. 셸 설정에 추가하세요:"
     echo "  export PATH=\"$DIR:\$PATH\"" ;;
esac
echo "이후 업데이트: ahwp update"
