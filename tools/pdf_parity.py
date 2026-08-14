#!/usr/bin/env python3
"""Hancom PDF parity batch runner for issue #79.

The runner compares locally rendered PDFs with either the single explicitly
allowed public oracle or private local Hancom oracles using the metrics defined
in docs/design/21-pdf-parity.md section 3. It validates and enforces every
locally checkable manifest pin before measuring any case. Published scoreboards
contain only names, SHA-256 digests, and numeric results.

Use scripts/pdf-parity.sh instead of invoking this module directly. The wrapper
checks external tools and builds the hwp binary and JSON Schema validator.
"""

import argparse
import functools
import glob
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

CONTRACT_MANIFEST = "hwp-pdf-parity-manifest-v1"
CONTRACT_SCORECARD = "hwp-pdf-parity-scorecard-v1"
CONTRACT_SCOREBOARD = "hwp-pdf-parity-scoreboard-v1"
CONTRACT_MANIFEST_V2 = "hwp-pdf-parity-manifest-v2"
CONTRACT_SCORECARD_V2 = "hwp-pdf-parity-scorecard-v2"
CONTRACT_SCOREBOARD_V2 = "hwp-pdf-parity-scoreboard-v2"

# v2 is intentionally platform-neutral.  The manifest pins the Poppler build
# and the input font bytes, but does not require a Windows or Hancom host label.
# Thresholds are part of the contract rather than caller-tunable knobs; this
# prevents a relaxed local profile from producing an apparently eligible case.
V2_GATE_NAMES = (
    "page_count",
    "media_box",
    "text",
    "fonts",
    "render_issues",
    "raster",
    "roi",
    "determinism",
)
V2_THRESHOLDS = {
    "media_box_pt": 0.5,
    "dx_px": 2,
    "dy_px": 2,
    "ink_ratio_min": 0.97,
    "ink_ratio_max": 1.03,
    "bad_pixel_pct_max": 0.05,
    "mae_max": 5.0,
    "roi_match_radius_px": 10,
    "roi_precision_min": 0.95,
    "roi_recall_min": 0.95,
}

# The current CLI reports typed issues on stderr.  A future CLI may write the
# same report as JSON; both paths are normalized into this privacy-safe summary.
ISSUE_LINE_RE = re.compile(
    r"(?:렌더:\s*)?(?P<stage>[a-z_]+)/(?P<severity>info|warning|incomplete|fatal)/"
    r"(?P<code>[a-z0-9_.-]+)\s+count=(?P<count>\d+)"
)
UNSUPPORTED_CODE_RE = re.compile(
    r"(?:unsupported|omitted|placeholder|fallback)", re.IGNORECASE
)

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMAS = REPO_ROOT / "schemas"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PLACEHOLDER_RE = re.compile(r"(?:todo|placeholder)", re.IGNORECASE)
FONT_EXTENSIONS = {".otc", ".otf", ".ttc", ".ttf"}

# A pdffonts row is: name, type, encoding, emb, sub, uni, object ID (two tokens).
FONT_ROW = re.compile(
    r"^(?P<name>.*?)\s+"
    r"(?P<type>CID Font Type 0C?|CID TrueType(?: \((?:OpenType|OT)\))?|CID Type 0C?(?: \(OT\))?"
    r"|TrueType(?: \(OpenType\))?|Type 0|Type 1C?(?: \(OT\))?|Type 3)\s+"
    r"(?P<encoding>\S+)\s+(?P<emb>yes|no)\s+(?P<sub>yes|no)\s+(?P<uni>yes|no)\s+"
    r"\d+\s+\d+\s*$"
)

COVERAGE_RE = re.compile(
    r"(?:글꼴 커버리지|font coverage) matched=(\d+) substituted=(\d+) "
    r"missing=(\d+) subset_fallback=(\d+)"
)


def die(msg: str) -> "SystemExit":
    print(f"[pdf-parity] 오류: {msg}", file=sys.stderr)
    return SystemExit(1)


def run(cmd: list, **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, check=True, **kw)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def poppler_version() -> str:
    # Some pdfinfo builds write their version to stderr.
    proc = subprocess.run(["pdfinfo", "-v"], capture_output=True, text=True)
    if proc.returncode != 0:
        raise die("pdfinfo 버전을 확인할 수 없음")
    out = (proc.stdout or proc.stderr).strip()
    return out.splitlines()[0] if out else "unknown"


def validator_path() -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return REPO_ROOT / f"target/debug/examples/validate_structured_corpus{suffix}"


