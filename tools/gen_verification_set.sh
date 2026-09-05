#!/usr/bin/env bash
# Hancom verification-set generator.
#
# The default Phase 2.2 set creates exactly fourteen private documents: one HWP and one
# HWPX for every canonical official-document profile. It self-rereads, validates, and
# hash-indexes each artifact, but never creates a Hancom pass receipt. Genuine Hancom
# observations stay a separate human step; see docs/hancom-verification-checklist.md.
#
# The historical rounds 13-23 generator remains available as --legacy so its existing
# investigation workflow is not silently discarded.
#
# Usage:
#   tools/gen_verification_set.sh [destination]
#   tools/gen_verification_set.sh --legacy [destination]
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
MODE="phase-02.2"
if [[ "${1:-}" == "--legacy" ]]; then
  MODE="legacy"
  shift
fi
DEST="${1:-$HOME/Documents/hwp-verification}"
export HWP_FONT_DIR="$REPO/fonts"   # hwp5 합성 lineseg 계산에 필수(5.1.x)

# Phase 2.2 verification bundles are private evidence. Resolve both paths
# before creating the destination so repository paths and symlink aliases are
# rejected before any artifact can be deleted or written there.
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required to resolve a private verification destination" >&2
  exit 1
}
REPO_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve(strict=False))' "$REPO")"
DEST_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).expanduser().resolve(strict=False))' "$DEST")"
if [[ "$DEST_REAL" == "$REPO_REAL" || "$DEST_REAL" == "$REPO_REAL/"* ]]; then
  echo "verification destination must be outside the repository: $DEST_REAL" >&2
  exit 1
fi
DEST="$DEST_REAL"
mkdir -p "$DEST"

# Binary: HWP_BIN wins (callers such as scripts/hancom-regression.sh pin the
# release binary), otherwise reuse debug if present, otherwise build release.
HWP="${HWP_BIN:-$REPO/target/debug/hwp}"
if [[ ! -x "$HWP" ]]; then
  HWP="$REPO/target/release/hwp"
  [[ -x "$HWP" ]] || cargo build --release --manifest-path "$REPO/Cargo.toml" -q
fi
[[ -x "$HWP" ]] || { echo "hwp 바이너리 없음"; exit 1; }

