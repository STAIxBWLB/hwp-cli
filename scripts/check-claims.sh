#!/usr/bin/env bash
# Forbidden-claim lint over release-facing copy (docs/release-readiness.md lines 26-27, 40-41).
# The release must not claim Hancom parity, pixel parity, full coverage of every real document
# form, or cross-platform-identical raster bytes. It states the excluded gates and their measured
# distance instead (docs/design/21-pdf-parity.md sections 4.5 and 4.6).
#
# Legitimate occurrences (a negated or quoted sentence) are exempted one exact sentence at a time
# through scripts/claim-allowlist.txt. Never exempt a file, a directory or a pattern.
set -euo pipefail
cd "$(dirname "$0")/.."

allowlist="scripts/claim-allowlist.txt"

# One alternative per line, each naming the checklist sentence it enforces. A single opaque regex
# literal is not reviewable, which is why this is a list.
patterns=(
    # "no 'Hancom parity' claim is made for a profile with exclusions" (readiness line 26)
    'hancom parity'
    '한컴 패리티'
    '한글 패리티'
    # "provide Hancom pixel parity" (readiness line 40)
    'pixel parity'
    '픽셀 패리티'
    '픽셀 단위 동일'
    # "cover every real document form" (readiness line 40)
    'full coverage'
    'complete coverage'
    '전체 커버'
    '완전 커버'
    '모든 문서 형태를 커버'
    # "prove cross-platform-identical raster bytes" (readiness line 41)
    'cross-platform[- ]identical raster'
    'identical raster bytes'
    '플랫폼 간 동일 래스터'
    '크로스 플랫폼 동일 래스터'
)
pattern="$(IFS='|'; printf '%s' "${patterns[*]}")"

# Guard the globs: an unmatched glob must not reach grep as a literal pattern.
shopt -s nullglob
files=(README.md README.ko.md CHANGELOG.md docs/manual/*.md skills/hwp/SKILL*.md)
shopt -u nullglob

existing=()
for f in "${files[@]}"; do
    [ -f "$f" ] && existing+=("$f")
done
if [ "${#existing[@]}" -eq 0 ]; then
    echo "check-claims: no files to scan" >&2
    exit 1
fi

hits="$(mktemp)"
allowed="$(mktemp)"
surviving="$(mktemp)"
trap 'rm -f "$hits" "$allowed" "$surviving"' EXIT

# file:line:text for every hit, so a finding names the exact place to fix.
grep -rniE -- "$pattern" "${existing[@]}" >"$hits" || true

if [ -f "$allowlist" ]; then
    grep -v '^[[:space:]]*#' "$allowlist" | grep -v '^[[:space:]]*$' >"$allowed" || true
fi

# Suppress a hit only when its text contains an allowlist entry as a fixed substring. Matching on
# the text (not on file:line) keeps an allowlisted sentence allowlisted when it moves, and because
# an entry is a whole sentence, the same words in a different sentence still fail.
if [ -s "$allowed" ]; then
    grep -vFf "$allowed" "$hits" >"$surviving" || true
else
    cp "$hits" "$surviving"
fi

if [ -s "$surviving" ]; then
    echo "check-claims: forbidden release claim(s) found:" >&2
    while IFS= read -r line; do
        echo "  $line" >&2
    done <"$surviving"
    cat >&2 <<'MSG'
Release copy must state the excluded gates (fonts, text, raster, roi) and their measured distance
from docs/design/21-pdf-parity.md sections 4.5 and 4.6 instead of claiming parity or coverage.
Fix the sentence. Add an entry to scripts/claim-allowlist.txt only for a negated or quoted
occurrence, and only as the exact sentence.
MSG
    exit 1
fi

echo "check-claims: OK (${#existing[@]} files scanned, no forbidden claim)"
