#!/usr/bin/env bash
# Differential comparison: our `ahwp cat` against pyhwp `hwp5txt`.
#
# Usage: tools/diff_pyhwp.sh [files...] (defaults to fixtures/hwp5/*.hwp).
#
# pyhwp replaces or omits table/text-box contents, so this reports whether our
# output contains pyhwp output rather than requiring exact equality.
# A difference can reveal our parser bug or a pyhwp limitation; both are useful.
set -uo pipefail
cd "$(dirname "$0")/.."

PYHWP=(uvx --python 3.9 --from pyhwp --with six hwp5txt)
FILES=("$@")
[ ${#FILES[@]} -eq 0 ] && FILES=(fixtures/hwp5/*.hwp)

normalize() {
    # Normalize whitespace and remove pyhwp placeholder lines.
    sed 's/<표>//g; s/<그림>//g' | tr -s ' \t\n' ' ' | sed 's/^ *//; s/ *$//'
}

fail=0
for f in "${FILES[@]}"; do
    ours=$(cargo run -q -p agent-hwp --bin ahwp -- cat "$f" 2>/dev/null | normalize)
    theirs=$("${PYHWP[@]}" "$f" 2>/dev/null | normalize)
    if [ -z "$theirs" ] && [ -z "$ours" ]; then
        echo "PASS(빈 문서)  $f"
    elif [ "$ours" = "$theirs" ]; then
        echo "PASS(일치)     $f"
    else
        # Check whether our output includes every word from pyhwp.
        missing=0
        while IFS= read -r word; do
            [ -z "$word" ] && continue
            case "$ours" in *"$word"*) ;; *) missing=$((missing+1));; esac
        done <<< "$(echo "$theirs" | tr ' ' '\n' | sort -u)"
        if [ "$missing" -eq 0 ]; then
            echo "PASS(포함)     $f  (우리 출력이 pyhwp의 상위집합)"
        else
            echo "FAIL           $f  (pyhwp 단어 ${missing}개 누락)"
            fail=1
        fi
    fi
done
exit $fail
