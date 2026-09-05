#!/usr/bin/env bash
# Documentation surface gate (docs/release-readiness.md, the tool-count and skill-coverage lines).
#
#   scripts/check-doc-surface.sh              run both checks against the working tree
#   scripts/check-doc-surface.sh --self-test  prove the count pattern still catches every shape
#
# Two independent checks, both reported before the script exits, so one run shows every finding:
#   1. every tool-count claim in README*.md, docs/manual/*.md and skills/hwp/SKILL*.md equals the
#      length of the live `hwp mcp` tools/list response;
#   2. skills/hwp/SKILL.md and SKILL.ko.md name every CLI subcommand and every MCP tool.
#
# The source of truth is the running server, not a hardcoded list. crates/hwp-cli/tests/
# cli_surface.rs asserts the same list from the Rust side.
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-check}"
case "$mode" in
check | --self-test) ;;
*)
    echo "usage: scripts/check-doc-surface.sh [--self-test]" >&2
    exit 2
    ;;
esac

ROOT="$PWD"
HWP_BIN="${HWP_BIN:-$ROOT/target/debug/hwp}"
if [ ! -x "$HWP_BIN" ] && [ -x "$ROOT/target/release/hwp" ]; then
    HWP_BIN="$ROOT/target/release/hwp"
fi
if [ ! -x "$HWP_BIN" ]; then
    echo "[doc-surface] building hwp-cli (no binary at $HWP_BIN)" >&2
    cargo build -p hwp-cli --quiet
    HWP_BIN="$ROOT/target/debug/hwp"
fi

export HWP_BIN
exec python3 - "$mode" <<'PY'
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from glob import glob
from pathlib import Path

mode = sys.argv[1]
hwp_bin = os.environ["HWP_BIN"]
ROOT = os.getcwd()

# --- the live server ---------------------------------------------------------------------------
# The same three-message JSON-RPC handshake crates/hwp-cli/tests/cli_surface.rs uses.
HANDSHAKE = [
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "doc-surface", "version": "0"},
        },
    },
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
]


def live_tools():
    payload = "\n".join(json.dumps(m) for m in HANDSHAKE) + "\n"
    proc = subprocess.run(
        [hwp_bin, "mcp"], input=payload, capture_output=True, text=True, timeout=120
    )
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("id") == 2 and "result" in msg:
            return sorted(t["name"] for t in msg["result"]["tools"])
    sys.stderr.write(
        "doc-surface: no tools/list response from `%s mcp`\n%s\n" % (hwp_bin, proc.stderr)
    )
    raise SystemExit(1)


# --- check one: tool-count claims -------------------------------------------------------------
# One entry per shape the documentation actually uses, each naming a file that uses it. A partial
# pattern is worse than no gate: it reports a pass while the docs still misstate the count.
# Never drop a shape to silence a finding - fix the copy, or allowlist the exact line.
COUNT_SHAPES = [
    # skills/hwp/SKILL.md: "Quick reports 22 tools and stays enabled"
    (r"(?P<n>\d+)\s+tools\b", "number before the English noun"),
    # README.ko.md line 44: "20개 도구를 노출한다", and the 개의 variant
    (r"(?P<n>\d+)\s*개의?\s*도구", "number with the 개 counter before the Korean noun"),
    # skills/hwp/SKILL.ko.md line 312: "같은 22종 도구를"
    (r"(?P<n>\d+)\s*종\s*도구", "number with the 종 counter before the Korean noun"),
    # README.ko.md line 58: "| MCP 서버 (20 도구) |" - counter-less form
    (r"(?P<n>\d+)\s+도구", "bare number before the Korean noun"),
    # README.md line 503: "### Exposed tools (20)"
    (r"tools\s*\(\s*(?P<n>\d+)", "English noun before a parenthesised number"),
    # README.ko.md line 469: "### 노출 도구 (22종)"
    (r"도구\s*\(\s*(?P<n>\d+)", "Korean noun before a parenthesised number"),
    # skills/hwp/SKILL.md lines 315-316 used to wrap "the same twenty / tools" across a break.
    # A spelled-out count is a finding whatever its value: write it as a numeral.
    (
        r"\b(?P<word>nineteen|twenty|twenty-one|twenty-two|twenty-three|twenty-four|thirty)"
        r"[\s-]+tools\b",
        "spelled-out number before the English noun",
    ),
]
COUNT_PATTERNS = [(re.compile(p, re.IGNORECASE), why) for p, why in COUNT_SHAPES]


