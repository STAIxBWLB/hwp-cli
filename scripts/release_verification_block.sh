#!/usr/bin/env bash
# scripts/release_verification_block.sh <version> <readiness-run-url>
# scripts/release_verification_block.sh --self-test
#
# Write the **Verification** block into that version's `## [X.Y.Z]` section of CHANGELOG.md.
# docs/release-readiness.md requires the release copy to state the excluded parity gates and their
# measured distance; scripts/release_notes.sh extracts the version section verbatim as the GitHub
# Release body, so the block has to exist in CHANGELOG.md before the release is cut. No line of the
# block may begin with `## `, which is where release_notes.sh stops.
#
# The block is bounded by the HTML comments `<!-- verification:begin -->` and
# `<!-- verification:end -->`. A rerun replaces exactly that region and nothing else, so anything
# an editor added after the block survives; markers that are missing on one side, duplicated, or
# out of order are refused rather than guessed at, and an unmarked legacy block is refused too.
# (The markers render as nothing in the release body.)
#
# The measurements are quoted from docs/design/21-pdf-parity.md sections 4.5 and 4.6. They are
# never recomputed here, and they come from the private composite run, not the public one-page
# gate, which declares no exclusions at all (section 4.3).
#
# HWP_CHANGELOG overrides the file that is rewritten (the self-test uses it; nothing else should).
set -euo pipefail
cd "$(dirname "$0")/.."

BEGIN_MARKER="<!-- verification:begin -->"
END_MARKER="<!-- verification:end -->"

usage() {
    echo "usage: scripts/release_verification_block.sh <version> <readiness-run-url>" >&2
    echo "       scripts/release_verification_block.sh --self-test" >&2
    echo "  example: scripts/release_verification_block.sh 0.18.0 \\" >&2
    echo "           https://github.com/STAIxBWLB/hwp-cli/actions/runs/1234567890" >&2
}

# --- self-test (no repository side effects: everything happens in a temporary file) -------------
# The regression this pins: a rerun must replace the bounded region and leave every other line of
# the section alone, including a bullet an editor appended after the block.
if [ "${1:-}" = "--self-test" ]; then
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    cl="$tmpdir/CHANGELOG.md"
    url1="https://github.com/STAIxBWLB/hwp-cli/actions/runs/111"
    url2="https://github.com/STAIxBWLB/hwp-cli/actions/runs/222"
    cat >"$cl" <<'EOF'
# Changelog

## [Unreleased]

**Added**

- something on the next version

## [9.9.9]

**Added**

- a released thing

---

## [9.9.8]

**Added**

- an older thing
EOF
    HWP_CHANGELOG="$cl" bash "$0" 9.9.9 "$url1" >/dev/null
    # An editor appends a bullet after the generated block, inside the same section.
    python3 - "$cl" <<'PY'
import sys
p = sys.argv[1]
lines = open(p, encoding="utf-8").read().splitlines()
at = lines.index("<!-- verification:end -->")
lines[at + 1 : at + 1] = ["", "- appended by hand after the block"]
open(p, "w", encoding="utf-8").write("\n".join(lines) + "\n")
PY
    cp "$cl" "$tmpdir/before"
    HWP_CHANGELOG="$cl" bash "$0" 9.9.9 "$url2" >/dev/null
    fail=0
    check() { # <description> <test result already evaluated by caller>
        if [ "$2" != "0" ]; then
            echo "self-test FAIL: $1" >&2
            fail=1
        fi
    }
    grep -qF -- "- appended by hand after the block" "$cl" && rc=0 || rc=1
    check "the hand-appended bullet survived the rerun" "$rc"
    grep -qF -- "$url2" "$cl" && rc=0 || rc=1
    check "the new run URL is present" "$rc"
    grep -qF -- "$url1" "$cl" && rc=1 || rc=0
    check "the old run URL is gone" "$rc"
    [ "$(grep -cF -- "$BEGIN_MARKER" "$cl")" = 1 ] && rc=0 || rc=1
    check "exactly one begin marker" "$rc"
    [ "$(grep -cF -- "$END_MARKER" "$cl")" = 1 ] && rc=0 || rc=1
    check "exactly one end marker" "$rc"
    # Only lines inside the marked region changed: every differing line must sit between the
    # markers of one of the two files.
    if python3 - "$tmpdir/before" "$cl" <<'PY'
