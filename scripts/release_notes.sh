#!/usr/bin/env bash
# Extract a single version section from CHANGELOG.md and print it as the release body.
#   scripts/release_notes.sh 0.4.1      or      scripts/release_notes.sh v0.4.1
set -euo pipefail
cd "$(dirname "$0")/.."

version="${1:?usage: release_notes.sh <version>}"
version="${version#v}"

awk -v want="[$version]" '
    /^## / {
        if (found) exit
        # Compare only the bracketed token of "## [0.4.1]".
        if ($2 == want) { found = 1; next }
    }
    found { print }
' CHANGELOG.md | sed -e '/^---$/d' -e '/./,$!d' | awk '
    { lines[NR] = $0 }
    END { last = NR; while (last > 0 && lines[last] ~ /^[[:space:]]*$/) last--
          for (i = 1; i <= last; i++) print lines[i] }
'

# A missing section yields empty output, so set a non-zero exit code for the caller.
if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "CHANGELOG.md has no [$version] section" >&2
    exit 1
fi
