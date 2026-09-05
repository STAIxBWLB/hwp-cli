#!/usr/bin/env bash
# Self test for scripts/hancom-regression.sh, run by scripts/check.sh.
#
# The gate's value is in what it refuses, so this exercises the refusals rather
# than a happy path: a delegated generator that dies early, a rerun that fails
# over a destination that already holds a published generation, an interruption
# between staging and publish, an allowlisted case failing for a reason the
# allowlist does not describe, an unmanaged publish target, and the coverage
# accounting that makes the index proof rather than assertion.
#
# The hwp binary and the delegated generator are stubbed (scripts/tests/stub-*),
# so this runs in about a second and needs no release build. What is under test
# is the gate's own control flow, which does not depend on real HWP bytes.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$REPO/scripts/hancom-regression.sh"
STUB_HWP="$REPO/scripts/tests/stub-hwp.sh"
STUB_GEN="$REPO/scripts/tests/stub-generator.sh"
INDEX_NAME='hancom-regression-index.json'

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s: %s\n' "$1" "$2" >&2; failures=$((failures + 1)); }

# run <destination> [env assignments...] - run the gate, echo its exit status.
run_gate() {
  local dest="$1"
  shift
  local status=0
  env HWP_BIN="$STUB_HWP" HWP_REGRESSION_GENERATOR="$STUB_GEN" "$@" \
    bash "$GATE" "$dest" >"$dest.log" 2>&1 || status=$?
  printf '%s' "$status"
}

# The expected-case manifest as the gate itself declares it, so the coverage
# assertion below checks the real list rather than a copy that can drift.
expected_cases() {
  awk '/^EXPECTED_CASES=\(/ { grab = 1; next } grab && /^\)/ { exit } grab { print }' "$GATE" \
    | tr -s ' \t' '\n' | sed '/^$/d'
}

# --- 1. a delegated generator that exits early fails closed ------------------
dest="$ROOT/early"
mkdir -p "$dest"
status="$(run_gate "$dest" STUB_EARLY_EXIT=5)"
if [[ "$status" == '1' ]]; then
  pass 'delegated early exit fails closed (exit 1)'
else
  fail 'delegated early exit' "expected exit 1, got $status"
fi
if [[ ! -e "$dest/current" ]]; then
  pass 'delegated early exit publishes nothing'
else
  fail 'delegated early exit' "current exists at $dest/current"
fi
if compgen -G "$dest/.staging-*" >/dev/null; then
  fail 'delegated early exit' 'the staging directory survived the trap'
else
  pass 'delegated early exit removes the staging directory'
fi

# --- 2. a clean-shaped baseline, and the index proving complete coverage -----
dest="$ROOT/baseline"
mkdir -p "$dest"
status="$(run_gate "$dest")"
# Three cases (M, N and the corpus read) can never publish on a host without
# private inputs, so the honest outcome is 3, not 0.
if [[ "$status" == '3' ]]; then
  pass 'a run with skips exits 3, not 0'
else
  fail 'baseline' "expected exit 3, got $status; see $dest.log"
fi
index="$dest/current/$INDEX_NAME"
if [[ -f "$index" ]]; then
  pass 'baseline publishes an index under current/'
else
  fail 'baseline' "no index at $index"
fi
expected_cases > "$ROOT/expected-cases.txt"
if [[ -f "$index" ]] && python3 - "$index" "$ROOT/expected-cases.txt" <<'PY'
import json
import re
import sys

index = json.load(open(sys.argv[1], encoding="utf-8"))
expected = {line.strip() for line in open(sys.argv[2], encoding="utf-8") if line.strip()}

# An artifact may carry a fragment suffix; the case that owns it is unsuffixed.
published = {re.sub(r"_[0-9]{3}$", "", a["series"]) for a in index["artifacts"]}
known = {k["case"] for k in index["known_failures"]}
skipped = {s["case"] for s in index["skips"]}

overlap = (published & known) | (published & skipped) | (known & skipped)
assert not overlap, f"a case reported two outcomes: {sorted(overlap)}"
covered = published | known | skipped
assert covered == expected, (
    f"missing: {sorted(expected - covered)}; unexpected: {sorted(covered - expected)}"
)
assert index["clean"] is False, "skips present but clean is not false"
assert index["binary"]["explicit"] is True
assert re.fullmatch(r"[0-9a-f]{64}", index["binary"]["sha256"])
assert all(re.fullmatch(r"[0-9a-f]{64}", a["sha256"]) for a in index["artifacts"])
assert {s["reason"] for s in index["skips"]} <= {
    "private_input_missing",
    "series_not_regenerable",
}
PY
then
  pass 'the index accounts for every expected case exactly once'
else
  fail 'coverage' 'the index does not prove complete coverage'
fi
receipts="$dest/current/receipts"
if [[ -d "$receipts" && ! -L "$receipts" && -z "$(ls -A "$receipts")" ]]; then
  pass 'receipts/ is a fresh empty directory, not a symlink'
else
  fail 'receipts' "not a fresh empty directory: $receipts"
