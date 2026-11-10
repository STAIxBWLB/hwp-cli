#!/usr/bin/env bash
# scripts/update_formula.sh X.Y.Z
#
# Update version and sha256 in Formula/ahwp.rb from the release vX.Y.Z assets.
# release.yml's update-formula job calls this on a tag push, and it runs locally too (the release
# assets must already be published; gh CLI authentication required).
#
# The checksum asset name is the legacy `hwp-<tag>-<target>` one the formula's URLs use: 1.x
# publishes `ahwp-` and `hwp-` names from the same bytes (issue #510's bridge), and the legacy
# name exists for every released version, so the formula installs at any moment. 2.0.0 retires
# the legacy assets, and that change moves this name and the formula URLs to `ahwp-` together.
#
# Each sha256 replaces the one after the url line that names its target triple. That stays correct
# when the formula's order changes, and a target it cannot place fails the run rather than leaving
# a silent stale checksum.
#
# 자체 점검:  scripts/update_formula.sh --self-test
set -euo pipefail

REPO_SLUG="STAIxBWLB/hwp-cli"
# formula 가 다루는 타깃(윈도우는 brew 대상 아님).
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu"

semver_ok() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; }

# One line of a checksum file ("<sha>  <file>"); take the sha only.
sha_only() { awk '{print $1; exit}'; }

# Rewrite the formula body (stdin)'s version and each target's sha256 to stdout.
# Arguments: <version> <triple=sha ...>
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

# --- Self-test (no network or git side effects) -------------------------------
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
  # A target the formula does not carry must fail (no silent pass).
  if printf '  version "0.0.1"\n' | patch_formula 1.2.3 missing-triple=x >/dev/null 2>&1; then
    echo "self-test FAIL: 누락 타깃이 통과함"; exit 1
  fi
  echo "self-test OK (version/sha 치환 + 누락 검출)"
  exit 0
fi

# --- Run ---------------------------------------------------------------------
ver="${1:-}"
[ -n "$ver" ] || { echo "usage: scripts/update_formula.sh X.Y.Z" >&2; exit 2; }
semver_ok "$ver" || { echo "오류: 시맨틱 버전이 아닙니다: '$ver'" >&2; exit 1; }

cd "$(git rev-parse --show-toplevel)"
formula="Formula/ahwp.rb"
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
