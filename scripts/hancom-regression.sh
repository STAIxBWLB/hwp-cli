#!/usr/bin/env bash
# Hancom acceptance-checklist regression regeneration.
#
# Regenerates every artifact docs/hancom-verification-checklist.md still
# verifies, from the release binary, into a private destination OUTSIDE this
# repository. Every artifact is gated on self-reread (hwp cat, no warning) and
# structural validation (hwp validate --json, valid with no warnings) before it
# is published. The sha256 index is written last, so an index file existing in
# the destination means the whole set passed.
#
# This script never creates a Hancom observation and never creates a pass
# receipt. A receipt exists only after a human opens the artifact in genuine
# Hancom Office and reports what happened.
#
# Usage:
#   scripts/hancom-regression.sh [--no-build] [destination]
#
#   --no-build  never invoke cargo; if target/release/hwp is absent, print the
#               exact build command and exit instead of building it here.
#
# Environment:
#   HWP_BIN   run this executable instead of target/release/hwp. An explicitly
#             supplied binary is recorded in the index with "explicit": true and
#             is exempt from the workspace-version match, because the caller has
#             taken responsibility for what it points at. With HWP_BIN unset the
#             gate uses target/release/hwp and nothing else: a debug build has
#             different optimization and different assertion behaviour, so
#             certifying its bytes would certify something the release never
#             ships.
#   HWP_REGRESSION_ALLOW_KNOWN_FAILURES
#             comma-separated checklist case ids (for example C5,C7,H2) whose
#             failure is already tracked as a GitHub issue. A listed case that
#             fails is recorded in the index as a known_failure instead of
#             failing the run; it is still not published and still not a pass.
#             Only an id in KNOWN_FAILURE_ISSUES below may be listed, and the
#             exclusion holds only for the failure that table describes: the
#             expected stage and a stable substring of the harness's own
#             message. A listed case that fails at another stage, or with a
#             different message, fails the run closed.
#   HWP_REGRESSION_GENERATOR
#             path to the delegated generator (default
#             tools/gen_verification_set.sh). For the self test only.
#
# Exit status:
#   0  clean pass: every expected case published, no known failure, no skip.
#   1  regression: a case failed, nothing was published, no index was written.
#   2  usage or precondition error: the run never started generating.
#   3  published, but NOT a clean pass: known failures and/or skips are present
#      and the index says `"clean": false`. A caller must not read 3 as a pass.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"

NO_BUILD=false
declare -a POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build) NO_BUILD=true ;;
    -h|--help) sed -n '2,/^set -uo/p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 0 ;;
    --) shift; POSITIONAL+=("$@"); break ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done
DEST="${POSITIONAL[0]:-$HOME/Documents/hwp-verification/regression}"
export HWP_FONT_DIR="$REPO/fonts"   # hwp5 synthetic lineseg computation needs it (5.1.x)

INDEX_NAME='hancom-regression-index.json'
INDEX_SCHEMA='hancom-regression-index-v1'

# --- known-failure hatch -----------------------------------------------------
# A case may be excluded only if it is in this table: the index entry has to
# name the issue that tracks it, so an exclusion is always an open defect on
# record and never a quiet allowance. Remove the row when the issue closes.
# Each row is <case>|<expected stage>|<fingerprint>|<issue>. Naming the case
# alone would excuse any failure of that case, including a new one: a crash
# during generation would be waved through under a fidelity defect's issue
# number. The stage says where the failure is allowed to happen and the
# fingerprint is a stable substring of the harness's own message, so the
# exclusion covers one known defect and nothing else.
KNOWN_FAILURE_VAR='HWP_REGRESSION_ALLOW_KNOWN_FAILURES'
KNOWN_FAILURE_ISSUES=(
  'C5|fidelity|밑줄모양(3) 소실|https://github.com/STAIxBWLB/hwp-cli/issues/236'
  'C7|fidelity|통합 효과 소실: 밑줄|https://github.com/STAIxBWLB/hwp-cli/issues/237'
  'H2|fidelity|0x25cb|https://github.com/STAIxBWLB/hwp-cli/issues/238'
)

# Stages a case can fail at. `fidelity` is the harness's own assertion about
# what survived the round trip; the others are the run breaking before that
# question is even reached.
# generation - the command produced nothing, or exited non-zero
# gate       - self-reread or structural validation refused the artifact
# fidelity   - the artifact exists and the harness says a feature was lost
# delegation - the delegated generator reported no outcome for the case
# coverage   - the case broke this script's own accounting rules
known_failure_spec() {
  local entry
  for entry in "${KNOWN_FAILURE_ISSUES[@]}"; do
    if [[ "$entry" == "$1|"* ]]; then
      printf '%s' "${entry#*|}"
      return 0
    fi
  done
  return 1
}

# Parse and validate the allow list before anything is generated: an id nobody
# tracks must stop the run, not silently widen the gate mid-way.
declare -a EXCLUDE=()
if [[ -n "${!KNOWN_FAILURE_VAR:-}" ]]; then
  IFS=',' read -r -a exclude_raw <<<"${!KNOWN_FAILURE_VAR}"
  for entry in "${exclude_raw[@]}"; do
    entry="${entry//[[:space:]]/}"
    [[ -n "$entry" ]] || continue
    if ! known_failure_spec "$entry" >/dev/null; then
      echo "$KNOWN_FAILURE_VAR: unknown case id '$entry'; only a case tracked in" >&2
      echo "KNOWN_FAILURE_ISSUES may be excluded. Tracked ids:" >&2
      printf '  %s\n' "${KNOWN_FAILURE_ISSUES[@]%%|*}" >&2
      exit 2
    fi
    EXCLUDE+=("$entry")
  done
