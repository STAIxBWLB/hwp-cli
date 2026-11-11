#!/usr/bin/env python3
"""Generate a diagnostic corpus and run self-verification.

Generate isolated Markdown features as HWP/HWPX, then check their structure,
rendering, strict conversion, text round-trip and external-parser readability.
Only self-verifiable checks run here; Hangul-specific behavior needs native testing.

Usage: HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py [output-directory]
"""
import os, sys, subprocess, re, zipfile, tempfile
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HWP = os.path.join(ROOT, "target", "debug", "ahwp")
OUTDIR = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target", "diagnostic-corpus")
os.makedirs(OUTDIR, exist_ok=True)
os.environ.setdefault("HWP_FONT_DIR", os.path.join(ROOT, "fonts"))

# ── Isolated Markdown cases (one feature per file) ──
CASES = {
    "single_para":       "간단한 한 문단입니다.\n",
    "multi_para":        "첫째 문단입니다.\n\n둘째 문단입니다.\n\n셋째 문단입니다.\n",
    "headings":          "# 제목1\n\n## 제목2\n\n### 제목3\n\n본문 내용.\n",
    "bullet_list":       "- 첫째 항목\n- 둘째 항목\n- 셋째 항목\n",
    "numbered_list":     "1. 첫째\n2. 둘째\n3. 셋째\n",
    "formatting":        "**굵게** *기울임* 그리고 `코드` 텍스트입니다.\n",
    "long_para":         ("긴 문단 줄바꿈 테스트. " * 20).strip() + "\n",
    "table_2x2":         "| 머리1 | 머리2 |\n|---|---|\n| 값A | 값B |\n| 값C | 값D |\n",
    "table_header_only": "| 가 | 나 | 다 |\n|---|---|---|\n",
    "table_multiline":   "| 짧음 | " + ("긴 셀 내용 " * 15).strip() + " |\n|---|---|\n| a | b |\n",
    "table_empty_cells": "| 가 | 나 | 다 |\n|---|---|---|\n| 1 |  | 3 |\n|  | 5 |  |\n",
    "multipage":         "\n\n".join(f"문단 번호 {i}. " + "내용을 채우는 문장입니다. " * 8 for i in range(60)) + "\n",
    "special_chars":     "특수문자: © ® ™ § ¶ — “ ” ‘ ’ … 数式 α β γ ½ ¼ →←↑↓\n",
    "mixed":             "머리말 문단.\n\n| 표 | 헤더 |\n|---|---|\n| 셀 | 값 |\n\n표 뒤 본문 문단.\n",
    "nested_list":       "- 상위1\n  - 하위1a\n  - 하위1b\n- 상위2\n",
    "blockquote":        "> 인용문입니다.\n> 둘째 줄.\n",
    "code_block":        "```\n코드 블록 라인1\n코드 라인2\n```\n",
    "link":              "[한컴 링크](https://www.hancom.com) 텍스트입니다.\n",
    "deep_heading":      "#### 제목4\n\n##### 제목5\n\n###### 제목6\n",
    "hr_rule":           "위 문단.\n\n---\n\n아래 문단.\n",
    "table_wide":        "| A | B | C | D | E | F |\n|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 |\n",
    "table_long":        "| 번호 | 값 |\n|---|---|\n" + "".join(f"| {i} | 항목{i} |\n" for i in range(1, 16)),
}

FIXTURES = [
    ("hello_world", "fixtures/hwp5/hello_world.hwp"),
    ("work_report", "fixtures/hwp5/work_report.hwp"),
    ("annual_report", "fixtures/hwp5/annual_report.hwp"),
    ("color_fill", "fixtures/hwp5/color_fill.hwp"),
    ("outline", "fixtures/hwp5/outline.hwp"),
    ("bookmark", "fixtures/hwp5/bookmark.hwp"),
    ("minimal_hwpx", "fixtures/hwpx/minimal.hwpx"),
]

def run(args, **kw):
    try:
        r = subprocess.run([HWP] + args, capture_output=True, text=True, timeout=120, **kw)
        return r.returncode, r.stdout, r.stderr
    except Exception as e:
        return -1, "", str(e)

def norm_text(s):
    return re.sub(r"\s+", " ", s).strip()

def structural_ok(path):
    """Validate HWPX, or check HWP5 readability through olefile/CFB."""
    if path.endswith(".hwpx"):
        rc, _, _ = run(["validate", path]); return rc == 0
    else:
        try:
            import olefile
            return olefile.isOleFile(path)
        except Exception:
            # Without olefile, approximate readability by checking for a nonempty file.
            return os.path.getsize(path) > 0

def external_parse_ok(path):
    """Parse HWPX ZIP/XML, or read HWP5 streams through pyhwp/olefile."""
    try:
        if path.endswith(".hwpx"):
            z = zipfile.ZipFile(path)
            for n in z.namelist():
                if n.endswith(".xml"): ET.fromstring(z.read(n))
            return True
        else:
            import olefile
            ole = olefile.OleFileIO(path)
            return ole.exists("BodyText/Section0") or ole.exists("BodyText")
    except Exception:
        return False

def render_ok(path):
    rc, _, err = run(["render", path, "-o", os.path.join(tempfile.gettempdir(), "diag_r.png"), "--pages", "1", "--dpi", "72"])
    return rc == 0

def strict_convert_ok(path, to):
    rc, _, err = run(["convert", path, "-o", os.path.join(tempfile.gettempdir(), f"diag_s.{to}"), "--to", to, "--strict"])
    return rc == 0, err.strip().splitlines()[-1] if (rc != 0 and err.strip()) else ""

