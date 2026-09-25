#!/usr/bin/env bash
# Self test for the #275 fixture-skip accounting, run by scripts/check.sh and the CI lint job.
#
# A test that returns early because a local-only fixture is absent reports `ok`, so check.sh
# counts those skips (skipped-for-missing-fixtures=N) from the lines that
# crates/hwp-cli/tests/common/fixture_skip.rs appends to $HWP_FIXTURE_SKIP_LOG. This proves the
# count moves, through a real test binary and the real environment variables:
#
#   1. a guarded path that is absent adds exactly one line, and a present one adds none
#      (the identity.rs probe, pointed at a path this script controls, so both cases run on any
#      checkout);
#   2. HWP_REQUIRE_FIXTURES=1 fails the absent case and passes the present one;
#   3. the invariant-2 gate (crates/hwp5/tests/identity.rs) reports through the helper against
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

# identity <log> <strict> <probe> [libtest args...] - run the identity test binary with its own
# accounting variables (never the caller's) and echo its exit status.
identity() {
    local log="$1" strict="$2" probe="$3" status=0
    shift 3
    env HWP_FIXTURE_SKIP_LOG="$log" HWP_REQUIRE_FIXTURES="$strict" HWP_FIXTURE_SKIP_PROBE="$probe" \
        cargo test --quiet -p hwp5 --test identity -- "$@" >"$log.out" 2>&1 || status=$?
    printf '%s' "$status"
}
lines() { # <log> - the tally check.sh prints: one line per skip, 0 for no file
    if [ -f "$1" ]; then wc -l <"$1" | tr -d ' '; else echo 0; fi
}

absent="$tmp/no-such-fixture.hwp"
present="$PWD/Cargo.toml"

echo "-- probe: a guarded path the script controls"
rc="$(identity "$tmp/absent.log" 0 "$absent" --ignored fixture_skip_accounting_probe)"
check "an absent fixture skips (the test still passes)" "$rc"
check "an absent fixture adds exactly one line" "$([ "$(lines "$tmp/absent.log")" = 1 ] && echo 0 || echo 1)"
check "the line names the missing path" "$(grep -qF "$absent" "$tmp/absent.log" 2>/dev/null && echo 0 || echo 1)"

rc="$(identity "$tmp/present.log" 0 "$present" --ignored fixture_skip_accounting_probe)"
check "a present fixture passes" "$rc"
check "a present fixture adds no line" "$([ "$(lines "$tmp/present.log")" = 0 ] && echo 0 || echo 1)"

echo "-- strict mode (HWP_REQUIRE_FIXTURES=1)"
rc="$(identity "$tmp/strict-absent.log" 1 "$absent" --ignored fixture_skip_accounting_probe)"
check "an absent fixture fails" "$([ "$rc" != 0 ] && echo 0 || echo 1)"
check "the failure names HWP_REQUIRE_FIXTURES" \
    "$(grep -q 'HWP_REQUIRE_FIXTURES=1' "$tmp/strict-absent.log.out" && echo 0 || echo 1)"
rc="$(identity "$tmp/strict-present.log" 1 "$present" --ignored fixture_skip_accounting_probe)"
check "a present fixture passes" "$rc"

echo "-- the invariant-2 gate against this checkout's fixtures"
rc="$(identity "$tmp/gate.log" 0 "$absent")"
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
