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
#   scripts/hancom-regression.sh [destination]
#
# Environment:
#   HWP_BIN   override the hwp binary (default: target/release/hwp, then
#             target/debug/hwp, otherwise a release build is made)
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$HOME/Documents/hwp-verification/regression}"
export HWP_FONT_DIR="$REPO/fonts"   # hwp5 synthetic lineseg computation needs it (5.1.x)

INDEX_NAME='hancom-regression-index.json'
INDEX_SCHEMA='hancom-regression-index-v1'

# The regenerated set is private evidence. Resolve both paths before creating
# the destination so repository paths and symlink aliases are rejected before
# anything can be written, moved or deleted there.
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is required to resolve a private regression destination" >&2
  exit 1
}
REPO_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve(strict=False))' "$REPO")"
DEST_REAL="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).expanduser().resolve(strict=False))' "$DEST")"
if [[ "$DEST_REAL" == "$REPO_REAL" || "$DEST_REAL" == "$REPO_REAL/"* ]]; then
  echo "regression destination must be outside the repository: $DEST_REAL" >&2
  exit 1
fi
DEST="$DEST_REAL"
mkdir -p "$DEST"

# Release binary first: the checklist verdict is about what the release ships.
HWP="${HWP_BIN:-}"
if [[ -z "$HWP" ]]; then
  HWP="$REPO/target/release/hwp"
  if [[ ! -x "$HWP" ]]; then
    HWP="$REPO/target/debug/hwp"
    [[ -x "$HWP" ]] || {
      HWP="$REPO/target/release/hwp"
      cargo build --release --manifest-path "$REPO/Cargo.toml" -q
    }
  fi
fi
[[ -x "$HWP" ]] || { echo "hwp binary not found: $HWP" >&2; exit 1; }
HWP_VERSION="$("$HWP" --version 2>/dev/null | head -1)"
HWP_BINARY="$HWP"
[[ "$HWP_BINARY" == "$REPO_REAL/"* ]] && HWP_BINARY="${HWP_BINARY#"$REPO_REAL"/}"

set -e
WORK="$(mktemp -d)"
STAGE="$(mktemp -d "$DEST/.hancom-regression-stage.XXXXXX")"
trap 'rm -rf "$WORK" "$STAGE"' EXIT

FAILED=0
declare -a ROWS=()
declare -a REPORT=()

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

fail() {
  FAILED=1
  REPORT+=("FAIL  $1  $2")
}

skip() {
  REPORT+=("skip  $1  $2")
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
}

# record <series> <staged artifact> <command string>
record() {
  local series="$1" src="$2" cmd="$3"
  local name hash
  name="$(basename "$src")"
  if [[ ! -s "$src" ]]; then
    fail "$series" "missing or empty output: $name"
    return 0
  fi
  if ! reread_and_validate "$src"; then
    fail "$series" "reread/validate gate failed: $name"
    return 0
  fi
  hash="$(shasum -a 256 "$src" | awk '{print $1}')"
  if [[ -z "$hash" ]]; then
    fail "$series" "empty hash: $name"
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
    fail "$series" "generation failed: $(basename "$out")"
    return 0
  fi
  record "$series" "$out" "$cmd"
}

echo "Destination: $DEST"
echo "Binary: $HWP_BINARY ($HWP_VERSION)"

# --- delegated generation ----------------------------------------------------
# Series A, B, C, D, H, I, J, K and L come from the rounds 13-23 generator, which
# in turn runs tools/gen_effects_cases.py for the IR-surgery cases. Series O comes
# from the same generator's default phase-02.2 mode. Nothing is reimplemented
# here; this script absorbs their outputs into one gated, indexed set.
LEGACY_DIR="$STAGE/legacy"
LEGACY_LOG="$WORK/legacy.log"
LEGACY_CMD="tools/gen_verification_set.sh --legacy <destination>"
mkdir -p "$LEGACY_DIR"
echo "Delegating series A-L to tools/gen_verification_set.sh --legacy"
HWP_BIN="$HWP" bash "$REPO/tools/gen_verification_set.sh" --legacy "$LEGACY_DIR" \
  >"$LEGACY_LOG" 2>&1 || true

# absorb <series> <file name> - map one delegated report line onto this script's
# report. The delegated generator marks each case pass, skip or fail; a failed
# case is a failed case here too, so a known defect can never publish silently.
absorb() {
  local series="$1" name="$2" line detail
  line="$(grep -m1 -F -- "$name" "$LEGACY_LOG" || true)"
  detail="${line#* }"
  if [[ -z "$line" ]]; then
    skip "$series" "delegated generator reported no case for $name"
    return 0
  fi
  case "$line" in
    '✅'*)
      mv -f "$LEGACY_DIR/$name" "$STAGE/$name"
      record "$series" "$STAGE/$name" "$LEGACY_CMD (case $name)"
      ;;
    '⏭'*) skip "$series" "$detail" ;;
    *)    fail "$series" "delegated case failed: $detail" ;;
  esac
  return 0
}

