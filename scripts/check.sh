#!/usr/bin/env bash
# Local CI mirror: Rust/fixture gates and public PDF parity from ci.yml.
# Continue after individual failures so one run reports every gate.
# Run this complete gate before opening a PR.
set -uo pipefail
cd "$(dirname "$0")/.." || exit

# Use the same 1.93.0 toolchain as CI when installed, keeping fmt/clippy deterministic.
# Otherwise use the host toolchain and print the existing warning.
if rustup toolchain list 2>/dev/null | grep '^1\.93\.0' >/dev/null; then
    CARGO="cargo +1.93.0"
else
    CARGO="cargo"
    echo "[check] 경고: rustup 1.93.0 툴체인 없음 — 호스트 도구 사용(rustfmt 버전 차이로 CI와 결과가 갈릴 수 있음)" >&2
fi

target_dir="${CARGO_TARGET_DIR:-target}"
# #275: a test that skips because a local-only fixture (fixtures/hwp5, fixtures/hwpx, ...) is
# absent still reports `ok`. Each such skip appends one line to this file
# (crates/hwp-cli/tests/common/fixture_skip.rs), and the summary line always prints the count,
# zero included, so a reduced-scope run cannot pass for a full one; `optional=M` is the part of N
# that is ground-truth sets fixtures/README.md lists as not currently held. Test binaries run from
# their crate directory, so the path must be absolute. HWP_REQUIRE_FIXTURES=1 turns the required
# skips into failures instead.
mkdir -p "$target_dir"
skip_log="$(cd "$target_dir" && pwd)/fixture-skips.log"
rm -f "$skip_log"
export HWP_FIXTURE_SKIP_LOG="$skip_log"

fail=0
run() {
    echo "== $*"
    "$@" || { fail=1; return 1; }
}

run $CARGO fmt --all --check
run $CARGO clippy --workspace --all-targets -- -D warnings
# cargo test stops at the first failing test binary, and a strict-mode failure panics before
# it logs, so after a failed test step the fixture-skip tally is a partial count (#350).
partial=""
run $CARGO test --workspace || partial=" (partial)"
run python3 -m unittest tools/test_pdf_parity.py tools/test_ci_gate.py
run bash scripts/tests/hancom-regression.sh
run bash scripts/check-structured-corpus.sh
run bash scripts/check-crate-edges.sh
run bash scripts/check-crate-edges.sh --self-test
run bash scripts/check-claims.sh
run bash scripts/check-claims.sh --self-test
run bash scripts/check-doc-surface.sh
run bash scripts/check-doc-surface.sh --self-test
run bash scripts/release_verification_block.sh --self-test
run bash scripts/tests/release-readiness-selfcheck.sh
run bash scripts/tests/fixture-skip-accounting.sh
run "$target_dir/debug/examples/validate_structured_corpus" \
    schemas/pdf-parity-history-v1.schema.json \
    fixtures/pdf-parity/public/scoreboard/history.json

# The public parity job is intentionally pinned to Ubuntu 24.04's pdfinfo 24.02.0.
# A developer's macOS/Homebrew Poppler normally differs, so auto mode skips that one
# environment-specific gate with an actionable message; HWP_PDF_PARITY=1 makes it required.
parity_mode="${HWP_PDF_PARITY:-auto}"
parity_expected="$(python3 -c 'import json; print(json.load(open("fixtures/pdf-parity/public/manifest.json", encoding="utf-8"))["pins"]["poppler_version"])')"
parity_actual=""
parity_result="skipped"
if command -v pdfinfo >/dev/null 2>&1; then
    parity_actual="$(pdfinfo -v 2>&1 | sed -n '1p')"
fi
if [ "$parity_mode" = "1" ] || {
    [ "$parity_mode" != "0" ] && [ "$parity_actual" = "$parity_expected" ];
}; then
    run bash scripts/fetch-pdf-parity-fonts.sh
    run env HWP_FONT_DIR=fixtures/pdf-parity/fonts \
        bash scripts/pdf-parity.sh run --oracle-dir fixtures/pdf-parity/public/oracle
    parity_result="ran"
else
    echo "== pdf-parity: SKIP (requires $parity_expected; set HWP_PDF_PARITY=1 to require)"
fi

skipped=0
optional=0
if [ -f "$skip_log" ]; then
    skipped="$(awk 'END {print NR}' "$skip_log")"
    optional="$(awk -F'\t' '$3 == "optional" {n++} END {print n+0}' "$skip_log")"
    echo "== fixture skips: $skipped ($optional optional), one line each in $skip_log" \
        "(HWP_REQUIRE_FIXTURES=1 fails the required ones)"
fi
tally="skipped-for-missing-fixtures=$skipped (optional=$optional)"

if [ "$fail" -ne 0 ]; then
    echo "== check: FAILED (위 게이트 중 실패 있음) $tally$partial =="
    exit 1
fi
echo "== check: OK (fmt/clippy/test/crate-edges/pdf-runner/structured-corpus/claims/doc-surface/release-block/readiness-selfcheck/skip-accounting/public-parity=$parity_result) $tally =="
