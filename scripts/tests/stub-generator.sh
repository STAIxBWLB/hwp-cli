#!/usr/bin/env bash
# Stand-in for tools/gen_verification_set.sh, used only by
# scripts/tests/hancom-regression.sh via HWP_REGRESSION_GENERATOR.
#
# It reproduces the real generator's report-line grammar (the leading marker
# plus the artifact name) and its two modes, so the gate's absorb path is
# exercised for real. Knobs:
#   STUB_EARLY_EXIT=<n>   print n report lines, then exit 1 without the rest.
#                         This is the early-exit the gate must fail closed on.
#   STUB_C5_FAILURE=<msg> report C5 as a failure carrying <msg>.
set -uo pipefail

MODE='phase-02.2'
if [[ "${1:-}" == '--legacy' ]]; then
  MODE='legacy'
  shift
fi
DEST="${1:?destination required}"
mkdir -p "$DEST"

LINES=0
emit_line() {
  LINES=$((LINES + 1))
  printf '%s\n' "$1"
  if [[ -n "${STUB_EARLY_EXIT:-}" && "$LINES" -ge "$STUB_EARLY_EXIT" ]]; then
    echo 'stub generator: simulated early exit' >&2
    exit 1
  fi
}

pass_case() {
  printf 'stub %s\n' "$1" > "$DEST/$1"
  emit_line "✅ $1 — stub case"
}

LEGACY_CASES=(
  'A1A2_work_report_변환.hwpx' 'A1A2_work_report_왕복.hwp'
  'A3A4_annual_report_변환.hwpx' 'A3A4_annual_report_왕복.hwp'
  'A5A6_품의_변환.hwpx' 'A5A6_품의_왕복.hwp'
  'B1_책갈피.hwp' 'B2_책갈피.hwpx' 'B3_하이퍼링크.hwp' 'B4_하이퍼링크.hwpx' 'B5_복합.hwp'
  'C1_그림자.hwpx' 'C2_외곽선.hwpx' 'C3_양각음각.hwpx' 'C4_첨자.hwpx' 'C5_밑줄모양.hwpx'
  'C6_번호형식.hwpx' 'C7_글자효과통합.hwpx' 'C8_요약정보.hwp' 'C9_요약정보.hwpx'
  'D1_도장.hwpx' 'D2_도장.hwp' 'D3_사용자탭.hwpx'
  'H1_md왕복.hwpx' 'H2_md왕복.hwp' 'I1_md이미지코드.hwpx' 'J1_쪽테두리.hwpx'
  'K1_셀병합.hwpx' 'K2_셀병합.hwp' 'K3_열조작.hwpx' 'L1_수식.hwpx'
)

if [[ "$MODE" == 'legacy' ]]; then
  failed=0
  for name in "${LEGACY_CASES[@]}"; do
    if [[ "$name" == 'C5_밑줄모양.hwpx' && -n "${STUB_C5_FAILURE:-}" ]]; then
      emit_line "❌ $name — ${STUB_C5_FAILURE}"
      failed=1
      continue
    fi
    pass_case "$name"
  done
  [[ "$failed" -eq 0 ]]
  exit
fi

INDEX="$DEST/phase-02.2-index.tsv"
printf 'profile\tformat\tartifact_sha256\tinternal_reread\tinternal_validate\n' > "$INDEX"
for profile in official report plan notice minutes press; do
  for format in hwp hwpx; do
    name="phase-02.2-${profile}.${format}"
    printf 'stub %s\n' "$name" > "$DEST/$name"
    digest="$(shasum -a 256 "$DEST/$name" | awk '{print $1}')"
    printf '%s\t%s\t%s\t%s\t%s\n' "$profile" "$format" "$digest" 'pass' 'pass' >> "$INDEX"
  done
done
echo "stub phase-02.2 set ready: $DEST"
