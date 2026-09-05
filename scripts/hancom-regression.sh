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
  ROWS+=("$series"$'\t'"$name"$'\t'"$src"$'\t'"$hash"$'\t'pass$'\t'pass$'\t'"$cmd")
  REPORT+=("pass  $series  $name")
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

# The synthetic base document carries the anchors the B series edits target.
cat > "$WORK/base.md" <<'MD'
# 실기 검증 문서

제목 문단입니다. 이 문장 여기에 하이퍼링크가 삽입됩니다.

둘째 문단으로 책갈피와 링크가 문서 흐름에 정상 배치되는지 확인합니다.
MD
if ! "$HWP" new --from "$WORK/base.md" -o "$WORK/base.hwp" >/dev/null 2>&1; then
  fail base "base.hwp generation failed"
fi

# --- B. Minimal per-feature files -------------------------------------------
if [[ -s "$WORK/base.hwp" ]]; then
  emit B1 "$STAGE/B1_책갈피.hwp" \
    "$HWP" edit "$WORK/base.hwp" -o "$STAGE/B1_책갈피.hwp" \
    --create-bookmark "제목=>검증책갈피"
else
  skip B1 "base.hwp unavailable"
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

{
  [[ ${#ROWS[@]} -eq 0 ]] || printf '%s\n' "${ROWS[@]}"
} | python3 -c '
import json
import sys
from datetime import datetime, timezone

schema, version, binary, out = sys.argv[1:5]
artifacts = []
for line in sys.stdin:
    line = line.rstrip("\n")
    if not line:
        continue
    series, name, _src, digest, reread, validate, command = line.split("\t")
    artifacts.append(
        {
            "file": name,
            "series": series,
            "command": command,
            "sha256": digest,
            "internal_reread": reread,
            "internal_validate": validate,
        }
    )
index = {
    "schema": schema,
    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "hwp_version": version,
    "hwp_binary": binary,
    "artifacts": artifacts,
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(index, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
' "$INDEX_SCHEMA" "$HWP_VERSION" "$HWP_BINARY" "$DEST/$INDEX_NAME"

echo
echo "=== regeneration report ==="
[[ ${#REPORT[@]} -eq 0 ]] || printf '%s\n' "${REPORT[@]}"
echo
echo "Published: $published artifact(s)"
echo "Index: $DEST/$INDEX_NAME"
echo 'No Hancom observation and no pass receipt was created by this run.'
