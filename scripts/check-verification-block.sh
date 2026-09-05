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
#   3. with `gh` available, the GitHub API says that run finished with conclusion `success`, that
#      it is a run of the release-readiness workflow, and that its head_sha is the commit being
#      released.
#
# There is no override flag. Missing evidence is a stop, not a warning: the block is the public
# claim that the release was evaluated, and a claim nobody can check is worse than none.
#
# Without `gh` (a developer machine with no GitHub CLI) checks 1 and 2 still run and check 3 is
# reported as unverified with a non-zero-free warning; the release workflow always has `gh`, so
# the published release is never gated on the operator's laptop having it.
#
# HWP_CHANGELOG overrides the file that is read (tests only).
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="STAIxBWLB/hwp-cli"
WORKFLOW_FILE="release-readiness.yml"
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
if ! command -v gh >/dev/null 2>&1; then
    echo "verification-block: WARNING - gh is not installed, so run $run_id was NOT verified" >&2
    echo "verification-block: block OK for $version (run $run_url, API check skipped)"
    exit 0
fi

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
[ "$head_sha" = "$sha" ] ||
    die "readiness run $run_id evaluated $head_sha, but the commit being released is $sha.
                    Dispatch $WORKFLOW_FILE against that commit and cite the new run."

echo "verification-block: OK for $version (run $run_id, $conclusion, head_sha $head_sha)"
