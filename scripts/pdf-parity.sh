#!/usr/bin/env bash
# scripts/pdf-parity.sh — Hancom 기준 PDF 동등성 배치 러너 (issue #79 PR 4).
#
#   scripts/pdf-parity.sh selftest [--source <doc>]   하네스 자기 검증 (fixture vs 자기 PDF)
#   scripts/pdf-parity.sh run [--oracle-dir <dir>]    manifest 케이스 집계 → 점수판
#
# 다섯 지표 정의와 게이트는 docs/design/21-pdf-parity.md §3/§4, 데이터 정책은 §7.
# 기준(oracle) PDF는 로컬 전용 — 기본 위치는 $HWP_PDF_PARITY_ORACLE_DIR 이며 절대
# 커밋하지 않는다. 커밋되는 산출물은 fixtures/pdf-parity/public/scoreboard/ 아래
# 수치 JSON/CSV뿐이다. 구현 상세는 tools/pdf_parity.py.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

for tool in pdfinfo pdffonts pdftotext pdftoppm python3; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "[pdf-parity] $tool 필요 (poppler · python3 설치)" >&2
    exit 1
  }
done

HWP_BIN="${HWP_BIN:-$ROOT/target/debug/hwp}"
if [ ! -x "$HWP_BIN" ]; then
  echo "[pdf-parity] hwp 빌드"
  cargo build -p hwp-cli --quiet
fi

MANIFEST="${PDF_PARITY_MANIFEST:-$ROOT/fixtures/pdf-parity/public/manifest.json}"
OUT_DIR="$ROOT/fixtures/pdf-parity/public/scoreboard"

cmd="${1:-}"
case "$cmd" in
  selftest)
    shift
    exec python3 tools/pdf_parity.py selftest --hwp-bin "$HWP_BIN" "$@"
    ;;
  run)
    shift
    # 산출물 스키마 검증기 (임의 개수의 schema/instance 쌍, tools/pdf_parity.py가 호출).
    cargo build -p hwp-cli --example validate_structured_corpus --quiet
    python3 tools/pdf_parity.py run --hwp-bin "$HWP_BIN" \
      --manifest "$MANIFEST" --out "$OUT_DIR" "$@"
    ;;
  *)
    echo "usage: $0 selftest [--source <doc>] | run [--oracle-dir <dir>]" >&2
    exit 2
    ;;
esac