fi

is_excluded() {
  local id
  if [[ ${#EXCLUDE[@]} -eq 0 ]]; then
    return 1
  fi
  for id in "${EXCLUDE[@]}"; do
    [[ "$id" == "$1" ]] && return 0
  done
  return 1
}

# The regenerated set is private evidence. Resolve both paths before creating
# the destination so repository paths and symlink aliases are rejected before
# anything can be written, moved or deleted there.
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required to resolve a private regression destination" >&2
  exit 2
}
REPO_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve(strict=False))' "$REPO")"
DEST_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).expanduser().resolve(strict=False))' "$DEST")"
if [[ "$DEST_REAL" == "$REPO_REAL" || "$DEST_REAL" == "$REPO_REAL/"* ]]; then
  echo "regression destination must be outside the repository: $DEST_REAL" >&2
  exit 2
fi
DEST="$DEST_REAL"
mkdir -p "$DEST"

# --- binary resolution -------------------------------------------------------
# The release binary, or an executable the caller took explicit responsibility
# for. There is no debug fallback: a release gate that silently certifies a
# debug build certifies bytes the release never produces.
RELEASE_BIN="$REPO/target/release/hwp"
BUILD_CMD="cargo build --release --locked --manifest-path $REPO/Cargo.toml"
HWP_EXPLICIT=false
HWP="${HWP_BIN:-}"
if [[ -n "$HWP" ]]; then
  HWP_EXPLICIT=true
else
  HWP="$RELEASE_BIN"
  if [[ ! -x "$HWP" ]]; then
    if [[ "$NO_BUILD" == true ]]; then
      echo "release binary missing: $RELEASE_BIN" >&2
      echo "--no-build was given; build it first with:" >&2
      echo "  $BUILD_CMD" >&2
      exit 2
    fi
    echo "Building the release binary: $BUILD_CMD"
    # shellcheck disable=SC2086  # BUILD_CMD is script-authored, word splitting is intended
    $BUILD_CMD -q || { echo "release build failed: $BUILD_CMD" >&2; exit 2; }
  fi
fi
[[ -x "$HWP" ]] || { echo "hwp binary not found or not executable: $HWP" >&2; exit 2; }

