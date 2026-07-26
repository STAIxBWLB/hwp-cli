#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""C·D 시리즈(글자효과·요약정보 + 도장 날인·사용자 탭) 검증 파일 생성기 + 자체 검증 게이트.

gen_verification_set.sh 가 A/B 시리즈를 만든 뒤 이 스크립트를 한 번 호출한다.
C 시리즈는 CLI 에 글자효과 플래그가 없으므로 **JSON IR 경유**로 만든다:
  1) 마크다운 → 우리 hwpx(base) 로 뼈대를 만들고,
  2) `hwp cat --format json` 으로 IR 을 떠서 python(stdlib) 으로 수술
     (char_shapes 에 효과 비트를 켠 문자모양을 추가하고 char_shape_runs 로 참조),
  3) `hwp new --from surgery.json` 으로 최종 파일을 방출한다.

D 시리즈는 두 경로를 쓴다: 도장 날인(D1/D2)은 `hwp edit --seal` CLI 로 부유 그림을
얹고(빨간 원 PNG 는 zlib 로 합성), 사용자 탭(D3)은 위 IR 수술로 tab_stops 를 정의해
문단이 참조하게 한 뒤 산출 header.xml 배선까지 확인한다.

각 파일은 다시 `hwp cat --format json` 으로 재읽기해 효과 비트/밑줄모양/번호형식/
metadata/도장(treat_as_char)/탭(pos·kind·fill) 가 실제로 살아있는지 단언한다. 하나라도
죽어 있으면 그 파일은 ❌ 처리한다.

표준 출력에는 파일당 한 줄(✅/❌ …)을 찍는다. bash 가 이 줄들을 REPORT 에 합치고
✅/❌ 개수로 pass/fail 을 집계한다. 종료코드: 실패가 있으면 1.