def validate_documents(pairs: list) -> None:
    """Validate every (schema name, document path) pair using the Rust validator."""
    validator = validator_path()
    if not validator.is_file():
        raise die("JSON 스키마 검증기 없음 (scripts/pdf-parity.sh run 사용 필요)")
    arguments = []
    for schema, document in pairs:
        arguments.extend([str(SCHEMAS / f"{schema}.schema.json"), str(document)])
    proc = subprocess.run(
        [str(validator), *arguments], capture_output=True, text=True
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip().splitlines()
        reason = detail[-1] if detail else "unknown validation failure"
        raise die(f"JSON 스키마 검증 실패: {reason}")


def require_concrete_pin(label: str, value: str) -> None:
    if not value.strip() or PLACEHOLDER_RE.search(value):
        raise die(f"manifest pin 미설정: {label}")


def font_directory() -> Path:
    return Path(os.environ.get("HWP_FONT_DIR", "fonts")).expanduser()


def verify_manifest_pins(manifest: dict) -> str:
    """Verify metadata, Poppler, and font file pins before any case is scored."""
    if manifest.get("contract") == CONTRACT_MANIFEST_V2:
        return verify_manifest_pins_v2(manifest)
    pins = manifest["pins"]
    for key in ("hancom_build", "windows_version", "pdf_settings", "poppler_version"):
        require_concrete_pin(key, pins[key])

    actual_poppler = poppler_version()
    if pins["poppler_version"] != actual_poppler:
        raise die(
            "Poppler 버전 불일치: "
            f"manifest={pins['poppler_version']!r}, actual={actual_poppler!r}"
        )

    directory = font_directory()
    if not directory.is_dir():
        raise die(f"고정 폰트 디렉터리 없음: {directory}")
    available = {
        sha256_file(path)
        for path in directory.rglob("*")
        if path.is_file() and path.suffix.lower() in FONT_EXTENSIONS
    }
    for family, expected in pins["fonts"].items():
        if not SHA256_RE.fullmatch(expected):
            raise die(f"manifest font SHA-256 미설정 또는 형식 오류: {family}")
        if expected not in available:
            raise die(f"고정 폰트 SHA-256 불일치 또는 파일 없음: {family}")
    return actual_poppler


def pdf_pages(pdf: Path) -> int:
    out = run(["pdfinfo", str(pdf)]).stdout
    m = re.search(r"^Pages:\s+(\d+)", out, re.M)
    if not m:
        raise die(f"pdfinfo Pages 파싱 실패: {pdf.name}")
    return int(m.group(1))


def _parse_media_box(output: str, pdf: Path, page: int) -> tuple:
    """Parse one pdfinfo -box MediaBox row without retaining a local path."""
    match = re.search(
        r"^(?:Page\s+\d+\s+)?MediaBox:\s+([-+]?\d+(?:\.\d+)?)\s+([-+]?\d+(?:\.\d+)?)\s+"
        r"([-+]?\d+(?:\.\d+)?)\s+([-+]?\d+(?:\.\d+)?)\s*$",
        output,
        re.MULTILINE,
    )
    if match is None:
        # A few Poppler builds omit -box rows but always print Page size.  The
        # page-size fallback represents the same rectangle at origin (0, 0).
        match = re.search(
            r"^(?:Page\s+\d+\s+)?(?:Page\s+)?size:\s+([-+]?\d+(?:\.\d+)?)\s+x\s+"
            r"([-+]?\d+(?:\.\d+)?)\s+pts",
            output,
            re.MULTILINE,
        )
        if match is not None:
            return (0.0, 0.0, float(match.group(1)), float(match.group(2)))
        raise die(f"pdfinfo MediaBox 파싱 실패 (page {page}, {pdf.name})")
    return tuple(float(match.group(index)) for index in range(1, 5))


def pdf_media_boxes(pdf: Path) -> list:
    """Return each page's MediaBox as (x0, y0, x1, y1) points."""
    pages = pdf_pages(pdf)
    boxes = []
    for page in range(1, pages + 1):
        output = run(["pdfinfo", "-box", "-f", str(page), "-l", str(page), str(pdf)]).stdout
        boxes.append(_parse_media_box(output, pdf, page))
    return boxes


def media_box_delta(ours: tuple, oracle: tuple) -> float:
    """Return the largest absolute coordinate delta between two boxes."""
    if len(ours) != 4 or len(oracle) != 4:
        raise die("MediaBox 좌표 수가 4개가 아님")
    return max(abs(float(left) - float(right)) for left, right in zip(ours, oracle))


def pdf_fonts(pdf: Path) -> list:
    """Return normalized pdffonts rows."""
    out = run(["pdffonts", str(pdf)]).stdout
    fonts = []
    for line in out.splitlines()[2:]:  # Skip the two header rows.
        if not line.strip():
            continue
        m = FONT_ROW.match(line)
        if not m:
            raise die(f"pdffonts 행 파싱 실패: {line!r} ({pdf.name})")
        fonts.append(
            {
                "name": m.group("name"),
                "type": m.group("type"),
                "embedded": m.group("emb") == "yes",
                "subset": m.group("sub") == "yes",
                "unicode": m.group("uni") == "yes",
            }
        )
    return fonts


def privacy_font_rows(fonts: list) -> list:
    """Prevent a malicious embedded font name from publishing a local path."""
    safe = []
    for font in fonts:
        entry = dict(font)
        name = str(entry.get("name", ""))
        if "/" in name or "\\" in name or name.startswith("~"):
            entry["name"] = "font-" + hashlib.sha256(name.encode("utf-8")).hexdigest()
        safe.append(entry)
    return safe


def page_text(pdf: Path, page: int) -> str:
    """Normalize one pdftotext page to visible text order, ignoring spacing."""
    out = run(["pdftotext", "-layout", "-f", str(page), "-l", str(page), str(pdf), "-"]).stdout
    return re.sub(r"\s+", "", out)


def rasterize(pdf: Path, page: int, dpi: int, prefix: Path) -> Path:
    run(["pdftoppm", "-png", "-r", str(dpi), "-f", str(page), "-l", str(page),
         str(pdf), str(prefix)])
    hits = glob.glob(f"{prefix}-*.png")
    if len(hits) != 1:
        raise die(f"pdftoppm 산출물 해석 실패: {prefix}-*.png → {hits}")
    return Path(hits[0])


def raster_diff(hwp_bin: Path, source_name: str, ours_png: Path, ref_png: Path,
                page: int, dpi: int) -> dict:
    out = run([
        str(hwp_bin), "diff", source_name,
        "--ours-png", str(ours_png), "--ref", str(ref_png),
        "--page", str(page), "--dpi", str(dpi), "--format", "json",
    ]).stdout
    rep = json.loads(out)
    if rep.get("contract") != "hwp-diff-report-v1":
        raise die(f"hwp diff 계약 불일치: {rep.get('contract')!r}")
    return {k: rep[k] for k in ("dx", "dy", "ink_ratio", "bad_pixel_pct", "mae")}


def render_ours(
    hwp_bin: Path,
    source: Path,
    out_pdf: Path,
    dpi: int,
    report_path: Path | None = None,
) -> dict:
    """Render a PDF and parse font coverage from optional JSON or stderr."""
    if report_path is not None:
        return render_candidate_once(hwp_bin, source, out_pdf, dpi, report_path)["report"].get(
            "coverage"
        )
    proc = subprocess.run(
        [str(hwp_bin), "render", str(source), "-o", str(out_pdf),
         "--format", "pdf", "--dpi", str(dpi)],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        raise die(f"hwp render 실패: {source.name}\n{proc.stderr.strip()}")
    m = COVERAGE_RE.search(proc.stderr)
    coverage = None
    if m:
        coverage = {
            "matched": int(m.group(1)),
            "substituted": int(m.group(2)),
            "missing": int(m.group(3)),
            "subset_fallback": int(m.group(4)),
        }
        coverage["substitution_free"] = (
            coverage["substituted"] == 0
            and coverage["missing"] == 0
            and coverage["subset_fallback"] == 0
        )
    return coverage


def _coverage_from_mapping(value: object) -> dict | None:
    if not isinstance(value, dict):
        return None
    keys = ("matched", "substituted", "missing", "subset_fallback")
    if not all(key in value for key in keys):
        return None
    try:
        coverage = {key: int(value[key]) for key in keys}
    except (TypeError, ValueError):
        return None
    if any(number < 0 for number in coverage.values()):
        return None
    coverage["substitution_free"] = (
        coverage["substituted"] == 0
        and coverage["missing"] == 0
        and coverage["subset_fallback"] == 0
    )
    return coverage


def _iter_issue_mappings(value: object):
    """Yield only typed issue summaries from a future JSON render report."""
    if isinstance(value, dict):
        for key in ("issues", "info", "render_issues"):
            entries = value.get(key)
            if isinstance(entries, list):
                for entry in entries:
                    if isinstance(entry, dict) and isinstance(entry.get("code"), str):
                        yield entry
        # Reports may be wrapped in {"render": {...}} or {"report": {...}}.
        for key in ("render", "report"):
            nested = value.get(key)
            if isinstance(nested, dict):
                yield from _iter_issue_mappings(nested)
    elif isinstance(value, list):
        for entry in value:
            yield from _iter_issue_mappings(entry)


def normalize_render_report(payload: object) -> dict:
    """Normalize a JSON render report to counts only; never publish its paths/details."""
    if isinstance(payload, dict) and isinstance(payload.get("render"), dict):
        # Keep the wrapper for issue discovery, but use its render object for
        # the top-level status and coverage fields.
        render_payload = payload["render"]
    elif isinstance(payload, dict):
        render_payload = payload
    else:
        render_payload = {}

    coverage = _coverage_from_mapping(render_payload.get("font_coverage"))
    if coverage is None and isinstance(payload, dict):
        coverage = _coverage_from_mapping(payload.get("font_coverage"))

    issue_summaries = []
    for issue in _iter_issue_mappings(payload):
        code = issue.get("code")
        severity = str(issue.get("severity", "warning")).lower()
        try:
            count = int(issue.get("count", 1))
        except (TypeError, ValueError):
            count = 1
        if not isinstance(code, str) or count < 0:
            continue
        issue_summaries.append(
            {
                "code": re.sub(r"[^a-z0-9_.-]", "_", code.lower()),
                "severity": severity if severity in {"info", "warning", "incomplete", "fatal"} else "warning",
                "count": count,
            }
        )

    # Certification-style reports carry per-font outcomes rather than the
    # compact coverage object.  Convert those outcomes without retaining names.
    fonts = render_payload.get("fonts") if isinstance(render_payload, dict) else None
    if coverage is None and isinstance(fonts, list):
        counts = {"matched": 0, "substituted": 0, "missing": 0, "subset_fallback": 0}
        for font in fonts:
            if not isinstance(font, dict):
                continue
            outcome = str(font.get("outcome", "")).lower()
            if outcome == "matched":
                counts["matched"] += 1
            elif outcome in {"substituted", "coverage_substituted"}:
                counts["substituted"] += 1
            elif outcome == "missing":
                counts["missing"] += 1
        if any(counts.values()):
            coverage = _coverage_from_mapping(counts)

    for summary in issue_summaries:
        code = summary["code"]
        if coverage is None and code == "font_substituted":
            # A count is enough to fail the gate even if the future report did
            # not include the aggregate coverage object.
            coverage = {"matched": 0, "substituted": summary["count"], "missing": 0,
                        "subset_fallback": 0, "substitution_free": False}
        elif coverage is None and code == "font_missing":
            coverage = {"matched": 0, "substituted": 0, "missing": summary["count"],
                        "subset_fallback": 0, "substitution_free": False}
        elif coverage is None and code == "font_subset_fallback":
            coverage = {"matched": 0, "substituted": 0, "missing": 0,
                        "subset_fallback": summary["count"], "substitution_free": False}

    # Treat typed font issues as authoritative even when a stale/inconsistent
    # aggregate object claims that the render was substitution-free.
    if coverage is not None:
        for summary in issue_summaries:
            field = {
                "font_substituted": "substituted",
                "font_missing": "missing",
                "font_subset_fallback": "subset_fallback",
            }.get(summary["code"])
            if field is not None:
                coverage[field] = max(coverage[field], summary["count"])
        coverage["substitution_free"] = (
            coverage["substituted"] == 0
            and coverage["missing"] == 0
            and coverage["subset_fallback"] == 0
        )

    unsupported = sum(
        issue["count"]
        for issue in issue_summaries
        if UNSUPPORTED_CODE_RE.search(issue["code"])
    )
    incomplete = sum(issue["count"] for issue in issue_summaries if issue["severity"] == "incomplete")
    fatal = sum(issue["count"] for issue in issue_summaries if issue["severity"] == "fatal")
    for field, target in (
        ("unsupported", "unsupported"),
        ("unsupported_count", "unsupported"),
        ("incomplete", "incomplete"),
        ("incomplete_count", "incomplete"),
        ("fatal", "fatal"),
        ("fatal_count", "fatal"),
    ):
        try:
            declared = int(render_payload.get(field, 0))
        except (AttributeError, TypeError, ValueError):
            declared = 0
        if target == "unsupported":
            unsupported = max(unsupported, declared)
        elif target == "incomplete":
            incomplete = max(incomplete, declared)
        else:
            fatal = max(fatal, declared)
    has_declared_issue_count = isinstance(render_payload, dict) and "issue_count" in render_payload
    try:
        declared_issue_count = int(render_payload.get("issue_count", 0))
    except (AttributeError, TypeError, ValueError):
        declared_issue_count = 0
    observed_issue_count = sum(
        issue["count"] for issue in issue_summaries if issue["severity"] != "info"
    )
    if has_declared_issue_count and declared_issue_count != observed_issue_count:
        # A truncated or internally inconsistent issue channel cannot be
        # treated as a clean render, even if the listed entries are warnings.
        mismatch = abs(declared_issue_count - observed_issue_count)
        fatal += mismatch
        issue_summaries.append({"code": "unclassified_issue", "severity": "fatal", "count": mismatch})
    has_declared_info_count = isinstance(render_payload, dict) and "info_count" in render_payload
    try:
        declared_info_count = int(render_payload.get("info_count", 0))
    except (AttributeError, TypeError, ValueError):
        declared_info_count = 0
    observed_info_count = sum(
        issue["count"] for issue in issue_summaries if issue["severity"] == "info"
    )
    if has_declared_info_count and declared_info_count != observed_info_count:
        mismatch = abs(declared_info_count - observed_info_count)
        fatal += mismatch
        issue_summaries.append({"code": "unclassified_info", "severity": "fatal", "count": mismatch})
    complete = bool(render_payload.get("complete", render_payload.get("issue_log_complete", True)))
    if render_payload.get("status") in {"failed", "partial", "skipped"}:
        complete = False
    return {
        "coverage": coverage,
        "complete": complete,
        "unsupported": unsupported,
        "incomplete": incomplete,
        "fatal": fatal,
        "issue_codes": sorted({issue["code"] for issue in issue_summaries}),
        "report_available": bool(payload),
    }


def parse_render_stderr(stderr: str) -> dict:
    """Parse current CLI stderr fallback into the same report summary as JSON."""
    match = COVERAGE_RE.search(stderr or "")
    coverage = None
    if match:
        coverage = _coverage_from_mapping(
            {
                "matched": match.group(1),
                "substituted": match.group(2),
                "missing": match.group(3),
                "subset_fallback": match.group(4),
            }
        )
    issues = []
    for match in ISSUE_LINE_RE.finditer(stderr or ""):
        issues.append(
            {
                "code": match.group("code").lower(),
                "severity": match.group("severity").lower(),
                "count": int(match.group("count")),
            }
        )
    unsupported = sum(
        issue["count"] for issue in issues if UNSUPPORTED_CODE_RE.search(issue["code"])
    )
    incomplete = sum(issue["count"] for issue in issues if issue["severity"] == "incomplete")
    fatal = sum(issue["count"] for issue in issues if issue["severity"] == "fatal")
    complete = "issue accumulator incomplete" not in (stderr or "").lower()
    return {
        "coverage": coverage,
        "complete": complete,
        "unsupported": unsupported,
        "incomplete": incomplete,
        "fatal": fatal,
        "issue_codes": sorted({issue["code"] for issue in issues}),
        "report_available": bool(stderr),
    }


def read_render_report(path: Path | None) -> dict | None:
    """Read an optional report path, rejecting symlinks and malformed JSON."""
    if path is None:
        return None
    try:
        if path.is_symlink() or not path.is_file():
            return None
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return normalize_render_report(payload)


def merge_render_reports(reports: list[tuple[str, dict]]) -> dict:
    """Merge two render reports while retaining only aggregate, safe fields."""
    reports = [item for item in reports if item[1] is not None]
    if not reports:
        return {
            "coverage": None,
            "complete": True,
            "unsupported": 0,
            "incomplete": 0,
            "fatal": 0,
            "issue_codes": [],
            "report_available": False,
            "report_source": "none",
        }
    coverages = [report["coverage"] for _, report in reports if report.get("coverage") is not None]
    coverage = None
    if coverages:
        coverage = {
            key: max(int(item[key]) for item in coverages)
            for key in ("matched", "substituted", "missing", "subset_fallback")
        }
        coverage["substitution_free"] = (
            coverage["substituted"] == 0
            and coverage["missing"] == 0
            and coverage["subset_fallback"] == 0
        )
    sources = {source for source, _ in reports}
    return {
        "coverage": coverage,
        "complete": all(report.get("complete", False) for _, report in reports),
        "unsupported": max(int(report.get("unsupported", 0)) for _, report in reports),
        "incomplete": max(int(report.get("incomplete", 0)) for _, report in reports),
        "fatal": max(int(report.get("fatal", 0)) for _, report in reports),
        "issue_codes": sorted({code for _, report in reports for code in report.get("issue_codes", [])}),
        "report_available": any(report.get("report_available", False) for _, report in reports),
        "report_source": next(iter(sources)) if len(sources) == 1 else "mixed",
    }


def render_candidate_once(
    hwp_bin: Path,
    source: Path,
    out_pdf: Path,
    dpi: int,
    report_path: Path | None = None,
) -> dict:
    """Render one candidate PDF and consume optional JSON, then stderr fallback."""
    command = [
        str(hwp_bin),
        "render",
        str(source),
        "-o",
        str(out_pdf),
        "--format",
        "pdf",
        "--dpi",
        str(dpi),
    ]
    if report_path is not None:
        # The current CLI accepts --report; older binaries reject no extra
        # flag because report_path is only supplied by v2 callers.  The
        # caller can still omit it and use the stderr fallback unchanged.
        command.extend(["--report", str(report_path)])
    proc = subprocess.run(
        command,
        capture_output=True,
        text=True,
    )
    if (
        proc.returncode != 0
        and report_path is not None
        and re.search(
            r"(?:(?:unknown|unexpected|unrecognized).*?(?:--report|report)|"
            r"(?:--report|report).*?(?:wasn't expected|not expected))",
            proc.stderr,
            re.I,
        )
    ):
        # Keep v2 usable with the pre-report CLI while the report-producing
        # command rolls out.  Its stderr contract remains the fallback source.
        proc = subprocess.run(
            command[:-2],
            capture_output=True,
            text=True,
        )
    if proc.returncode != 0:
        raise die(f"hwp render 실패: {source.name}\n{proc.stderr.strip()}")
    report = read_render_report(report_path)
    fallback = parse_render_stderr(proc.stderr)
    if report is None:
        report = fallback
        source_kind = "stderr" if fallback["report_available"] else "none"
    else:
        # The JSON report is authoritative for typed counts; stderr may still
        # carry the old coverage line while CLI report output is being rolled.
        if report.get("coverage") is None and fallback.get("coverage") is not None:
            report["coverage"] = fallback["coverage"]
        source_kind = "mixed" if fallback.get("report_available") else "json"
    report["report_source"] = source_kind
    return {"report": report, "sha256": sha256_file(out_pdf), "path": out_pdf}


def score_case(name: str, source: Path, oracle: Path, hwp_bin: Path, dpi: int,
               work: Path) -> dict:
    """Collect the five metric groups for one case."""
    coverage = render_ours(hwp_bin, source, work / "ours.pdf", dpi)
    ours_pdf = work / "ours.pdf"

    ours_pages, oracle_pages = pdf_pages(ours_pdf), pdf_pages(oracle)
    delta = ours_pages - oracle_pages

    ours_fonts, oracle_fonts = pdf_fonts(ours_pdf), pdf_fonts(oracle)
    ours_fonts_ok = all(
        font["embedded"] and font["subset"] and font["unicode"]
        for font in ours_fonts
    )
    oracle_fonts_ok = all(
        font["embedded"] and font["subset"] and font["unicode"]
        for font in oracle_fonts
    )
    fonts_ok = ours_fonts_ok and oracle_fonts_ok

    compared = min(ours_pages, oracle_pages)
    equal = 0
    first_diff = None
    for n in range(1, compared + 1):
        if page_text(ours_pdf, n) == page_text(oracle, n):
            equal += 1
        elif first_diff is None:
            first_diff = n

    raster = []
    for n in range(1, compared + 1):
        ours_png = rasterize(ours_pdf, n, dpi, work / f"ours-{n}")
        ref_png = rasterize(oracle, n, dpi, work / f"ref-{n}")
        entry = {"page": n}
        entry.update(raster_diff(hwp_bin, source.name, ours_png, ref_png, n, dpi))
        raster.append(entry)

    substitution_free = coverage is not None and coverage["substitution_free"]
    reasons = []
    if delta != 0:
        reasons.append("page_count_delta")
    if coverage is None:
        reasons.append("font_coverage_unavailable")
    elif not substitution_free:
        reasons.append("font_substitution")
    if not fonts_ok:
        reasons.append("pdf_font_contract")

    return {
        "contract": CONTRACT_SCORECARD,
        "case": name,
        "source": {"name": source.name, "sha256": sha256_file(source)},
        "oracle": {"name": oracle.name, "sha256": sha256_file(oracle)},
        "dpi": dpi,
        "scored": not reasons,
        "unscored_reasons": reasons,
        "pages": {"ours": ours_pages, "oracle": oracle_pages, "delta": delta},
        "fonts": {
            "ours": ours_fonts,
            "oracle": oracle_fonts,
            "ours_all_embedded_subset_unicode": ours_fonts_ok,
            "oracle_all_embedded_subset_unicode": oracle_fonts_ok,
            "all_embedded_subset_unicode": fonts_ok,
        },
        "font_coverage": coverage,
        "substitution_free": substitution_free,
        "text": {
            "pages_compared": compared,
            "pages_equal": equal,
            "first_diff_page": first_diff,
        },
        "raster": raster,
    }


@functools.lru_cache(maxsize=16)
def _png_gray(path: Path) -> tuple[int, int, bytes]:
    """Decode the small PNGs used by Poppler with no third-party dependency."""
    signature = b"\x89PNG\r\n\x1a\n"
    raw = path.read_bytes()
    if not raw.startswith(signature):
        raise ValueError(f"not a PNG: {path.name}")
    offset = len(signature)
    width = height = bit_depth = color_type = None
    interlace = 0
    compressed = bytearray()
    while offset + 12 <= len(raw):
        length = struct.unpack(">I", raw[offset:offset + 4])[0]
        kind = raw[offset + 4:offset + 8]
        data_start = offset + 8
        data_end = data_start + length
        if data_end + 4 > len(raw):
            raise ValueError(f"truncated PNG: {path.name}")
        data = raw[data_start:data_end]
        if kind == b"IHDR":
            if len(data) != 13:
                raise ValueError(f"invalid PNG header: {path.name}")
            width, height, bit_depth, color_type, _, _, interlace = struct.unpack(
                ">IIBBBBB", data
            )
        elif kind == b"IDAT":
            compressed.extend(data)
        elif kind == b"IEND":
            break
        offset = data_end + 4
    if not width or not height or bit_depth != 8 or interlace != 0:
        raise ValueError(f"unsupported PNG encoding: {path.name}")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"unsupported PNG color type: {path.name}")
    row_bytes = width * channels
    decoded = zlib.decompress(bytes(compressed))
    expected = height * (row_bytes + 1)
    if len(decoded) != expected:
        raise ValueError(f"invalid PNG raster length: {path.name}")

    rows = []
    previous = bytearray(row_bytes)
    cursor = 0
    for _ in range(height):
        filter_type = decoded[cursor]
        current = bytearray(decoded[cursor + 1:cursor + 1 + row_bytes])
        cursor += row_bytes + 1
        for index in range(row_bytes):
            left = current[index - channels] if index >= channels else 0
            up = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 1:
                current[index] = (current[index] + left) & 0xFF
            elif filter_type == 2:
                current[index] = (current[index] + up) & 0xFF
            elif filter_type == 3:
                current[index] = (current[index] + ((left + up) // 2)) & 0xFF
            elif filter_type == 4:
                estimate = left + up - upper_left
                pa = abs(estimate - left)
                pb = abs(estimate - up)
                pc = abs(estimate - upper_left)
                predictor = left if pa <= pb and pa <= pc else up if pb <= pc else upper_left
                current[index] = (current[index] + predictor) & 0xFF
            elif filter_type != 0:
                raise ValueError(f"unsupported PNG filter: {filter_type}")
        rows.append(current)
        previous = current

    gray = bytearray(width * height)
    for y, row in enumerate(rows):
        for x in range(width):
            values = row[x * channels:(x + 1) * channels]
            if color_type == 0:
                value = values[0]
            elif color_type == 2:
                value = (299 * values[0] + 587 * values[1] + 114 * values[2]) // 1000
            elif color_type == 4:
                value = values[0]
            else:
                # Composite transparent pixels against white before measuring ink.
                alpha = values[3]
                value = (values[0] * alpha + 255 * (255 - alpha)) // 255
            gray[y * width + x] = value
    return width, height, bytes(gray)


def roi_ink_precision_recall(
    ours_png: Path,
    oracle_png: Path,
    x: float,
    y: float,
    width: float,
    height: float,
    ink_threshold: int = 250,
    match_radius_px: int = 0,
) -> tuple[float, float]:
    """Compare ROI ink with a pinned square-neighborhood matching tolerance.

    Precision counts candidate ink that has oracle ink within ``match_radius_px``;
    recall applies the same test in the opposite direction. This is deliberately
    not a blur or an image resize: missing features still have no nearby match,
    while equivalent vector glyphs rasterized with different hinting and advance
    rounding do not fail on exact-pixel antialiasing differences.
    """
    if not isinstance(match_radius_px, int) or match_radius_px < 0:
        raise ValueError("ROI match radius must be a non-negative integer")
    ours_width, ours_height, ours_pixels = _png_gray(ours_png)
    oracle_width, oracle_height, oracle_pixels = _png_gray(oracle_png)
    if abs(ours_width - oracle_width) > 2 or abs(ours_height - oracle_height) > 2:
        raise ValueError("ROI PNG dimensions differ")
    common_width = min(ours_width, oracle_width)
    common_height = min(ours_height, oracle_height)
    left = max(0, min(common_width, int(x * common_width)))
    top = max(0, min(common_height, int(y * common_height)))
    right = max(left, min(common_width, int((x + width) * common_width + 0.999999)))
    bottom = max(top, min(common_height, int((y + height) * common_height + 0.999999)))
    roi_width = right - left
    roi_height = bottom - top
    ours_ink = bytearray(roi_width * roi_height)
    oracle_ink = bytearray(roi_width * roi_height)
    for row in range(top, bottom):
        ours_start = row * ours_width + left
        ours_end = row * ours_width + right
        oracle_start = row * oracle_width + left
        oracle_end = row * oracle_width + right
        offset = (row - top) * roi_width
        ours_ink[offset:offset + roi_width] = bytes(
            pixel < ink_threshold for pixel in ours_pixels[ours_start:ours_end]
        )
        oracle_ink[offset:offset + roi_width] = bytes(
            pixel < ink_threshold for pixel in oracle_pixels[oracle_start:oracle_end]
        )

    def prefix(mask: bytearray) -> list[int]:
        stride = roi_width + 1
        result = [0] * (stride * (roi_height + 1))
        for row in range(roi_height):
            row_sum = 0
            source_offset = row * roi_width
            current_offset = (row + 1) * stride
            previous_offset = row * stride
            for column in range(roi_width):
                row_sum += mask[source_offset + column]
                result[current_offset + column + 1] = (
                    result[previous_offset + column + 1] + row_sum
                )
        return result

    def nearby_count(mask_prefix: list[int], column: int, row: int) -> int:
        stride = roi_width + 1
        x0 = max(0, column - match_radius_px)
        y0 = max(0, row - match_radius_px)
        x1 = min(roi_width, column + match_radius_px + 1)
        y1 = min(roi_height, row + match_radius_px + 1)
        return (
            mask_prefix[y1 * stride + x1]
            - mask_prefix[y0 * stride + x1]
            - mask_prefix[y1 * stride + x0]
            + mask_prefix[y0 * stride + x0]
        )

    ours_prefix = prefix(ours_ink)
    oracle_prefix = prefix(oracle_ink)
    ours_count = sum(ours_ink)
    reference_count = sum(oracle_ink)
    precision_matches = recall_matches = 0
    for row in range(roi_height):
        offset = row * roi_width
        for column in range(roi_width):
            index = offset + column
            if ours_ink[index] and nearby_count(oracle_prefix, column, row):
                precision_matches += 1
            if oracle_ink[index] and nearby_count(ours_prefix, column, row):
                recall_matches += 1
    precision = precision_matches / ours_count if ours_count else 1.0 if reference_count == 0 else 0.0
    recall = recall_matches / reference_count if reference_count else 1.0 if ours_count == 0 else 0.0
    return precision, recall


def _normalize_roi(roi: dict) -> dict:
    """Accept v2 x/y/width/height and the equivalent point form."""
    if not isinstance(roi, dict):
        raise die("ROI 항목이 객체가 아님")
    result = dict(roi)
    if "top_left" in result or "bottom_right" in result:
        top_left = result.get("top_left")
        bottom_right = result.get("bottom_right")
        if isinstance(top_left, dict):
            top_left = (top_left.get("x"), top_left.get("y"))
        if isinstance(bottom_right, dict):
            bottom_right = (bottom_right.get("x"), bottom_right.get("y"))
        if not (
            isinstance(top_left, (list, tuple))
            and isinstance(bottom_right, (list, tuple))
            and len(top_left) == 2
            and len(bottom_right) == 2
        ):
            raise die("ROI top_left/bottom_right 좌표 형식 오류")
        result["x"], result["y"] = float(top_left[0]), float(top_left[1])
        result["width"] = float(bottom_right[0]) - result["x"]
        result["height"] = float(bottom_right[1]) - result["y"]
    required = ("name", "page", "x", "y", "width", "height")
    if any(key not in result for key in required):
        raise die("ROI 필수 필드 누락")
    try:
        values = {key: float(result[key]) for key in ("x", "y", "width", "height")}
        page = int(result["page"])
    except (TypeError, ValueError):
        raise die("ROI 좌표 형식 오류")
    if (
        page < 1
        or values["x"] < 0
        or values["y"] < 0
        or values["width"] <= 0
        or values["height"] <= 0
        or values["x"] + values["width"] > 1
        or values["y"] + values["height"] > 1
    ):
        raise die("ROI는 top-left 기준 0..1 정규화 영역이어야 함")
    return {
        "name": str(result["name"]),
        "page": page,
        **values,
    }


def render_candidate_twice(
    hwp_bin: Path,
    source: Path,
    work: Path,
    dpi: int,
    report_path: Path | None = None,
) -> dict:
    """Perform two independent candidate renders and retain only safe summaries."""
    results = []
    for index in (1, 2):
        output = work / f"ours-{index}.pdf"
        report = None
        if report_path is not None:
            report = report_path
            if "{run}" in str(report_path) or "{index}" in str(report_path):
                report = Path(str(report_path).format(run=index, index=index))
        results.append(render_candidate_once(hwp_bin, source, output, dpi, report))
    merged = merge_render_reports(
        [(result["report"].get("report_source", "none"), result["report"]) for result in results]
    )
    return {
        "first": results[0],
        "second": results[1],
        "report": merged,
        "byte_equal": results[0]["sha256"] == results[1]["sha256"],
        "sha256": [results[0]["sha256"], results[1]["sha256"]],
    }


def _v2_thresholds(manifest: dict) -> dict:
    supplied = manifest.get("thresholds") or {}
    thresholds = dict(V2_THRESHOLDS)
    for key, expected in V2_THRESHOLDS.items():
        if key in supplied:
            try:
                actual = float(supplied[key])
            except (TypeError, ValueError):
                raise die(f"v2 threshold 형식 오류: {key}")
            if actual != expected:
                raise die(f"v2 threshold 변경 불가: {key}={actual} (계약값 {expected})")
    return thresholds


def _v2_gate_lists(results: dict[str, bool]) -> tuple[list[str], list[str]]:
    passed = [gate for gate in V2_GATE_NAMES if results.get(gate, False)]
    failed = [gate for gate in V2_GATE_NAMES if not results.get(gate, False)]
    return passed, failed


def score_case_v2(
    name: str,
    source: Path,
    oracle: Path,
    hwp_bin: Path,
    dpi: int,
    work: Path,
    thresholds: dict | None = None,
    rois: list | None = None,
    report_path: Path | None = None,
) -> dict:
    """Score all Issue #79 v2 gates for one source/oracle pair."""
    thresholds = {**V2_THRESHOLDS, **(thresholds or {})}
    report_path = report_path or work / "render-report-{run}.json"
    render = render_candidate_twice(hwp_bin, source, work, dpi, report_path)
    ours_pdf = Path(render["first"]["path"])
    oracle_pages = pdf_pages(oracle)
    ours_pages = pdf_pages(ours_pdf)
    page_delta = ours_pages - oracle_pages

    try:
        ours_boxes = pdf_media_boxes(ours_pdf)
        oracle_boxes = pdf_media_boxes(oracle)
    except (OSError, ValueError, SystemExit):
        ours_boxes, oracle_boxes = [], []
    media_pages = min(len(ours_boxes), len(oracle_boxes))
    media_entries = []
    for page in range(media_pages):
        delta = media_box_delta(ours_boxes[page], oracle_boxes[page])
        media_entries.append(
            {
                "page": page + 1,
                "ours": list(ours_boxes[page]),
                "oracle": list(oracle_boxes[page]),
                "delta_pt": delta,
            }
        )
    max_media_delta = max((entry["delta_pt"] for entry in media_entries), default=0.0)
    media_equal = (
        ours_pages == oracle_pages
        and len(ours_boxes) == ours_pages
        and len(oracle_boxes) == oracle_pages
        and len(media_entries) == ours_pages
    )
    media_passed = media_equal and max_media_delta <= thresholds["media_box_pt"]

    fonts_parse_ok = True
    try:
        candidate_fonts = pdf_fonts(ours_pdf)
        oracle_fonts = pdf_fonts(oracle)
    except (OSError, SystemExit, subprocess.SubprocessError):
        fonts_parse_ok = False
        candidate_fonts, oracle_fonts = [], []
    candidate_fonts = privacy_font_rows(candidate_fonts)
    oracle_fonts = privacy_font_rows(oracle_fonts)
    candidate_fonts_ok = bool(candidate_fonts) and all(
        font["embedded"] and font["subset"] and font["unicode"] for font in candidate_fonts
    )
    oracle_fonts_ok = bool(oracle_fonts) and all(
        font["embedded"] and font["subset"] and font["unicode"] for font in oracle_fonts
    )
    # Image-only PDFs legitimately have no font rows; in that case the
    # candidate contract is vacuously satisfied as long as both lists are empty.
    if fonts_parse_ok and not candidate_fonts and not oracle_fonts:
        candidate_fonts_ok = oracle_fonts_ok = True
    # The normative F1 gate is about the candidate PDF.  Oracle font rows are
    # retained for diagnostics, but an unusual oracle producer must not turn a
    # candidate that satisfies F1 into an ineligible result.
    fonts_passed = fonts_parse_ok and candidate_fonts_ok

    compared = min(ours_pages, oracle_pages)
    equal = 0
    first_diff = None
    for page in range(1, compared + 1):
        try:
            same = page_text(ours_pdf, page) == page_text(oracle, page)
        except (OSError, SystemExit):
            same = False
        if same:
            equal += 1
        elif first_diff is None:
            first_diff = page
    text_passed = ours_pages == oracle_pages and compared == ours_pages and equal == compared

    raster_entries = []
    roi_results = []
    raster_failed = False
    try:
        for page in range(1, compared + 1):
            ours_png = rasterize(ours_pdf, page, dpi, work / f"ours-raster-{page}")
            ref_png = rasterize(oracle, page, dpi, work / f"ref-raster-{page}")
            metrics = raster_diff(hwp_bin, source.name, ours_png, ref_png, page, dpi)
            entry = {"page": page, **metrics}
            entry["passed"] = (
                abs(entry["dx"]) <= thresholds["dx_px"]
                and abs(entry["dy"]) <= thresholds["dy_px"]
                and thresholds["ink_ratio_min"] <= entry["ink_ratio"] <= thresholds["ink_ratio_max"]
                and entry["bad_pixel_pct"] <= thresholds["bad_pixel_pct_max"]
                and entry["mae"] <= thresholds["mae_max"]
            )
            raster_entries.append(entry)
            raster_failed |= not entry["passed"]
            for roi in rois or []:
                normalized = _normalize_roi(roi)
                if normalized["page"] != page:
                    continue
                try:
                    precision, recall = roi_ink_precision_recall(
                        ours_png,
                        ref_png,
                        normalized["x"],
                        normalized["y"],
                        normalized["width"],
                        normalized["height"],
                        match_radius_px=thresholds["roi_match_radius_px"],
                    )
                except (OSError, ValueError):
                    precision, recall = 0.0, 0.0
                roi_results.append(
                    {
                        **normalized,
                        "precision": precision,
                        "recall": recall,
                        "passed": precision >= thresholds["roi_precision_min"]
                        and recall >= thresholds["roi_recall_min"],
                    }
                )
    except (OSError, ValueError, SystemExit):
        raster_failed = True
    raster_passed = bool(raster_entries) and not raster_failed and len(raster_entries) == compared
    roi_passed = bool(rois) and bool(roi_results) and len(roi_results) == len(rois or []) and all(
        result["passed"] for result in roi_results
    )

    report = render["report"]
    coverage = report.get("coverage")
    substitutions = 0 if coverage is None else (
        coverage["substituted"] + coverage["missing"] + coverage["subset_fallback"]
    )
    render_passed = (
        coverage is not None
        and coverage.get("substitution_free", False)
        and report.get("complete", False)
        and report.get("unsupported", 0) == 0
        and report.get("incomplete", 0) == 0
        and report.get("fatal", 0) == 0
    )
    determinism_passed = bool(render["byte_equal"])
    gate_results = {
        "page_count": page_delta == 0,
        "media_box": media_passed,
        "text": text_passed,
        "fonts": fonts_passed,
        "render_issues": render_passed,
        "raster": raster_passed,
        "roi": roi_passed,
        "determinism": determinism_passed,
    }
    passed_gates, failed_gates = _v2_gate_lists(gate_results)
    return {
        "contract": CONTRACT_SCORECARD_V2,
        "case": name,
        "source": {"name": source.name, "sha256": sha256_file(source)},
        "oracle": {"name": oracle.name, "sha256": sha256_file(oracle)},
        "dpi": dpi,
        "thresholds": thresholds,
        "passed_gates": passed_gates,
        "failed_gates": failed_gates,
        "eligible": not failed_gates,
        "pages": {"ours": ours_pages, "oracle": oracle_pages, "delta": page_delta},
        "media_box": {
            "pages_compared": media_pages,
            "pages_equal": len(media_entries),
            "max_delta_pt": max_media_delta,
            "within_threshold": media_passed,
            "per_page": media_entries,
        },
        "fonts": {
            "candidate": candidate_fonts,
            "oracle": oracle_fonts,
            "candidate_all_embedded_subset_unicode": candidate_fonts_ok,
            "oracle_all_embedded_subset_unicode": oracle_fonts_ok,
            "passed": fonts_passed,
        },
        "render": {
            "report_source": report.get("report_source", "none"),
            "report_available": bool(report.get("report_available", False)),
            "complete": bool(report.get("complete", False)),
            "font_coverage": coverage,
            "substitutions": substitutions,
            "unsupported": int(report.get("unsupported", 0)),
            "incomplete": int(report.get("incomplete", 0)),
            "fatal": int(report.get("fatal", 0)),
            "issue_codes": report.get("issue_codes", []),
            "passed": render_passed,
        },
        "text": {
            "pages_compared": compared,
            "pages_equal": equal,
            "first_diff_page": first_diff,
            "order_equal": text_passed,
            "passed": text_passed,
        },
        "raster": raster_entries,
        "rois": roi_results,
        "determinism": {
            "runs": 2,
            "byte_equal": bool(render["byte_equal"]),
            "sha256": render["sha256"],
            "passed": determinism_passed,
        },
    }


def summarize(card: dict) -> dict:
    if card.get("contract") == CONTRACT_SCORECARD_V2:
        return summarize_v2(card)
    raster = card["raster"]
    return {
        "name": card["case"],
        "scored": card["scored"],
        "unscored_reasons": card["unscored_reasons"],
        "pages_delta": card["pages"]["delta"],
        "text_pages_equal": card["text"]["pages_equal"],
        "text_pages_compared": card["text"]["pages_compared"],
        "raster_pages": len(raster),
        "max_abs_dx": max((abs(r["dx"]) for r in raster), default=0),
        "max_abs_dy": max((abs(r["dy"]) for r in raster), default=0),
        "ink_ratio_min": min((r["ink_ratio"] for r in raster), default=1.0),
        "ink_ratio_max": max((r["ink_ratio"] for r in raster), default=1.0),
        "bad_pixel_pct_max": max((r["bad_pixel_pct"] for r in raster), default=0.0),
        "mae_max": max((r["mae"] for r in raster), default=0.0),
        "fonts_all_embedded_subset_unicode": card["fonts"]
        ["all_embedded_subset_unicode"],
        "substitution_free": card["substitution_free"],
    }


def summarize_v2(card: dict) -> dict:
    """Build the platform-neutral scoreboard row without local paths/text."""
    raster = card["raster"]
    roi = card["rois"]
    return {
        "name": card["case"],
        "source_sha256": card["source"]["sha256"],
        "oracle_sha256": card["oracle"]["sha256"],
        "passed_gates": card["passed_gates"],
        "failed_gates": card["failed_gates"],
        "eligible": card["eligible"],
        "pages_delta": card["pages"]["delta"],
        "media_box_max_delta_pt": card["media_box"]["max_delta_pt"],
        "text_pages_equal": card["text"]["pages_equal"],
        "text_pages_compared": card["text"]["pages_compared"],
        "raster_pages": len(raster),
        "max_abs_dx": max((abs(entry["dx"]) for entry in raster), default=0),
        "max_abs_dy": max((abs(entry["dy"]) for entry in raster), default=0),
        "ink_ratio_min": min((entry["ink_ratio"] for entry in raster), default=1.0),
        "ink_ratio_max": max((entry["ink_ratio"] for entry in raster), default=1.0),
        "bad_pixel_pct_max": max((entry["bad_pixel_pct"] for entry in raster), default=0.0),
        "mae_max": max((entry["mae"] for entry in raster), default=0.0),
        "roi_count": len(roi),
        "roi_precision_min": min((entry["precision"] for entry in roi), default=0.0),
        "roi_recall_min": min((entry["recall"] for entry in roi), default=0.0),
        "fonts_passed": card["fonts"]["passed"],
        "render_issues_passed": card["render"]["passed"],
        "deterministic": card["determinism"]["passed"],
    }


def guard_no_paths(payload: str, forbidden: list) -> None:
    """Reject local absolute paths from committable output payloads."""
    for needle in forbidden:
        if needle and needle in payload:
            raise die(f"산출물에 로컬 경로 누출: {needle}")


def write_json(path: Path, value: dict, forbidden: list) -> None:
    payload = json.dumps(value, ensure_ascii=False, indent=2) + "\n"
    guard_no_paths(payload, forbidden)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(payload, encoding="utf-8")


def resolve_case_file(root: Path, filename: str, label: str) -> Path:
    """Resolve a regular case file without allowing a symlink escape."""
    try:
        resolved_root = root.resolve(strict=True)
    except OSError:
        raise die(f"{label} 디렉터리 없음: {root}")
    candidate = resolved_root / filename
    if candidate.is_symlink() or not candidate.is_file():
        raise die(f"{label} 파일 없음 또는 심볼릭 링크: {filename}")
    try:
        resolved = candidate.resolve(strict=True)
        contained = os.path.commonpath([str(resolved_root), str(resolved)]) == str(
            resolved_root
        )
    except (OSError, ValueError):
        contained = False
    if not contained:
        raise die(f"{label} 파일이 허용 디렉터리를 벗어남: {filename}")
    return candidate


def prepare_cases(manifest: dict, manifest_path: Path, oracle_dir: Path) -> list:
    """Reject duplicate names and verify every pinned artifact before rendering."""
    if not manifest["cases"]:
        raise die("manifest cases가 비어 있음")
    source_root = manifest_path.parent / "source"
    case_names = [case["name"] for case in manifest["cases"]]
    if len(set(case_names)) != len(case_names):
        duplicate = next(name for name in case_names if case_names.count(name) > 1)
        raise die(f"중복 case 이름: {duplicate}")
    prepared = []
    for case in manifest["cases"]:
        name = case["name"]
        source = resolve_case_file(source_root, case["source"], f"케이스 {name} source")
        oracle = resolve_case_file(oracle_dir, case["oracle"], f"케이스 {name} oracle")
        source_hash = sha256_file(source)
        oracle_hash = sha256_file(oracle)
        if source_hash != case["source_sha256"]:
            raise die(f"케이스 {name}: source SHA-256 불일치")
        if oracle_hash != case["oracle_sha256"]:
            raise die(f"케이스 {name}: oracle SHA-256 불일치")
        prepared.append(
            {
                "name": name,
                "source": source,
                "oracle": oracle,
                "source_sha256": source_hash,
                "oracle_sha256": oracle_hash,
                "rois": case.get("rois", []),
                "render_report": case.get("render_report"),
            }
        )
    return prepared


def validate_output_directory(out_dir: Path) -> None:
    if out_dir.is_symlink() or (out_dir.exists() and not out_dir.is_dir()):
        raise die(f"점수판 출력 디렉터리 경로가 안전하지 않음: {out_dir}")


def publish_artifacts(stage: Path, out_dir: Path, filenames: list) -> None:
    """Publish a validated file set with rollback for any mid-publish failure."""
    validate_output_directory(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    backup_dir = stage / ".previous"
    backup_dir.mkdir()
    published = []
    backups = []
    try:
        for filename in filenames:
            source = stage / filename
            destination = out_dir / filename
            if destination.is_symlink() or (
                destination.exists() and not destination.is_file()
            ):
                raise die(f"기존 점수판 산출물 경로가 안전하지 않음: {filename}")
            if destination.exists():
                backup = backup_dir / filename
                os.replace(destination, backup)
                backups.append((backup, destination))
            os.replace(source, destination)
            published.append(destination)
    except BaseException:
        for destination in reversed(published):
            if destination.is_file() or destination.is_symlink():
                destination.unlink()
        for backup, destination in reversed(backups):
            if backup.exists():
                os.replace(backup, destination)
        raise


def cmd_run_v1(args) -> int:
    manifest_path = Path(args.manifest)
    validate_documents([("pdf-parity-manifest-v1", manifest_path)])
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("contract") != CONTRACT_MANIFEST:
        raise die(f"manifest 계약 불일치: {manifest.get('contract')!r}")
    oracle_dir = Path(args.oracle_dir).expanduser()
    if not oracle_dir.is_dir():
        raise die(f"기준 PDF 디렉터리 없음: {oracle_dir} (로컬 전용, 커밋 금지)")
    out_dir = Path(args.out)
    validate_output_directory(out_dir)
    hwp_bin = Path(args.hwp_bin)
    if not hwp_bin.is_file():
        raise die(f"hwp 실행 파일 없음: {hwp_bin}")
    dpi = int(manifest["pins"]["dpi"])
    actual_poppler = verify_manifest_pins(manifest)
    cases = prepare_cases(manifest, manifest_path, oracle_dir)

    forbidden = [
        str(oracle_dir.resolve()),
        str(Path.home()),
        str(REPO_ROOT) + os.sep,
    ]
    cards = []
    for case in cases:
        name = case["name"]
        with tempfile.TemporaryDirectory(prefix="hwp-parity-") as tmp:
            card = score_case(
                name,
                case["source"],
                case["oracle"],
                hwp_bin,
                dpi,
                Path(tmp),
            )
        if card["source"]["sha256"] != case["source_sha256"]:
            raise die(f"케이스 {name}: 측정 중 source 파일 변경 감지")
        if card["oracle"]["sha256"] != case["oracle_sha256"]:
            raise die(f"케이스 {name}: 측정 중 oracle 파일 변경 감지")
        cards.append(card)
        mark = "scored" if card["scored"] else f"unscored({','.join(card['unscored_reasons'])})"
        print(f"[pdf-parity] {name}: pages Δ{card['pages']['delta']}, "
              f"text {card['text']['pages_equal']}/{card['text']['pages_compared']}, "
              f"raster {len(card['raster'])}쪽 — {mark}")

    board = {
        "contract": CONTRACT_SCOREBOARD,
        "generated_by": "scripts/pdf-parity.sh",
        "poppler_version": actual_poppler,
        "dpi": dpi,
        "cases": [summarize(c) for c in cards],
    }

    out_dir.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pdf-parity-stage-", dir=out_dir.parent) as tmp:
        stage = Path(tmp)
        for card in cards:
            write_json(stage / f"{card['case']}.json", card, forbidden)
        write_json(stage / "scoreboard.json", board, forbidden)

        csv_lines = ["case,page,dx,dy,ink_ratio,bad_pixel_pct,mae"]
        for card in cards:
            for raster in card["raster"]:
                csv_lines.append(
                    f"{card['case']},{raster['page']},{raster['dx']},{raster['dy']},"
                    f"{raster['ink_ratio']:.6f},{raster['bad_pixel_pct']:.6f},"
                    f"{raster['mae']:.4f}"
                )
        csv_payload = "\n".join(csv_lines) + "\n"
        guard_no_paths(csv_payload, forbidden)
        (stage / "scoreboard.csv").write_text(csv_payload, encoding="utf-8")

        pairs = [("pdf-parity-scoreboard-v1", stage / "scoreboard.json")]
        pairs.extend(
            ("pdf-parity-scorecard-v1", stage / f"{card['case']}.json")
            for card in cards
        )
        validate_documents(pairs)
        filenames = [f"{card['case']}.json" for card in cards]
        filenames.extend(["scoreboard.json", "scoreboard.csv"])
        publish_artifacts(stage, out_dir, filenames)
    print("[pdf-parity] 스키마 검증 통과")
    print(f"[pdf-parity] 점수판: {out_dir}/scoreboard.json (+ .csv, 케이스 {len(cards)}건)")
    return 0


def cmd_run_v2(args) -> int:
    manifest_path = Path(args.manifest)
    validate_documents([("pdf-parity-manifest-v2", manifest_path)])
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("contract") != CONTRACT_MANIFEST_V2:
        raise die(f"manifest 계약 불일치: {manifest.get('contract')!r}")
    oracle_dir = Path(args.oracle_dir).expanduser()
    if not oracle_dir.is_dir():
        raise die(f"기준 PDF 디렉터리 없음: {oracle_dir} (로컬 전용, 커밋 금지)")
    out_dir = Path(args.out)
    validate_output_directory(out_dir)
    hwp_bin = Path(args.hwp_bin)
    if not hwp_bin.is_file():
        raise die(f"hwp 실행 파일 없음: {hwp_bin}")
    dpi = int(manifest["pins"]["dpi"])
    actual_poppler = verify_manifest_pins_v2(manifest)
    cases = prepare_cases(manifest, manifest_path, oracle_dir)
    thresholds = _v2_thresholds(manifest)

    forbidden = [
        str(oracle_dir.resolve()),
        str(Path.home()),
        str(REPO_ROOT) + os.sep,
    ]
    cards = []
    for case in cases:
        report_path = None
        if getattr(args, "render_report", None):
            report_path = Path(args.render_report).expanduser()
        elif case.get("render_report"):
            # Manifest report paths are names only.  Resolve them under the
            # oracle directory's parent, never publish or echo the result.
            report_path = manifest_path.parent / case["render_report"]
        with tempfile.TemporaryDirectory(prefix="hwp-parity-v2-") as tmp:
            card = score_case_v2(
                case["name"],
                case["source"],
                case["oracle"],
                hwp_bin,
                dpi,
                Path(tmp),
                thresholds,
                case.get("rois", []),
                report_path,
            )
        if card["source"]["sha256"] != case["source_sha256"]:
            raise die(f"케이스 {case['name']}: 측정 중 source 파일 변경 감지")
        if card["oracle"]["sha256"] != case["oracle_sha256"]:
            raise die(f"케이스 {case['name']}: 측정 중 oracle 파일 변경 감지")
        cards.append(card)
        mark = "eligible" if card["eligible"] else f"ineligible({','.join(card['failed_gates'])})"
        print(
            f"[pdf-parity] {case['name']}: pages Δ{card['pages']['delta']}, "
            f"text {card['text']['pages_equal']}/{card['text']['pages_compared']}, "
            f"gates {len(card['passed_gates'])}/{len(V2_GATE_NAMES)} — {mark}"
        )

    gate_sets = [set(card["passed_gates"]) for card in cards]
    all_passed = [gate for gate in V2_GATE_NAMES if all(gate in passed for passed in gate_sets)]
    all_failed = [gate for gate in V2_GATE_NAMES if gate not in all_passed]
    board = {
        "contract": CONTRACT_SCOREBOARD_V2,
        "poppler_version": actual_poppler,
        "dpi": dpi,
        "thresholds": thresholds,
        "eligible": all(card["eligible"] for card in cards),
        "passed_gates": all_passed,
        "failed_gates": all_failed,
        "cases": [summarize_v2(card) for card in cards],
    }

    out_dir.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pdf-parity-v2-stage-", dir=out_dir.parent) as tmp:
        stage = Path(tmp)
        for card in cards:
            write_json(stage / f"{card['case']}.json", card, forbidden)
        write_json(stage / "scoreboard.json", board, forbidden)

        csv_lines = [
            "case,eligible,failed_gates,page_delta,media_box_max_delta_pt,"
            "text_pages_equal,text_pages_compared,raster_pages,max_abs_dx,max_abs_dy,"
            "ink_ratio_min,ink_ratio_max,bad_pixel_pct_max,mae_max,roi_count,"
            "roi_precision_min,roi_recall_min,fonts_passed,render_issues_passed,deterministic"
        ]
        for row in board["cases"]:
            csv_lines.append(
                f"{row['name']},{str(row['eligible']).lower()},"
                f"{';'.join(row['failed_gates'])},{row['pages_delta']},"
                f"{row['media_box_max_delta_pt']:.6f},{row['text_pages_equal']},"
                f"{row['text_pages_compared']},{row['raster_pages']},{row['max_abs_dx']},"
                f"{row['max_abs_dy']},{row['ink_ratio_min']:.6f},{row['ink_ratio_max']:.6f},"
                f"{row['bad_pixel_pct_max']:.6f},{row['mae_max']:.4f},{row['roi_count']},"
                f"{row['roi_precision_min']:.6f},{row['roi_recall_min']:.6f},"
                f"{str(row['fonts_passed']).lower()},{str(row['render_issues_passed']).lower()},"
                f"{str(row['deterministic']).lower()}"
            )
        csv_payload = "\n".join(csv_lines) + "\n"
        guard_no_paths(csv_payload, forbidden)
        (stage / "scoreboard.csv").write_text(csv_payload, encoding="utf-8")

        pairs = [("pdf-parity-scoreboard-v2", stage / "scoreboard.json")]
        pairs.extend(
            ("pdf-parity-scorecard-v2", stage / f"{card['case']}.json") for card in cards
        )
        validate_documents(pairs)
        filenames = [f"{card['case']}.json" for card in cards]
        filenames.extend(["scoreboard.json", "scoreboard.csv"])
        publish_artifacts(stage, out_dir, filenames)
    print("[pdf-parity] v2 스키마 검증 통과")
    print(f"[pdf-parity] v2 점수판: {out_dir}/scoreboard.json (+ .csv, 케이스 {len(cards)}건)")
    # A failed gate is a valid measured result, but an ineligible run must be
    # visible to CI callers instead of being mistaken for a green release gate.
    return 0 if board["eligible"] else 1


def verify_manifest_pins_v2(manifest: dict) -> str:
    """Verify v2's platform-neutral Poppler and font-byte pins."""
    pins = manifest["pins"]
    require_concrete_pin("poppler_version", pins["poppler_version"])
    actual_poppler = poppler_version()
    if pins["poppler_version"] != actual_poppler:
        raise die(
            "Poppler 버전 불일치: "
            f"manifest={pins['poppler_version']!r}, actual={actual_poppler!r}"
        )
    directory = font_directory()
    if not directory.is_dir():
        raise die(f"고정 폰트 디렉터리 없음: {directory}")
    available = {
        sha256_file(path)
        for path in directory.rglob("*")
        if path.is_file() and path.suffix.lower() in FONT_EXTENSIONS
    }
    for family, expected in pins["fonts"].items():
        if not SHA256_RE.fullmatch(expected) or expected not in available:
            raise die(f"고정 폰트 SHA-256 불일치 또는 파일 없음: {family}")
    return actual_poppler


def cmd_run(args) -> int:
    """Dispatch v1/v2 after reading only the non-sensitive contract marker."""
    manifest_path = Path(args.manifest)
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise die(f"manifest 읽기 실패: {error}")
    contract = manifest.get("contract")
    if contract == CONTRACT_MANIFEST_V2:
        return cmd_run_v2(args)
    if contract == CONTRACT_MANIFEST:
        return cmd_run_v1(args)
    raise die(f"manifest 계약 불일치: {contract!r}")


def cmd_selftest(args) -> int:
    """Verify that comparing one rendered PDF with itself is exact."""
    source = Path(args.source) if args.source else None
    if source is None:
        candidates = sorted((REPO_ROOT / "fixtures/hwp5").glob("*.hwp"))
        if not candidates:
            raise die("selftest용 fixture 없음 (fixtures/hwp5/*.hwp)")
        source = candidates[0]
    if not source.is_file():
        raise die(f"fixture 없음: {source}")
    dpi = 150
    with tempfile.TemporaryDirectory(prefix="hwp-parity-selftest-") as tmp:
        work = Path(tmp)
        oracle = work / "oracle.pdf"
        render_ours(Path(args.hwp_bin), source, oracle, dpi)
        # Compare two independent renders so the oracle is never overwritten in place.
        card = score_case("selftest", source, oracle,
                          Path(args.hwp_bin), dpi, work)
    problems = []
    if card["pages"]["delta"] != 0:
        problems.append("쪽수 불일치")
    t = card["text"]
    if t["pages_equal"] != t["pages_compared"]:
        problems.append(f"텍스트 불일치 (첫 쪽 {t['first_diff_page']})")
    for r in card["raster"]:
        if (
            (r["dx"], r["dy"]) != (0, 0)
            or r["bad_pixel_pct"] != 0.0
            or r["ink_ratio"] != 1.0
            or r["mae"] != 0.0
        ):
            problems.append(f"래스터 불일치 (쪽 {r['page']})")
    if not card["fonts"]["all_embedded_subset_unicode"]:
        problems.append("임베드/서브셋/유니코드 아닌 폰트")
    if not card["substitution_free"]:
        problems.append("글꼴 커버리지 확인 실패 또는 대체 감지")
    if problems:
        for p in problems:
            print(f"[pdf-parity] selftest 실패: {p}", file=sys.stderr)
        return 1
    print(f"[pdf-parity] selftest 통과: {source.name} — "
          f"{card['pages']['ours']}쪽, 텍스트 {t['pages_equal']}/{t['pages_compared']}, "
          f"래스터 {len(card['raster'])}쪽 전부 일치")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Hancom 기준 PDF 동등성 배치 러너")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p_run = sub.add_parser("run", help="manifest의 케이스를 점수판으로 집계")
    p_run.add_argument("--manifest", required=True)
    p_run.add_argument("--oracle-dir",
                       default=os.environ.get("HWP_PDF_PARITY_ORACLE_DIR", ""))
    p_run.add_argument("--out", required=True)
    p_run.add_argument("--hwp-bin", required=True)
    p_run.add_argument(
        "--render-report",
        default=None,
        help="optional JSON render report path (v2; stderr remains a compatible fallback)",
    )

    p_self = sub.add_parser("selftest", help="하네스 자기 검증 (fixture vs 자기 자신)")
    p_self.add_argument("--hwp-bin", required=True)
    p_self.add_argument("--source", default=None)

    args = ap.parse_args()
    if args.cmd == "run":
        if not args.oracle_dir:
            raise die("--oracle-dir 또는 HWP_PDF_PARITY_ORACLE_DIR 필요")
        return cmd_run(args)
    return cmd_selftest(args)


if __name__ == "__main__":
    sys.exit(main())
