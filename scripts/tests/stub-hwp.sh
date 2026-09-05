#!/usr/bin/env bash
# Stand-in for the hwp binary, used only by scripts/tests/hancom-regression.sh.
#
# The regression gate's failure handling, coverage accounting and publish
# protocol are what the self test exercises; none of that depends on real HWP
# bytes, and building the release binary would make the gate too slow to run
# from scripts/check.sh. This stub answers the six subcommands the gate calls
# and writes plausible non-empty outputs.
set -uo pipefail

sub="${1:-}"
shift || true

case "$sub" in
  --version|-V) echo "hwp ${STUB_HWP_VERSION:-9.9.9}"; exit 0 ;;
  cat) exit 0 ;;                                            # no stderr: gate passes
  validate) printf '{"valid":true,"warnings":[]}\n'; exit 0 ;;
  compare) printf '{"stub":"compare","differences":0}\n'; exit 0 ;;
esac

out=''
outdir=''
loss=''
input=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o|--output) out="${2:-}"; shift 2 ;;
    --out-dir) outdir="${2:-}"; shift 2 ;;
    --loss-report) loss="${2:-}"; shift 2 ;;
    --from|--preset|--format|--set-cell|--set-cell-para) shift 2 ;;
    -*) shift ;;
    *) [[ -n "$input" ]] || input="$1"; shift ;;
  esac
done

if [[ "$sub" == split ]]; then
  [[ -n "$outdir" && -n "$input" ]] || exit 1
  stem="$(basename "${input%.*}")"
  ext="${input##*.}"
  mkdir -p "$outdir"
  printf 'stub fragment 1 of %s\n' "$stem" > "$outdir/${stem}-001.$ext"
  printf 'stub fragment 2 of %s\n' "$stem" > "$outdir/${stem}-002.$ext"
else
  [[ -n "$out" ]] || exit 1
  printf 'stub artifact from %s of %s\n' "$sub" "${input:-none}" > "$out"
fi

[[ -z "$loss" ]] || printf '{"contract":"preservation-report-v1","events":[]}\n' > "$loss"
exit 0
