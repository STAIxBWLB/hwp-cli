#!/usr/bin/env bash
# Hancom PDF parity batch runner for issue #79.
#
#   scripts/pdf-parity.sh selftest [--source <doc>]   Verify the harness against itself.
#   scripts/pdf-parity.sh run [--oracle-dir <dir>]    Aggregate manifest cases.
#
# Metric definitions and gates live in docs/design/21-pdf-parity.md sections 3-4;
# the data policy is section 7. One anonymized public oracle is explicitly
# allowed; all other oracle PDFs remain local. Scoreboards contain numeric and
# digest data only.
# v2 manifests are platform-neutral and enforce every normative §4.1 gate;
# v1 manifests remain accepted for existing local baselines.
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
    # Build the closed-schema validator used before scoring and before publishing.
    cargo build -p hwp-cli --example validate_structured_corpus --quiet
    python3 tools/pdf_parity.py run --hwp-bin "$HWP_BIN" \
      --manifest "$MANIFEST" --out "$OUT_DIR" "$@"
    ;;
  *)
    echo "usage: $0 selftest [--source <doc>] | run [--oracle-dir <dir>]" >&2
    exit 2
    ;;
esac
