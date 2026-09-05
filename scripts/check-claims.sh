#!/usr/bin/env bash
# Forbidden-claim lint over release-facing copy (docs/release-readiness.md lines 26-27, 40-41).
#
#   scripts/check-claims.sh              scan the working tree
#   scripts/check-claims.sh --self-test  prove the pattern, the negation rule and the allowlist
#
# The release must not claim Hancom parity, pixel parity, full coverage of every real document
# form, or cross-platform-identical raster bytes. It states the excluded gates and their measured
# distance instead (docs/design/21-pdf-parity.md sections 4.5 and 4.6).
#
# A sentence that negates the claim (the negation word comes before the phrase) is not a finding.
# Anything else is exempted one exact line at a time through scripts/claim-allowlist.txt, whose
# entries are `<relative path><TAB><exact line>`: appending a new claim to an allowlisted line
# changes the line, so the exemption stops applying. Never exempt a file, a directory or a
# pattern.
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-check}"
case "$mode" in
check | --self-test) ;;
*)
    echo "usage: scripts/check-claims.sh [--self-test]" >&2
    exit 2
    ;;
esac

exec python3 - "$mode" <<'PY'
import re
import sys
import tempfile
from glob import glob
from pathlib import Path

mode = sys.argv[1]
ROOT = Path.cwd()
ALLOWLIST = "scripts/claim-allowlist.txt"

# One alternative per line, each naming the checklist sentence it enforces, both word orders and
# the documented equivalents. A single opaque regex literal is not reviewable, which is why this
# is a list. Never drop a shape to silence a finding: fix the copy.
CLAIM_SHAPES = [
    # readiness line 26: "no 'Hancom parity' claim is made for a profile with exclusions"
    (r"hancom(\s+office)?\s+parity", "Hancom parity"),
    (r"parity\s+with\s+hancom", "parity with Hancom"),
    (r"identical\s+to\s+hancom", "identical to Hancom"),
    (r"한컴(\s*오피스)?\s*(parity|패리티)", "한컴 parity"),
    (r"한글\s*(parity|패리티)", "한글 parity"),
    (r"한컴(과|와)\s*(완전히\s*)?(똑같|동일)", "한컴과 동일"),
    # readiness line 40: "provide Hancom pixel parity"
    (r"pixel[-\s]*(level[-\s]*)?parity", "pixel parity"),
    (r"parity\s+at\s+the\s+pixel\s+level", "parity at the pixel level"),
    (r"픽셀\s*(수준|단위)?\s*패리티", "픽셀 패리티"),
    (r"픽셀\s*(수준|단위)(으로|로)?\s*(똑같|동일)", "픽셀 수준으로 동일"),
    # readiness line 40: "cover every real document form"
    (r"(full|complete|total)\s+coverage", "full coverage"),
    (r"cover(s|ed|ing)?\s+(every|all)\s+(real[-\s]+)?document\s+(form|type|shape)s?",
     "covers every real document form"),
    (r"(완전|전체)\s*(하게\s*)?커버", "완전 커버"),
    (r"모든\s*(실제\s*)?문서\s*(형식|형태|유형)", "모든 실제 문서 형식"),
    # readiness line 41: "prove cross-platform-identical raster bytes"
    (r"cross[-\s]platform[-\s]*identical\s+raster", "cross-platform identical raster"),
    (r"(byte|pixel)[-\s]*identical\s+raster", "byte-identical raster"),
    (r"identical\s+raster\s+bytes", "identical raster bytes"),
    (r"플랫폼\s*(간|간에)\s*(동일한?|같은)\s*(raster|래스터|래스터화)", "플랫폼 간 동일 raster"),
    (r"(raster|래스터)(가|는|도)?\s*플랫폼\s*간\s*(동일|같)", "래스터가 플랫폼 간 동일"),
]
CLAIM_PATTERNS = [(re.compile(p, re.IGNORECASE), name) for p, name in CLAIM_SHAPES]

# A negation counts only when it comes BEFORE the phrase in the same sentence, which is how the
# one legitimate occurrence in the repository reads ("No Hancom parity claim ..."). A negation
# after the phrase is still reported, because "... parity, which we do not claim" is exactly the
# shape a reader quotes out of context.
NEGATIONS = re.compile(r"\b(no|not|never)\b|없|않|아니", re.IGNORECASE)
SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+")


def scan_set(root):
    root = Path(root)
    files = [root / "README.md", root / "README.ko.md", root / "CHANGELOG.md"]
    files += sorted(Path(p) for p in glob(str(root / "docs/manual/*.md")))
    files += sorted(Path(p) for p in glob(str(root / "skills/hwp/SKILL*.md")))
    return [f for f in files if f.is_file()]


