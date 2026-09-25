#!/usr/bin/env bash
# Self test for the #275 fixture-skip accounting, run by scripts/check.sh and the CI lint job.
#
# A test that returns early because a local-only fixture is absent reports `ok`, so check.sh
# counts those skips (skipped-for-missing-fixtures=N (optional=M)) from the lines that
# crates/hwp-cli/tests/common/fixture_skip.rs appends to $HWP_FIXTURE_SKIP_LOG. This proves the
# count moves, through a real test binary and the real environment variables:
#
#   1. a guarded path that is absent adds exactly one line, and a present one adds none
#      (the identity.rs probe, pointed at a path this script controls, so both cases run on any
#      checkout);
#   2. HWP_REQUIRE_FIXTURES=1 fails the absent case and passes the present one;
#   3. an optional fixture (a ground-truth set fixtures/README.md lists as not currently held) is
#      counted and tagged `optional` when absent, and strict mode does not fail it;
#   4. the invariant-2 gate (crates/hwp5/tests/identity.rs) reports through the helper against
#      this checkout's real fixtures: one or more lines without fixtures/hwp5/, none with it. On
#      CI, which has no fixtures, this is what fails if a guard stops being counted.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

fail=0
check() { # <description> <rc already evaluated by the caller>
    if [ "$2" = "0" ]; then
        printf '  ok   %s\n' "$1"
    else
        printf '  FAIL %s\n' "$1" >&2
        fail=1
    fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# identity <log> <strict> <optional> <probe> [libtest args...] - run the identity test binary
# with its own accounting variables (never the caller's) and echo its exit status.
identity() {
    local log="$1" strict="$2" optional="$3" probe="$4" status=0
    shift 4
    env HWP_FIXTURE_SKIP_LOG="$log" HWP_REQUIRE_FIXTURES="$strict" \
        HWP_FIXTURE_SKIP_PROBE="$probe" HWP_FIXTURE_SKIP_PROBE_OPTIONAL="$optional" \
        cargo test --quiet -p hwp5 --test identity -- "$@" >"$log.out" 2>&1 || status=$?
    printf '%s' "$status"
}
probe() { identity "$1" "$2" "$3" "$4" --ignored fixture_skip_accounting_probe; }
# The two numbers check.sh prints: every line (N) and the lines tagged optional (M).
lines() { if [ -f "$1" ]; then awk 'END {print NR}' "$1"; else echo 0; fi; }
optional_lines() {
    if [ -f "$1" ]; then awk -F'\t' '$3 == "optional" {n++} END {print n+0}' "$1"; else echo 0; fi
}

absent="$tmp/no-such-fixture.hwp"
present="$PWD/Cargo.toml"

echo "-- probe: a guarded path the script controls"
rc="$(probe "$tmp/absent.log" 0 0 "$absent")"
check "an absent fixture skips (the test still passes)" "$rc"
check "an absent fixture adds exactly one line" "$([ "$(lines "$tmp/absent.log")" = 1 ] && echo 0 || echo 1)"
check "the line names the missing path" "$(grep -qF "$absent" "$tmp/absent.log" 2>/dev/null && echo 0 || echo 1)"
check "the line is not counted as optional" \
    "$([ "$(optional_lines "$tmp/absent.log")" = 0 ] && echo 0 || echo 1)"

rc="$(probe "$tmp/present.log" 0 0 "$present")"
check "a present fixture passes" "$rc"
check "a present fixture adds no line" "$([ "$(lines "$tmp/present.log")" = 0 ] && echo 0 || echo 1)"

echo "-- strict mode (HWP_REQUIRE_FIXTURES=1)"
rc="$(probe "$tmp/strict-absent.log" 1 0 "$absent")"
check "an absent fixture fails" "$([ "$rc" != 0 ] && echo 0 || echo 1)"
check "the failure names HWP_REQUIRE_FIXTURES" \
    "$(grep -q 'HWP_REQUIRE_FIXTURES=1' "$tmp/strict-absent.log.out" && echo 0 || echo 1)"
rc="$(probe "$tmp/strict-present.log" 1 0 "$present")"
check "a present fixture passes" "$rc"

echo "-- optional fixtures (ground-truth sets not currently held)"
rc="$(probe "$tmp/optional.log" 0 1 "$absent")"
check "an absent optional fixture skips" "$rc"
check "it adds one line, counted as optional" "$([ "$(lines "$tmp/optional.log")" = 1 ] &&
    [ "$(optional_lines "$tmp/optional.log")" = 1 ] && echo 0 || echo 1)"
rc="$(probe "$tmp/optional-strict.log" 1 1 "$absent")"
check "strict mode does not fail an absent optional fixture" "$rc"
check "strict mode still counts it, as optional" "$([ "$(lines "$tmp/optional-strict.log")" = 1 ] &&
    [ "$(optional_lines "$tmp/optional-strict.log")" = 1 ] && echo 0 || echo 1)"

echo "-- the invariant-2 gate against this checkout's fixtures"
rc="$(identity "$tmp/gate.log" 0 0 "$absent")"
check "the identity gate passes" "$rc"
if [ -e fixtures/hwp5/hello_world.hwp ]; then
    check "fixtures/hwp5 present: the gate records no skip" \
        "$([ "$(lines "$tmp/gate.log")" = 0 ] && echo 0 || echo 1)"
else
    check "fixtures/hwp5 absent: the gate's skip is counted" \
        "$([ "$(lines "$tmp/gate.log")" -ge 1 ] && echo 0 || echo 1)"
fi

if [ "$fail" -ne 0 ]; then
    for out in "$tmp"/*.out; do
        echo "--- $out" >&2
        tail -n 20 "$out" >&2
    done
    echo "== fixture-skip accounting self-test: FAILED" >&2
    exit 1
fi
echo "== fixture-skip accounting self-test: OK"
