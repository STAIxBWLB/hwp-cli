#!/usr/bin/env bash
# 고정 코퍼스 폰트를 upstream에서 받아 manifest의 SHA-256으로 검증한다.
# 폰트 바이트는 커밋하지 않는다(대형 바이너리). 해시가 맞는 파일이 이미 있으면 재다운로드하지 않는다.
set -euo pipefail
cd "$(dirname "$0")/.."

# Windows 러너에는 python3 셰임이 없을 수 있다.
py=python3
command -v "$py" >/dev/null 2>&1 || py=python
manifest_path="${HWP_CORPUS_MANIFEST_PATH:-corpus/structured-v1/manifest.json}"

exec "$py" - "$manifest_path" <<'PY'
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import PurePosixPath
from urllib.parse import urlsplit, urlunsplit, quote

MAX_FETCH_BYTES = 32 * 1024 * 1024
ALLOWED_HOST = "raw.githubusercontent.com"

manifest_path = sys.argv[1]
base = os.path.dirname(manifest_path)
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
    handle, tmp = tempfile.mkstemp(dir=os.path.dirname(dest))
    os.close(handle)
    try:
        subprocess.run(
            ["curl", "--fail", "--silent", "--show-error", "--location",
             "--proto", "=https", "--retry", "3", "--max-time", "120",
             "--max-filesize", str(MAX_FETCH_BYTES), "--output", tmp, url],
            check=True,
        )
        if os.path.getsize(tmp) > MAX_FETCH_BYTES:
            sys.exit(f"corpus font exceeds the fetch bound: {url}")
        got = sha256(tmp)
        if got != want:
            sys.exit(f"corpus font hash mismatch for {url}\n  expected {want}\n  got      {got}")
        os.chmod(tmp, 0o644)
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