def read_allowlist(path):
    """{(relative path, exact line)} - an entry exempts one whole line of one file."""
    entries = set()
    path = Path(path)
    if not path.is_file():
        return entries
    for n, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        if "\t" not in raw:
            sys.stderr.write(
                "check-claims: %s line %d is malformed: an entry is "
                "`<relative path><TAB><exact line>`\n" % (path, n)
            )
            raise SystemExit(1)
        rel, _, line = raw.partition("\t")
        entries.add((rel.strip(), line))
    return entries


def findings(root, allowlist):
    """[(relative path, line number, claim name, line text)] for every unexcused claim."""
    out = []
    for path in scan_set(root):
        rel = str(path.relative_to(root))
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for sentence in SENTENCE_SPLIT.split(line):
                for pattern, name in CLAIM_PATTERNS:
                    m = pattern.search(sentence)
                    if not m:
                        continue
                    neg = NEGATIONS.search(sentence)
                    if neg and neg.start() < m.start():
                        continue
                    if (rel, line) in allowlist:
                        continue
                    out.append((rel, n, name, line.strip()))
    return out


def report(found):
    print("check-claims: forbidden release claim(s) found:", file=sys.stderr)
    for rel, n, name, text in found:
        print("  %s:%d: %s -- %s" % (rel, n, name, text), file=sys.stderr)
    print(
        "Release copy must state the excluded gates (fonts, text, raster, roi) and their measured\n"
        "distance from docs/design/21-pdf-parity.md sections 4.5 and 4.6 instead of claiming parity\n"
        "or coverage. Fix the sentence. An entry in scripts/claim-allowlist.txt exempts one exact\n"
        "line of one file and nothing else.",
        file=sys.stderr,
    )


# --- self-test ----------------------------------------------------------------------------------
# Adversarial fixtures: every POSITIVE line must be caught and every NEGATIVE line must pass, in a
# temporary tree with its own allowlist. A shape removed from CLAIM_SHAPES, a negation rule
# widened to "anywhere in the sentence", or an allowlist match loosened from the exact line makes
# this fail, so none of the three can be relaxed unnoticed.
ALLOWED_LINE = "- **No Hancom parity claim** The final verdict is always whether the file opens correctly in Hancom Office."

POSITIVE = [
    "This release provides parity with Hancom Office.",
    "Rendering is identical to Hancom for every page.",
    "한컴과 픽셀 수준으로 동일합니다.",
    "우리는 한컴 패리티를 제공합니다.",
    "The renderer reaches pixel parity on this corpus.",
    "It delivers pixel-level parity with the reference.",
    "We cover every real document form.",
    "The suite covers all document forms in the wild.",
    "The corpus gives full coverage of the format.",
    "실제 문서 형식을 완전 커버합니다.",
    "모든 실제 문서 형식을 지원합니다.",
    "The gate proves cross-platform identical raster output.",
    "It produces byte-identical raster output on every host.",
    "플랫폼 간 동일한 래스터를 보장합니다.",
    # An allowlisted line with a claim appended is a different line, so the exemption lapses.
    ALLOWED_LINE + " It also provides pixel parity.",
]

NEGATIVE = [
    "This does not provide pixel parity.",
    "No Hancom parity is claimed for a profile with exclusions.",
    "The public gate never claims full coverage of every real document form.",
    "We do not prove cross-platform identical raster bytes.",
    ALLOWED_LINE,
]


def self_test():
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "scripts").mkdir()
        (tmp / "scripts/claim-allowlist.txt").write_text(
            "# self-test allowlist\nREADME.md\t%s\n" % ALLOWED_LINE, encoding="utf-8"
        )
        allowlist = read_allowlist(tmp / "scripts/claim-allowlist.txt")
        readme = tmp / "README.md"

        for line in POSITIVE:
            readme.write_text("# Title\n\n%s\n" % line, encoding="utf-8")
            found = findings(tmp, allowlist)
            if len(found) < 1 or any(f[1] != 3 for f in found):
                ok = False
                print(
                    "  self-test: positive case not caught at README.md:3: %s" % line,
                    file=sys.stderr,
                )

        for line in NEGATIVE:
            readme.write_text("# Title\n\n%s\n" % line, encoding="utf-8")
            found = findings(tmp, allowlist)
            if found:
                ok = False
                print("  self-test: negative case reported: %s" % line, file=sys.stderr)
                report(found)
    return ok


# --- run ------------------------------------------------------------------------------------------
if mode == "--self-test":
    if not self_test():
        print("check-claims --self-test: FAILED", file=sys.stderr)
        raise SystemExit(1)
    print(
        "check-claims --self-test: OK (%d claims caught, %d negated or allowlisted lines passed)"
        % (len(POSITIVE), len(NEGATIVE))
    )
    raise SystemExit(0)

files = scan_set(ROOT)
if not files:
    print("check-claims: no files to scan", file=sys.stderr)
    raise SystemExit(1)

found = findings(ROOT, read_allowlist(ROOT / ALLOWLIST))
if found:
    report(found)
    raise SystemExit(1)

print("check-claims: OK (%d files scanned, no forbidden claim)" % len(files))
PY
