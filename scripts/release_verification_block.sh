#!/usr/bin/env bash
# scripts/release_verification_block.sh <version> <readiness-run-url>
#
# Write the **Verification** block into that version's `## [X.Y.Z]` section of CHANGELOG.md.
# docs/release-readiness.md requires the release copy to state the excluded parity gates and their
# measured distance; scripts/release_notes.sh extracts the version section verbatim as the GitHub
# Release body, so the block has to exist in CHANGELOG.md before the release is cut. No line of the
# block may begin with `## `, which is where release_notes.sh stops.
#
# The measurements are quoted from docs/design/21-pdf-parity.md sections 4.5 and 4.6. They are
# never recomputed here, and they come from the private composite run, not the public one-page
# gate, which declares no exclusions at all (section 4.3).
#
# Running it twice for the same version replaces the block rather than appending a second one.
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:-}"
run_url="${2:-}"
usage() {
    echo "usage: scripts/release_verification_block.sh <version> <readiness-run-url>" >&2
    echo "  example: scripts/release_verification_block.sh 0.18.0 \\" >&2
    echo "           https://github.com/STAIxBWLB/hwp-cli/actions/runs/1234567890" >&2
}

[ -n "$version" ] && [ -n "$run_url" ] || {
    echo "오류: 버전과 릴리스 준비 실행 URL이 모두 필요합니다." >&2
    usage
    exit 2
}
version="${version#v}"

if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
    echo "오류: 시맨틱 버전 형식이 아닙니다: '$version'" >&2
    exit 1
fi
if ! printf '%s' "$run_url" |
    grep -Eq '^https://github\.com/STAIxBWLB/hwp-cli/actions/runs/[0-9]+(/[A-Za-z0-9/_-]+)?$'; then
    echo "오류: 이 저장소의 actions run URL이 아닙니다: '$run_url'" >&2
    echo "      예: https://github.com/STAIxBWLB/hwp-cli/actions/runs/1234567890" >&2
    exit 1
fi
if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "오류: CHANGELOG.md에 [$version] 절이 없습니다. 절을 먼저 연 뒤 다시 실행하세요." >&2
    exit 1
fi

# The block as prose, so a reviewer reads what the release will say.
block="$(
    cat <<EOF
**Verification**

- Release-readiness run: $run_url
- The private PDF-parity profile excludes four gates. The public one-page gate declares none
  (docs/design/21-pdf-parity.md section 4.3), so these exclusions describe the private profile
  only.
- \`fonts\`: the document declares its dominant body face through a \`substFont\`, and the oracle
  host did not have that face either, so our substitutions resolve to the same faces the oracle
  embedded. The gate's \`substitution_free\` criterion is unreachable for this case by
  construction (section 4.5).
- \`text\`: 1/13 pages byte-equal; the differences are pagination, not characters. Page 4's only
  diff is one line that crosses a page boundary (section 4.6).
- \`raster\`: \`bad_pixel_pct\` 0.1418-0.2332, MAE 18.5-27.9, ink ratio 0.854-1.435, max abs dx/dy
  40px (section 4.6).
- \`roi\`: 3 of 4 pass; only the page-2 diagram region fails (precision 0.848, recall 0.892)
  (section 4.6).
- Provenance: these distances were measured by the private composite run of 2026-08-16, not by
  the public one-page gate.
EOF
)"

export HWP_VERSION="$version" HWP_BLOCK="$block"
python3 - <<'PY'
import os
import sys
import tempfile

version = os.environ["HWP_VERSION"]
block = os.environ["HWP_BLOCK"].splitlines()
path = "CHANGELOG.md"

lines = open(path, encoding="utf-8").read().splitlines()
heading = "## [%s]" % version
start = next(i for i, l in enumerate(lines) if l.startswith(heading))
end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))

window = lines[start + 1 : end]

# Drop an existing block: the bold label and everything up to the next bold label, a separator,
# or the end of the section. This is what makes a repeated run replace rather than append.
if "**Verification**" in window:
    at = window.index("**Verification**")
    stop = at + 1
    while stop < len(window):
        s = window[stop].strip()
        if s == "---" or (s.startswith("**") and s.endswith("**")):
            break
        stop += 1
    window = window[:at] + window[stop:]

# Keep the section's trailing separator and blank lines after the block, so the block is the last
# content of the section and the file's shape is unchanged.
tail = []
while window and window[-1].strip() in ("", "---"):
    tail.insert(0, window.pop())

new_window = window + [""] + block + tail
lines[start + 1 : end] = new_window

fd, tmp = tempfile.mkstemp(dir=".", prefix=".CHANGELOG.", suffix=".tmp")
try:
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    os.replace(tmp, path)
except BaseException:
    if os.path.exists(tmp):
        os.unlink(tmp)
    raise

bullets = sum(1 for l in block if l.startswith("- "))
print("release_verification_block: %s, %d bullets written" % (version, bullets), file=sys.stdout)
PY