if [[ "$MODE" == "phase-02.2" ]]; then
  # Legacy mode reports individual failures. The Phase 2.2 bundle must stop
  # on the first one and preserve the previously published index.
  set -e
  WORK="$(mktemp -d)"
  STAGE="$(mktemp -d "$DEST/.phase-02.2-stage.XXXXXX")"
  trap 'rm -rf "$WORK" "$STAGE"' EXIT
  INDEX="$DEST/phase-02.2-index.tsv"
  STAGED_INDEX="$STAGE/phase-02.2-index.tsv"
  MARKS='1. | 가. | 1) | 가) | (1) | (가) | ① | ㉮'

  make_source() {
    local output="$1"
    {
      printf '%s\n\n' 'Phase 2.2 official profile verification'
      printf '%s\n\n' 'SENTINEL BEFORE LIST: preserve this paragraph before every numbered level.'
      printf '%s\n' '1. Level 1 marker'
      for level2 in $(seq 1 15); do
        printf '   1. Level 2 sibling %s\n' "$level2"
        if [[ "$level2" -eq 1 ]]; then
          printf '%s\n' '      1. Level 3 marker'
          printf '%s\n' '         1. Level 4 marker'
          printf '%s\n' '            1. Level 5 marker'
          for level6 in $(seq 1 15); do
            printf '               1. Level 6 sibling %s\n' "$level6"
            if [[ "$level6" -eq 1 ]]; then
              printf '%s\n' '                  1. Level 7 marker'
              for level8 in $(seq 1 15); do
                printf '                     1. Level 8 sibling %s\n' "$level8"
              done
            fi
          done
        fi
      done
      printf '\n%s\n\n' 'SENTINEL AFTER LIST: preserve this paragraph after every numbered level.'
      for filler in $(seq 1 90); do
        printf 'Body continuity paragraph %s for page-number and layout observation.\n\n' "$filler"
      done
    } > "$output"
  }

  reread_and_validate() {
    local artifact="$1"
    local reread_stderr validate_json
    if ! reread_stderr="$("$HWP" cat "$artifact" 2>&1 >/dev/null)"; then
      echo "self-reread failed: $(basename "$artifact")" >&2
      return 1
    fi
    if [[ -n "$reread_stderr" ]] && grep -qiE 'warning|warn|error|오류|경고|손상' <<<"$reread_stderr"; then
      echo "self-reread warning: $(basename "$artifact"): $reread_stderr" >&2
      return 1
    fi
    if ! validate_json="$("$HWP" validate --json "$artifact")"; then
      echo "structure validation failed: $(basename "$artifact")" >&2
      return 1
    fi
    python3 -c '
import json
import sys
result = json.load(sys.stdin)
if not result.get("valid") or result.get("warnings"):
    raise SystemExit(1)
' <<<"$validate_json" || return 1
  }

  profile_metadata() {
    case "$1" in
      official) printf '%s\t%s\t%s\t%s\t%s\n' 'Malgun Gothic' '12' '160' '0' 'off' ;;
      report|plan) printf '%s\t%s\t%s\t%s\t%s\n' 'HCR Batang' '15' '160' '15' 'on (- N -)' ;;
      notice) printf '%s\t%s\t%s\t%s\t%s\n' 'Malgun Gothic' '15' '160' '10' 'on (- N -)' ;;
      minutes) printf '%s\t%s\t%s\t%s\t%s\n' 'HCR Batang' '14' '130' '0' 'off' ;;
      press) printf '%s\t%s\t%s\t%s\t%s\n' 'HCR Batang' '14' '160' '10' 'on (- N -)' ;;
    esac
  }

  SOURCE="$WORK/phase-02.2-official-numbering.md"
  make_source "$SOURCE"
  printf '%s\n' $'profile\tformat\tartifact_sha256\texpected_font\tbody_pt\tline_spacing_percent\tmargins_mm\theader_footer_mm\tpage_number\tnumbering\thwp5_encoding\tinternal_reread\tinternal_validate' > "$STAGED_INDEX"

  for profile in official report plan notice minutes press; do
    IFS=$'\t' read -r font body_pt line_spacing header_footer page_number <<<"$(profile_metadata "$profile")"
    for format in hwp hwpx; do
      artifact="$STAGE/phase-02.2-${profile}.${format}"
      if ! "$HWP" new --from "$SOURCE" --preset "$profile" --output "$artifact" >/dev/null; then
        echo "generation failed: $(basename "$artifact")" >&2
        exit 1
      fi
      if ! reread_and_validate "$artifact"; then
        exit 1
      fi
      if ! hash="$(shasum -a 256 "$artifact" | awk '{print $1}')"; then
        echo "hash failed: $(basename "$artifact")" >&2
        exit 1
      fi
      [[ -n "$hash" ]] || { echo "empty hash: $(basename "$artifact")" >&2; exit 1; }
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$profile" "$format" "$hash" "$font" "$body_pt" "$line_spacing" \
        'top=20,bottom=10,left=20,right=20' "$header_footer" "$page_number" \
        "$MARKS; continuation: 하 -> 거 at levels 2, 6, and 8" \
        "$([[ "$format" == hwp ]] && printf 'safe/direct' || printf 'n/a (HWPX)')" \
        'pass' 'pass' >> "$STAGED_INDEX"
    done
  done

  artifact_count="$(find "$STAGE" -maxdepth 1 -type f \( -name 'phase-02.2-*.hwp' -o -name 'phase-02.2-*.hwpx' \) | wc -l | tr -d ' ')"
  [[ "$artifact_count" == '12' ]] || { echo "expected 12 artifacts, found $artifact_count" >&2; exit 1; }
  [[ -s "$STAGED_INDEX" ]] || { echo 'content-free index missing or empty' >&2; exit 1; }

  # A destination that still holds an older bundle (the retired seven-profile set
  # published fourteen artifacts) would keep its retired files alongside the new
  # ones while the index and the count below claim twelve. Clear the whole
  # generation before publishing so glob-based consumers see exactly the indexed set.
  rm -f "$DEST"/phase-02.2-*.hwp "$DEST"/phase-02.2-*.hwpx

  # The index is the completion receipt. Publish it only after every staged
  # artifact passed self-reread and structure validation.
  for profile in official report plan notice minutes press; do
    for format in hwp hwpx; do
      artifact="phase-02.2-${profile}.${format}"
      mv -f "$STAGE/$artifact" "$DEST/$artifact"
    done
  done
  mv -f "$STAGED_INDEX" "$INDEX"
  echo "Phase 2.2 private verification set ready: $DEST"
  echo "Artifacts: 12 (all self-reread and structurally validated)"
  echo "Index: $INDEX"
  echo 'No Hancom observation or pass receipt has been created.'
  exit 0
fi

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
cp "$REPO/docs/hancom-verification-checklist.ko.md" "$DEST/README.md" 2>/dev/null || true

echo
echo "=== 자체 검증 결과 (통과 $pass / 실패 $fail) ==="
printf '%s\n' "${REPORT[@]}"
echo
echo "→ 한글(한컴오피스)에서 $DEST 의 파일들을 열어 손상/변조 경고 없이 열리는지,"
echo "  내용이 정상인지 확인해 주세요. 판정 기준: $DEST/README.md"
[[ $fail -eq 0 ]]