import sys

BEGIN, END = "<!-- verification:begin -->", "<!-- verification:end -->"


def outside(path):
    keep, inside = [], False
    for line in open(path, encoding="utf-8").read().splitlines():
        if line.strip() == BEGIN:
            inside = True
            continue
        if line.strip() == END:
            inside = False
            continue
        if not inside:
            keep.append(line)
    return keep


before, after = outside(sys.argv[1]), outside(sys.argv[2])
if before != after:
    print("self-test FAIL: content outside the markers changed", file=sys.stderr)
    import difflib

    sys.stderr.writelines(difflib.unified_diff(before, after, "before", "after", lineterm="\n"))
    raise SystemExit(1)
PY
    then rc=0; else rc=1; fi
    check "only the marked region changed" "$rc"
    # A malformed marker pair is refused rather than guessed at.
    sed -i.bak "s|$END_MARKER||" "$cl" && rm -f "$cl.bak"
    if HWP_CHANGELOG="$cl" bash "$0" 9.9.9 "$url1" >/dev/null 2>&1; then
        check "an unpaired begin marker is refused" 1
    fi
    if [ "$fail" -ne 0 ]; then
        echo "release_verification_block --self-test: FAILED" >&2
        exit 1
    fi
    echo "release_verification_block --self-test: OK (rerun replaced only the marked region)"
    exit 0
fi

version="${1:-}"
run_url="${2:-}"

[ -n "$version" ] && [ -n "$run_url" ] || {
    echo "오류: 버전과 릴리스 준비 실행 URL이 모두 필요합니다." >&2
    usage
    exit 2
}
version="${version#v}"
changelog="${HWP_CHANGELOG:-CHANGELOG.md}"

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
if ! grep -q "^## \[$version\]" "$changelog"; then
    echo "오류: $changelog에 [$version] 절이 없습니다. 절을 먼저 연 뒤 다시 실행하세요." >&2
    exit 1
fi

# The block as prose, so a reviewer reads what the release will say.
block="$(
    cat <<EOF
$BEGIN_MARKER
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
$END_MARKER
EOF
)"

export HWP_VERSION="$version" HWP_BLOCK="$block" HWP_CHANGELOG_PATH="$changelog"
python3 - <<'PY'
import os
import sys
import tempfile

BEGIN, END = "<!-- verification:begin -->", "<!-- verification:end -->"

version = os.environ["HWP_VERSION"]
block = os.environ["HWP_BLOCK"].splitlines()
path = os.environ["HWP_CHANGELOG_PATH"]

lines = open(path, encoding="utf-8").read().splitlines()
heading = "## [%s]" % version
start = next(i for i, l in enumerate(lines) if l.startswith(heading))
end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))

window = lines[start + 1 : end]
begins = [i for i, l in enumerate(window) if l.strip() == BEGIN]
ends = [i for i, l in enumerate(window) if l.strip() == END]


def refuse(why):
    sys.stderr.write(
        "오류: %s에 [%s] 절의 Verification 마커가 %s. 파일을 수정하지 않았습니다.\n"
        "      블록은 %s 와 %s 사이에만 존재해야 하며, 손상된 마커는 손으로 고칩니다.\n"
        % (path, version, why, BEGIN, END)
    )
    raise SystemExit(1)


if len(begins) > 1 or len(ends) > 1:
    refuse("중복되었습니다")
if len(begins) != len(ends):
    refuse("한쪽만 있습니다")
if begins and begins[0] > ends[0]:
    refuse("순서가 뒤바뀌었습니다")
if not begins and any(l.strip() == "**Verification**" for l in window):
    refuse("없는데 마커 없는 Verification 블록이 있습니다")

if begins:
    # Replace exactly the bounded region; everything before and after it is left untouched, so a
    # bullet an editor appended after the block survives the rerun.
    new_window = window[: begins[0]] + block + window[ends[0] + 1 :]
else:
    # First run: the block becomes the last content of the section, before its trailing separator.
    tail = []
    while window and window[-1].strip() in ("", "---"):
        tail.insert(0, window.pop())
    new_window = window + [""] + block + tail

lines[start + 1 : end] = new_window

directory = os.path.dirname(os.path.abspath(path))
fd, tmp = tempfile.mkstemp(dir=directory, prefix=".CHANGELOG.", suffix=".tmp")
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
