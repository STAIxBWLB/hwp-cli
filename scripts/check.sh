#!/usr/bin/env bash
# 로컬 CI 미러 — .github/workflows/ci.yml의 Rust/fixture 게이트와 public PDF parity를 실행.
# ci.yml처럼 앞 게이트가 실패해도 나머지를 끝까지 실행해 한 번에 보고한다.
# PR 전에 이 스크립트 한 방으로 로컬 검증을 끝낸다.
set -uo pipefail
cd "$(dirname "$0")/.." || exit

# CI는 dtolnay/rust-toolchain@1.93.0으로 고정한다. 로컬에 같은 툴체인이 있으면 그것을
# 써서 rustfmt/clippy 결과를 CI와 바이트 동일하게 맞춘다(없으면 호스트 + 경고).
if rustup toolchain list 2>/dev/null | grep -q '^1\.93\.0'; then
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
    "$@" || fail=1
}

run $CARGO fmt --all --check
run $CARGO clippy --workspace --all-targets -- -D warnings
run $CARGO test --workspace
run python3 -m unittest tools/test_pdf_parity.py
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
    echo "== check: FAILED (위 게이트 중 실패 있음) $tally =="
    exit 1
fi
echo "== check: OK (fmt/clippy/test/crate-edges/pdf-runner/structured-corpus/claims/doc-surface/release-block/readiness-selfcheck/skip-accounting/public-parity=$parity_result) $tally =="
