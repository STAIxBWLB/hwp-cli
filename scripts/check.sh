#!/usr/bin/env bash
# 로컬 CI 미러 — .github/workflows/ci.yml의 4종 게이트를 동일한 커맨드로 실행.
# ci.yml처럼 앞 게이트가 실패해도 나머지를 끝까지 실행해 한 번에 보고한다.
# PR 전에 이 스크립트 한 방으로 로컬 검증을 끝낸다.
set -uo pipefail
cd "$(dirname "$0")/.."

# CI는 dtolnay/rust-toolchain@1.93.0으로 고정한다. 로컬에 같은 툴체인이 있으면 그것을
# 써서 rustfmt/clippy 결과를 CI와 바이트 동일하게 맞춘다(없으면 호스트 + 경고).
if rustup toolchain list 2>/dev/null | grep -q '^1\.93\.0'; then
    CARGO="cargo +1.93.0"
else
    CARGO="cargo"
    echo "[check] 경고: rustup 1.93.0 툴체인 없음 — 호스트 도구 사용(rustfmt 버전 차이로 CI와 결과가 갈릴 수 있음)" >&2
fi

fail=0
run() {
    echo "== $*"
    "$@" || fail=1
}

run $CARGO fmt --all --check
run $CARGO clippy --workspace --all-targets -- -D warnings
run $CARGO test --workspace
run python3 -m unittest tools/test_pdf_parity.py
run bash scripts/check-structured-corpus.sh

if [ "$fail" -ne 0 ]; then
    echo "== check: FAILED (위 게이트 중 실패 있음) =="
    exit 1
fi
echo "== check: OK (fmt/clippy/test/pdf-parity/structured-corpus = CI 게이트) =="