의존성: 파이썬 표준 라이브러리만 사용(json/subprocess/…). 외부 패키지 금지.
"""

import argparse
import base64
import binascii
import copy
import datetime
import json
import os
import re
import struct
import subprocess
import sys
import zipfile
import zlib

# ── 글자모양 attr 비트(hwp5 스펙 = crates/hwp-model/src/header.rs 접근자와 동일) ──
ITALIC = 1 << 0
BOLD = 1 << 1
UL_BOTTOM = 1 << 2   # 밑줄 종류(bits2~3)=1 → 글자 아래
OUTLINE = 1 << 8     # 외곽선(bits8~10)
SHADOW = 1 << 11     # 그림자(bits11~12)
EMBOSS = 1 << 13     # 양각
ENGRAVE = 1 << 14    # 음각
SUPER = 1 << 15      # 위첨자
SUB = 1 << 16        # 아래첨자

# 밑줄 모양 코드(crates/hwpx read/write header line_type_code/name 과 대칭).
UL_DOT = 3           # 점선
UL_CIRCLE = 7        # 원형 점선(물결에 가장 가까운, 왕복되는 비실선)
UL_DOUBLE = 8        # 이중선

HWP = None  # 바이너리 경로(런타임 설정)
WORK = None  # 작업 디렉터리


def run_json(args):
    """hwp 서브커맨드를 실행하고 stdout(JSON)을 파싱해 돌려준다."""
    out = subprocess.run(
        [HWP, *args], capture_output=True, text=True
    )
    return out


def base_ir(md_text, tag):
    """마크다운 → base hwpx → IR(dict). 뼈대(폰트/테두리/스타일)를 얻기 위함."""
    md = os.path.join(WORK, f"{tag}.md")
    base = os.path.join(WORK, f"{tag}_base.hwpx")
    with open(md, "w", encoding="utf-8") as f:
        f.write(md_text)
    subprocess.run([HWP, "new", "--from", md, "-o", base],
                   capture_output=True, text=True)
    r = subprocess.run([HWP, "cat", base, "--format", "json"],
                       capture_output=True, text=True)
    return json.loads(r.stdout)


def emit(ir, out_path, tag):
    """IR(dict) → JSON 파일 → hwp new --from → out_path."""
    j = os.path.join(WORK, f"{tag}_ir.json")
    with open(j, "w", encoding="utf-8") as f:
        json.dump(ir, f, ensure_ascii=False)
    p = subprocess.run([HWP, "new", "--from", j, "-o", out_path],
                       capture_output=True, text=True)
    return p


def reread(path):
    r = subprocess.run([HWP, "cat", path, "--format", "json"],
                       capture_output=True, text=True)
    return json.loads(r.stdout)


def validate_ok(path):
    """hwpx 는 `hwp validate` 통과 여부를, 그 외는 재읽기 성공 여부를 본다."""
    r = subprocess.run([HWP, "validate", path],
                       capture_output=True, text=True)
    return "유효" in r.stdout, (r.stdout + r.stderr).strip().splitlines()[-1:] or [""]


def para_text(p):
    return "".join(c.get("Text", "") for c in p.get("chars", []) if isinstance(c, dict))


def para_has_tab(p):
    """문단에 탭이 있는지 — C15 정규화 이후 탭은 항상 InlineCtrl(code=9)로 저장된다.
    (하위호환: 과거 오염 산출물의 raw Text '\\t'도 함께 인정한다.)"""
    for c in p.get("chars", []):
        if not isinstance(c, dict):
            continue
        if "\t" in c.get("Text", ""):
            return True
        ic = c.get("InlineCtrl")
        if isinstance(ic, dict) and ic.get("code") == 9:
            return True
    return False


def find_para(ir, needle):
    """본문 문단 중 텍스트에 needle 이 든 첫 문단을 돌려준다."""
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            if needle in para_text(p):
                return p
    raise RuntimeError(f"문단 못 찾음: {needle!r}")


def clone_shape(ir, base_id):
    """char_shapes[base_id]를 복제해 새 항목으로 추가하고 (id, dict)를 돌려준다."""
    cs = copy.deepcopy(ir["header"]["char_shapes"][base_id])
    ir["header"]["char_shapes"].append(cs)
    return len(ir["header"]["char_shapes"]) - 1, cs


def run_shapes(ir, p):
    """문단 p 의 char_shape_runs 가 참조하는 (pos, shape_dict) 목록."""
    return [(pos, ir["header"]["char_shapes"][sid]) for pos, sid in p["char_shape_runs"]]


def filetime(dt):
    """aware datetime → FILETIME raw u64(1601-01-01 UTC, 100ns)."""
    epoch = datetime.datetime(1601, 1, 1, tzinfo=datetime.timezone.utc)
    return int((dt - epoch).total_seconds() * 10_000_000)


# 2026-07-15 UTC 근처의 그럴듯한 작성/수정 일시.
CREATE_FT = filetime(datetime.datetime(2026, 7, 15, 9, 0, 0,
                                       tzinfo=datetime.timezone.utc))
MODIFY_FT = filetime(datetime.datetime(2026, 7, 15, 14, 30, 0,
                                       tzinfo=datetime.timezone.utc))

META = {
    "title": "실기 검증 요약정보 문서",
    "author": "홍길동",
    "subject": "글자효과 및 요약정보 검증",
    "keywords": "hwp, 실기검증, 요약정보",
    "description": "C 시리즈 요약정보 검증용 문서입니다.",
    "last_saved_by": "검증 담당자",
    "create_time": CREATE_FT,
    "modify_time": MODIFY_FT,
}


class Fail(Exception):
    pass


def whole(ir, p, shape_id):
    """문단 전체에 shape_id 를 적용."""
    p["char_shape_runs"] = [[0, shape_id]]


def partial(ir, p, base_id, effect_id, idx):
    """문단의 idx 번째 글자에만 effect_id, 나머지는 base_id."""
    p["char_shape_runs"] = [[0, base_id], [idx, effect_id], [idx + 1, base_id]]


# ── D 시리즈 공용: PNG 합성 · 문서 조회 헬퍼 ────────────────────────────────

def _png_chunk(tag, data):
    """PNG 청크(길이+타입+데이터+CRC32)."""
    return (struct.pack(">I", len(data)) + tag + data
            + struct.pack(">I", binascii.crc32(tag + data) & 0xFFFFFFFF))


def make_circle_png(size, rgb):
    """size×size 8bit RGB PNG에 채워진 원(도장용). filter 0 + zlib 압축."""
    r, g, b = rgb
    cx = cy = (size - 1) / 2.0
    rad = size / 2.0
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # 스캔라인 filter: none
        for x in range(size):
            inside = (x - cx) ** 2 + (y - cy) ** 2 <= rad * rad
            raw += bytes((r, g, b)) if inside else bytes((255, 255, 255))
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)  # 8bit, colortype2=RGB
    return (b"\x89PNG\r\n\x1a\n" + _png_chunk(b"IHDR", ihdr)
            + _png_chunk(b"IDAT", zlib.compress(bytes(raw), 9))
            + _png_chunk(b"IEND", b""))


def seal_png():
    """40×40px 빨간 원 도장 PNG를 WORK에 한 번 만들고 경로를 돌려준다."""
    path = os.path.join(WORK, "seal_red_circle.png")
    if not os.path.exists(path):
        with open(path, "wb") as f:
            f.write(make_circle_png(40, (218, 32, 32)))
    return path


def new_base(md_text, tag, ext):
    """마크다운 → base 문서(ext=hwpx/hwp). 경로 반환."""
    md = os.path.join(WORK, f"{tag}.md")
    base = os.path.join(WORK, f"{tag}_base.{ext}")
    with open(md, "w", encoding="utf-8") as f:
        f.write(md_text)
    subprocess.run([HWP, "new", "--from", md, "-o", base],
                   capture_output=True, text=True)
    return base


def find_pictures(ir):
    """본문 문단들의 Picture 컨트롤 목록(도장은 본문 문단 top-level에 놓인다)."""
    pics = []
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            for c in p.get("controls", []):
                if isinstance(c, dict) and "Picture" in c:
                    pics.append(c["Picture"])
    return pics


def doc_has_text(ir, needle):
    """본문 문단 텍스트 어딘가에 needle이 있으면 True."""
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            if needle in para_text(p):
                return True
    return False


# ── 개별 케이스 ────────────────────────────────────────────────────────────

def c1_shadow(dest):
    out = os.path.join(dest, "C1_그림자.hwpx")
    ir = base_ir("그림자 효과 텍스트\n", "c1")
    p = find_para(ir, "그림자 효과")
    base = p["char_shape_runs"][0][1]
    sid, cs = clone_shape(ir, base)
    cs["attr"] |= SHADOW
    cs["shadow_color"] = 0x808080         # 회색
    cs["shadow_gap"] = [5, 5]
    whole(ir, p, sid)
    emit(ir, out, "c1")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    tp = find_para(r, "그림자 효과")
    hit = [c for _, c in run_shapes(r, tp) if c["attr"] & SHADOW]
    if not hit:
        raise Fail("그림자 비트(bit11) 소실")
    if hit[0]["shadow_gap"] == [0, 0]:
        raise Fail("그림자 간격 소실")
    return out, "그림자(attr bit11)+shadow_color=#808080+gap(5,5)"


def c2_outline(dest):
    out = os.path.join(dest, "C2_외곽선.hwpx")
    ir = base_ir("외곽선 효과 텍스트\n", "c2")
    p = find_para(ir, "외곽선 효과")
    base = p["char_shape_runs"][0][1]
    sid, cs = clone_shape(ir, base)
    cs["attr"] |= OUTLINE
    whole(ir, p, sid)
    emit(ir, out, "c2")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    tp = find_para(r, "외곽선 효과")
    if not any(c["attr"] & OUTLINE for _, c in run_shapes(r, tp)):
        raise Fail("외곽선 비트(bit8) 소실")
    return out, "외곽선(attr bit8, SOLID)"


def c3_emboss_engrave(dest):
    out = os.path.join(dest, "C3_양각음각.hwpx")
    ir = base_ir("양각 효과 문단입니다\n\n음각 효과 문단입니다\n", "c3")
    pe = find_para(ir, "양각 효과")
    pg = find_para(ir, "음각 효과")
    se, cse = clone_shape(ir, pe["char_shape_runs"][0][1])
    cse["attr"] |= EMBOSS
    whole(ir, pe, se)
    sg, csg = clone_shape(ir, pg["char_shape_runs"][0][1])
    csg["attr"] |= ENGRAVE
    whole(ir, pg, sg)
    emit(ir, out, "c3")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    if not any(c["attr"] & EMBOSS for _, c in run_shapes(r, find_para(r, "양각 효과"))):
        raise Fail("양각 비트(bit13) 소실")
    if not any(c["attr"] & ENGRAVE for _, c in run_shapes(r, find_para(r, "음각 효과"))):
        raise Fail("음각 비트(bit14) 소실")
    return out, "양각(bit13) 문단 + 음각(bit14) 문단"


def c4_scripts(dest):
    out = os.path.join(dest, "C4_첨자.hwpx")
    # 첫 문단(제목)이 구역정의 컨트롤을 흡수하도록 앞에 제목을 둔다 → 첨자 문단은
    # 순수 텍스트가 되어 글자 인덱스 = WCHAR 위치가 일치한다(부분 구간 정확).
    ir = base_ir("# 첨자 검증\n\nx2의 2가 위첨자\n\nH2O의 2가 아래첨자\n", "c4")
    pu = find_para(ir, "위첨자")
    pd = find_para(ir, "아래첨자")
    # 각 문단에서 첫 '2' 글자에만 첨자(x2 / H2O 의 '2').
    iu = para_text(pu).index("2")
    bu = pu["char_shape_runs"][0][1]
    su, csu = clone_shape(ir, bu)
    csu["attr"] |= SUPER
    partial(ir, pu, bu, su, iu)
    idd = para_text(pd).index("2")
    bd = pd["char_shape_runs"][0][1]
    sd, csd = clone_shape(ir, bd)
    csd["attr"] |= SUB
    partial(ir, pd, bd, sd, idd)
    emit(ir, out, "c4")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    # 위첨자: idx 를 시작으로 하는 run 의 shape 에 bit15.
    tu = find_para(r, "위첨자")
    if not any(pos == iu and c["attr"] & SUPER for pos, c in run_shapes(r, tu)):
        raise Fail("위첨자 비트(bit15) 소실")
    td = find_para(r, "아래첨자")
    if not any(pos == idd and c["attr"] & SUB for pos, c in run_shapes(r, td)):
        raise Fail("아래첨자 비트(bit16) 소실")
    return out, "위첨자(bit15)·아래첨자(bit16) 부분 구간(각 문단의 '2' 글자)"


def c5_underline(dest):
    out = os.path.join(dest, "C5_밑줄모양.hwpx")
    ir = base_ir("점선 밑줄 문단\n\n이중 밑줄 문단\n\n물결 밑줄 문단\n", "c5")
    specs = [("점선 밑줄", UL_DOT), ("이중 밑줄", UL_DOUBLE), ("물결 밑줄", UL_CIRCLE)]
    for needle, shape in specs:
        p = find_para(ir, needle)
        sid, cs = clone_shape(ir, p["char_shape_runs"][0][1])
        cs["attr"] |= UL_BOTTOM
        cs["underline_shape"] = shape
        whole(ir, p, sid)
    emit(ir, out, "c5")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    for needle, shape in specs:
        tp = find_para(r, needle)
        hit = [c for _, c in run_shapes(r, tp)
               if (c["attr"] >> 2) & 3 == 1 and c["underline_shape"] == shape]
        if not hit:
            raise Fail(f"{needle} 밑줄모양({shape}) 소실")
    return out, "밑줄 종류=아래(kind1) + 모양 점선(3)/이중(8)/원형점선(7)"


def c6_numbering(dest):
    out = os.path.join(dest, "C6_번호형식.hwpx")
    ir = base_ir("첫째 조항 문단\n\n둘째 조항 문단\n\n셋째 조항 문단\n", "c6")
    # 사용자 번호형식: "제^1조." (^1=1수준 번호 자리), 시작번호 5.
    ir["header"]["numbering_levels"] = [[
        {"start": 5, "fmt": "Digit", "template": "제^1조."}
    ]]
    # 각 조항 문단 para_shape 에 번호 머리(NUMBER)와 numbering_id 를 표기(IR 의미 보존).
    # numbering_id 는 numbering_levels 인덱스(0-기반). 첫 번호 정의(위 index 0)를 가리키므로 0.
    # (writer 는 idRef=numbering_id+1 로 OWPML 1-기반 id 로 방출하고, reader 가 다시 0-기반으로
    # 정규화한다 — 왕복이 0-기반으로 닫힌다.)
    for needle in ("첫째 조항", "둘째 조항", "셋째 조항"):
        p = find_para(ir, needle)
        src = p["para_shape"]
        np = copy.deepcopy(ir["header"]["para_shapes"][src])
        np["attr1"] = np.get("attr1", 0) | (2 << 23) | (1 << 25)  # head_type=NUMBER, level1
        np["numbering_id"] = 0
        nid = len(ir["header"]["para_shapes"])
        ir["header"]["para_shapes"].append(np)
        p["para_shape"] = nid
    emit(ir, out, "c6")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    # XML 수준 배선 검증: 산출 header.xml의 NUMBER 머리 idRef가 실제 방출된 numbering id를
    # 가리켜야 한다(dangling 금지). 0-기반 numbering_id=0 → idRef=1 → <hh:numbering id="1">.
    with zipfile.ZipFile(out) as z:
        hx = z.read("Contents/header.xml").decode("utf-8")
    num_ids = set(re.findall(r'<hh:numbering id="(\d+)"', hx))
    head_refs = re.findall(r'<hh:heading type="NUMBER" idRef="(\d+)"', hx)
    if not head_refs:
        raise Fail("header.xml에 NUMBER 머리 문단 없음")
    dangling = [ref for ref in head_refs if ref not in num_ids]
    if dangling:
        raise Fail(f"번호 머리 idRef dangling(미정의): {dangling} not in {sorted(num_ids)}")
    r = reread(out)
    lv = r["header"].get("numbering_levels") or [[]]
    if not lv or not lv[0]:
        raise Fail("번호 정의 소실")
    n0 = lv[0][0]
    if n0.get("template") != "제^1조." or n0.get("start") != 5:
        raise Fail(f"번호형식 소실: {n0}")
    # 재읽기에서 각 조항 문단의 heading 링크가 살아 있고, numbering_id 가 사용자 번호
    # 정의('제^1조.')를 실제로 가리키는지(0-기반 인덱스로 해소되는지) 확인한다.
    shapes = r["header"]["para_shapes"]
    for needle in ("첫째 조항", "둘째 조항", "셋째 조항"):
        p = find_para(r, needle)
        ps = shapes[p["para_shape"]]
        head_type = (ps.get("attr1", 0) >> 23) & 0x3
        if head_type != 2:
            raise Fail(f"heading 미표시({needle}): head_type={head_type}")
        nid_para = ps.get("numbering_id", 0)
        linked = lv[nid_para] if nid_para < len(lv) else None
        if not linked or linked[0].get("template") != "제^1조.":
            raise Fail(f"heading numbering 링크 소실({needle}): numbering_id={nid_para}")
    return out, "번호 정의 template='제^1조.' start=5 + 문단 heading 링크(NUMBER, 0-기반 numbering_id) 보존"


def c7_all(dest):
    out = os.path.join(dest, "C7_글자효과통합.hwpx")
    lines = [
        "그림자: 이 문단은 글자 그림자",
        "외곽선: 이 문단은 외곽선",
        "양각: 이 문단은 양각",
        "음각: 이 문단은 음각",
        "위첨자: x2의 2가 위첨자",
        "아래첨자: H2O의 2가 아래첨자",
        "밑줄: 이 문단은 이중 밑줄",
    ]
    ir = base_ir("\n\n".join(lines) + "\n", "c7")

    def w(needle, mut):
        p = find_para(ir, needle)
        sid, cs = clone_shape(ir, p["char_shape_runs"][0][1])
        mut(cs)
        whole(ir, p, sid)

    def sh(cs):
        cs["attr"] |= SHADOW
        cs["shadow_color"] = 0x808080
        cs["shadow_gap"] = [5, 5]
    w("그림자:", sh)
    w("외곽선:", lambda cs: cs.__setitem__("attr", cs["attr"] | OUTLINE))
    w("양각:", lambda cs: cs.__setitem__("attr", cs["attr"] | EMBOSS))
    w("음각:", lambda cs: cs.__setitem__("attr", cs["attr"] | ENGRAVE))

    def ul(cs):
        cs["attr"] |= UL_BOTTOM
        cs["underline_shape"] = UL_DOUBLE
    w("밑줄:", ul)
    # 첨자는 라벨 뒤 'x2'/'H2O'의 '2'만. 라벨 "위첨자: x2..." 에서 '2'는 index 5.
    for needle, bit in (("위첨자:", SUPER), ("아래첨자:", SUB)):
        p = find_para(ir, needle)
        txt = para_text(p)
        # 라벨 뒤 첫 숫자 '2'의 위치.
        idx = txt.index("2")
        base = p["char_shape_runs"][0][1]
        sid, cs = clone_shape(ir, base)
        cs["attr"] |= bit
        partial(ir, p, base, sid, idx)
    emit(ir, out, "c7")

    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    shapes = r["header"]["char_shapes"]
    have = {
        "그림자": any(c["attr"] & SHADOW for c in shapes),
        "외곽선": any(c["attr"] & OUTLINE for c in shapes),
        "양각": any(c["attr"] & EMBOSS for c in shapes),
        "음각": any(c["attr"] & ENGRAVE for c in shapes),
        "위첨자": any(c["attr"] & SUPER for c in shapes),
        "아래첨자": any(c["attr"] & SUB for c in shapes),
        "밑줄": any(((c["attr"] >> 2) & 3) == 1 and c["underline_shape"] != 1
                    for c in shapes),
    }
    missing = [k for k, v in have.items() if not v]
    if missing:
        raise Fail("통합 효과 소실: " + ", ".join(missing))
    return out, "7효과(그림자·외곽선·양각·음각·위첨자·아래첨자·이중밑줄) 문단별"


def c8_summary_hwp(dest):
    out = os.path.join(dest, "C8_요약정보.hwp")
    ir = base_ir("요약정보 검증 본문 문단입니다.\n", "c8")
    ir["metadata"] = dict(META)
    p = emit(ir, out, "c8")
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("hwp 생성 실패: " + (p.stderr.strip()[-160:]))
    r = reread(out)
    m = r.get("metadata", {})
    for k, v in META.items():
        if m.get(k) != v:
            raise Fail(f"metadata.{k} 불일치: {m.get(k)!r} != {v!r}")
    return out, "요약정보 8필드(제목/지은이/주제/키워드/설명/최종저장자/작성·수정 FILETIME)"


def c9_summary_hwpx(dest):
    out = os.path.join(dest, "C9_요약정보.hwpx")
    ir = base_ir("요약정보 검증 본문 문단입니다.\n", "c9")
    ir["metadata"] = dict(META)
    emit(ir, out, "c9")
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    m = r.get("metadata", {})
    # hwpx OPF(content.hpf)는 이제 정품 형식으로 8필드를 모두 방출한다:
    # creator/subject/keyword/description/lastsaveby meta + CreatedDate/ModifiedDate(ISO-8601).
    # 문자열 6필드는 그대로 왕복. FILETIME 2필드는 ISO 초 정밀도라 하위 100ns가 절사되므로
    # 기대값도 초 단위로 내림(FT_PER_SEC=10,000,000 배수)해 비교한다. META의 작성/수정 시각은
    # 이미 초 경계라 절사 후에도 값이 동일하다.
    FT_PER_SEC = 10_000_000
    for k in ("title", "author", "subject", "keywords", "description", "last_saved_by"):
        if m.get(k) != META[k]:
            raise Fail(f"metadata.{k} 불일치: {m.get(k)!r} != {META[k]!r}")
    for k in ("create_time", "modify_time"):
        want = (META[k] // FT_PER_SEC) * FT_PER_SEC  # 초 절사
        if m.get(k) != want:
            raise Fail(f"metadata.{k} 불일치(초 절사): {m.get(k)!r} != {want!r}")
    return out, "요약정보 8필드(제목/지은이/주제/키워드/설명/최종저장자/작성·수정 FILETIME; ISO 초 정밀)"


# ── D 시리즈: 도장 날인(GM-7) · 사용자 탭 정의(GC-4) ────────────────────────

def d1_seal_hwpx(dest):
    out = os.path.join(dest, "D1_도장.hwpx")
    base = new_base("# 결재 문서\n\n결재란: (인)\n", "d1", "hwpx")
    png = seal_png()
    p = subprocess.run(
        [HWP, "edit", base, "-o", out, "--seal", f"(인)=>{png}@18mm"],
        capture_output=True, text=True,
    )
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("edit --seal 실패: " + p.stderr.strip()[-160:])
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    floating = [pic for pic in find_pictures(r) if not pic.get("treat_as_char", True)]
    if not floating:
        raise Fail("부유(treat_as_char=false) Picture 없음")
    if not doc_has_text(r, "(인)"):
        raise Fail("앵커 '(인)' 텍스트 소실")
    return out, "부유 도장(treat_as_char=false, 18mm 빨간원) + 앵커 '(인)' 유지"


def d2_seal_hwp(dest):
    out = os.path.join(dest, "D2_도장.hwp")
    base = new_base("# 결재 문서\n\n결재란: (인)\n", "d2", "hwp")
    png = seal_png()
    p = subprocess.run(
        [HWP, "edit", base, "-o", out, "--seal", f"(인)=>{png}@18mm"],
        capture_output=True, text=True,
    )
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("edit --seal 실패: " + p.stderr.strip()[-160:])
    r = reread(out)
    floating = [pic for pic in find_pictures(r) if not pic.get("treat_as_char", True)]
    if not floating:
        raise Fail("부유(treat_as_char=false) Picture 없음")
    if not doc_has_text(r, "(인)"):
        raise Fail("앵커 '(인)' 텍스트 소실")
    return out, "부유 도장(treat_as_char=false, 18mm 빨간원) + 앵커 '(인)' 유지(hwp5)"


def d3_tab_hwpx(dest):
    out = os.path.join(dest, "D3_사용자탭.hwpx")
    # 탭 문자가 든 본문 문단(마크다운 텍스트의 \t가 우리 리더에서 보존됨).
    ir = base_ir("# 사용자 탭 검증\n\n이름\t직책\t서명\n", "d3")
    tgt = None
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            if para_has_tab(p):
                tgt = p
                break
        if tgt:
            break
    if tgt is None:
        raise Fail("탭 문자가 든 문단을 만들지 못함")

    # 새 탭 정의: 왼쪽 30mm(≈8504 HWPUNIT)·가운데 80mm(≈22677), 채움 대시(DASH=2).
    # 정품 한글이 저장한 hwpx에서 관찰된 leader 값은 NONE/DASH뿐이라 DASH로 방출한다.
    tab_stops = ir["header"].setdefault("tab_stops", [])
    tab_id = len(tab_stops)
    items = [
        {"pos": 8504, "kind": 0, "fill": 2},    # 왼쪽 탭, 대시
        {"pos": 22677, "kind": 2, "fill": 2},   # 가운데 탭, 대시
    ]
    tab_stops.append({"attr": 0, "items": items})

    # 대상 문단 para_shape를 복제해 tab_def_id로 새 탭 정의를 참조.
    np = copy.deepcopy(ir["header"]["para_shapes"][tgt["para_shape"]])
    np["tab_def_id"] = tab_id
    nid = len(ir["header"]["para_shapes"])
    ir["header"]["para_shapes"].append(np)
    tgt["para_shape"] = nid

    emit(ir, out, "d3")
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")

    # 산출 header.xml 배선 확인: 정품 구조(hp:switch/case[unit=HWPUNIT,pos=X]/
    # default[pos=2X])로 감싼 tabItem을 가진 tabPr가 있고, 그 id를 참조하는 paraPr가 있어야.
    # naked tabItem은 한글 먹통 원인이므로 방출되면 안 된다.
    with zipfile.ZipFile(out) as z:
        hx = z.read("Contents/header.xml").decode("utf-8")
    m = re.search(
        r'<hh:tabPr id="(\d+)"[^>]*>'
        r'(?:<hp:switch><hp:case hp:required-namespace="[^"]*HwpUnitChar">'
        r'<hh:tabItem[^>]*unit="HWPUNIT"/></hp:case>'
        r'<hp:default><hh:tabItem[^>]*/></hp:default></hp:switch>)+'
        r'</hh:tabPr>',
        hx,
    )
    if not m:
        raise Fail("header.xml에 switch로 감싼 tabItem을 가진 tabPr 없음")
    wired_id = m.group(1)
    # naked tabItem(switch 밖 직속) 방출 금지 확인: hp:switch 블록을 제거한 뒤에도
    # tabItem이 남아 있으면 case/default가 아닌 곳(먹통 원인)에 방출된 것이다.
    if "<hh:tabItem" in re.sub(r"<hp:switch>.*?</hp:switch>", "", hx, flags=re.S):
        raise Fail("naked tabItem(먹통 원인) 방출됨")
    if not re.search(rf'<hh:paraPr id="\d+" tabPrIDRef="{wired_id}"', hx):
        raise Fail(f"tabPrIDRef={wired_id} 배선된 paraPr 없음")

    # 본문 탭 방출 형식(핵심): 탭은 <hp:t> **안**의 중첩 <hp:tab width leader type/>로
    # 나와야 한다(정품 mixed content). t 밖 형제 bare 탭은 한글이 폭 0으로 무시한다(D3
    # 밀착 결함의 원인). 속성은 문단 탭 정의에서 유도: 항목0=왼쪽(kind0)/DASH(fill2)→
    # type="1" leader="3", 항목1=가운데(kind2)/DASH(fill2)→ type="3" leader="3".
    with zipfile.ZipFile(out) as z:
        sx = z.read("Contents/section0.xml").decode("utf-8")
    tabs = re.findall(r"<hp:tab\b[^/]*/>", sx)
    if len(tabs) != 2:
        raise Fail(f"본문 탭 2개가 방출되지 않음: {len(tabs)}개")
    for t in tabs:
        for a in ("width", "leader", "type"):
            if f'{a}="' not in t:
                raise Fail(f"본문 <hp:tab>에 {a} 속성 없음: {t}")
    # t 밖 bare 탭 금지: <hp:t>…</hp:t> 블록을 지운 뒤에도 <hp:tab이 남으면 형제로 방출된 것.
    if "<hp:tab" in re.sub(r"<hp:t\b.*?</hp:t>", "", sx, flags=re.S):
        raise Fail("bare <hp:tab>(hp:t 밖 형제 — 한글 무시 원인) 방출됨")
    if 'leader="3" type="1"' not in tabs[0] or 'leader="3" type="3"' not in tabs[1]:
        raise Fail(f"탭 종류/채움 유도 불일치(왼쪽·가운데 대시 기대): {tabs}")

    # 재읽기 round-trip: 본문 탭이 InlineCtrl(9)로 복원되는지 + case값만 취해 중복 없이
    # 탭 정의의 pos/kind/fill 보존.
    r = reread(out)
    n_tabs = sum(
        1
        for sec in r["sections"]
        for p in sec["paragraphs"]
        for c in p.get("chars", [])
        if isinstance(c, dict)
        and isinstance(c.get("InlineCtrl"), dict)
        and c["InlineCtrl"].get("code") == 9
    )
    if n_tabs != 2:
        raise Fail(f"재읽기 본문 탭(InlineCtrl 9) 2개 아님(중첩 탭 읽기 실패?): {n_tabs}")
    mine = [td for td in r["header"].get("tab_stops", []) if td.get("items")]
    if not mine:
        raise Fail("재읽기 tab_stops에 사용자 탭 정의 소실")
    got = mine[0]["items"]
    if len(got) != len(items):
        raise Fail(f"재읽기 항목 수 불일치(default 중복 수집?): {len(got)} != {len(items)}")
    if [(i["pos"], i["kind"], i["fill"]) for i in got] != \
       [(i["pos"], i["kind"], i["fill"]) for i in items]:
        raise Fail(f"탭 항목 pos/kind/fill 불일치: {got}")
    return out, "사용자 탭 정의(왼쪽30mm=8504·가운데80mm=22677, 채움 대시) + 문단 tabPrIDRef 배선(hp:switch 정품 구조) + 본문 탭 hp:t 안 중첩 방출(type/leader 유도)"


# ── H 시리즈: markdown 왕복(각주·취소선·순서/중첩 목록) ─────────────────────
# GI-1/GI-2: markdown 들여오기가 각주/취소선/순서·중첩 목록을 IR로 재구성하고
# hwpx/hwp5 쓰기가 이를 수용하는지 실기 파일로 확인한다.

# 각주 1개 + 취소선 + 순서목록(3항목, start=1) + 중첩 글머리 목록이 든 검증 md.
H_MD = (
    "문단에 각주[^1]가 있다.\n\n"
    "~~지운 글~~ 과 보통 글.\n\n"
    "1. 첫째\n"
    "2. 둘째\n"
    "   - 안쪽 가\n"
    "   - 안쪽 나\n"
    "3. 셋째\n\n"
    "[^1]: 각주 본문이다.\n"
)


def _ctrl_id_bytes(cid):
    """JSON ctrl_id(정수 배열 또는 문자열)를 bytes로 정규화."""
    return cid.encode("latin-1") if isinstance(cid, str) else bytes(cid)


def _has_footnote(ir):
    """본문에 fn/en GenericControl(본문 문단 리스트 포함)이 있는가."""
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            for c in p.get("controls", []):
                if isinstance(c, dict) and "Generic" in c:
                    g = c["Generic"]
                    if _ctrl_id_bytes(g.get("ctrl_id")) in (b"fn  ", b"en  ") \
                            and g.get("paragraph_lists"):
                        return True
    return False


def _has_strike_run(ir):
    """어떤 char_shape_run이 strike=true 문자모양을 실제로 참조하는가."""
    strike_ids = {i for i, c in enumerate(ir["header"]["char_shapes"])
                  if c.get("strike")}
    for sec in ir["sections"]:
        for p in sec["paragraphs"]:
            for _, sid in p.get("char_shape_runs", []):
                if sid in strike_ids:
                    return True
    return False


def _list_heads(ir):
    """목록 문단모양의 (head_type, numbering_id) 목록(2=번호,3=글머리)."""
    out = []
    for c in ir["header"]["para_shapes"]:
        ht = (c.get("attr1", 0) >> 23) & 3
        if ht in (2, 3):
            out.append((ht, c.get("numbering_id", 0)))
    return out


def _write_md(md_text, tag, out):
    """md_text → hwp new --from → out. 실패면 Fail."""
    md = os.path.join(WORK, f"{tag}.md")
    with open(md, "w", encoding="utf-8") as f:
        f.write(md_text)
    p = subprocess.run([HWP, "new", "--from", md, "-o", out],
                       capture_output=True, text=True)
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("hwp new 실패: " + p.stderr.strip()[-160:])


def h1_md_hwpx(dest):
    out = os.path.join(dest, "H1_md왕복.hwpx")
    _write_md(H_MD, "h1", out)
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    if not _has_footnote(r):
        raise Fail("각주 컨트롤(fn) 소실")
    if not _has_strike_run(r):
        raise Fail("취소선(strike) run 소실")
    heads = _list_heads(r)
    if not any(ht == 2 for ht, _ in heads):
        raise Fail("순서목록(NUMBER) 머리 소실")
    if not any(ht == 3 for ht, _ in heads):
        raise Fail("중첩 글머리(BULLET) 머리 소실")
    return out, "md 왕복: 각주(fn)+취소선(strike run)+순서목록(NUMBER)+중첩 글머리(BULLET) 보존"


def h2_md_hwp(dest):
    out = os.path.join(dest, "H2_md왕복.hwp")
    _write_md(H_MD, "h2", out)
    r = reread(out)
    if not _has_footnote(r):
        raise Fail("각주 컨트롤(fn) 소실")
    heads = _list_heads(r)
    if not any(ht == 2 for ht, _ in heads):
        raise Fail("순서목록(NUMBER) 머리 소실")
    if not any(ht == 3 for ht, _ in heads):
        raise Fail("중첩 글머리(BULLET) 머리 소실")
    # 번호/글머리 정의 참조가 dangling 아님(defect A10).
    nums = len(r["header"].get("numberings", []))
    buls = len(r["header"].get("bullets", []))
    for ht, nid in heads:
        if ht == 2 and nid >= nums:
            raise Fail(f"번호 정의 dangling: {nid} >= {nums}")
        if ht == 3 and nid >= buls:
            raise Fail(f"글머리 정의 dangling: {nid} >= {buls}")
    if not doc_has_text(r, "지운 글"):
        raise Fail("취소선 텍스트 소실")
    # 취소선: hwp5는 CHAR_SHAPE 속성 bit18(취소선 여부=1)로 기록한다. 리더는 bit18을
    # 신뢰하지 않아(변경추적 오검 회피) strike 플래그는 false지만, 한글은 bit18로 렌더한다.
    # → strike run이 실제로 있는지 bit18로 단언(⚠ 한글 수용은 H2 실기로 최종 확정).
    shapes = r["header"]["char_shapes"]
    strike_ids = {i for i, c in enumerate(shapes) if (c.get("attr", 0) >> 18) & 1}
    if not strike_ids:
        raise Fail("취소선 CharShape(속성 bit18) 소실")
    has_strike_run = any(
        sid in strike_ids
        for sec in r["sections"]
        for p in sec["paragraphs"]
        for _, sid in p.get("char_shape_runs", [])
    )
    if not has_strike_run:
        raise Fail("취소선 run(bit18 참조) 소실")
    # BULLET 레코드가 정품 필드 패턴과 일치하는지(바이트 수준, 사업계획서 전수 대조):
    # 25B, [8..12]=글자모양 id 없음(ffffffff), [12..14]=글머리표 문자('•'=0x2022).
    # 오프셋이 어긋나면(과거 20B/오프셋8 합성) 한글이 마커를 미표시한다(1차 H2 결함).
    bullets = r["header"].get("bullets") or []
    if not bullets:
        raise Fail("BULLET 레코드 소실")
    data = bytes.fromhex(bullets[0]["data"])
    if len(data) != 25:
        raise Fail(f"BULLET 레코드 길이 {len(data)}B != 정품 25B")
    if data[8:12] != b"\xff\xff\xff\xff":
        raise Fail(f"BULLET 글자모양 id 필드(8..12) 불일치: {data[8:12].hex()}")
    ch = int.from_bytes(data[12:14], "little")
    if ch != 0x2022:
        raise Fail(f"BULLET 글머리표 문자(오프셋12)가 '•' 아님: {hex(ch)}")
    return out, ("md 왕복: 각주(fn)+순서/글머리 머리(정의 non-dangling)+취소선 run(속성 bit18) "
                 "+ BULLET 25B 정품 패턴(문자 오프셋12) (취소선 bit18 한글수용은 실기 확정 대상)")


# ── I 시리즈: markdown 이미지·인라인 코드(GI-3·GI-4) ─────────────────────────

def i1_md_image_code(dest):
    out = os.path.join(dest, "I1_md이미지코드.hwpx")
    # 이미지: 재사용 가능한 40×40 빨간 원 PNG. 인라인 코드: `let x = 1;`.
    png_bytes = open(seal_png(), "rb").read()
    fig = os.path.join(WORK, "i1_fig.png")
    with open(fig, "wb") as f:
        f.write(png_bytes)
    md = os.path.join(WORK, "i1.md")
    with open(md, "w", encoding="utf-8") as f:
        f.write("# 이미지·코드\n\n본문에 `let x = 1;` 인라인 코드.\n\n![그림설명](i1_fig.png)\n")
    p = subprocess.run([HWP, "new", "--from", md, "-o", out],
                       capture_output=True, text=True)
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("hwp new 실패: " + p.stderr.strip()[-160:])
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")
    r = reread(out)
    # 이미지: Picture 존재.
    if not find_pictures(r):
        raise Fail("Picture 소실")
    # 인라인 코드: 함초롬돋움(face_ids[0]=1) 글자모양 + 그걸 참조하는 run.
    shapes = r["header"]["char_shapes"]
    code_ids = {i for i, c in enumerate(shapes) if (c.get("face_ids") or [0])[0] == 1}
    if not code_ids:
        raise Fail("코드 글자모양(함초롬돋움 face=1) 소실")
    has_code_run = any(
        sid in code_ids
        for sec in r["sections"]
        for pp in sec["paragraphs"]
        for _, sid in pp.get("char_shape_runs", [])
    )
    if not has_code_run:
        raise Fail("코드 run(face=1 참조) 소실")
    if r["header"]["fonts"][0][1].get("name") != "함초롬돋움":
        raise Fail("함초롬돋움 글꼴 테이블 배선 소실")
    # 이미지 바이트 일치: --embed-bin으로 base64 임베드해 원본과 대조(무손실 확인).
    ej = os.path.join(WORK, "i1_embed.json")
    subprocess.run([HWP, "convert", out, "-o", ej, "--embed-bin"],
                   capture_output=True, text=True)
    bs = json.load(open(ej, encoding="utf-8")).get("bin_streams", [])
    if not bs or not bs[0].get("data_b64"):
        raise Fail("임베드 bin 데이터 없음")
    got = base64.b64decode(bs[0]["data_b64"])
    if got != png_bytes:
        raise Fail(f"이미지 바이트 불일치: {len(got)}B != 원본 {len(png_bytes)}B")
    return out, "md 이미지(인라인 Picture+bin 무손실)+인라인 코드(함초롬돋움 face run)"


# ── J 시리즈: 쪽 테두리 교차변환 + 렌더(GC-2·GC-3) ───────────────────────────
# 정답지(11.19 제안요청서 hwp)를 우리 hwpx로 변환한 실기본. 세 층위로 검증한다:
#   ① validate 통과, ② 교차변환이 실테두리를 승계했는가(pageBorderFill BOTH의
#      borderFillIDRef가 '빈 테두리'(id1)가 아니라 실선 BORDER_FILL을 가리키는가),
#   ③ 우리 렌더가 이 문서의 쪽 테두리를 실제로 그리는가(원본 hwp5 PNG의 종이
#      가장자리 4변에 연속 잉크선이 있는가 — 픽셀 검사).
# 주의: 렌더 잉크 검사는 **원본 hwp5**를 대상으로 한다. 쪽 테두리 렌더는 hwp5
# 읽기 경로(page_border_fills_raw, 정품 실측 확정)로만 구동되고, hwpx 읽기는
# 이 필드를 채우지 않기(pass-through=secpr_raw_children) 때문이다. 산출물(hwpx)은
# ①②로 구조 검증하고, 렌더 정합은 같은 문서의 hwp5 원본으로 확인한다.
# 이 파일은 정답지 유래이므로 산출물만 dest(~/Documents/hwp-실기검증)에 두고,
# 원본·중간 PNG는 저장소·fixtures에 남기지 않는다(PNG는 WORK에만 쓴다).


class Skip(Exception):
    """정답지 코퍼스 입력이 없을 때 케이스를 건너뛴다(⏭)."""


def find_page_border_src():
    """11.19 제안요청서 hwp(쪽 테두리 실테두리 표본)를 코퍼스에서 찾는다."""
    root = os.path.expanduser("~/Documents/hwp_samples")
    if not os.path.isdir(root):
        return None
    for dirpath, _dirs, files in os.walk(root):
        for fn in files:
            if (fn.endswith(".hwp") and fn.startswith("제안요청서")
                    and "11.19" in fn):
                return os.path.join(dirpath, fn)
    return None


def _repo_font_dir():
    """HWP 바이너리 경로에서 저장소 fonts/ 디렉터리를 유도(없으면 None)."""
    # HWP = <repo>/target/{debug,release}/hwp → repo = dirname×3.
    repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(HWP))))
    fd = os.path.join(repo, "fonts")
    return fd if os.path.isdir(fd) else None


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    return a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)


def decode_png_rgba(path):
    """8bit RGBA(color type 6) PNG를 (w, h, bytearray)로 디코드(stdlib만)."""
    d = open(path, "rb").read()
    if d[:8] != b"\x89PNG\r\n\x1a\n":
        raise Fail("PNG 시그니처 불일치")
    w = h = None
    idat = bytearray()
    o = 8
    while o < len(d):
        ln = struct.unpack(">I", d[o:o + 4])[0]
        typ = d[o + 4:o + 8]
        chunk = d[o + 8:o + 8 + ln]
        if typ == b"IHDR":
            w, h, bd, ct = struct.unpack(">IIBB", chunk[:10])
            if bd != 8 or ct != 6:
                raise Fail(f"PNG 형식 미지원(bd={bd} ct={ct})")
        elif typ == b"IDAT":
            idat += chunk
        elif typ == b"IEND":
            break
        o += 12 + ln
    raw = zlib.decompress(bytes(idat))
    stride = w * 4
    out = bytearray(w * h * 4)
    pos = 0
    for y in range(h):
        ft = raw[pos]
        pos += 1
        base = y * stride
        for x in range(stride):
            v = raw[pos + x]
            a = out[base + x - 4] if x >= 4 else 0
            b = out[base - stride + x] if y > 0 else 0
            c = out[base - stride + x - 4] if (y > 0 and x >= 4) else 0
            if ft == 0:
                r = v
            elif ft == 1:
                r = (v + a) & 255
            elif ft == 2:
                r = (v + b) & 255
            elif ft == 3:
                r = (v + ((a + b) >> 1)) & 255
            else:
                r = (v + _paeth(a, b, c)) & 255
            out[base + x] = r
        pos += stride
    return w, h, out


def _is_dark(px, i):
    """흰 배경 알파합성 후 luma<128이면 잉크(어두움)."""
    r, g, b, a = px[i], px[i + 1], px[i + 2], px[i + 3]
    luma = (r * 0.299 + g * 0.587 + b * 0.114) * a / 255 + 255 * (1 - a / 255)
    return luma < 128


def page_edge_ink(path, band=40, thresh=0.5):
    """4변(위/아래/왼/오)에서 가장자리 band픽셀 안에 '연속 잉크선'이 있는지.
    각 변에서 폭(높이)의 thresh 비율 이상이 어두운 행(열)이 하나라도 있으면 True.
    글자 획은 변의 극히 일부만 덮으므로 통과하지 못하고, 테두리 선만 잡힌다."""
    w, h, px = decode_png_rgba(path)

    def row_frac(y):
        return sum(_is_dark(px, (y * w + x) * 4) for x in range(w)) / w

    def col_frac(x):
        return sum(_is_dark(px, (y * w + x) * 4) for y in range(h)) / h

    top = max(row_frac(y) for y in range(0, band))
    bot = max(row_frac(y) for y in range(h - band, h))
    left = max(col_frac(x) for x in range(0, band))
    right = max(col_frac(x) for x in range(w - band, w))
    return {
        "위": top >= thresh, "아래": bot >= thresh,
        "왼": left >= thresh, "오": right >= thresh,
    }, (top, bot, left, right)


def _border_side_types(hx, bf_id):
    """header.xml에서 borderFill id의 4변(left/right/top/bottom) type 목록."""
    m = re.search(rf'<hh:borderFill id="{bf_id}".*?</hh:borderFill>', hx, re.S)
    if not m:
        return None
    block = m.group(0)
    types = []
    for side in ("leftBorder", "rightBorder", "topBorder", "bottomBorder"):
        sm = re.search(rf'<hh:{side} type="([^"]+)"', block)
        types.append(sm.group(1) if sm else "NONE")
    return types


def j1_page_border(dest):
    out = os.path.join(dest, "J1_쪽테두리.hwpx")
    src = find_page_border_src()
    if not src:
        raise Skip("정답지 11.19 제안요청서 hwp 없음(코퍼스 미보유)")

    # ① 교차변환 + validate.
    p = subprocess.run([HWP, "convert", src, "-o", out],
                       capture_output=True, text=True)
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        raise Fail("convert 실패: " + p.stderr.strip()[-160:])
    ok, _ = validate_ok(out)
    if not ok:
        raise Fail("validate 실패")

    # ② 실테두리 승계: section0.xml의 pageBorderFill(BOTH/EVEN/ODD) borderFillIDRef가
    #    header.xml에서 실제 BORDER_FILL로 해소되고, BOTH는 실선(NONE 아님)·EVEN/ODD는
    #    빈 테두리(전 변 NONE)여야 한다 — 홀짝 빈 테두리 vs 본문 실선 구분 승계 확인.
    with zipfile.ZipFile(out) as z:
        sx = z.read("Contents/section0.xml").decode("utf-8")
        hx = z.read("Contents/header.xml").decode("utf-8")
    refs = dict(re.findall(
        r'<hp:pageBorderFill type="(BOTH|EVEN|ODD)" borderFillIDRef="(\d+)"', sx))
    for t in ("BOTH", "EVEN", "ODD"):
        if t not in refs:
            raise Fail(f"pageBorderFill {t} 소실")
    both_types = _border_side_types(hx, refs["BOTH"])
    if both_types is None:
        raise Fail(f"BOTH borderFillIDRef={refs['BOTH']} dangling(BORDER_FILL 미정의)")
    solid = [t for t in both_types if t != "NONE"]
    if not solid:
        raise Fail(f"BOTH 테두리 실선 없음(전 변 NONE): id={refs['BOTH']}")
    even_types = _border_side_types(hx, refs["EVEN"])
    if even_types is None:
        raise Fail(f"EVEN borderFillIDRef={refs['EVEN']} dangling")
    if any(t != "NONE" for t in even_types):
        raise Fail(f"EVEN이 빈 테두리가 아님(승계 구분 실패): {even_types}")

    # ③ 렌더 잉크 검사: 원본 hwp5를 렌더해 종이 4변에 연속 테두리선이 있는지.
    png = os.path.join(WORK, "j1_border.png")
    rargs = [HWP, "render", src, "-o", png, "--pages", "1", "--dpi", "96"]
    fd = _repo_font_dir()
    if fd:
        rargs += ["--font-dir", fd]
    rp = subprocess.run(rargs, capture_output=True, text=True)
    if not os.path.exists(png) or os.path.getsize(png) == 0:
        raise Fail("render 실패: " + rp.stderr.strip()[-160:])
    edges, fracs = page_edge_ink(png)
    missing = [k for k, v in edges.items() if not v]
    if missing:
        raise Fail(f"쪽 테두리 잉크 없는 변: {missing} (변별 어두움비율 위/아래/왼/오="
                   + "/".join(f"{f:.2f}" for f in fracs) + ")")

    # ④ 페이지 걸침 표 분할 충실도(GE-8): 4쪽 "세부 과업내용"(26행 3열, "해커톤")·
    #    6쪽 "주차/교육내용/비고"(6행 3열, "주차")가, 사용자가 한글에서 저장한
    #    정답지(표분할정답지.hwpx) 실측값과 일치해야 한다:
    #    (a) treatAsChar="0"(부유) — 글자처럼("1")이면 표가 "한 글자"로 배치돼 페이지
    #        분할이 불가하고 하단을 관통한다(정답지 직대조로 확정된 GE-8 진범).
    #    (b) hp:sz width×height = 원본 개체 공통 속성 높이(행높이 합산 재계산 금지).
    #    (c) pageBreak="CELL" 유지, (d) 각 셀에 줄 배치(linesegarray) 존재.
    #    상수는 정답지 파일 실측(코퍼스 유래이므로 파일 비동봉 — 값만 단언).
    def _find_tbl(rows, cols, kw):
        for m in re.finditer(r'<hp:tbl .*?</hp:tbl>', sx, re.S):
            b = m.group(0)
            if (re.search(rf'\browCnt="{rows}"', b)
                    and re.search(rf'\bcolCnt="{cols}"', b) and kw in b):
                return b
        return None
    for label, rows, kw, w_exp, h_exp in (
            ("세부 과업내용(4쪽)", 26, "해커톤", 47339, 54801),
            ("주차/교육내용/비고(6쪽)", 6, "주차", 47622, 17021)):
        blk = _find_tbl(rows, 3, kw)
        if blk is None:
            raise Fail(f"페이지 걸침 표 미발견: {label} ({rows}행 3열 '{kw}')")
        pb = re.search(r'pageBreak="([^"]*)"', blk).group(1)
        if pb != "CELL":
            raise Fail(f"{label} pageBreak={pb}(정답지 CELL 기대)")
        pos = re.search(r'<hp:pos [^>]*/>', blk).group(0)
        tc = re.search(r'treatAsChar="(\d)"', pos).group(1)
        if tc != "0":
            raise Fail(f"{label} treatAsChar={tc}(정답지 0=부유 기대 — 1이면 페이지 분할 불가)")
        szm = re.search(r'<hp:sz width="(\d+)"[^>]*?height="(\d+)"', blk)
        w, h = int(szm.group(1)), int(szm.group(2))
        if (w, h) != (w_exp, h_exp):
            raise Fail(f"{label} sz={w}×{h}(정답지 {w_exp}×{h_exp} 기대 — 원본 개체 높이 승계)")
        if "<hp:linesegarray>" not in blk:
            raise Fail(f"{label} 표 셀 줄 배치(linesegarray) 없음 — 한글이 셀 분할 불가")

    return out, (f"교차변환 실테두리 승계(BOTH id={refs['BOTH']} 실선 {len(solid)}변 / "
                 f"EVEN·ODD id={refs['EVEN']} 빈테두리) + 렌더 종이 4변 연속 잉크선"
                 f"(위/아래/왼/오={'/'.join(f'{f:.2f}' for f in fracs)}) + "
                 f"페이지 걸침 표 2개 정답지 정합(treatAsChar=0·sz·CELL·줄배치, GE-8)")


# ── K 시리즈: 표 셀 병합/분할·열 조작 (GK-1·GK-2) ──────────────────────────
# base 표를 markdown으로 만든 뒤 `hwp edit` 구조 편집을 적용하고, 재읽기 구조 단언 +
# validate(hwpx)로 자체 검증한다. 병합 표 생성은 새 조합이라 한글 실기가 차단성이다.

def _first_table(ir):
    """본문(및 셀 중첩) 첫 표 dict를 재귀로 찾는다."""
    def walk(node):
        if isinstance(node, dict):
            t = node.get("Table")
            if isinstance(t, dict) and "cells" in t:
                return t
            for v in node.values():
                r = walk(v)
                if r:
                    return r
        elif isinstance(node, list):
            for x in node:
                r = walk(x)
                if r:
                    return r
        return None
    t = walk(ir.get("sections", []))
    if not t:
        raise RuntimeError("표 없음")
    return t


def _assert_table_invariants(t):
    """정답지 실측 불변식: Σ(col_span×row_span)=rows×cols, row_cell_counts 정합."""
    area = sum(c.get("col_span", 1) * c.get("row_span", 1) for c in t["cells"])
    full = t["rows"] * t["cols"]
    assert area == full, f"면적 합 {area} != rows×cols {full}"
    counts = [0] * t["rows"]
    for c in t["cells"]:
        counts[c["row"]] += 1
    assert counts == t["row_cell_counts"], \
        f"row_cell_counts {t['row_cell_counts']} != 계산 {counts}"


def _new_table(tag, ext, rows_cells):
    """rows_cells(첫 행=헤더) → GFM 표 markdown → `hwp new` → base 표 파일."""
    lines = ["| " + " | ".join(rows_cells[0]) + " |",
             "|" + "|".join(["---"] * len(rows_cells[0])) + "|"]
    for row in rows_cells[1:]:
        lines.append("| " + " | ".join(row) + " |")
    md = os.path.join(WORK, f"{tag}.md")
    with open(md, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    base = os.path.join(WORK, f"{tag}_base.{ext}")
    p = subprocess.run([HWP, "new", "--from", md, "-o", base],
                       capture_output=True, text=True)
    if not os.path.exists(base):
        raise RuntimeError(f"base 표 생성 실패: {p.stderr.strip()}")
    return base


def _edit(base, out, *ops):
    p = subprocess.run([HWP, "edit", base, "-o", out, *ops],
                       capture_output=True, text=True)
    if not os.path.exists(out):
        raise RuntimeError(f"edit 실패({' '.join(ops)}): {p.stderr.strip()}")
    return p


def k1_merge_hwpx(dest):
    base = _new_table("k1", "hwpx", [["A", "B"], ["C", "D"]])
    out = os.path.join(dest, "K1_셀병합.hwpx")
    _edit(base, out, "--merge-cells", "0:0:0:0:1")
    t = _first_table(reread(out))
    a = next(c for c in t["cells"] if c["row"] == 0 and c["col"] == 0)
    assert a["col_span"] == 2, f"col_span={a['col_span']}"
    assert len(t["cells"]) == 3, f"셀 {len(t['cells'])}개(3 기대)"
    _assert_table_invariants(t)
    ok, msg = validate_ok(out)
    if not ok:
        raise RuntimeError(f"validate 실패: {msg}")
    return out, "2×2 상단 병합 → 앵커 col_span=2, 셀 3개, validate ✅"


def k2_merge_hwp(dest):
    base = _new_table("k2", "hwp", [["A", "B"], ["C", "D"]])
    out = os.path.join(dest, "K2_셀병합.hwp")
    _edit(base, out, "--merge-cells", "0:0:0:0:1")
    ir = reread(out)
    t = _first_table(ir)
    a = next(c for c in t["cells"] if c["row"] == 0 and c["col"] == 0)
    assert a["col_span"] == 2, f"col_span={a['col_span']}"
    assert len(t["cells"]) == 3, f"셀 {len(t['cells'])}개(3 기대)"
    _assert_table_invariants(t)
    # hwp5 합성 게이트: 재읽기 후 병합 앵커 셀에 두 셀 내용(A·B)이 보존돼야.
    anchor_text = "".join(para_text(p) for p in a["paragraphs"])
    if "A" not in anchor_text or "B" not in anchor_text:
        raise RuntimeError(f"병합 내용(A·B) 보존 실패: 앵커 텍스트 {anchor_text!r}")
    # 정품 패턴: 셀 세로 정렬 = 가운데(list_attr bits5-6 == 1). hwp5 writer가 그대로
    # 방출하므로, 0(위)이면 셀 내용이 상단에 붙는다(위 여백<아래 여백).
    bad = [c for c in t["cells"] if (c["list_attr"] >> 5) & 3 != 1]
    if bad:
        raise RuntimeError(f"셀 세로 정렬이 가운데(0x20)가 아님: {[hex(c['list_attr']) for c in bad]}")
    return out, "2×2 상단 병합(hwp5 합성 경로) → col_span=2, 내용 보존, 셀 세로정렬=가운데(0x20), 재읽기 ✅"


def k3_cols_hwpx(dest):
    base = _new_table("k3", "hwpx", [["A", "B", "C"], ["D", "E", "F"]])
    out = os.path.join(dest, "K3_열조작.hwpx")
    # 위치1에 열 추가(3→4열) 후 열3 삭제(4→3열, 원래 C열 제거).
    _edit(base, out, "--add-col", "0:1", "--delete-col", "0:3")
    t = _first_table(reread(out))
    assert t["cols"] == 3, f"cols={t['cols']}(3 기대)"
    assert len(t["cells"]) == 6, f"셀 {len(t['cells'])}개(6 기대)"
    assert all(c["col_span"] == 1 and c["row_span"] == 1 for c in t["cells"]), "병합 없음"
    _assert_table_invariants(t)
    ok, msg = validate_ok(out)
    if not ok:
        raise RuntimeError(f"validate 실패: {msg}")
    return out, "3열 표 열 추가+삭제 → 순 3열·6셀, validate ✅"


# ── L 시리즈: 수식 방출 (GE-14·GE-15) ───────────────────────────────────────
# hwpx writer 의 <hp:equation> arm 은 정답지(수식 든 정품 hwpx) 없이 표준 추정 속성으로
# 방출한다. 한글이 이 수식을 열어 실제로 조판하는지가 유일한 최종 판정이라 실기 케이스를 둔다.

EQED_ID = [101, 113, 101, 100]          # b"eqed"
EQED_PAYLOAD = "646571650000000000000000"  # 역순 ctrl_id("deqe") + 0 패딩 12B


def _attach_equation(p, script, width=4000, height=1300, inline=True):
    """문단 p 끝에 수식 컨트롤(ExtCtrl 코드 11 + Generic.equation)을 단다."""
    controls = p.setdefault("controls", [])
    p["chars"].append({"ExtCtrl": {
        "code": 11, "ctrl_id": EQED_ID, "payload": EQED_PAYLOAD,
        "ctrl_index": len(controls),
    }})
    controls.append({"Generic": {
        "ctrl_id": EQED_ID, "data": "", "paragraph_lists": [], "extras": [],
        "equation": {"script": script, "width": width, "height": height,
                     "inline": inline, "x": 0, "y": 0},
    }})


def l1_equation_hwpx(dest):
    # 분수·근호·합·첨자 + XML 특수문자(<, &, >) — esc() ↔ 엔티티 해석의 짝을 실기로 검증.
    scripts = [
        "a over b + sqrt {x^2 + y^2}",
        "sum from {i=1} to {n} i = {n(n+1)} over 2",
        "x < y & y > z",
    ]
    ir = base_ir(
        "# 수식 실기 검증\n\n아래 문단 끝에 수식 3개가 글자처럼 들어간다.\n\n"
        "수식 자리:\n\n분수·근호·합기호와 부등호(<)·앰퍼샌드(&)가 모두 보여야 한다.\n",
        "l1",
    )
    p = find_para(ir, "수식 자리")
    for s in scripts:
        _attach_equation(p, s)
    out = os.path.join(dest, "L1_수식.hwpx")
    r = emit(ir, out, "l1")
    if not os.path.exists(out):
        raise RuntimeError(f"생성 실패: {r.stderr.strip()[:200]}")
    # 재읽기 단언: 3개가 다 살아 있고 스크립트 원문(특수문자 포함)이 그대로여야 한다.
    got = [c["Generic"]["equation"]
           for sec in reread(out)["sections"] for q in sec["paragraphs"]
           for c in q.get("controls", [])
           if isinstance(c, dict) and "Generic" in c and c["Generic"].get("equation")]
    if len(got) != len(scripts):
        raise RuntimeError(f"수식 {len(got)}개 왕복(3 기대) — writer arm 또는 read 유실")
    for want, have in zip(scripts, got):
        if have["script"] != want:
            raise RuntimeError(f"스크립트 변형: {have['script']!r} != {want!r}")
    ok, msg = validate_ok(out)
    if not ok:
        raise RuntimeError(f"validate 실패: {msg}")
    return out, "수식 3종(분수·합·특수문자) hwpx 방출 → 스크립트 원문 왕복, validate ✅"


CASES = [
    ("C1_그림자.hwpx", c1_shadow),
    ("C2_외곽선.hwpx", c2_outline),
    ("C3_양각음각.hwpx", c3_emboss_engrave),
    ("C4_첨자.hwpx", c4_scripts),
    ("C5_밑줄모양.hwpx", c5_underline),
    ("C6_번호형식.hwpx", c6_numbering),
    ("C7_글자효과통합.hwpx", c7_all),
    ("C8_요약정보.hwp", c8_summary_hwp),
    ("C9_요약정보.hwpx", c9_summary_hwpx),
    ("D1_도장.hwpx", d1_seal_hwpx),
    ("D2_도장.hwp", d2_seal_hwp),
    ("D3_사용자탭.hwpx", d3_tab_hwpx),
    ("H1_md왕복.hwpx", h1_md_hwpx),
    ("H2_md왕복.hwp", h2_md_hwp),
    ("I1_md이미지코드.hwpx", i1_md_image_code),
    ("J1_쪽테두리.hwpx", j1_page_border),
    ("K1_셀병합.hwpx", k1_merge_hwpx),
    ("K2_셀병합.hwp", k2_merge_hwp),
    ("K3_열조작.hwpx", k3_cols_hwpx),
    ("L1_수식.hwpx", l1_equation_hwpx),
]


def main():
    global HWP, WORK
    ap = argparse.ArgumentParser()
    ap.add_argument("--hwp", required=True)
    ap.add_argument("--dest", required=True)
    ap.add_argument("--work", required=True)
    a = ap.parse_args()
    HWP, WORK = a.hwp, a.work
    os.makedirs(a.dest, exist_ok=True)
    os.makedirs(a.work, exist_ok=True)

    fails = 0
    for label, fn in CASES:
        try:
            _, detail = fn(a.dest)
            print(f"✅ {label} — {detail}")
        except Skip as s:  # 코퍼스 입력 부재: 실패가 아니라 건너뜀(⏭)
            print(f"⏭  {label} — {s}")
        except Exception as e:  # noqa: BLE001 (검증 게이트: 어떤 실패든 ❌ 처리)
            fails += 1
            print(f"❌ {label} — {e}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
