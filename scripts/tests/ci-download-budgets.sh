#!/usr/bin/env bash
# Self check for the CI download budgets (#412).
#
# A step limit shorter than the retry budget of the download it wraps cancels a slow but
# recoverable download, which defeats taking the network out of the pull-request path. Each fetch
# script states its budget as constants; this reads them from the scripts and the step limits from
# ci.yml and fails when a limit does not cover (targets * (RETRY_MAX_TIME + MAX_TIME)).
#
#   scripts/tests/ci-download-budgets.sh            check the working tree, then prove the check
#                                                   can fail on a deliberately too-small limit
#   scripts/tests/ci-download-budgets.sh --tree DIR check only the tree at DIR (used internally)
#
# Fixture-free: it runs in the CI lint job and in scripts/check.sh.
set -uo pipefail

if [ "${1:-}" = "--tree" ]; then
    tree="$2"
    mutate=0
else
    tree="$(cd "$(dirname "$0")/../.." && pwd)"
    mutate=1
fi
cd "$tree" || exit 1

fail=0
check() { # <description> <rc>
    if [ "$2" = "0" ]; then
        printf '  ok   %s\n' "$1"
    else
        printf '  FAIL %s\n' "$1" >&2
        fail=1
    fi
}

# Budgets as the scripts state them.
sh_const() { sed -n "s/^$2=\([0-9]*\)\$/\1/p" "$1" | head -1; }
py_const() { sed -n "s/^$2 = \([0-9]*\).*/\1/p" "$1" | head -1; }

parity=scripts/fetch-pdf-parity-fonts.sh
corpus=scripts/fetch-corpus-fonts.sh
p_retry="$(sh_const "$parity" retry_max_time)"
p_max="$(sh_const "$parity" max_time)"
c_retry="$(py_const "$corpus" RETRY_MAX_TIME)"
c_max="$(py_const "$corpus" MAX_TIME)"
for v in "$p_retry" "$p_max" "$c_retry" "$c_max"; do
    if [ -z "$v" ]; then
        echo "  FAIL a budget constant is missing from a fetch script" >&2
        exit 1
    fi
done

# Download targets: the parity script fetches each font once; the corpus script fetches the
# font, its license and its metadata for every manifest entry.
parity_targets="$(grep -c '^fetch_font [A-Za-z]' "$parity")"
corpus_targets="$(python3 - <<'PY'
import json
print(3 * len(json.load(open("corpus/structured-v1/manifest.json"))["fonts"]))
PY
)"

# The timeout-minutes line of a step, found by a substring of its name.
step_minutes() {
    awk -v name="$1" '
        index($0, "- name: ") && index($0, name) { found = 1; next }
        found && /timeout-minutes:/ { gsub(/[^0-9]/, "", $0); print $0; exit }
        found && /^      - / { exit }' .github/workflows/ci.yml
}
parity_minutes="$(step_minutes "Fetch exact OFL fonts")"
corpus_minutes="$(step_minutes "Structured document corpus")"
if [ -z "$parity_minutes" ] || [ -z "$corpus_minutes" ]; then
    echo "  FAIL a step limit is missing from ci.yml" >&2
    exit 1
fi

parity_need=$((parity_targets * (p_retry + p_max)))
corpus_need=$((corpus_targets * (c_retry + c_max)))
echo "-- parity fonts: $parity_targets x ($p_retry + $p_max) s = $parity_need s; limit $parity_minutes min"
check "Fetch exact OFL fonts covers its download budget" \
    "$([ $((parity_minutes * 60)) -ge "$parity_need" ] && echo 0 || echo 1)"
# The corpus step also builds and runs the corpus after the downloads: require 300 s beyond the
# download budget for that, and stay inside the lint job's 30 minutes.
echo "-- corpus fonts: $corpus_targets x ($c_retry + $c_max) s = $corpus_need s (+300 s of work); limit $corpus_minutes min"
check "Structured document corpus covers its download budget plus the work after it" \
    "$([ $((corpus_minutes * 60)) -ge $((corpus_need + 300)) ] && echo 0 || echo 1)"
check "Structured document corpus stays inside the lint job's 30 minutes" \
    "$([ "$corpus_minutes" -lt 30 ] && echo 0 || echo 1)"

if [ "$mutate" = 1 ]; then
    # The check must be able to fail. Copy what it reads into a scratch tree, shrink the parity
    # step's limit below its budget there, and require the check to reject that tree.
    echo "-- the check rejects a limit that is too small"
    scratch="$(mktemp -d)"
    trap 'rm -rf "$scratch"' EXIT
    mkdir -p "$scratch/.github/workflows" "$scratch/scripts" "$scratch/corpus/structured-v1"
    cp .github/workflows/ci.yml "$scratch/.github/workflows/"
    cp "$parity" "$corpus" "$scratch/scripts/"
    cp corpus/structured-v1/manifest.json "$scratch/corpus/structured-v1/"
    awk '
        index($0, "- name: Fetch exact OFL fonts") { hit = 1 }
        hit && /timeout-minutes:/ { sub(/timeout-minutes: [0-9]+/, "timeout-minutes: 5"); hit = 0 }
        { print }' .github/workflows/ci.yml >"$scratch/.github/workflows/ci.yml"
    if bash "$0" --tree "$scratch" >/dev/null 2>&1; then
        check "a 5 minute limit for two ${p_retry}+${p_max} s downloads is rejected" 1
    else
        check "a 5 minute limit for two ${p_retry}+${p_max} s downloads is rejected" 0
    fi
fi

if [ "$fail" != 0 ]; then
    echo "ci-download-budgets: FAILED" >&2
    exit 1
fi
[ "$mutate" = 1 ] && echo "ci-download-budgets: OK"
exit 0
