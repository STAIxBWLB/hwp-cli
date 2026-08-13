#!/usr/bin/env python3
"""PDF 동등성 배치 러너 (issue #79 PR 4) — docs/design/21-pdf-parity.md §3의 다섯 지표.

케이스마다 우리 PDF(hwp render --format pdf)와 한컴 기준 PDF를 나란히 재서
1) pdffonts 임베드/서브셋/유니코드, 2) pdfinfo 쪽수, 3) pdftotext -layout 정규화
텍스트 등가, 4~5) 같은 Poppler(pdftoppm -r 150) 래스터의 dx/dy·ink_ratio·
bad_pixel_pct·mae(hwp diff --format json)를 모아 점수카드 JSON을 만든다.

데이터 정책(docs/design/21 §7): 기준 PDF는 로컬 전용, 커밋되는 산출물은 이름과
SHA-256, 수치뿐 — 절대 경로가 새지 않도록 쓰기 전에 가드를 둔다.

직접 실행보다 scripts/pdf-parity.sh 래퍼를 쓴다(도구 점검·hwp 빌드 포함).
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

# pdffonts 한 행: name type encoding emb sub uni object-ID(2토큰).
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
    # pdfinfo -v는 stderr로 버전을 내는 배포판이 있어 양쪽을 모두 본다.
    proc = subprocess.run(["pdfinfo", "-v"], capture_output=True, text=True)
    out = (proc.stdout or proc.stderr).strip()
    return out.splitlines()[0] if out else "unknown"


def pdf_pages(pdf: Path) -> int:
    out = run(["pdfinfo", str(pdf)]).stdout
    m = re.search(r"^Pages:\s+(\d+)", out, re.M)
    if not m:
        raise die(f"pdfinfo Pages 파싱 실패: {pdf.name}")
    return int(m.group(1))


def pdf_fonts(pdf: Path) -> list:
    """pdffonts → [{name, type, embedded, subset, unicode}]."""
    out = run(["pdffonts", str(pdf)]).stdout
    fonts = []
    for line in out.splitlines()[2:]:  # 헤더 2행 건너뜀
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
    """pdftotext -layout 한 쪽 — 공백 런 붕괴·빈 줄 제거로 정규화."""
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
    """우리 PDF를 만들고 폰트 커버리지(stderr)를 파싱해 돌려준다."""
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
    """케이스 하나의 다섯 지표를 모아 점수카드를 만든다."""
    coverage = render_ours(hwp_bin, source, work / "ours.pdf", dpi)
    ours_pdf = work / "ours.pdf"

    ours_pages, oracle_pages = pdf_pages(ours_pdf), pdf_pages(oracle)
    delta = ours_pages - oracle_pages

    ours_fonts, oracle_fonts = pdf_fonts(ours_pdf), pdf_fonts(oracle)
    fonts_ok = all(f["embedded"] and f["subset"] and f["unicode"] for f in ours_fonts)

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

    substitution_free = True if coverage is None else coverage["substitution_free"]
    reasons = []
    if delta != 0:
        reasons.append("page_count_delta")
    if not substitution_free:
        reasons.append("font_substitution")

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
            "ours_all_embedded_subset_unicode": fonts_ok,
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
        ["ours_all_embedded_subset_unicode"],
        "substitution_free": card["substitution_free"],
    }


def guard_no_paths(payload: str, forbidden: list) -> None:
    """커밋될 산출물에 로컬 절대 경로가 새지 않았는지 검사한다."""
    for needle in forbidden:
        if needle and needle in payload:
            raise die(f"산출물에 로컬 경로 누출: {needle}")


def write_json(path: Path, value: dict, forbidden: list) -> None:
    payload = json.dumps(value, ensure_ascii=False, indent=2) + "\n"
    guard_no_paths(payload, forbidden)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(payload, encoding="utf-8")


def cmd_run(args) -> int:
    manifest = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    if manifest.get("contract") != CONTRACT_MANIFEST:
        raise die(f"manifest 계약 불일치: {manifest.get('contract')!r}")
    oracle_dir = Path(args.oracle_dir).expanduser()
    if not oracle_dir.is_dir():
        raise die(f"기준 PDF 디렉터리 없음: {oracle_dir} (로컬 전용, 커밋 금지)")
    out_dir = Path(args.out)
    dpi = int(manifest["pins"]["dpi"])
    source_root = Path(args.manifest).parent / "source"

    forbidden = [str(oracle_dir), str(Path.home()), str(REPO_ROOT) + os.sep]
    cards = []
    for case in manifest["cases"]:
        name = case["name"]
        source = source_root / case["source"]
        oracle = oracle_dir / case["oracle"]
        for p in (source, oracle):
            if not p.is_file():
                raise die(f"케이스 {name}: 파일 없음 — {p.name}")
        with tempfile.TemporaryDirectory(prefix="hwp-parity-") as tmp:
            card = score_case(name, source, oracle, Path(args.hwp_bin), dpi, Path(tmp))
        write_json(out_dir / f"{name}.json", card, forbidden)
        cards.append(card)
        mark = "scored" if card["scored"] else f"unscored({','.join(card['unscored_reasons'])})"
        print(f"[pdf-parity] {name}: pages Δ{card['pages']['delta']}, "
              f"text {card['text']['pages_equal']}/{card['text']['pages_compared']}, "
              f"raster {len(card['raster'])}쪽 — {mark}")

    board = {
        "contract": CONTRACT_SCOREBOARD,
        "generated_by": "scripts/pdf-parity.sh",
        "poppler_version": poppler_version(),
        "dpi": dpi,
        "cases": [summarize(c) for c in cards],
    }
    write_json(out_dir / "scoreboard.json", board, forbidden)

    csv_lines = ["case,page,dx,dy,ink_ratio,bad_pixel_pct,mae"]
    for c in cards:
        for r in c["raster"]:
            csv_lines.append(
                f"{c['case']},{r['page']},{r['dx']},{r['dy']},"
                f"{r['ink_ratio']:.6f},{r['bad_pixel_pct']:.6f},{r['mae']:.4f}"
            )
    csv_payload = "\n".join(csv_lines) + "\n"
    guard_no_paths(csv_payload, forbidden)
    (out_dir / "scoreboard.csv").write_text(csv_payload, encoding="utf-8")

    # 스키마 검증 (래퍼가 빌드해 둔 예제 검증기 재사용).
    validator = REPO_ROOT / "target/debug/examples/validate_structured_corpus"
    if validator.is_file():
        pairs = []
        for schema, doc in [
            ("pdf-parity-scoreboard-v1", out_dir / "scoreboard.json"),
            ("pdf-parity-manifest-v1", Path(args.manifest)),
        ] + [("pdf-parity-scorecard-v1", out_dir / f"{c['case']}.json") for c in cards]:
            pairs += [str(REPO_ROOT / f"schemas/{schema}.schema.json"), str(doc)]
        subprocess.run([str(validator)] + pairs, check=True)
        print("[pdf-parity] 스키마 검증 통과")
    else:
        print("[pdf-parity] 경고: 스키마 검증기 없음 — 검증 건너뜀", file=sys.stderr)
    print(f"[pdf-parity] 점수판: {out_dir}/scoreboard.json (+ .csv, 케이스 {len(cards)}건)")
    return 0


def cmd_selftest(args) -> int:
    """하네스 자기 검증: 같은 PDF를 기준으로 돌리면 모든 지표가 완벽해야 한다."""
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
        render_ours(Path(args.hwp_bin), source, work / "ours.pdf", dpi)
        # 기준 = 우리 PDF 자신.
        card = score_case("selftest", source, work / "ours.pdf",
                          Path(args.hwp_bin), dpi, work)
    problems = []
    if card["pages"]["delta"] != 0:
        problems.append("쪽수 불일치")
    t = card["text"]
    if t["pages_equal"] != t["pages_compared"]:
        problems.append(f"텍스트 불일치 (첫 쪽 {t['first_diff_page']})")
    for r in card["raster"]:
        if (r["dx"], r["dy"]) != (0, 0) or r["bad_pixel_pct"] != 0.0 or r["ink_ratio"] != 1.0:
            problems.append(f"래스터 불일치 (쪽 {r['page']})")
    if not card["fonts"]["ours_all_embedded_subset_unicode"]:
        problems.append("임베드/서브셋/유니코드 아닌 폰트")
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
