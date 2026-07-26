#!/usr/bin/env bash
# scripts/update_formula.sh X.Y.Z
#
# 릴리스 vX.Y.Z 의 자산 체크섬으로 Formula/hwp.rb 의 version·sha256 을 갱신한다.
# release.yml 의 update-formula 잡이 태그 푸시 때 호출하며, 로컬에서도 쓸 수 있다
# (릴리스 자산이 이미 올라와 있어야 한다 — gh CLI 인증 필요).
#
# sha256 은 "직전 url 라인의 타깃 트리플"에 매칭해 바꾼다. formula 의 순서가 바뀌어도
# 안전하고, 하나라도 못 바꾸면 실패한다(조용한 오래된 체크섬 방지).
#
# 자체 점검:  scripts/update_formula.sh --self-test
set -euo pipefail

REPO_SLUG="STAIxBWLB/hwp-cli"
# formula 가 다루는 타깃(윈도우는 brew 대상 아님).
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu"

semver_ok() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; }

# 체크섬 파일 1줄("<sha>  <파일명>")에서 sha 만 뽑는다.
sha_only() { awk '{print $1; exit}'; }

# formula 본문(stdin) 의 version 과 각 타깃 sha256 을 치환해 stdout 으로 낸다.
# 인자: <version> <triple=sha ...>
patch_formula() {
  local ver="$1"; shift
  awk -v ver="$ver" -v pairs="$*" '
    BEGIN {
      n = split(pairs, kv, " ")
      for (i = 1; i <= n; i++) { split(kv[i], p, "="); sha[p[1]] = p[2] }
    }
    # version "0.2.0"
    /^[[:space:]]*version "/ && !seen_ver { sub(/"[^"]*"/, "\"" ver "\""); seen_ver = 1; print; next }
    # url ... hwp-v#{version}-<triple>.tar.gz  → 다음 sha256 라인이 이 타깃의 것
    /^[[:space:]]*url "/ {
      cur = ""
      for (t in sha) if (index($0, "-" t ".tar.gz") > 0) cur = t
      print; next
    }
    /^[[:space:]]*sha256 "/ && cur != "" {
      sub(/"[^"]*"/, "\"" sha[cur] "\""); done[cur] = 1; cur = ""; print; next
    }
    { print }
    END {
      for (t in sha) if (!(t in done)) { print "치환 실패: " t > "/dev/stderr"; exit 1 }
      if (!seen_ver) { print "치환 실패: version" > "/dev/stderr"; exit 1 }
    }
  '
}

# --- 자체 점검(네트워크·git 부작용 없음) -------------------------------------
if [ "${1:-}" = "--self-test" ]; then
  out=$(printf '%s\n' \
    '  version "0.0.1"' \
    '      url "https://x/hwp-v#{version}-aarch64-apple-darwin.tar.gz"' \
    '      sha256 "old1"' \
    '      url "https://x/hwp-v#{version}-x86_64-unknown-linux-gnu.tar.gz"' \
    '      sha256 "old2"' \
    | patch_formula 9.9.9 aarch64-apple-darwin=new1 x86_64-unknown-linux-gnu=new2)
  printf '%s' "$out" | grep -q 'version "9.9.9"' || { echo "self-test FAIL: version"; exit 1; }
  printf '%s' "$out" | grep -q 'sha256 "new1"'   || { echo "self-test FAIL: sha1"; exit 1; }
  printf '%s' "$out" | grep -q 'sha256 "new2"'   || { echo "self-test FAIL: sha2"; exit 1; }
  # 대상 타깃이 formula 에 없으면 실패해야 한다(조용한 통과 금지).
  if printf '  version "0.0.1"\n' | patch_formula 1.2.3 missing-triple=x >/dev/null 2>&1; then
    echo "self-test FAIL: 누락 타깃이 통과함"; exit 1
  fi
  echo "self-test OK (version/sha 치환 + 누락 검출)"
  exit 0
fi

# --- 실행 --------------------------------------------------------------------
ver="${1:-}"
[ -n "$ver" ] || { echo "usage: scripts/update_formula.sh X.Y.Z" >&2; exit 2; }
semver_ok "$ver" || { echo "오류: 시맨틱 버전이 아닙니다: '$ver'" >&2; exit 1; }

cd "$(git rev-parse --show-toplevel)"
formula="Formula/hwp.rb"
[ -f "$formula" ] || { echo "오류: $formula 없음" >&2; exit 1; }

pairs=""
for t in $TARGETS; do
  asset="hwp-v$ver-$t.sha256"
  sha="$(gh release download "v$ver" -R "$REPO_SLUG" -p "$asset" -O - 2>/dev/null | sha_only)" || true
  [ -n "$sha" ] || { echo "오류: 체크섬 자산을 받지 못함: $asset" >&2; exit 1; }
  pairs="$pairs $t=$sha"
done

tmp="$(mktemp)"
# shellcheck disable=SC2086 # pairs 는 공백 구분 인자 목록으로 전달해야 한다.
patch_formula "$ver" $pairs < "$formula" > "$tmp"
mv "$tmp" "$formula"
echo "✅ $formula → v$ver 갱신 완료"
