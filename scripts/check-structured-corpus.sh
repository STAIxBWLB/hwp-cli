#!/usr/bin/env bash
# Frozen structured corpus source-build gate. No external HWP/HWPX samples or ambient fonts.
set -euo pipefail
cd "$(dirname "$0")/.."

inventory="corpus/structured-v1/TRACKED_FILES.txt"
actual=$(mktemp)
expected=$(mktemp)
runtime_root=""
report_parent=""
cleanup() {
    rm -f "$actual" "$expected"
    if [ -n "$runtime_root" ]; then
        rm -rf "$runtime_root"
    fi
    if [ -n "$report_parent" ]; then
        rm -rf "$report_parent"
    fi
}
trap cleanup EXIT
find corpus/structured-v1 -path corpus/structured-v1/fonts -prune -o -type f -print |
    LC_ALL=C sort >"$actual"
LC_ALL=C sort "$inventory" >"$expected"
if ! cmp -s "$actual" "$expected"; then
    echo "structured corpus inventory differs from TRACKED_FILES.txt" >&2
    diff -u "$expected" "$actual" >&2 || true
    exit 1
fi

while IFS= read -r path; do
    test -f "$path"
    if git check-ignore -q "$path"; then
        echo "structured corpus input is ignored: $path" >&2
        exit 1
    fi
    if ! git ls-files --error-unmatch -- "$path" >/dev/null 2>&1; then
        if [ "${CI:-}" = "true" ]; then
            echo "structured corpus input is absent from the Git index: $path" >&2
            exit 1
        fi
        echo "[corpus] pending Git index entry: $path" >&2
    fi
done <"$inventory"

for path in \
    schemas/structured-corpus-v1.schema.json \
    schemas/structured-corpus-run-v1.schema.json \
    schemas/structured-corpus-artifacts-v1.schema.json; do
    test -f "$path"
    if git check-ignore -q "$path"; then
        echo "structured corpus schema is ignored: $path" >&2
        exit 1
    fi
    if ! git ls-files --error-unmatch -- "$path" >/dev/null 2>&1; then
        if [ "${CI:-}" = "true" ]; then
            echo "structured corpus schema is absent from the Git index: $path" >&2
            exit 1
        fi
        echo "[corpus] pending Git index entry: $path" >&2
    fi
done

# Build the exact tracked corpus in an external runtime root so downloaded,
# hash-pinned font bytes never modify the checkout under verification.
runtime_root=$(mktemp -d)
while IFS= read -r path; do
    mkdir -p "$runtime_root/$(dirname "$path")"
    cp "$path" "$runtime_root/$path"
done <"$inventory"
runtime_manifest="$runtime_root/corpus/structured-v1/manifest.json"
HWP_CORPUS_MANIFEST_PATH="$runtime_manifest" bash scripts/fetch-corpus-fonts.sh

report_parent=$(mktemp -d)
report="$report_parent/report"
cargo run --locked --offline -p hwp-cli -- corpus \
    --manifest "$runtime_manifest" \
    --report "$report"

cargo run --locked --offline -p hwp-cli --example validate_structured_corpus -- \
    schemas/structured-corpus-v1.schema.json "$runtime_manifest" \
    schemas/structured-corpus-run-v1.schema.json "$report/summary.json" \
    schemas/structured-corpus-artifacts-v1.schema.json "$report/artifacts.json"

echo "[corpus] passed: $report"
