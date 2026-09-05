#!/usr/bin/env bash
# scripts/check-verification-block.sh <version> <commit-ish>
#
# The release-boundary gate, shared by scripts/release.sh (before it bumps, commits or tags) and
# by .github/workflows/release.yml (before it publishes the tagged release), so both ends of the
# release check the same thing in the same way.
#
# It fails unless ALL of this holds:
#   1. the version's CHANGELOG section carries exactly one `<!-- verification:begin -->` /
#      `<!-- verification:end -->` pair, in that order, around a `**Verification**` block;
#   2. the block cites exactly one release-readiness run URL, and it is an Actions run URL of
#      this repository;
#   3. with `gh` available, the GitHub API says that run finished with conclusion `success` and
#      that it is a run of the release-readiness workflow;
#   4. that run's own record artifact says it evaluated the commit being released, and passed.
#
# Why 4 and not a head_sha comparison: release-readiness.yml checks out `inputs.ref`, which is
# independent of the ref the run was dispatched from. A dispatch run's head_sha is the DISPATCH
# ref's commit, so comparing it to the release commit checks the wrong thing in both directions -
# it rejects a correct run dispatched from main against a tag, and it accepts a run dispatched
# from the release commit that evaluated something else entirely. The binding therefore reads the
# run record's `evaluated_sha`. head_sha is still queried, and is required to match the record's
# `workflow_source_sha`, which ties the downloaded record to the cited run's dispatch.
# (This supersedes the head_sha check this script shipped with.)
#
# There is no override flag. Missing evidence is a stop, not a warning: the block is the public
# claim that the release was evaluated, and a claim nobody can check is worse than none. A record
# that is absent, expired, unparsable or short of a required field is a stop for the same reason.
#
# Without `gh` (a developer machine with no GitHub CLI) checks 1 and 2 still run and checks 3-4 are
# reported as unverified with a non-zero-free warning; the release workflow always has `gh`, so
# the published release is never gated on the operator's laptop having it.
#
# HWP_CHANGELOG overrides the file that is read, and HWP_READINESS_RECORD substitutes a local
# record file for the artifact download and the run API lookup (both tests only).
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="STAIxBWLB/hwp-cli"
WORKFLOW_FILE="release-readiness.yml"
CONTRACT="hwp-release-readiness-record-v1"
ARTIFACT_NAME="$CONTRACT"
RECORD_FILE="release-readiness-record.json"
BEGIN_MARKER="<!-- verification:begin -->"
END_MARKER="<!-- verification:end -->"

version="${1:-}"
commitish="${2:-}"
changelog="${HWP_CHANGELOG:-CHANGELOG.md}"
version="${version#v}"

if [ -z "$version" ] || [ -z "$commitish" ]; then
    echo "usage: scripts/check-verification-block.sh <version> <commit-ish>" >&2
    exit 2
fi

die() {
    echo "verification-block: $*" >&2
    exit 1
}

# --- 1. the marker-bounded block ----------------------------------------------------------------
section="$(awk -v want="[$version]" '
    /^## / { if (found) exit; if ($2 == want) { found = 1; next } }
    found  { print }' "$changelog")"
[ -n "$section" ] || die "$changelog has no [$version] section."

begins="$(grep -cF -- "$BEGIN_MARKER" <<<"$section" || true)"
ends="$(grep -cF -- "$END_MARKER" <<<"$section" || true)"
if [ "$begins" != 1 ] || [ "$ends" != 1 ]; then
    die "the [$version] section must carry exactly one $BEGIN_MARKER and one $END_MARKER
                    (found $begins and $ends). Regenerate it with
                    scripts/release_verification_block.sh $version <readiness-run-url>."
fi

begin_at="$(grep -nF -- "$BEGIN_MARKER" <<<"$section" | cut -d: -f1)"
end_at="$(grep -nF -- "$END_MARKER" <<<"$section" | cut -d: -f1)"
[ "$begin_at" -lt "$end_at" ] ||
    die "the [$version] Verification markers are out of order (begin at line $begin_at of the
                    section, end at $end_at)."

block="$(sed -n "$((begin_at + 1)),$((end_at - 1))p" <<<"$section")"
grep -q '^\*\*Verification\*\*' <<<"$block" ||
    die "the marked region of [$version] carries no **Verification** label."

urls="$(grep -oE "https://github\.com/$REPO/actions/runs/[0-9]+" <<<"$block" | sort -u || true)"
count="$(printf '%s' "$urls" | grep -c . || true)"
[ "$count" = 1 ] ||
    die "the [$version] Verification block must cite exactly one $REPO Actions run URL (found $count)."
run_url="$urls"
run_id="${run_url##*/}"

# --- 2. the commit the evidence must belong to --------------------------------------------------
if printf '%s' "$commitish" | grep -Eq '^[0-9a-f]{40}$'; then
    sha="$commitish"
else
    sha="$(git rev-parse --verify "${commitish}^{commit}" 2>/dev/null || true)"
    [ -n "$sha" ] || die "cannot resolve '$commitish' to a commit."
fi

