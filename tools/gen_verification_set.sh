#!/usr/bin/env bash
# 한글 실기 검증 세트 생성기 (⑬~㉓ 쓰기 경로 + 글자효과·요약정보).
#
# ~/Documents/hwp-verification/(또는 $1)에 검증 파일을 생성한다: A(실무 파이프라인)·
# B(책갈피/하이퍼링크)·C(글자효과·요약정보, JSON IR 경유). 각 파일은 우리 리더로
# 자체 재검증(재읽기 무경고 + C는 효과 보존 단언)한 뒤에만 통과 표시된다 — 깨진
# 파일을 넘기지 않기 위함. 실제 한글 수용 여부는 사용자가 한컴오피스에서 열어
# 확인(docs/실기검증-체크리스트.md).
#
# 사용: tools/gen_verification_set.sh [대상디렉터리]
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$HOME/Documents/hwp-verification}"
export HWP_FONT_DIR="$REPO/fonts"   # hwp5 합성 lineseg 계산에 필수(5.1.x)

# 바이너리: debug가 있으면 재사용, 없으면 release 빌드.
HWP="$REPO/target/debug/hwp"
if [[ ! -x "$HWP" ]]; then
  HWP="$REPO/target/release/hwp"
  [[ -x "$HWP" ]] || cargo build --release --manifest-path "$REPO/Cargo.toml" -q
fi
[[ -x "$HWP" ]] || { echo "hwp 바이너리 없음"; exit 1; }

mkdir -p "$DEST"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FIX="$REPO/fixtures/hwp5"
# 코퍼스 대표 품의(%fmu 수식+표+쪽번호). 없으면 A5/A6 생략.
PUMUI="$(find "$HOME/Documents/hwp_samples" -name "*.hwp" -path "*재료 구입*" 2>/dev/null | head -1)"

# base.md — 앵커 "제목"·"여기에"를 본문에 포함.
cat > "$WORK/base.md" <<'MD'
# 실기 검증 문서

제목 문단입니다. 이 문장 여기에 하이퍼링크가 삽입됩니다.

둘째 문단으로 책갈피와 링크가 문서 흐름에 정상 배치되는지 확인합니다.
MD

pass=0; fail=0
declare -a REPORT

# 파일 생성 후 자체 재읽기 게이트: cat이 경고 없이 내용을 내면 OK.
check() {
  local f="$1" label="$2"
  if [[ ! -s "$f" ]]; then REPORT+=("❌ $label — 파일 없음/빈 파일"); ((fail++)); return; fi
  local err; err="$("$HWP" cat "$f" 2>&1 >/dev/null)"
  local txt; txt="$("$HWP" cat "$f" 2>/dev/null | tr -d '[:space:]')"
  if echo "$err" | grep -qiE "경고|오류|손상|error|warn"; then
    REPORT+=("❌ $label — 재읽기 경고: $(echo "$err" | head -1)"); ((fail++)); return
  fi
  if [[ -z "$txt" ]]; then REPORT+=("❌ $label — 추출 텍스트 없음"); ((fail++)); return; fi
  REPORT+=("✅ $label"); ((pass++))
}

echo "생성 대상: $DEST"
echo "폰트: $HWP_FONT_DIR"

# ── A. 실무 문서 전체 파이프라인 ──
gen_pipeline() {  # <입력hwp> <접두>
  local src="$1" pfx="$2"
  [[ -f "$src" ]] || { REPORT+=("⏭  ${pfx} — 입력 없음: $(basename "$src")"); return; }
  "$HWP" convert "$src" -o "$DEST/${pfx}_변환.hwpx" >/dev/null 2>&1
  check "$DEST/${pfx}_변환.hwpx" "${pfx}_변환.hwpx (hwp→우리 hwpx)"
  "$HWP" convert "$DEST/${pfx}_변환.hwpx" -o "$DEST/${pfx}_왕복.hwp" >/dev/null 2>&1
  check "$DEST/${pfx}_왕복.hwp" "${pfx}_왕복.hwp (hwp→hwpx→우리 hwp)"
}
gen_pipeline "$FIX/work_report.hwp"   "A1A2_work_report"
gen_pipeline "$FIX/annual_report.hwp" "A3A4_annual_report"
[[ -n "$PUMUI" ]] && gen_pipeline "$PUMUI" "A5A6_품의"