def count_scan_set(root):
    root = Path(root)
    files = [root / "README.md", root / "README.ko.md"]
    files += sorted(Path(p) for p in glob(str(root / "docs/manual/*.md")))
    files += sorted(Path(p) for p in glob(str(root / "skills/hwp/SKILL*.md")))
    return [f for f in files if f.is_file()]


def read_allowlist(path):
    if not Path(path).is_file():
        return []
    out = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        out.append(stripped)
    return out


def count_findings(root, expected, allowlist):
    """Return [(relative path, line number, matched text, claimed, why)] over a 2-line window."""
    findings = []
    for path in count_scan_set(root):
        lines = path.read_text(encoding="utf-8").splitlines()
        rel = str(path.relative_to(root))
        for i, line in enumerate(lines):
            # A count can wrap across a line break in reflowed prose, so match over the line and
            # its successor joined by a single space.
            window = " ".join(lines[i : i + 2])
            head = len(line)
            seen = set()
            for pattern, why in COUNT_PATTERNS:
                for m in pattern.finditer(window):
                    # Report each match once, at the line where it begins. A match starting on the
                    # next line is picked up by that line's own window.
                    if m.start() > head:
                        continue
                    claimed = m.groupdict().get("n")
                    if claimed is not None and int(claimed) == expected:
                        continue
                    if any(entry in window for entry in allowlist):
                        continue
                    key = (m.start(), m.group(0))
                    if key in seen:
                        continue
                    seen.add(key)
                    findings.append((rel, i + 1, m.group(0), claimed, why))
    return findings


def report_counts(findings, expected):
    for rel, line, text, claimed, why in findings:
        claim = claimed if claimed is not None else "a spelled-out number"
        print(
            "  %s:%d: claims %s, server reports %d (%s) -- %s"
            % (rel, line, claim, expected, why, text.strip()),
            file=sys.stderr,
        )


# --- check two: skill coverage -----------------------------------------------------------------
# Coupling point: the subcommand list is parsed out of the `pub enum Cmd` block of
# crates/hwp-cli/src/cli.rs. If that enum is restructured (renamed, split, or given clap
# `#[command(name = ...)]` overrides), this parse must be revisited rather than the gate relaxed.
def cli_subcommands(root):
    src = (Path(root) / "crates/hwp-cli/src/cli.rs").read_text(encoding="utf-8").splitlines()
    try:
        start = next(i for i, l in enumerate(src) if l.startswith("pub enum Cmd {"))
    except StopIteration:
        sys.stderr.write("doc-surface: `pub enum Cmd {` not found in crates/hwp-cli/src/cli.rs\n")
        raise SystemExit(1)
    end = next(i for i in range(start + 1, len(src)) if src[i] == "}")
    names = []
    for line in src[start + 1 : end]:
        m = re.match(r"    ([A-Z][A-Za-z0-9]*)\s*[\{\(,]", line)
        if m:
            names.append(m.group(1).lower())
    if not names:
        sys.stderr.write("doc-surface: parsed no variants out of `pub enum Cmd`\n")
        raise SystemExit(1)
    return names


def read_exemptions(path):
    exempt = {}
    if not Path(path).is_file():
        return exempt
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        name, _, reason = stripped.partition(" ")
        exempt[name] = reason.strip()
    return exempt


def coverage_findings(root, tools, exempt):
    skill_paths = [Path(root) / "skills/hwp/SKILL.md", Path(root) / "skills/hwp/SKILL.ko.md"]
    texts = {str(p.relative_to(root)): p.read_text(encoding="utf-8") for p in skill_paths}
    findings = []
    for name in cli_subcommands(root):
        if name in exempt:
            continue
        for rel, text in texts.items():
            if "hwp %s" % name not in text:
                findings.append((rel, "CLI subcommand", "hwp %s" % name))
    for tool in tools:
        for rel, text in texts.items():
            if tool not in text:
                findings.append((rel, "MCP tool", tool))
    return findings


