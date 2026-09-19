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
# is the gate's own control flow, which does not depend on real HWP bytes. The
# one exception is the font-manifest end-to-end check, which runs the debug
# binary's certify on an emitted policy (HWP_REGRESSION_CERTIFY_BIN overrides).
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

# Exercise the same path forms users commonly pass from the repository root.
# The gate must resolve both executables before its P3 directory change.
run_gate_relative_paths() {
  local dest="$1"
  shift
  local status=0
  (cd "$REPO" && \
    env HWP_BIN='scripts/tests/stub-hwp.sh' \
      HWP_REGRESSION_GENERATOR='scripts/tests/stub-generator.sh' "$@" \
      bash "$GATE" "$dest") >"$dest.log" 2>&1 || status=$?
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

# Relative executable paths must survive P3's `(cd "$WORK")`, and both
# delegated generator modes must still publish their outputs.
dest="$ROOT/relative-paths"
mkdir -p "$dest"
status="$(run_gate_relative_paths "$dest")"
if [[ "$status" == '3' ]] \
  && python3 - "$dest/current/$INDEX_NAME" <<'PY'
import json
import sys

index = json.load(open(sys.argv[1], encoding="utf-8"))
series = {artifact["series"] for artifact in index["artifacts"]}
assert "P3_compare_readonly" in series
assert "A1" in series
assert "O_official_hwp" in series
PY
then
  pass 'relative binary and delegated paths survive P3 and publish outputs'
else
  fail 'relative paths' "expected exit 3 with P3 and delegated outputs, got $status"
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
# The real table is empty whenever every case passes, so the hatch is exercised
# on a patched copy of the gate that tracks one C5 row. HWP_REGRESSION_REPO keeps
# the copy pointed at the real checkout.
GATE_TRACKED="$ROOT/gate-tracked-c5.sh"
sed "s|^KNOWN_FAILURE_ISSUES=()|KNOWN_FAILURE_ISSUES=('C5\|fidelity\|밑줄모양(3) 소실\|https://example.invalid/issues/0')|" \
  "$GATE" > "$GATE_TRACKED"
if grep -q "KNOWN_FAILURE_ISSUES=('C5" "$GATE_TRACKED"; then
  pass 'self test tracks one C5 row in a patched gate copy'
else
  fail 'patched gate' 'could not inject the C5 known-failure row'
fi
GATE_REAL="$GATE"
GATE="$GATE_TRACKED"
dest="$ROOT/wrong-reason"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_REPO="$REPO" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=C5 \
  STUB_C5_FAILURE='C5_밑줄모양.hwpx — 예상치 못한 새로운 실패')"
if [[ "$status" == '1' ]] && grep -q 'excluded for' "$dest.log"; then
  pass 'an allowlisted case failing for a different reason fails closed'
else
  fail 'wrong reason' "expected exit 1 naming the fingerprint mismatch, got $status"
fi

# The same case failing for the reason the table describes is excused.
dest="$ROOT/right-reason"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_REPO="$REPO" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=C5 \
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
status="$(run_gate "$dest" HWP_REGRESSION_REPO="$REPO" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=Z9)"
if [[ "$status" == '2' && ! -e "$dest/current" ]]; then
  pass 'an untracked case id is refused with exit 2'
else
  fail 'unknown id' "expected exit 2, got $status"
fi
GATE="$GATE_REAL"

# With the real (empty) table, no id at all may be excluded.
dest="$ROOT/empty-table"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_REGRESSION_ALLOW_KNOWN_FAILURES=C5)"
if [[ "$status" == '2' && ! -e "$dest/current" ]] && grep -q '(none)' "$dest.log"; then
  pass 'the real table tracks no case, so every exclusion is refused'
else
  fail 'empty table' "expected exit 2 naming an empty table, got $status"
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

# --- 8. the certification font manifest --------------------------------------
# Without HWP_CERT_FONT_DIR the run says so loudly and the index records "none",
# so a fontless set cannot pass as one certified against real fonts.
if grep -q 'WARNING: HWP_CERT_FONT_DIR is not set' "$ROOT/baseline.log" \
  && python3 - "$ROOT/baseline/current" "$INDEX_NAME" <<'PY'
import glob
import json
import os
import sys

gen, index_name = sys.argv[1:3]
assert json.load(open(os.path.join(gen, index_name)))["font_manifest"] == "none"
assert not os.path.exists(os.path.join(gen, "fonts"))
for policy in glob.glob(os.path.join(gen, "*.policy.json")):
    assert "manifest" not in json.load(open(policy))["document"]["fonts"]
PY
then
  pass 'without HWP_CERT_FONT_DIR the run warns and records font_manifest none'