# --- 3. the run itself ---------------------------------------------------------------------------
head_sha=""
record=""
if [ -n "${HWP_READINESS_RECORD:-}" ]; then
    # tests only: stand in for the artifact download and the run API lookup.
    record="$HWP_READINESS_RECORD"
    [ -f "$record" ] || die "HWP_READINESS_RECORD=$record does not exist."
    echo "verification-block: WARNING - HWP_READINESS_RECORD is set, so run $run_id was NOT queried" >&2
elif ! command -v gh >/dev/null 2>&1; then
    echo "verification-block: WARNING - gh is not installed, so run $run_id was NOT verified" >&2
    echo "verification-block: block OK for $version (run $run_url, API and record checks skipped)"
    exit 0
else
    api="$(gh api "repos/$REPO/actions/runs/$run_id" \
        --jq '[.status, .conclusion // "none", .name // "", .path // "", .head_sha // ""] | @tsv' 2>&1)" ||
        die "gh api repos/$REPO/actions/runs/$run_id failed:
                    $api"
    IFS=$'\t' read -r status conclusion name path head_sha <<<"$api"

    [ "$status" = completed ] ||
        die "readiness run $run_id is '$status', not completed."
    [ "$conclusion" = success ] ||
        die "readiness run $run_id concluded '$conclusion', not success."
    case "$path" in
    */"$WORKFLOW_FILE" | "$WORKFLOW_FILE") ;;
    *)
        # `name` is the workflow's display name; accept it when the path is unavailable.
        printf '%s' "$name" | grep -Eqi '^release[ -]readiness$' ||
            die "run $run_id is workflow '$name' ($path), not $WORKFLOW_FILE."
        ;;
    esac

    # --- 4. the record that run produced --------------------------------------------------------
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    download="$(gh run download "$run_id" --repo "$REPO" --name "$ARTIFACT_NAME" --dir "$tmpdir" 2>&1)" ||
        die "cannot download the $ARTIFACT_NAME artifact of run $run_id (expired, deleted or never
                    uploaded), so what that run evaluated cannot be established:
                    $download"
    record="$tmpdir/$RECORD_FILE"
    [ -f "$record" ] ||
        die "the $ARTIFACT_NAME artifact of run $run_id carries no $RECORD_FILE."
fi

command -v python3 >/dev/null 2>&1 ||
    die "python3 is required to read the run record."
python3 - "$record" "$sha" "$CONTRACT" "$run_id" "$head_sha" <<'PY' || exit 1
import json
import sys

path, want_sha, contract, run_id, head_sha = sys.argv[1:6]


def die(message):
    print(f"verification-block: {message}", file=sys.stderr)
    raise SystemExit(1)


try:
    with open(path, encoding="utf-8") as handle:
        record = json.load(handle)
except (OSError, ValueError) as error:
    die(f"the run record {path} could not be read as JSON: {error}")
if not isinstance(record, dict):
    die(f"the run record {path} is not a JSON object.")


def field(name):
    value = record.get(name)
    if not isinstance(value, str) or not value.strip():
        die(f"the run record of run {run_id} has no {name} field.")
    return value.strip()


if field("schema") != contract:
    die(f"the run record of run {run_id} declares schema {record['schema']!r}, not {contract!r}.")
evaluated = field("evaluated_sha").lower()
if evaluated != want_sha.lower():
    die(
        f"readiness run {run_id} evaluated {evaluated}, but the commit being released is"
        f" {want_sha}.\n                    Dispatch release-readiness.yml against that commit"
        " and cite the new run."
    )
source = field("workflow_source_sha").lower()
if head_sha and source != head_sha.lower():
    die(
        f"the record of run {run_id} says its workflow came from {source}, but the run's head_sha"
        f" is {head_sha}."
    )
if not field("run_url").rstrip("/").endswith(f"/{run_id}"):
    die(f"the record of run {run_id} cites run_url {record['run_url']!r}, which is another run.")
if field("result") != "pass":
    die(f"readiness run {run_id} recorded result {record['result']!r}, not 'pass'.")

gates = record.get("gates")
if not isinstance(gates, list) or not gates:
    die(f"the record of run {run_id} carries no gates.")
statuses = {}
for gate in gates:
    if not isinstance(gate, dict):
        die(f"the record of run {run_id} carries a gate that is not an object.")
    name, status = gate.get("name"), gate.get("status")
    if not isinstance(name, str) or not isinstance(status, str):
        die(f"the record of run {run_id} carries a gate with no name or no status.")
    statuses[name] = status
blocking = sorted(f"{n}={s}" for n, s in statuses.items() if s in ("fail", "pending"))
if blocking:
    die(f"readiness run {run_id} records non-passing gates: {', '.join(blocking)}.")
if statuses.get("clean-tree") != "pass":
    die(
        f"readiness run {run_id} records clean-tree={statuses.get('clean-tree')!r}, not 'pass', so"
        " it did not evaluate a clean checkout."
    )
print(f"verification-block: record OK (evaluated_sha {evaluated}, {len(gates)} gates, result pass)")
PY

echo "verification-block: OK for $version (run $run_id, evaluated_sha $sha, head_sha ${head_sha:-not queried})"
