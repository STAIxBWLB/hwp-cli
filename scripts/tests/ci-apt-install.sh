#!/usr/bin/env bash
# Self test for scripts/ci-apt-install.sh (#412 item 5), run by scripts/check.sh and the CI lint
# job. sudo, apt-get and timeout are stubs on PATH, so this exercises the script's decisions
# (cache first, network fallback, bounded retries, what it caches) on any machine, with no
# package manager and no network.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
script="$PWD/scripts/ci-apt-install.sh"

fail=0
check() { # <description> <rc>
    if [ "$2" = "0" ]; then
        printf '  ok   %s\n' "$1"
    else
        printf '  FAIL %s\n' "$1" >&2
        fail=1
    fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
bin="$tmp/bin"
mkdir -p "$bin"

# sudo and timeout run their command; tee/sed targets under /etc are redirected to a scratch file.
cat >"$bin/sudo" <<'STUB'
#!/usr/bin/env bash
case "$1" in
tee | sed) cat >/dev/null 2>&1 || true; exit 0 ;;
esac
exec "$@"
STUB
cat >"$bin/timeout" <<'STUB'
#!/usr/bin/env bash
shift
exec "$@"
STUB
# apt-get logs its verb and flags. STUB_FAIL_INSTALL_NO_DOWNLOAD makes an offline install fail;
# STUB_FAIL_ALL makes every online install fail; a successful online install leaves one .deb in
# the archives directory.
cat >"$bin/apt-get" <<'STUB'
#!/usr/bin/env bash
args="$*"
verb=""
for a in "$@"; do case "$a" in update | install) verb="$a" ;; esac; done
echo "$verb $args" >>"$STUB_LOG"
case "$args" in
*--no-download*)
    [ "${STUB_FAIL_INSTALL_NO_DOWNLOAD:-0}" = 1 ] && exit 100
    # Real apt behaviour, reproduced on Ubuntu 24.04 (exit 100, "Unable to fetch some archives"):
    # with --no-download, every archive the install needs must already sit in apt's archive
    # directory, whether or not a path to it is also given. A stub that says yes to anything
    # let the broken invocation pass this test while the hosted runner refused it.
    have=0
    for f in "$STUB_CACHE_DIR"/*.deb; do
        [ -e "$f" ] || continue
        [ -f "$APT_ARCHIVES/$(basename "$f")" ] || { echo "E: Unable to fetch some archives" >&2; exit 100; }
        have=$((have + 1))
    done
    [ "$have" -gt 0 ] || { echo "E: nothing cached" >&2; exit 100; }
    exit 0
    ;;
esac
[ "$verb" = install ] || exit 0
[ "${STUB_FAIL_ALL:-0}" = 1 ] && exit 100
mkdir -p "$APT_ARCHIVES"
echo deb >"$APT_ARCHIVES/fetched.deb"
exit 0
STUB
chmod +x "$bin"/*

# run <case dir> [env assignments...] -- <packages...>: runs the script, echoes its status.
run() {
    local dir="$1"
    shift
    local envs=()
    while [ "$1" != "--" ]; do
        envs+=("$1")
        shift
    done
    shift
    mkdir -p "$dir/archives"
    env PATH="$bin:$PATH" STUB_LOG="$dir/apt.log" STUB_CACHE_DIR="$dir/cache" APT_ARCHIVES="$dir/archives" APT_RETRY_SLEEP=0 \
        "${envs[@]}" bash "$script" "$dir/cache" "$@" >"$dir/out" 2>&1
    echo $?
}
# count <file> <fixed string>: how many lines contain the string (0 when the file is absent).
count() { if [ -f "$1" ]; then grep -c -F -e "$2" "$1" || true; else echo 0; fi; }

echo "-- no cache: network install, then the fetched .deb files are kept"
d="$tmp/nocache"
rc="$(run "$d" -- poppler-utils)"
check "installs" "$rc"
check "updates then installs once" "$([ "$(count "$d/apt.log" 'update ')" = 1 ] && [ "$(count "$d/apt.log" 'install ')" = 1 ] && echo 0 || echo 1)"
check "never forbids download" "$([ "$(count "$d/apt.log" --no-download)" = 0 ] && echo 0 || echo 1)"
check "keeps the fetched .deb for the next run" "$([ -f "$d/cache/fetched.deb" ] && echo 0 || echo 1)"

echo "-- warm cache: offline install, no update, no network"
d="$tmp/warm"
mkdir -p "$d/cache"
echo deb >"$d/cache/poppler.deb"
rc="$(run "$d" -- poppler-utils)"
check "installs" "$rc"
check "one offline install and nothing else" "$([ "$(wc -l <"$d/apt.log" | tr -d ' ')" = 1 ] && [ "$(count "$d/apt.log" --no-download)" = 1 ] && echo 0 || echo 1)"
check "does not refresh the cache from the archives" "$([ ! -f "$d/cache/fetched.deb" ] && echo 0 || echo 1)"
check "puts the cached .deb into apt's archive directory first" "$([ -f "$d/archives/poppler.deb" ] && echo 0 || echo 1)"
check "installs by package name, not by .deb path" "$(grep -q 'install .* poppler-utils' "$d/apt.log" && ! grep -q '\.deb' "$d/apt.log" && echo 0 || echo 1)"

echo "-- stale cache: the offline install fails, the network path takes over"
d="$tmp/stale"
mkdir -p "$d/cache"
echo deb >"$d/cache/old.deb"
rc="$(run "$d" STUB_FAIL_INSTALL_NO_DOWNLOAD=1 -- poppler-utils)"
check "still installs" "$rc"
check "tried offline first, then updated and installed" "$([ "$(count "$d/apt.log" --no-download)" = 1 ] && [ "$(count "$d/apt.log" 'update ')" = 1 ] && echo 0 || echo 1)"
check "replaced the stale .deb files" "$([ -f "$d/cache/fetched.deb" ] && [ ! -f "$d/cache/old.deb" ] && echo 0 || echo 1)"

echo "-- every attempt fails: bounded at three attempts, then a failure"
d="$tmp/dead"
rc="$(run "$d" STUB_FAIL_ALL=1 -- poppler-utils)"
check "fails" "$([ "$rc" != 0 ] && echo 0 || echo 1)"
check "exactly three install attempts" "$([ "$(count "$d/apt.log" 'install ')" = 3 ] && echo 0 || echo 1)"
check "names the packages in the message" "$(grep -q 'poppler-utils could not be installed after 3 attempts' "$d/out" && echo 0 || echo 1)"

echo "-- usage"
rc="$(env PATH="$bin:$PATH" bash "$script" onlycache >/dev/null 2>&1; echo $?)"
check "refuses a call without packages" "$([ "$rc" = 2 ] && echo 0 || echo 1)"

if [ "$fail" != 0 ]; then
    echo "ci-apt-install self test: FAILED" >&2
    exit 1
fi
echo "ci-apt-install self test: OK"