fi
if grep -q '"require_artifact_sha256": true' "$dest/current"/*.policy.json; then
  pass 'each emitted policy binds its receipt to the artifact hash'
else
  fail 'policy' 'require_artifact_sha256 missing from the emitted policies'
fi

# --- 3. a failed rerun leaves the previous generation untouched --------------
before_target="$(readlink "$dest/current")"
before_hash="$(shasum -a 256 "$index" | awk '{print $1}')"
before_count="$(find "$dest/current" -type f | wc -l | tr -d ' ')"
status="$(run_gate "$dest" STUB_EARLY_EXIT=3)"
after_target="$(readlink "$dest/current")"
after_hash="$(shasum -a 256 "$index" | awk '{print $1}')"
after_count="$(find "$dest/current" -type f | wc -l | tr -d ' ')"
if [[ "$status" == '1' && "$before_target" == "$after_target" \
  && "$before_hash" == "$after_hash" && "$before_count" == "$after_count" ]]; then
  pass 'a failed rerun leaves the previous generation byte-identical'
else
  fail 'failed rerun' "status=$status target $before_target -> $after_target, index $before_hash -> $after_hash, files $before_count -> $after_count"
fi
if compgen -G "$dest/.staging-*" >/dev/null; then
  fail 'failed rerun' 'a staging directory was left behind'
else
  pass 'a failed rerun leaves no staging directory'
fi
generations="$(find "$dest" -maxdepth 1 -type d -name 'gen-*' | wc -l | tr -d ' ')"
if [[ "$generations" == '1' ]]; then
  pass 'a failed rerun creates no second generation'
else
  fail 'failed rerun' "expected 1 generation, found $generations"
fi

# --- 4. an interruption between staging and publish ---------------------------
# SIGKILL cannot be trapped, so the guarantee under test is not cleanup but the
# publish protocol: `current` is either absent or a complete generation. A
# half-written one would show up as an index that is missing or unparsable.
dest="$ROOT/interrupt"
mkdir -p "$dest"
env HWP_BIN="$STUB_HWP" HWP_REGRESSION_GENERATOR="$STUB_GEN" \
  bash "$GATE" "$dest" >"$dest.log" 2>&1 &
gate_pid=$!
for _ in $(seq 1 400); do
  compgen -G "$dest/.staging-*" >/dev/null && break
  sleep 0.01
done
kill -9 "$gate_pid" 2>/dev/null || true
wait "$gate_pid" 2>/dev/null || true
if [[ ! -e "$dest/current" ]]; then
  pass 'an interruption before publish leaves no current'
elif python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$dest/current/$INDEX_NAME" 2>/dev/null; then
  pass 'an interruption after publish leaves a complete current'
else
  fail 'interrupt' "current exists without a complete index at $dest/current"
fi

# Debris from such an interruption must never be adopted by a later run.
dest="$ROOT/debris"
mkdir -p "$dest/.staging-19700101T000000Z.999"
printf 'not a real artifact\n' > "$dest/.staging-19700101T000000Z.999/ghost.hwpx"
status="$(run_gate "$dest")"
if [[ "$status" == '3' ]] && [[ ! -e "$dest/current/ghost.hwpx" ]]; then
  pass 'leftover staging debris is never adopted into a new generation'
else
  fail 'debris' "status=$status, ghost.hwpx present=$([[ -e "$dest/current/ghost.hwpx" ]] && echo yes || echo no)"
fi

# --- 5. an allowlisted case failing for another reason fails closed ----------
dest="$ROOT/wrong-reason"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=C5 \
  STUB_C5_FAILURE='C5_밑줄모양.hwpx — 예상치 못한 새로운 실패')"
if [[ "$status" == '1' ]] && grep -q 'excluded for' "$dest.log"; then
  pass 'an allowlisted case failing for a different reason fails closed'
else
  fail 'wrong reason' "expected exit 1 naming the fingerprint mismatch, got $status"
fi

# The same case failing for the reason the table describes is excused.
dest="$ROOT/right-reason"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=C5 \
  STUB_C5_FAILURE='C5_밑줄모양.hwpx — 점선 밑줄 밑줄모양(3) 소실')"
if [[ "$status" == '3' ]] \
  && python3 -c 'import json,sys; i=json.load(open(sys.argv[1])); assert [k for k in i["known_failures"] if k["case"] == "C5" and k["stage"] == "fidelity"]' \
    "$dest/current/$INDEX_NAME"; then
  pass 'the tracked failure is excused and recorded with its stage'
else
  fail 'right reason' "expected exit 3 with a C5 known_failure row, got $status"
fi

# An id nobody tracks is refused before anything is generated.
dest="$ROOT/unknown-id"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=Z9)"
if [[ "$status" == '2' && ! -e "$dest/current" ]]; then
  pass 'an untracked case id is refused with exit 2'
else
  fail 'unknown id' "expected exit 2, got $status"
fi

# --- 6. an unmanaged publish target is refused ------------------------------
dest="$ROOT/symlinked"
mkdir -p "$dest" "$ROOT/elsewhere"
ln -s "$ROOT/elsewhere" "$dest/current"
status="$(run_gate "$dest")"
if [[ "$status" == '2' && "$(readlink "$dest/current")" == "$ROOT/elsewhere" ]]; then
  pass 'a current symlink aimed outside the destination is rejected'
else
  fail 'symlinked current' "expected exit 2 with the symlink untouched, got $status"
fi

dest="$ROOT/real-dir"
mkdir -p "$dest/current"
printf 'someone elses file\n' > "$dest/current/keep.txt"
status="$(run_gate "$dest")"
if [[ "$status" == '2' && -f "$dest/current/keep.txt" ]]; then
  pass 'a real directory at current is rejected, not merged into'
else
  fail 'real dir current' "expected exit 2 with the directory untouched, got $status"
fi

# --- 7. the destination guard still refuses the repository -------------------
status=0
env HWP_BIN="$STUB_HWP" HWP_REGRESSION_GENERATOR="$STUB_GEN" \
  bash "$GATE" "$REPO" >/dev/null 2>&1 || status=$?
if [[ "$status" == '2' ]]; then
  pass 'the repository root is refused as a destination'
else
  fail 'dest guard' "expected exit 2, got $status"
fi

if [[ "$failures" -ne 0 ]]; then
  echo "== hancom-regression self test: $failures failure(s) =="
  exit 1
fi
echo '== hancom-regression self test: OK =='