# ── B. 기능별 최소 파일 ──
"$HWP" new --from "$WORK/base.md" -o "$WORK/base.hwp" >/dev/null 2>&1
if [[ -s "$WORK/base.hwp" ]]; then
  "$HWP" edit "$WORK/base.hwp" -o "$DEST/B1_책갈피.hwp"  --create-bookmark "제목=>검증책갈피" >/dev/null 2>&1
  check "$DEST/B1_책갈피.hwp" "B1_책갈피.hwp (bokm 생성 ⑬)"
  "$HWP" edit "$WORK/base.hwp" -o "$DEST/B2_책갈피.hwpx" --create-bookmark "제목=>검증책갈피" >/dev/null 2>&1
  check "$DEST/B2_책갈피.hwpx" "B2_책갈피.hwpx (hp:bookmark ⑭)"
  "$HWP" edit "$WORK/base.hwp" -o "$DEST/B3_하이퍼링크.hwp"  --create-hyperlink "여기에=>한컴=>https://www.hancom.com" >/dev/null 2>&1
  check "$DEST/B3_하이퍼링크.hwp" "B3_하이퍼링크.hwp (%hlk 생성 ⑮)"
  "$HWP" edit "$WORK/base.hwp" -o "$DEST/B4_하이퍼링크.hwpx" --create-hyperlink "여기에=>한컴=>https://www.hancom.com" >/dev/null 2>&1
  check "$DEST/B4_하이퍼링크.hwpx" "B4_하이퍼링크.hwpx (fieldBegin HYPERLINK ⑮)"
  "$HWP" edit "$WORK/base.hwp" -o "$DEST/B5_복합.hwp" \
      --create-bookmark "제목=>검증책갈피" \
      --create-hyperlink "여기에=>한컴=>https://www.hancom.com" >/dev/null 2>&1
  check "$DEST/B5_복합.hwp" "B5_복합.hwp (책갈피+하이퍼링크)"
else
  REPORT+=("❌ base.hwp 생성 실패 — B 시리즈 생략")
fi

# ── C. 글자효과·요약정보 (JSON IR 경유 — tools/gen_effects_cases.py) ──
# CLI에 효과 플래그가 없어 IR을 python(stdlib)으로 수술해 만든다. 헬퍼가 파일당
# ✅/❌ 한 줄을 찍고(효과 보존 단언 포함), 여기서 REPORT/pass/fail에 합친다.
if command -v python3 >/dev/null 2>&1; then
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    REPORT+=("$line")
    case "$line" in
      ✅*) ((pass++)) ;;
      ❌*) ((fail++)) ;;
      *)   ;;
    esac
  done < <(python3 "$REPO/tools/gen_effects_cases.py" --hwp "$HWP" --dest "$DEST" --work "$WORK")
else
  REPORT+=("⏭  C 시리즈 — python3 없음")
fi

# 체크리스트 사본.
cp "$REPO/docs/실기검증-체크리스트.md" "$DEST/README.md" 2>/dev/null || true

echo
echo "=== 자체 검증 결과 (통과 $pass / 실패 $fail) ==="
printf '%s\n' "${REPORT[@]}"
echo
echo "→ 한글(한컴오피스)에서 $DEST 의 파일들을 열어 손상/변조 경고 없이 열리는지,"
echo "  내용이 정상인지 확인해 주세요. 판정 기준: $DEST/README.md"
[[ $fail -eq 0 ]]
