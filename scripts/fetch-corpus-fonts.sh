#!/usr/bin/env bash
# Fetch the pinned corpus fonts from upstream and verify them against the manifest SHA-256.
# Font bytes are never committed (large binaries). A file that is already present with the
# right hash is not downloaded again.
#
# HWP_CORPUS_FONT_CACHE=<dir>: a local cache whose file names are the SHA-256 of their content.
# A cache entry with the wanted hash is copied without touching the network (the copy is verified
# again), and a downloaded file is added to the cache after it is verified. CI keeps this
# directory with actions/cache so an outage of the font host does not turn a pull request red.
#
# Download time budget, per file: a cold cache pays this, a warm one never reaches it.
#   curl attempt   <= CONNECT_TIMEOUT to connect, <= MAX_TIME in total
#   whole retries  <= RETRY_MAX_TIME before a new attempt may start (--retry-max-time)
#   worst case      = RETRY_MAX_TIME + MAX_TIME per file (the last attempt starts just inside the
#                     retry window). Measured against a server that accepts and never answers.
# The CI step that runs this must allow files * (RETRY_MAX_TIME + MAX_TIME) plus the work after it.
set -euo pipefail
cd "$(dirname "$0")/.."

# A Windows runner may not have a python3 shim.
py=python3
command -v "$py" >/dev/null 2>&1 || py=python
manifest_path="${HWP_CORPUS_MANIFEST_PATH:-corpus/structured-v1/manifest.json}"

exec "$py" - "$manifest_path" <<'PY'
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import PurePosixPath
from urllib.parse import urlsplit, urlunsplit, quote

MAX_FETCH_BYTES = 32 * 1024 * 1024
CONNECT_TIMEOUT = 20  # seconds
MAX_TIME = 120  # seconds per attempt
RETRY_MAX_TIME = 150  # seconds before no new attempt starts; see the budget in the header
ALLOWED_HOST = "raw.githubusercontent.com"

manifest_path = sys.argv[1]
base = os.path.dirname(manifest_path)
cache_dir = os.environ.get("HWP_CORPUS_FONT_CACHE") or None
manifest = json.load(open(manifest_path, "rb"))


def checked_url(url):
    parts = urlsplit(url)
    if parts.scheme != "https" or parts.netloc != ALLOWED_HOST:
        sys.exit(f"corpus font source rejected: {url}")
    return parts


def checked_dest(rel):
    path = PurePosixPath(rel)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        sys.exit(f"corpus font path rejected: {rel}")
    return os.path.join(base, *path.parts)


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sibling_url(parts, name):
    parent = PurePosixPath(parts.path).parent
    return urlunsplit(parts._replace(path=str(parent / quote(name, safe=""))))


def fetch(url, dest, want):
    checked_url(url)
    if os.path.exists(dest) and sha256(dest) == want:
        return False
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    cached = os.path.join(cache_dir, want) if cache_dir else None
    if cached and os.path.isfile(cached) and sha256(cached) == want:
        # Verified against the manifest hash, so a damaged or swapped cache entry is not used.
        shutil.copyfile(cached, dest)
        os.chmod(dest, 0o644)
        return False
    handle, tmp = tempfile.mkstemp(dir=os.path.dirname(dest))
    os.close(handle)
    try:
        subprocess.run(
            ["curl", "--fail", "--silent", "--show-error", "--location",
             "--proto", "=https", "--retry", "3", "--retry-delay", "5", "--retry-all-errors",
             "--connect-timeout", str(CONNECT_TIMEOUT), "--max-time", str(MAX_TIME),
             "--retry-max-time", str(RETRY_MAX_TIME),
             "--max-filesize", str(MAX_FETCH_BYTES), "--output", tmp, url],
            check=True,
        )
        if os.path.getsize(tmp) > MAX_FETCH_BYTES:
            sys.exit(f"corpus font exceeds the fetch bound: {url}")
        got = sha256(tmp)
        if got != want:
            sys.exit(f"corpus font hash mismatch for {url}\n  expected {want}\n  got      {got}")
        os.chmod(tmp, 0o644)
        if cached:
            os.makedirs(cache_dir, exist_ok=True)
            shutil.copyfile(tmp, cached)
        os.replace(tmp, dest)
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)
    return True


for font in manifest["fonts"]:
    parts = checked_url(font["source_url"])
    targets = [
        (font["source_url"], font["path"], font["sha256"]),
        (sibling_url(parts, PurePosixPath(font["license_path"]).name),
         font["license_path"], font["license_sha256"]),
        (sibling_url(parts, PurePosixPath(font["metadata_path"]).name),
         font["metadata_path"], font["metadata_sha256"]),
    ]
    for src, rel, want in targets:
        action = "fetched" if fetch(src, checked_dest(rel), want) else "cached"
        print(f"[corpus-fonts] {action}: {rel}")
PY