# Absolute path, version line and content hash of the executable that produced
# the set: without them an index cannot say which build it is evidence for.
HWP_PATH="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).expanduser().resolve(strict=False))' "$HWP")"
HWP_VERSION="$("$HWP" --version 2>/dev/null | head -1)"
HWP_SHA256="$(shasum -a 256 "$HWP" | awk '{print $1}')"
[[ -n "$HWP_VERSION" && -n "$HWP_SHA256" ]] || {
  echo "could not read --version or sha256 from: $HWP_PATH" >&2
  exit 2
}
WORKSPACE_VERSION="$(awk '
  /^\[workspace\.package\]/ { in_section = 1; next }
  /^\[/                     { in_section = 0 }
  in_section && /^version[[:space:]]*=/ { gsub(/[^0-9.]/, ""); print; exit }
' "$REPO/Cargo.toml")"
if [[ "$HWP_EXPLICIT" != true && "$HWP_VERSION" != "hwp $WORKSPACE_VERSION" ]]; then
  echo "binary version mismatch: $HWP_PATH reports '$HWP_VERSION'," >&2
  echo "but the workspace is at $WORKSPACE_VERSION. Rebuild with:" >&2
  echo "  $BUILD_CMD" >&2
  echo "or set HWP_BIN explicitly to certify a different build on purpose." >&2
  exit 2
fi
BINARY_JSON="$(python3 -c '
import json
import sys
print(json.dumps({
    "path": sys.argv[1],
    "version": sys.argv[2],
    "sha256": sys.argv[3],
    "explicit": sys.argv[4] == "true",
}))
' "$HWP_PATH" "$HWP_VERSION" "$HWP_SHA256" "$HWP_EXPLICIT")"

set -e
WORK="$(mktemp -d)"
STAGE="$(mktemp -d "$DEST/.hancom-regression-stage.XXXXXX")"
trap 'rm -rf "$WORK" "$STAGE"' EXIT

FAILED=0
declare -a ROWS=()
declare -a REPORT=()
declare -a KNOWN_FAILURES=()   # <case>\t<reason>\t<issue>, one per excluded case
declare -a OUTCOMES=()         # <case>\t<outcome>, exactly one per expected case
declare -a SKIPS=()            # <case>\t<reason code>\t<detail>, one per skip

# --- expected-case manifest --------------------------------------------------
# Every case this gate is responsible for. A case that ends the run without
# exactly one typed outcome (published, known_failure or skipped) is a hole in
# the evidence, so the run fails closed rather than publishing a set that only
# looks complete. Series E, F and G are deliberately absent: the checklist
# records that bisection as closed and its diagnostic outputs removed.
EXPECTED_CASES=(
  A1 A2 A3 A4 A5 A6
  B1 B2 B3 B4 B5
  C1 C2 C3 C4 C5 C6 C7 C8 C9
  D1 D2 D3
  H1 H2
  I1
  J1
  K1 K2 K3
  L1
  M N
  O_official_hwp O_official_hwpx
  O_report_hwp O_report_hwpx
  O_plan_hwp O_plan_hwpx
  O_notice_hwp O_notice_hwpx
  O_minutes_hwp O_minutes_hwpx
  O_press_hwp O_press_hwpx
  P1_merge P2_split P3_compare_readonly P4_distdoc_read
  P5_set_cell_blank_line P6_set_cell_para
)

# The only reasons a case may end as `skipped`. Anything else is a failure:
# an untyped skip is how a silent gap gets written into an index that then
# reads as complete coverage.
SKIP_REASONS=(private_input_missing series_not_regenerable)

# A published artifact name may carry a fragment suffix (hwp split emits one
# file per section); the case that owns those artifacts is the unsuffixed id.
case_of() {
  local series="$1"
  [[ "$series" =~ ^(.*)_[0-9]{3}$ ]] && series="${BASH_REMATCH[1]}"
  printf '%s' "$series"
}

has_outcome() {
  [[ ${#OUTCOMES[@]} -eq 0 ]] && return 1
  printf '%s\n' "${OUTCOMES[@]}" | cut -f1 | grep -qxF "$1"
}

outcome() {
  if has_outcome "$1"; then
    return 0   # a case with several artifacts reports its outcome once
  fi
  OUTCOMES+=("$1"$'\t'"$2")
}

# Self-reread and structure gate, identical to tools/gen_verification_set.sh.
reread_and_validate() {
  local artifact="$1"
  local reread_stderr validate_json
  if ! reread_stderr="$("$HWP" cat "$artifact" 2>&1 >/dev/null)"; then
    echo "self-reread failed: $(basename "$artifact")" >&2
    return 1
  fi
  if [[ -n "$reread_stderr" ]] && grep -qiE 'warning|warn|error|오류|경고|손상' <<<"$reread_stderr"; then
    echo "self-reread warning: $(basename "$artifact"): $reread_stderr" >&2
    return 1
  fi
  if ! validate_json="$("$HWP" validate --json "$artifact")"; then
    echo "structure validation failed: $(basename "$artifact")" >&2
    return 1
  fi
  python3 -c '
import json
import sys
result = json.load(sys.stdin)
if not result.get("valid") or result.get("warnings"):
    raise SystemExit(1)
' <<<"$validate_json" || return 1
}

# Render an argv as the reproducible command string stored in the index.
# Working, staging and destination prefixes are stripped and any surviving
# absolute path is redacted, so no private path ever reaches the index.
index_command() {
  local out='' arg
  for arg in "$@"; do
    arg="${arg//"$HWP"/hwp}"
    arg="${arg//"$WORK"\//}"
    arg="${arg//"$STAGE"\//}"
    arg="${arg//"$DEST"\//}"
    arg="${arg//"$REPO_REAL"\//}"
    [[ "$arg" == /* ]] && arg='<redacted-path>'
    case "$arg" in
      *[![:alnum:]/._=:@-]*) arg="'${arg//\'/\'\\\'\'}'" ;;
    esac
    out+="${out:+ }$arg"
  done
  printf '%s' "$out"
}

# A failure of a case listed in HWP_REGRESSION_ALLOW_KNOWN_FAILURES is recorded
# against its tracking issue and the run continues. It is still not published
# and still not a pass; any other failure fails the whole run closed.
# fail <case> <stage> <detail>. An excluded case is excused only for the exact
# failure its table row describes; anything else about that case still fails the
# run closed.
fail() {
  local case_id="$1" stage="$2" detail="$3"
  local expected_stage fingerprint issue
  if is_excluded "$case_id"; then
    IFS='|' read -r expected_stage fingerprint issue <<<"$(known_failure_spec "$case_id")"
    if [[ "$stage" != "$expected_stage" ]]; then
      FAILED=1
      REPORT+=("FAIL  $case_id  excluded at stage '$expected_stage' but failed at '$stage': $detail")
      outcome "$case_id" failed
      return 0
    fi
    if [[ "$detail" != *"$fingerprint"* ]]; then
      FAILED=1
      REPORT+=("FAIL  $case_id  excluded for '$fingerprint' but failed differently: $detail")
      outcome "$case_id" failed
      return 0
    fi
    KNOWN_FAILURES+=("$case_id"$'\t'"$stage"$'\t'"$fingerprint"$'\t'"$detail"$'\t'"$issue")
    REPORT+=("known  $case_id  [$stage] $detail")
    outcome "$case_id" known_failure
    return 0
  fi
  FAILED=1
  REPORT+=("FAIL  $case_id  [$stage] $detail")
  outcome "$case_id" failed
}

# skip <case> <reason code> <detail>. The reason code is closed vocabulary: an
# unrecognized one is itself a failure, so a new kind of hole cannot enter the
# index disguised as an ordinary skip.
skip() {
  local case_id="$1" reason="$2" detail="$3" allowed
  for allowed in "${SKIP_REASONS[@]}"; do
    if [[ "$reason" == "$allowed" ]]; then
      SKIPS+=("$case_id"$'\t'"$reason"$'\t'"$detail")
      REPORT+=("skip  $case_id  [$reason] $detail")
      outcome "$case_id" skipped
      return 0
    fi
  done
  fail "$case_id" coverage "unrecognized skip reason '$reason': $detail"
}

# push_row <series> <staged artifact> <command> <sha256> <reread> <validate>
# A row is one tab-separated line, so a command carrying a newline or a tab (a
# --set-cell value with a blank-line paragraph break does) is escaped rather
# than allowed to split the row.
push_row() {
  local cmd="$3"
  cmd="${cmd//$'\n'/\\n}"
  cmd="${cmd//$'\t'/\\t}"
  ROWS+=("$1"$'\t'"$(basename "$2")"$'\t'"$2"$'\t'"$4"$'\t'"$5"$'\t'"$6"$'\t'"$cmd")
  REPORT+=("pass  $1  $(basename "$2")")
  outcome "$(case_of "$1")" published
}

# record <series> <staged artifact> <command string>
record() {
  local series="$1" src="$2" cmd="$3"
  local name hash
  name="$(basename "$src")"
  if [[ ! -s "$src" ]]; then
    fail "$series" generation "missing or empty output: $name"
    return 0
  fi
  if ! reread_and_validate "$src"; then
    fail "$series" gate "reread/validate gate failed: $name"
    return 0
  fi
  hash="$(shasum -a 256 "$src" | awk '{print $1}')"
  if [[ -z "$hash" ]]; then
    fail "$series" gate "empty hash: $name"
    return 0
  fi
  push_row "$series" "$src" "$cmd" "$hash" pass pass
  return 0
}

# emit <series> <staged artifact> <argv...> - generate then record.
emit() {
  local series="$1" out="$2"
  shift 2
  local cmd
  cmd="$(index_command "$@")"
  if ! "$@" >/dev/null 2>&1; then
    fail "$series" generation "generation failed: $(basename "$out")"
    return 0
  fi
  record "$series" "$out" "$cmd"
}

echo "Destination: $DEST"
echo "Binary: $HWP_PATH ($HWP_VERSION, sha256 ${HWP_SHA256:0:12}..., explicit=$HWP_EXPLICIT)"

# --- delegated generation ----------------------------------------------------
# Series A, B, C, D, H, I, J, K and L come from the rounds 13-23 generator, which
# in turn runs tools/gen_effects_cases.py for the IR-surgery cases. Series O comes
# from the same generator's default phase-02.2 mode. Nothing is reimplemented
# here; this script absorbs their outputs into one gated, indexed set.
GENERATOR="${HWP_REGRESSION_GENERATOR:-$REPO/tools/gen_verification_set.sh}"
LEGACY_DIR="$STAGE/legacy"
LEGACY_LOG="$WORK/legacy.log"
LEGACY_CMD="tools/gen_verification_set.sh --legacy <destination>"
mkdir -p "$LEGACY_DIR"
echo "Delegating series A-L to tools/gen_verification_set.sh --legacy"
LEGACY_STATUS=0
HWP_BIN="$HWP" bash "$GENERATOR" --legacy "$LEGACY_DIR" \
  >"$LEGACY_LOG" 2>&1 || LEGACY_STATUS=$?

# absorb <case> <file name> [<fallback log key>] - map one delegated report line
# onto this script's outcome table. The delegated generator marks each case
# pass, skip or fail. A case the generator said nothing about is not a skip: an
# early exit looks exactly like that from here, so it fails closed.
absorb() {
  local case_id="$1" name="$2" fallback="${3:-}" line detail
  line="$(grep -m1 -F -- "$name" "$LEGACY_LOG" || true)"
  if [[ -z "$line" && -n "$fallback" ]]; then
    line="$(grep -m1 -F -- "$fallback" "$LEGACY_LOG" || true)"
  fi
  if [[ -z "$line" ]]; then
    fail "$case_id" delegation "delegated generator reported no outcome for $name (exit $LEGACY_STATUS)"
    return 0
  fi
  detail="${line#* }"
  detail="${detail# }"
  case "$line" in
    '✅'*)
      mv -f "$LEGACY_DIR/$name" "$STAGE/$name"
      record "$case_id" "$STAGE/$name" "$LEGACY_CMD (case $name)"
      ;;
    '⏭'*) skip "$case_id" private_input_missing "$detail" ;;
    *)    fail "$case_id" fidelity "delegated case failed: $detail" ;;
  esac
  return 0
}

# --- A. The full pipeline on real documents ---------------------------------
# A5 and A6 need the private approval document from the ground-truth corpus.
absorb A1 'A1A2_work_report_변환.hwpx' 'A1A2_work_report'
absorb A2 'A1A2_work_report_왕복.hwp' 'A1A2_work_report'
absorb A3 'A3A4_annual_report_변환.hwpx' 'A3A4_annual_report'
absorb A4 'A3A4_annual_report_왕복.hwp' 'A3A4_annual_report'
absorb A5 'A5A6_품의_변환.hwpx' 'A5A6_품의'
absorb A6 'A5A6_품의_왕복.hwp' 'A5A6_품의'

# --- B. Minimal per-feature files -------------------------------------------
absorb B1 'B1_책갈피.hwp'
absorb B2 'B2_책갈피.hwpx'
absorb B3 'B3_하이퍼링크.hwp'
absorb B4 'B4_하이퍼링크.hwpx'
absorb B5 'B5_복합.hwp'

# --- C. Character effects and summary information ---------------------------
absorb C1 'C1_그림자.hwpx'
absorb C2 'C2_외곽선.hwpx'
absorb C3 'C3_양각음각.hwpx'
absorb C4 'C4_첨자.hwpx'
absorb C5 'C5_밑줄모양.hwpx'
absorb C6 'C6_번호형식.hwpx'
absorb C7 'C7_글자효과통합.hwpx'
absorb C8 'C8_요약정보.hwp'
absorb C9 'C9_요약정보.hwpx'

# --- D. Seal stamping and user tabs -----------------------------------------
absorb D1 'D1_도장.hwpx'
absorb D2 'D2_도장.hwp'
absorb D3 'D3_사용자탭.hwpx'

# Series E, F and G are not regenerated. The checklist records that bisection as
# closed on 2026-07-18 (the raw 0x09 body tab, fixed) and its diagnostic outputs
# were removed from the folder to prevent false readings.

# --- H and I. The markdown import round-trip --------------------------------
absorb H1 'H1_md왕복.hwpx'
absorb H2 'H2_md왕복.hwp'
absorb I1 'I1_md이미지코드.hwpx'

# --- J. Page border cross-conversion ----------------------------------------
# The source is a private bordered document from the ground-truth corpus.
absorb J1 'J1_쪽테두리.hwpx'

# --- K. Cell merging and column manipulation --------------------------------
absorb K1 'K1_셀병합.hwpx'
absorb K2 'K2_셀병합.hwp'
absorb K3 'K3_열조작.hwpx'

# --- L. Equation emission ----------------------------------------------------
absorb L1 'L1_수식.hwpx'

# --- M and N. Private source-preserving and package-surgical edits ----------
# Both series need a private, genuinely complex source document that is not
# committed, and the series N harness (run-authoring-validation-c.sh) is private
# and absent from this repository. Their case matrices also address anchors that
# only exist inside that specific document, so they cannot be generated from a
# path alone. They stay a manual step; the variables below only record whether
# the owner has a source on hand.
if [[ -n "${HWP_SERIES_M_SOURCE:-}" && -f "${HWP_SERIES_M_SOURCE:-}" ]]; then
  skip M series_not_regenerable "source supplied via HWP_SERIES_M_SOURCE; the case matrix is document-specific and stays manual"
else
  skip M series_not_regenerable "no source: set HWP_SERIES_M_SOURCE to a private complex HWP"
fi
if [[ -n "${HWP_SERIES_N_SOURCE:-}" && -f "${HWP_SERIES_N_SOURCE:-}" ]]; then
  skip N series_not_regenerable "source supplied via HWP_SERIES_N_SOURCE; run-authoring-validation-c.sh is private and not reconstructed here"
else
  skip N series_not_regenerable "no source: set HWP_SERIES_N_SOURCE to a private complex HWPX"
fi

# --- O. Phase 2.2 official profiles -----------------------------------------
# The delegated generator already gates and hashes each of its twelve artifacts
# and writes those columns into phase-02.2-index.tsv; reuse them rather than
# re-reading and re-hashing the same bytes.
O_DIR="$STAGE/phase-02.2"
O_CMD="tools/gen_verification_set.sh <destination>"
mkdir -p "$O_DIR"
echo "Delegating series O to tools/gen_verification_set.sh (phase-02.2 mode)"
if HWP_BIN="$HWP" bash "$GENERATOR" "$O_DIR" >"$WORK/phase-02.2.log" 2>&1; then
  while IFS=$'\t' read -r profile format digest _rest; do
    [[ "$profile" == 'profile' || -z "$profile" ]] && continue
    o_name="phase-02.2-${profile}.${format}"
    if [[ -s "$O_DIR/$o_name" ]]; then
      mv -f "$O_DIR/$o_name" "$STAGE/$o_name"
      push_row "O_${profile}_${format}" "$STAGE/$o_name" "$O_CMD (profile $profile, $format)" \
        "$digest" pass pass
    else
      fail "O_${profile}_${format}" generation "delegated generator published no $o_name"
    fi
  done < "$O_DIR/phase-02.2-index.tsv"
else
  # Fail every case the delegation owns. A single "O" line would leave twelve
  # expected cases with no outcome, which is exactly the hole this table closes.
  for o_profile in official report plan notice minutes press; do
    for o_format in hwp hwpx; do
      fail "O_${o_profile}_${o_format}" delegation "delegated phase-02.2 generation failed; see the generator output"
    done
  done
fi

# --- P. Document-level workflows and Phase 3.1 cell paragraphs (D-01) -------
# These have no checklist series of their own yet; they carry a P prefix so they
# sort after O and stay last in the index.
SAMPLE="$REPO/fixtures/samples/report-tables.hwpx"
if [[ -f "$SAMPLE" ]]; then
  cp "$SAMPLE" "$WORK/merge_a.hwpx"
  cat > "$WORK/merge_b.md" <<'MD'
# 병합 검증 문서

첫째 문단입니다. 병합된 두 번째 구역의 본문이 살아 있는지 확인합니다.

둘째 문단입니다. 구역 경계 뒤의 문단 흐름을 확인합니다.
MD
  "$HWP" new --from "$WORK/merge_b.md" -o "$WORK/merge_b.hwpx" >/dev/null 2>&1 || true
fi

if [[ -s "${WORK}/merge_a.hwpx" && -s "${WORK}/merge_b.hwpx" ]]; then
  emit P1_merge "$STAGE/P1_merge.hwpx" \
    "$HWP" merge "$WORK/merge_a.hwpx" "$WORK/merge_b.hwpx" \
    -o "$STAGE/P1_merge.hwpx" --loss-report "$WORK/merge-loss.json"
else
  fail P1_merge generation "no merge inputs: the committed fixture fixtures/samples/report-tables.hwpx is missing"
fi

# Split the merged document: one section per merge input, so the fragments are
# the round trip of the merge case.
if [[ -s "$STAGE/P1_merge.hwpx" ]]; then
  mkdir -p "$WORK/split"
  if "$HWP" split "$STAGE/P1_merge.hwpx" --out-dir "$WORK/split" \
      --loss-report "$WORK/split-loss.json" >/dev/null 2>&1; then
    fragment_index=0
    for fragment in "$WORK"/split/*.hwpx; do
      [[ -e "$fragment" ]] || continue
      fragment_index=$((fragment_index + 1))
      fragment_name="$(basename "$fragment")"
      mv -f "$fragment" "$STAGE/$fragment_name"
      record "$(printf 'P2_split_%03d' "$fragment_index")" "$STAGE/$fragment_name" \
        "hwp split P1_merge.hwpx --out-dir <directory> --loss-report <file>"
    done
    [[ "$fragment_index" -gt 0 ]] || fail P2_split generation "split produced no fragment"
  else
    fail P2_split generation "split failed on P1_merge.hwpx"
  fi
else
  fail P2_split generation "no merged document to split: P1_merge did not publish"
fi

# hwp compare is read-only: it never opens in Hancom, so this row carries the
# comparison report's own hash and no receipt policy. Run it from the working
# directory with relative arguments so no absolute path lands in the report.
if [[ -s "$STAGE/P1_merge-001.hwpx" ]]; then
  cp "$STAGE/P1_merge-001.hwpx" "$WORK/P1_merge-001.hwpx"
  compare_status=0
  (cd "$WORK" && "$HWP" compare P1_merge-001.hwpx merge_a.hwpx --format json) \
    > "$STAGE/P3_compare.json" 2>/dev/null || compare_status=$?
  # diff(1) exit codes: 0 identical, 1 differences found, 2 the run itself failed.
  if [[ "$compare_status" -le 1 && -s "$STAGE/P3_compare.json" ]]; then
    compare_hash="$(shasum -a 256 "$STAGE/P3_compare.json" | awk '{print $1}')"
    push_row P3_compare_readonly "$STAGE/P3_compare.json" \
      "hwp compare P1_merge-001.hwpx merge_a.hwpx --format json" \
      "$compare_hash" 'n/a (read-only)' 'n/a (read-only)'
  else
    fail P3_compare_readonly generation "compare run failed (exit $compare_status)"
  fi
else
  fail P3_compare_readonly generation "no split fragment to compare: P2_split did not publish"
fi

# GA-2 distribution-document read. The source is a genuine corpus document; it is
# never copied into the destination and its path never reaches the index.
if [[ -n "${HWP_CORPUS_DIR:-}" && -d "${HWP_CORPUS_DIR:-}" ]]; then
  distdoc="$(find "$HWP_CORPUS_DIR" -type f -name 'dist-*.hwp' 2>/dev/null | sort | head -1)"
  if [[ -n "$distdoc" ]]; then
    emit P4_distdoc_read "$STAGE/P4_distdoc.hwpx" \
      "$HWP" convert "$distdoc" -o "$STAGE/P4_distdoc.hwpx"
  else
    skip P4_distdoc_read private_input_missing "no dist-*.hwp under HWP_CORPUS_DIR"
  fi
else
  skip P4_distdoc_read private_input_missing "no corpus: set HWP_CORPUS_DIR to the ground-truth corpus directory"
fi

# Phase 3.1 multi-paragraph cells: a blank line in a --set-cell value becomes two
# paragraphs in the cell, and --set-cell-para shapes every paragraph of that cell.
if [[ -f "$SAMPLE" ]]; then
  cell_value="$(printf '첫째 문단입니다.\n\n둘째 문단입니다.')"
  emit P5_set_cell_blank_line "$STAGE/P5_셀문단.hwpx" \
    "$HWP" edit "$SAMPLE" -o "$STAGE/P5_셀문단.hwpx" --set-cell "0:1:1=$cell_value"
  emit P6_set_cell_para "$STAGE/P6_셀문단모양.hwpx" \
    "$HWP" edit "$SAMPLE" -o "$STAGE/P6_셀문단모양.hwpx" \
    --set-cell "0:1:1=$cell_value" \
    --set-cell-para "0:1:1=>line-spacing:180,align:center"
else
  fail P5_set_cell_blank_line generation "the committed fixture fixtures/samples/report-tables.hwpx is missing"
  fail P6_set_cell_para generation "the committed fixture fixtures/samples/report-tables.hwpx is missing"
fi

# --- reconcile the delegated exit status ------------------------------------
# The legacy generator exits non-zero when one of its cases failed, and every
# such case has already become a `failed` or `known_failure` outcome above. A
# non-zero exit with no failing case is something else entirely - a crash, a
# missing interpreter, a destination refusal - and it must not be swallowed.
if [[ "$LEGACY_STATUS" -ne 0 ]]; then
  legacy_failures=0
  for delegated_case in A1 A2 A3 A4 A5 A6 B1 B2 B3 B4 B5 \
    C1 C2 C3 C4 C5 C6 C7 C8 C9 D1 D2 D3 H1 H2 I1 J1 K1 K2 K3 L1; do
    if [[ ${#OUTCOMES[@]} -gt 0 ]] && printf '%s\n' "${OUTCOMES[@]}" \
      | grep -qE "^${delegated_case}"$'\t'"(failed|known_failure)$"; then
      legacy_failures=$((legacy_failures + 1))
    fi
  done
  if [[ "$legacy_failures" -eq 0 ]]; then
    FAILED=1
    REPORT+=("FAIL  delegation  tools/gen_verification_set.sh --legacy exited $LEGACY_STATUS with no failing case to explain it")
  fi
fi

# --- coverage ----------------------------------------------------------------
# Exactly one typed outcome per expected case. Zero outcomes means the run lost
# a case somewhere; more than one means two code paths claimed it. Either way
# the set is not the evidence the index would say it is.
for expected_case in "${EXPECTED_CASES[@]}"; do
  outcome_count=0
  if [[ ${#OUTCOMES[@]} -gt 0 ]]; then
    outcome_count="$(printf '%s\n' "${OUTCOMES[@]}" | cut -f1 | grep -cxF "$expected_case" || true)"
  fi
  if [[ "$outcome_count" -ne 1 ]]; then
    FAILED=1
    REPORT+=("FAIL  $expected_case  expected exactly one outcome, got $outcome_count")
  fi
done
for recorded_case in ${OUTCOMES[@]+"${OUTCOMES[@]}"}; do
  recorded_case="${recorded_case%%$'\t'*}"
  if ! printf '%s\n' "${EXPECTED_CASES[@]}" | grep -qxF "$recorded_case"; then
    FAILED=1
    REPORT+=("FAIL  $recorded_case  reported an outcome but is not in the expected-case manifest")
  fi
done

# A listed case that did not fail is a stale entry: say so, so the list does not
# outlive the defect it excuses.
for excluded_id in ${EXCLUDE[@]+"${EXCLUDE[@]}"}; do
  excluded_seen=0
  for recorded in ${KNOWN_FAILURES[@]+"${KNOWN_FAILURES[@]}"}; do
    [[ "${recorded%%$'\t'*}" == "$excluded_id" ]] && excluded_seen=1
  done
  [[ "$excluded_seen" -eq 1 ]] && continue
  if [[ ${#ROWS[@]} -gt 0 ]] && printf '%s\n' "${ROWS[@]}" | cut -f1 | grep -qxF "$excluded_id"; then
    REPORT+=("listed but passed  $excluded_id  remove it from $KNOWN_FAILURE_VAR")
  else
    REPORT+=("listed but not run  $excluded_id  the case never reported a result")
  fi
done

# Report how many cases the hatch excused, in both outcomes, so a run that is
# not a clean pass can never read as one.
outcome_summary() {
  local published_cases=0
  if [[ ${#OUTCOMES[@]} -gt 0 ]]; then
    published_cases="$(printf '%s\n' "${OUTCOMES[@]}" | grep -c $'\tpublished$' || true)"
  fi
  echo "Cases: ${#EXPECTED_CASES[@]} expected, $published_cases published, ${#KNOWN_FAILURES[@]} known failure(s), ${#SKIPS[@]} skipped"
  if [[ ${#KNOWN_FAILURES[@]} -gt 0 ]]; then
    echo "  known failures: $(printf '%s\n' "${KNOWN_FAILURES[@]}" | cut -f1 | paste -sd, -) (excluded by $KNOWN_FAILURE_VAR)"
  fi
  if [[ ${#SKIPS[@]} -gt 0 ]]; then
    echo "  skipped: $(printf '%s\n' "${SKIPS[@]}" | cut -f1 | paste -sd, -)"
  fi
  if [[ ${#KNOWN_FAILURES[@]} -gt 0 || ${#SKIPS[@]} -gt 0 ]]; then
    echo '  A run with known failures or skips is not a clean pass.'
  fi
}

# --- publish and index ------------------------------------------------------
if [[ "$FAILED" -ne 0 ]]; then
  echo
  echo "=== regeneration report ==="
  [[ ${#REPORT[@]} -eq 0 ]] || printf '%s\n' "${REPORT[@]}"
  echo
  echo "Regeneration failed; nothing was published and no index was written."
  outcome_summary
  echo 'No Hancom observation and no pass receipt was created by this run.'
  exit 1
fi

# Publish every staged artifact first. The index is the completion receipt, so
# it is written only after the last move succeeds.
published=0
while IFS=$'\t' read -r name src; do
  [[ -n "$name" ]] || continue
  mv -f "$src" "$DEST/$name"
  published=$((published + 1))
done < <([[ ${#ROWS[@]} -eq 0 ]] || printf '%s\n' "${ROWS[@]}" | cut -f2,3)

# Then the certify wiring: one policy per artifact naming the one receipt that
# will bind to it, and an empty receipts directory for those receipts to land in.
# The index is the completion receipt, so it is written after both.
KNOWN_FILE="$WORK/known-failures.tsv"
: >"$KNOWN_FILE"
[[ ${#KNOWN_FAILURES[@]} -eq 0 ]] || printf '%s\n' "${KNOWN_FAILURES[@]}" >"$KNOWN_FILE"
SKIP_FILE="$WORK/skips.tsv"
: >"$SKIP_FILE"
[[ ${#SKIPS[@]} -eq 0 ]] || printf '%s\n' "${SKIPS[@]}" >"$SKIP_FILE"
{
  [[ ${#ROWS[@]} -eq 0 ]] || printf '%s\n' "${ROWS[@]}"
} | python3 -c '
import json
import os
import sys
from datetime import datetime, timezone

schema, binary_json, dest, index_name, known_path, skip_path, excluded_by = sys.argv[1:8]
binary = json.loads(binary_json)
receipts = os.path.join(dest, "receipts")
os.makedirs(receipts, exist_ok=True)

artifacts = []
for line in sys.stdin:
    line = line.rstrip("\n")
    if not line:
        continue
    series, name, _src, digest, reread, validate, command = line.split("\t")
    row = {
        "file": name,
        "series": series,
        "command": command,
        "sha256": digest,
        "internal_reread": reread,
        "internal_validate": validate,
        "policy": None,
    }
    # A read-only report never opens in Hancom, so it gets no receipt policy.
    if not reread.startswith("n/a"):
        receipt_rel = os.path.join("receipts", name + ".receipt.json")
        policy_name = name + ".policy.json"
        row["policy"] = policy_name
        row["receipt"] = receipt_rel
        # The policy sits beside its artifact rather than in a subdirectory:
        # certify resolves receipt relative to the policy file and refuses any
        # path holding a ".." component.
        policy = {
            "schema_version": "1.0",
            "document": {
                # The receipt attests a Hancom open. Font substitution on the
                # verifier host must not decide that question.
                "fonts": {"forbid_substitution": False},
                "hancom_open": {"receipt": receipt_rel, "require_pass": True},
            },
            "render": {},
        }
        with open(os.path.join(dest, policy_name), "w", encoding="utf-8") as handle:
            json.dump(policy, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
    artifacts.append(row)

# Cases excluded by the known-failure hatch. They have no artifact row: they
# failed, were not published, and are not a pass. The entry names the issue that
# tracks the defect and the variable that let the run continue.
known_failures = []
with open(known_path, encoding="utf-8") as handle:
    for line in handle:
        line = line.rstrip("\n")
        if not line:
            continue
        case, stage, fingerprint, reason, issue = line.split("\t")
        known_failures.append(
            {
                "case": case,
                "stage": stage,
                "fingerprint": fingerprint,
                "reason": reason,
                "issue": issue,
                "excluded_by": excluded_by,
            }
        )

# Cases that produced no artifact because an input this repository does not
# carry was absent. Recording them is what lets the index prove complete
# coverage: published + known_failures + skips accounts for every expected case.
skips = []
with open(skip_path, encoding="utf-8") as handle:
    for line in handle:
        line = line.rstrip("\n")
        if not line:
            continue
        case, reason, detail = line.split("\t")
        skips.append({"case": case, "reason": reason, "detail": detail})

index = {
    "schema": schema,
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    # Which build this set is evidence for. Without the hash an index cannot
    # distinguish two runs of the same version number.
    "binary": binary,
    # A clean pass is every expected case published. Anything excused or
    # skipped leaves a hole in the evidence, and a consumer that only checks
    # whether an index file exists must still be able to see that hole.
    "clean": not known_failures and not skips,
    "artifacts": artifacts,
    "known_failures": known_failures,
    "skips": skips,
}
with open(os.path.join(dest, index_name), "w", encoding="utf-8") as handle:
    json.dump(index, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
' "$INDEX_SCHEMA" "$BINARY_JSON" "$DEST" "$INDEX_NAME" "$KNOWN_FILE" "$SKIP_FILE" "$KNOWN_FAILURE_VAR"

echo
echo "=== regeneration report ==="
[[ ${#REPORT[@]} -eq 0 ]] || printf '%s\n' "${REPORT[@]}"
echo
echo "Published: $published artifact(s)"
echo "Index: $DEST/$INDEX_NAME"
echo "Receipts directory (empty): $DEST/receipts"
echo
echo 'After a real Hancom observation, write that artifact its receipt at the path'
echo 'its index row names, then prove the binding with one command per artifact:'
echo "  hwp certify $DEST/<artifact> \\"
echo "    --policy $DEST/<artifact>.policy.json \\"
echo '    --report <a fresh report directory>'
echo 'The receipt binding is the "hancom_open" check in that report.'
echo
outcome_summary
echo 'No Hancom observation and no pass receipt was created by this run.'

# A published-but-incomplete run gets its own exit status. Sharing 0 with a
# clean pass is how "we shipped with three known failures" becomes "we shipped
# green" one release later.
if [[ ${#KNOWN_FAILURES[@]} -gt 0 || ${#SKIPS[@]} -gt 0 ]]; then
  echo 'Exit 3: the index says "clean": false. This run is not a clean pass.'
  exit 3
fi
exit 0