else
  fail 'no font dir' 'no stderr warning, or the index does not record font_manifest none'
fi

# Synthetic font-like bytes: the gate only copies and hashes them. A non-font
# file beside them must not be pinned.
fontdir="$ROOT/cert-fonts"
mkdir -p "$fontdir"
printf 'synthetic font b\n' > "$fontdir/b-Regular.ttf"
printf 'synthetic font a\n' > "$fontdir/a-Regular.otf"
printf 'synthetic font c\n' > "$fontdir/c[wght].ttc"
printf 'license text\n' > "$fontdir/OFL.txt"
dest="$ROOT/fonts"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_CERT_FONT_DIR="$fontdir")"
if [[ "$status" == '3' ]] && python3 - "$dest/current" "$INDEX_NAME" "$fontdir" <<'PY'
import glob
import hashlib
import json
import os
import sys

gen, index_name, source = sys.argv[1:4]
expected = [
    {"path": "fonts/" + name, "sha256": hashlib.sha256(open(os.path.join(source, name), "rb").read()).hexdigest()}
    for name in ["a-Regular.otf", "b-Regular.ttf", "c[wght].ttc"]
]
assert json.load(open(os.path.join(gen, index_name)))["font_manifest"] == expected
for pin in expected:
    copied = os.path.join(gen, pin["path"])
    assert os.path.isfile(copied) and not os.path.islink(copied)
    assert hashlib.sha256(open(copied, "rb").read()).hexdigest() == pin["sha256"]
assert sorted(os.listdir(os.path.join(gen, "fonts"))) == ["a-Regular.otf", "b-Regular.ttf", "c[wght].ttc"]
policies = glob.glob(os.path.join(gen, "*.policy.json"))
assert policies
for policy in policies:
    fonts = json.load(open(policy))["document"]["fonts"]
    assert fonts == {"manifest": expected, "forbid_substitution": False}, policy
PY
then
  pass 'HWP_CERT_FONT_DIR pins a sorted, sha256-correct manifest in every policy and the index'
else
  fail 'font manifest' "expected exit 3 with pinned fonts, got $status; see $dest.log"
fi

# certify refuses a manifest pinning the same bytes twice, so the gate refuses
# such a directory before generating anything.
cp "$fontdir/a-Regular.otf" "$fontdir/a-Copy.ttf"
dest="$ROOT/fonts-dup"
mkdir -p "$dest"
status="$(run_gate "$dest" HWP_CERT_FONT_DIR="$fontdir")"
if [[ "$status" == '2' && ! -e "$dest/current" ]]; then
  pass 'a font directory holding duplicate bytes is refused with exit 2'
else
  fail 'duplicate fonts' "expected exit 2 and no current, got $status"
fi
rm "$fontdir/a-Copy.ttf"

# End to end: the real binary accepts an emitted policy, snapshots the pinned
# files and reaches the fonts rule with a non-empty resolution log. The stub
# artifact is not a document, so a committed sample stands in beside a copy of
# the policy. Synthetic bytes resolve nothing; the pins being accepted is the
# point. scripts/check.sh and CI build target/debug/hwp before this runs.
CERT_BIN="${HWP_REGRESSION_CERTIFY_BIN:-${CARGO_TARGET_DIR:-$REPO/target}/debug/hwp}"
certdir="$ROOT/certify"
mkdir -p "$certdir"
if [[ ! -x "$CERT_BIN" ]]; then
  fail 'certify end to end' "no hwp binary at $CERT_BIN; run cargo build -p hwp-cli first"
else
  cp -R "$ROOT/fonts/current/fonts" "$certdir/fonts"
  cp "$REPO/fixtures/samples/report-tables.hwpx" "$certdir/sample.hwpx"
  policy="$(find "$ROOT/fonts/current/" -maxdepth 1 -name '*.hwpx.policy.json' | head -1)"
  cp "$policy" "$certdir/sample.hwpx.policy.json"
  "$CERT_BIN" certify "$certdir/sample.hwpx" --policy "$certdir/sample.hwpx.policy.json" \
    --report "$certdir/report" >"$certdir.log" 2>&1 || true
  if python3 - "$certdir/report/report.json" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
rules = [rule for rule in report["checks"]["rules"] if rule["id"] == "fonts"]
assert len(rules) == 1, rules
assert report["render"]["fonts"], "empty font resolution log"
PY
  then
    pass 'hwp certify accepts an emitted policy and reaches the fonts rule'
  else
    fail 'certify end to end' "no report with a fonts rule and resolution log; see $certdir.log"
  fi
fi

if [[ "$failures" -ne 0 ]]; then
  echo "== hancom-regression self test: $failures failure(s) =="
  exit 1
fi
echo '== hancom-regression self test: OK =='
