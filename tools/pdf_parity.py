#!/usr/bin/env python3
"""Hancom PDF parity batch runner for issue #79 PR 4.

The runner compares locally rendered PDFs with private Hancom oracle PDFs using
the five metrics defined in docs/design/21-pdf-parity.md section 3. It validates
and enforces every locally checkable manifest pin before measuring any case.
Only names, SHA-256 digests, and numeric results are allowed in published files.

Use scripts/pdf-parity.sh instead of invoking this module directly. The wrapper
checks external tools and builds the hwp binary and JSON Schema validator.
"""

import argparse
import glob
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

CONTRACT_MANIFEST = "hwp-pdf-parity-manifest-v1"
CONTRACT_SCORECARD = "hwp-pdf-parity-scorecard-v1"
CONTRACT_SCOREBOARD = "hwp-pdf-parity-scoreboard-v1"

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMAS = REPO_ROOT / "schemas"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PLACEHOLDER_RE = re.compile(r"(?:todo|placeholder)", re.IGNORECASE)
FONT_EXTENSIONS = {".otc", ".otf", ".ttc", ".ttf"}

# A pdffonts row is: name, type, encoding, emb, sub, uni, object ID (two tokens).
FONT_ROW = re.compile(
    r"^(?P<name>.*?)\s+"
    r"(?P<type>CID Font Type 0C?|CID TrueType(?: \(OpenType\))?|CID Type 0C?(?: \(OT\))?"
    r"|TrueType(?: \(OpenType\))?|Type 0|Type 1C?(?: \(OT\))?|Type 3)\s+"
    r"(?P<encoding>\S+)\s+(?P<emb>yes|no)\s+(?P<sub>yes|no)\s+(?P<uni>yes|no)\s+"
    r"\d+\s+\d+\s*$"
)

COVERAGE_RE = re.compile(
    r"글꼴 커버리지 matched=(\d+) substituted=(\d+) missing=(\d+) subset_fallback=(\d+)"
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


def page_text(pdf: Path, page: int) -> str:
    """Normalize one pdftotext -layout page by collapsing whitespace."""
    out = run(["pdftotext", "-layout", "-f", str(page), "-l", str(page), str(pdf), "-"]).stdout
    lines = [re.sub(r"\s+", " ", ln).strip() for ln in out.splitlines()]
    return "\n".join(ln for ln in lines if ln)


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


def render_ours(hwp_bin: Path, source: Path, out_pdf: Path, dpi: int) -> dict:
    """Render a PDF and parse the font coverage report from stderr."""
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


def summarize(card: dict) -> dict:
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


def cmd_run(args) -> int:
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
