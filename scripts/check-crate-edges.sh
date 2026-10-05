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
# The whole normal-dependency tree is checked, not only direct edges (#407): hwp-render taking a
# normal dependency on hwpx would pull in hwp-convert through hwpx -> hwp-convert.
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
manifest=Cargo.toml

# Normal dependencies of a crate, direct and transitive, one name per line. A cargo failure is a
# nonzero status with cargo's error left on stderr, never an empty list that reads as "ok".
deps() {
    local tree
    tree="$(cargo tree -q --manifest-path "$manifest" -p "$1" -e normal --prefix none)" || return
    printf '%s\n' "$tree" | tail -n +2 | awk '{print $1}' | sort -u
}

# Status 0 if the crate depends on a name matching the pattern, 1 if not, 2 if cargo tree failed.
# The match reads a here-string, not a pipe: under pipefail `... | grep -q` can turn a hit into a
# failure (SIGPIPE upstream), and `grep -c` exits 1 on a zero count.
has_dep() {
    local names
    names="$(deps "$1")" || return 2
    grep -Eqx "$2" <<<"$names"
}

forbid() {
    local crate="$1" forbidden="$2"
    has_dep "$crate" "$forbidden"
    case $? in
    0)
        echo "FAIL: $crate depends on $forbidden, directly or transitively" \
            "(AGENTS.md invariant 1)" >&2
        fail=1
        ;;
    1) echo "ok: $crate does not depend on $forbidden" ;;
    *)
        echo "FAIL: cargo tree failed for $crate, so its edges were not checked" >&2
        fail=1
        ;;
    esac
}

if [ "${1:-}" = "--self-test" ]; then
    # The gate must be able to fail: a dependency that demonstrably exists has to be caught.
    if ! has_dep hwp-cli hwp-render; then
        echo "FAIL: self-test could not see hwp-cli -> hwp-render, so the matcher is broken" >&2
        exit 1
    fi
    # No internal crate reaches another only transitively today, so a throwaway workspace
    # supplies the case: edge-a -> edge-b -> edge-c, with no direct edge-a -> edge-c.
    fixture="$(mktemp -d)"
    trap 'rm -rf "$fixture"' EXIT
    printf '[workspace]\nmembers = ["edge-a", "edge-b", "edge-c"]\nresolver = "2"\n' \
        >"$fixture/Cargo.toml"
    for c in a b c; do
        mkdir -p "$fixture/edge-$c/src"
        : >"$fixture/edge-$c/src/lib.rs"
        printf '[package]\nname = "edge-%s"\nversion = "0.0.0"\nedition = "2021"\n' "$c" \
            >"$fixture/edge-$c/Cargo.toml"
    done
    printf '[dependencies]\nedge-b = { path = "../edge-b" }\n' >>"$fixture/edge-a/Cargo.toml"
    printf '[dependencies]\nedge-c = { path = "../edge-c" }\n' >>"$fixture/edge-b/Cargo.toml"
    manifest="$fixture/Cargo.toml"
    if ! has_dep edge-a edge-c; then
        echo "FAIL: self-test missed the transitive edge edge-a -> edge-b -> edge-c" >&2
        exit 1
    fi
    echo "ok: self-test caught the transitive edge edge-a -> edge-b -> edge-c"
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