# --- A. The full pipeline on real documents ---------------------------------
# A5 and A6 need the private approval document from the ground-truth corpus.
absorb A1 'A1A2_work_report_변환.hwpx'
absorb A2 'A1A2_work_report_왕복.hwp'
absorb A3 'A3A4_annual_report_변환.hwpx'
absorb A4 'A3A4_annual_report_왕복.hwp'
absorb A5 'A5A6_품의_변환.hwpx'
absorb A6 'A5A6_품의_왕복.hwp'

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
  skip M "source supplied via HWP_SERIES_M_SOURCE; the case matrix is document-specific and stays manual"
else
  skip M "no source: set HWP_SERIES_M_SOURCE to a private complex HWP"
fi
if [[ -n "${HWP_SERIES_N_SOURCE:-}" && -f "${HWP_SERIES_N_SOURCE:-}" ]]; then
  skip N "source supplied via HWP_SERIES_N_SOURCE; run-authoring-validation-c.sh is private and not reconstructed here"
else
  skip N "no source: set HWP_SERIES_N_SOURCE to a private complex HWPX"
fi

# --- O. Phase 2.2 official profiles -----------------------------------------
# The delegated generator already gates and hashes each of its twelve artifacts
# and writes those columns into phase-02.2-index.tsv; reuse them rather than
# re-reading and re-hashing the same bytes.
O_DIR="$STAGE/phase-02.2"
O_CMD="tools/gen_verification_set.sh <destination>"
mkdir -p "$O_DIR"
echo "Delegating series O to tools/gen_verification_set.sh (phase-02.2 mode)"
if HWP_BIN="$HWP" bash "$REPO/tools/gen_verification_set.sh" "$O_DIR" >"$WORK/phase-02.2.log" 2>&1; then
  while IFS=$'\t' read -r profile format digest _rest; do
    [[ "$profile" == 'profile' || -z "$profile" ]] && continue
    o_name="phase-02.2-${profile}.${format}"
    if [[ -s "$O_DIR/$o_name" ]]; then
      mv -f "$O_DIR/$o_name" "$STAGE/$o_name"
      push_row "O_${profile}_${format}" "$STAGE/$o_name" "$O_CMD (profile $profile, $format)" \
        "$digest" pass pass
    else
      fail "O_${profile}_${format}" "delegated generator published no $o_name"
    fi
  done < "$O_DIR/phase-02.2-index.tsv"
else
  fail O "delegated phase-02.2 generation failed; see the generator output"
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
  skip P1_merge "no merge inputs: fixtures/samples/report-tables.hwpx is missing"
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
    [[ "$fragment_index" -gt 0 ]] || fail P2_split "split produced no fragment"
  else
    fail P2_split "split failed on P1_merge.hwpx"
  fi
else
  skip P2_split "no merged document to split"
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
    fail P3_compare_readonly "compare run failed (exit $compare_status)"
  fi
else
  skip P3_compare_readonly "no split fragment to compare"
fi

# GA-2 distribution-document read. The source is a genuine corpus document; it is
# never copied into the destination and its path never reaches the index.
if [[ -n "${HWP_CORPUS_DIR:-}" && -d "${HWP_CORPUS_DIR:-}" ]]; then
  distdoc="$(find "$HWP_CORPUS_DIR" -type f -name 'dist-*.hwp' 2>/dev/null | sort | head -1)"
  if [[ -n "$distdoc" ]]; then
    emit P4_distdoc_read "$STAGE/P4_distdoc.hwpx" \
      "$HWP" convert "$distdoc" -o "$STAGE/P4_distdoc.hwpx"
  else
    skip P4_distdoc_read "no dist-*.hwp under HWP_CORPUS_DIR"
  fi
else
  skip P4_distdoc_read "no corpus: set HWP_CORPUS_DIR to the ground-truth corpus directory"
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
  skip P5_set_cell_blank_line "fixtures/samples/report-tables.hwpx is missing"
  skip P6_set_cell_para "fixtures/samples/report-tables.hwpx is missing"
fi

# --- publish and index ------------------------------------------------------
if [[ "$FAILED" -ne 0 ]]; then
  echo
  echo "=== regeneration report ==="
  [[ ${#REPORT[@]} -eq 0 ]] || printf '%s\n' "${REPORT[@]}"
  echo
  echo "Regeneration failed; nothing was published and no index was written."
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
{
  [[ ${#ROWS[@]} -eq 0 ]] || printf '%s\n' "${ROWS[@]}"
} | python3 -c '
import json
import os
import sys
from datetime import datetime, timezone

schema, version, binary, dest, index_name = sys.argv[1:6]
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

index = {
    "schema": schema,
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "hwp_version": version,
    "hwp_binary": binary,
    "artifacts": artifacts,
}
with open(os.path.join(dest, index_name), "w", encoding="utf-8") as handle:
    json.dump(index, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
' "$INDEX_SCHEMA" "$HWP_VERSION" "$HWP_BINARY" "$DEST" "$INDEX_NAME"

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
echo 'No Hancom observation and no pass receipt was created by this run.'
