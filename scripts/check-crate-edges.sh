#!/usr/bin/env bash
# AGENTS.md invariant 1 as a runnable gate.
#
# `hwp-model` is the hub and every other crate is a spoke: `hwp5` and `hwpx` do not depend on
# each other, and neither do `hwp-convert` and `hwp-render`. That last pair is why the segment
# id rule exists twice (crates/hwp-convert/src/segment_id.rs and
# crates/hwp-render/src/segment_id.rs) instead of once in a shared crate, so a normal dependency
# added between them would quietly make the duplication pointless. Until now nothing in the
# repository stopped one; the invariant lived only in prose.
#
# Dev-dependencies are deliberately out of scope (`-e normal`): hwp-render's tests already use
# hwp-convert to build documents, which does not put either crate in the other's build graph.
#
# Matching is on the dependency NAME, never on a substring of the `cargo tree` line: this
# repository's root directory is named `hwp-cli`, so every path-dependency line contains that
# string and an unanchored `grep -c 'hwp-\(render\|cli\)'` reports a passing tree as a failure.
set -uo pipefail
cd "$(dirname "$0")/.." || exit

fail=0

# Direct normal dependencies of a crate, one name per line.
deps() {
    cargo tree -p "$1" -e normal --depth 1 --prefix none 2>/dev/null |
        tail -n +2 | awk '{print $1}'
}

forbid() {
    local crate="$1" forbidden="$2" count
    count="$(deps "$crate" | grep -Ecx "$forbidden")"
    if [ "$count" -ne 0 ]; then
        echo "FAIL: $crate has a normal dependency on $forbidden (AGENTS.md invariant 1)" >&2
        fail=1
    else
        echo "ok: $crate does not depend on $forbidden"
    fi
}

if [ "${1:-}" = "--self-test" ]; then
    # The gate must be able to fail: a dependency that demonstrably exists has to be caught.
    if deps hwp-cli | grep -Ecx 'hwp-render' | grep -qx 0; then
        echo "FAIL: self-test could not see hwp-cli -> hwp-render, so the matcher is broken" >&2
        exit 1
    fi
    echo "== crate-edges self-test: OK =="
    exit 0
fi

forbid hwp-render hwp-convert
forbid hwp-convert hwp-render
forbid hwp5 hwpx
forbid hwpx hwp5
forbid hwp-model 'hwp5|hwpx|hwp-convert|hwp-render|hwp-cli'

if [ "$fail" -ne 0 ]; then
    echo "== crate-edges: FAILED ==" >&2
    exit 1
fi
echo "== crate-edges: OK =="
