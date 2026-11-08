#!/usr/bin/env bash
# Install apt packages on a hosted Ubuntu runner without making the package mirror a hard
# dependency of a pull request (#412 item 5).
#
#   scripts/ci-apt-install.sh <deb-cache-dir> <package>...
#
# 1. If <deb-cache-dir> holds .deb files from an earlier run (CI restores it with actions/cache),
#    put them in apt's archive directory and install the packages with downloads forbidden.
#    Success needs no network at all.
# 2. Otherwise, or when the cached set no longer fits this runner image (a dependency moved,
#    apt would have to download or downgrade), fall back to the mirror with the bounds the
#    inline steps used: the official archive pinned, short per-request timeouts, three attempts,
#    each capped by `timeout`. The failure that motivated those bounds was a mirror that accepts
#    the connection and then stalls (#119), which only a per-attempt `timeout` survives.
# 3. After a network install, the .deb files apt fetched are copied into <deb-cache-dir> so the
#    job's cache post-step can save them. A runner that keeps no archive leaves the cache empty,
#    which only means the next run uses the network again.
#
# The cache is an accelerator, never a source of truth: callers still assert what they need
# (the pdf-parity job compares `pdfinfo -v` with the manifest's pin after this script).
# APT_ARCHIVES and APT_RETRY_SLEEP exist for scripts/tests/ci-apt-install.sh; CI uses the defaults.
set -euo pipefail

archives="${APT_ARCHIVES:-/var/cache/apt/archives}"
retry_sleep="${APT_RETRY_SLEEP:-10}"

if [ "$#" -lt 2 ]; then
    echo "usage: ci-apt-install.sh <deb-cache-dir> <package>..." >&2
    exit 2
fi
cache_dir="$1"
shift

shopt -s nullglob
cached=("$cache_dir"/*.deb)
if [ "${#cached[@]}" -gt 0 ]; then
    # `--no-download` makes apt look for every archive it needs in its own archive directory,
    # even for a .deb named on the command line, and a fresh runner's directory is empty. Given
    # only paths, apt fails with "Unable to fetch some archives" (reproduced on Ubuntu 24.04:
    # exit 100), so the cached files are placed there first and apt finds them as if it had
    # fetched them. The package list is also given by name so apt resolves the same dependency
    # set the cached run installed.
    sudo mkdir -p "$archives"
    sudo cp -- "${cached[@]}" "$archives"/
    if sudo apt-get install -y --no-download --no-install-recommends "$@"; then
        echo "installed from the ${#cached[@]} cached .deb files in $cache_dir: $*"
        exit 0
    fi
    echo "cached .deb files do not fit this runner image; using the network" >&2
    # Take back what was placed: left in the archive directory, the stale files would be copied
    # into the cache again after the network install below.
    for file in "${cached[@]}"; do
        sudo rm -f -- "$archives/$(basename "$file")"
    done
fi

echo "https://archive.ubuntu.com/ubuntu" | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
sudo sed -i 's|http://azure\.archive\.ubuntu\.com/ubuntu|https://archive.ubuntu.com/ubuntu|g' \
    /etc/apt/sources.list /etc/apt/sources.list.d/*.sources 2>/dev/null || true
apt_get=(sudo apt-get -o Acquire::http::Timeout=10 -o Acquire::https::Timeout=10 -o Acquire::Retries=1)
for attempt in 1 2 3; do
    if timeout 120 "${apt_get[@]}" update && timeout 120 "${apt_get[@]}" install --no-install-recommends -y "$@"; then
        fetched=("$archives"/*.deb)
        if [ "${#fetched[@]}" -gt 0 ]; then
            mkdir -p "$cache_dir"
            rm -f "$cache_dir"/*.deb
            cp "${fetched[@]}" "$cache_dir"/
            echo "kept ${#fetched[@]} .deb files in $cache_dir for the next run"
        else
            echo "apt kept no .deb files; nothing to cache" >&2
        fi
        exit 0
    fi
    echo "apt install attempt ${attempt} failed or timed out; retrying" >&2
    sleep "$retry_sleep"
done
echo "$* could not be installed after 3 attempts" >&2
exit 1