# --- self-test ---------------------------------------------------------------------------------
# Seed one stale occurrence of each shape into a temporary copy of the scan set, one shape at a
# time, and require the count check to report exactly that line. A shape removed from
# COUNT_SHAPES makes this fail, so the pattern cannot be narrowed silently.
def seeds(expected):
    stale = expected + 1
    return [
        ("number before the English noun", "README.md", ["The server exposes %d tools." % stale]),
        ("개 counter", "README.md", ["서버는 %d개 도구를 노출한다." % stale]),
        ("개의 counter", "README.md", ["서버는 %d개의 도구를 노출한다." % stale]),
        ("종 counter", "README.md", ["서버는 %d종 도구를 노출한다." % stale]),
        ("counter-less Korean noun", "README.md", ["| MCP 서버 (%d 도구) |" % stale]),
        ("parenthesised English", "README.md", ["### Exposed tools (%d)" % stale]),
        ("parenthesised Korean", "README.md", ["### 노출 도구 (%d종)" % stale]),
        ("spelled-out English", "README.md", ["The server exposes the same twenty tools."]),
        (
            "count wrapped across a line break",
            "README.md",
            ["The server exposes %d" % stale, "tools in total."],
        ),
    ]


def self_test(expected, allowlist):
    ok = True
    scan = count_scan_set(ROOT)
    with tempfile.TemporaryDirectory() as tmp:
        for path in scan:
            dest = Path(tmp) / path.relative_to(ROOT)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, dest)
        clean = count_findings(tmp, expected, allowlist)
        if clean:
            ok = False
            print("  self-test: the unseeded copy already reports findings:", file=sys.stderr)
            report_counts(clean, expected)
        base = (Path(tmp) / "README.md").read_text(encoding="utf-8").splitlines()
        for label, rel, extra in seeds(expected):
            target = Path(tmp) / rel
            target.write_text("\n".join(base + extra) + "\n", encoding="utf-8")
            found = count_findings(tmp, expected, allowlist)
            want_line = len(base) + 1
            hit = [f for f in found if f[0] == rel and f[1] == want_line]
            if len(found) != len(hit) or len(hit) != 1:
                ok = False
                print(
                    "  self-test: shape '%s' expected exactly one finding at %s:%d, got %d"
                    % (label, rel, want_line, len(found)),
                    file=sys.stderr,
                )
                report_counts(found, expected)
            target.write_text("\n".join(base) + "\n", encoding="utf-8")
    return ok


# --- run ---------------------------------------------------------------------------------------
tools = live_tools()
expected = len(tools)
allowlist = read_allowlist(Path(ROOT) / "scripts/doc-count-allowlist.txt")

if mode == "--self-test":
    if not self_test(expected, allowlist):
        print("doc-surface --self-test: FAILED", file=sys.stderr)
        raise SystemExit(1)
    print(
        "doc-surface --self-test: OK (%d shapes seeded and caught, server reports %d tools)"
        % (len(seeds(expected)), expected)
    )
    raise SystemExit(0)

fail = False

counts = count_findings(ROOT, expected, allowlist)
if counts:
    fail = True
    print("doc-surface: stale tool-count claim(s):", file=sys.stderr)
    report_counts(counts, expected)
    print(
        "Fix the count in the copy. A line whose count shape is not a tool count goes in\n"
        "scripts/doc-count-allowlist.txt verbatim; never narrow the pattern.",
        file=sys.stderr,
    )

exempt = read_exemptions(Path(ROOT) / "scripts/skill-coverage-exempt.txt")
gaps = coverage_findings(ROOT, tools, exempt)
if gaps:
    fail = True
    print("doc-surface: bundled skill omits:", file=sys.stderr)
    for rel, kind, name in gaps:
        print("  %s: %s %s is not named" % (rel, kind, name), file=sys.stderr)
    print(
        "Document it in both skills/hwp/SKILL.md and SKILL.ko.md, or add a subcommand to\n"
        "scripts/skill-coverage-exempt.txt with a reason on the same line.",
        file=sys.stderr,
    )

if fail:
    raise SystemExit(1)

print(
    "doc-surface: OK (%d tools live; counts agree in %d files; skill covers every command and tool)"
    % (expected, len(count_scan_set(ROOT)))
)
PY
