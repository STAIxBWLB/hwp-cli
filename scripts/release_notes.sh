#!/usr/bin/env bash
# CHANGELOG.md에서 한 버전 절만 뽑아 릴리스 본문(한/영 병기)으로 출력한다.
#   scripts/release_notes.sh 0.4.1      또는      scripts/release_notes.sh v0.4.1
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: release_notes.sh <version>}"
version="${version#v}"

awk -v want="[$version]" '
    /^## / {
        if (found) exit
        # "## [0.4.1]" 의 대괄호 토큰만 비교한다.
        if ($2 == want) { found = 1; next }
    }
    found { print }
' CHANGELOG.md | sed -e '/^---$/d' -e '/./,$!d' | awk '
    { lines[NR] = $0 }
    END { last = NR; while (last > 0 && lines[last] ~ /^[[:space:]]*$/) last--
          for (i = 1; i <= last; i++) print lines[i] }
'

# 절을 못 찾으면 빈 출력이 되므로 호출자가 실패로 다루도록 종료코드를 세운다.
if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "CHANGELOG.md에 [$version] 절이 없습니다" >&2
    exit 1
fi