def get_text(path):
    # Read Markdown/text directly; use ahwp cat for native HWP/HWPX inputs.
    if path.endswith((".md", ".txt")):
        try: return open(path, encoding="utf-8").read()
        except Exception: return None
    # Remove Markdown table syntax before comparing text.
    rc, out, _ = run(["cat", path]); return out if rc == 0 else None

def strip_md(s):
    s = re.sub(r"^\s*\d+\.\s", " ", s, flags=re.M)   # Numbered list markers.
    s = re.sub(r"[|`*#>_-]", " ", s)                 # Markup characters.
    s = re.sub(r"---+", " ", s)
    return norm_text(s)

def text_preserved(md_text, gen_path):
    """Measure source word retention in generated-file text after stripping Markdown."""
    cat = get_text(gen_path)
    if cat is None: return None, "cat 실패"
    a = strip_md(md_text); b = norm_text(cat)
    aw = [w for w in a.split() if len(w) > 1]
    if not aw: return True, ""
    common = sum(1 for w in aw if w in b)
    ratio = common / len(aw)
    return (ratio >= 0.9), (f"텍스트 {ratio:.0%}" if ratio < 0.9 else "")

def main():
    tmp = tempfile.mkdtemp()
    rows = []  # (name, kind, checks dict)

    # ── Generated cases: Markdown to HWP and HWPX ──
    for name, md in CASES.items():
        mdpath = os.path.join(OUTDIR, f"{name}.md")
        open(mdpath, "w").write(md)
        for fmt in ("hwp", "hwpx"):
            out = os.path.join(OUTDIR, f"{name}.{fmt}")
            rc, _, err = run(["new", "--from", mdpath, "-o", out])
            checks = {}
            if rc != 0:
                checks["생성"] = ("❌", err.strip().splitlines()[-1] if err.strip() else "실패")
                rows.append((f"gen/{name}", fmt, checks)); continue
            checks["생성"] = ("✅", "")
            checks["구조"] = ("✅" if structural_ok(out) else "❌", "")
            checks["외부파서"] = ("✅" if external_parse_ok(out) else "❌", "")
            checks["렌더"] = ("✅" if render_ok(out) else "❌", "")
            ok, note = strict_convert_ok(out, "md")
            checks["strict변환"] = ("✅" if ok else "⚠️", note)
            rtok, rtnote = text_preserved(md, out)
            checks["텍스트보존"] = ("✅" if rtok else ("⚠️" if rtok is not None else "❌"), rtnote)
            rows.append((f"gen/{name}", fmt, checks))

    # ── Fixtures: structure, external parser, render, strict conversion and round-trip ──
    for name, rel in FIXTURES:
        src = os.path.join(ROOT, rel)
        if not os.path.exists(src): continue
        fmt = "hwpx" if src.endswith(".hwpx") else "hwp"
        checks = {}
        checks["구조"] = ("✅" if structural_ok(src) else "❌", "")
        checks["외부파서"] = ("✅" if external_parse_ok(src) else "❌", "")
        checks["렌더"] = ("✅" if render_ok(src) else "❌", "")
        # Cross-format conversion: HWP5 to HWPX or HWPX to HWP5.
        cross = "hwpx" if fmt == "hwp" else "hwp"
        crosspath = os.path.join(OUTDIR, f"fixture_{name}_to_{cross}.{cross}")
        rc, _, err = run(["convert", src, "-o", crosspath, "--to", cross])
        checks["교차변환"] = ("✅" if rc == 0 else "❌", err.strip().splitlines()[-1] if (rc != 0 and err.strip()) else "")
        ok, note = strict_convert_ok(src, cross)
        checks["strict교차"] = ("✅" if ok else "⚠️", note)
        # Structure and rendering of cross-format output.
        if rc == 0:
            checks["교차구조"] = ("✅" if structural_ok(crosspath) else "❌", "")
            checks["교차렌더"] = ("✅" if render_ok(crosspath) else "❌", "")
        rows.append((f"fixture/{name}", fmt, checks))

    # Report.
    allchecks = []
    for _,_,c in rows:
        for k in c:
            if k not in allchecks: allchecks.append(k)
    print("# 진단 코퍼스 검증 리포트\n")
    print(f"생성 위치: `{OUTDIR}`\n")
    print("| 케이스 | 포맷 | " + " | ".join(allchecks) + " |")
    print("|---|---|" + "|".join(["---"]*len(allchecks)) + "|")
    fails = []
    for name, fmt, checks in rows:
        cells = []
        for k in allchecks:
            v = checks.get(k)
            if v is None: cells.append("·")
            else:
                sym, note = v
                cells.append(sym + (f" {note[:24]}" if note and sym != "✅" else ""))
                if sym == "❌": fails.append((name, fmt, k, note))
        print(f"| {name} | {fmt} | " + " | ".join(cells) + " |")
    print(f"\n## 요약\n- 총 {len(rows)} 케이스, 검사 {len(allchecks)}종")
    if fails:
        print(f"- ❌ 실패 {len(fails)}건:")
        for n,f,k,note in fails: print(f"  - {n} ({f}) — {k}: {note}")
    else:
        print("- ❌ 실패 0건 (self-verifiable 검사 전부 통과)")
    print("\n※ ⚠️=경고(strict 데이터손실/텍스트근사), ✅=통과, ❌=실패, ·=해당없음")
    print("※ 한글(한컴오피스) 실기 필요 이슈(annual 글상자 드롭 등)는 이 자체검증으로 안 잡힘 — 별도 실기.")

if __name__ == "__main__":
    main()
