#!/usr/bin/env bash
# Fetch the exact OFL font bytes pinned by the public PDF parity manifest.
set -euo pipefail
cd "$(dirname "$0")/.."

font_dir="${HWP_PDF_PARITY_FONT_DIR:-fixtures/pdf-parity/fonts}"
max_fetch_bytes=33554432

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

fetch_font() {
  local filename="$1"
  local expected_sha256="$2"
  local font_path="$font_dir/$filename"
  local font_url="https://raw.githubusercontent.com/notofonts/noto-cjk/Sans2.004/Sans/OTF/Korean/$filename"
  local staged_font
  local actual_sha256

  if [ -f "$font_path" ] && [ "$(sha256 "$font_path")" = "$expected_sha256" ]; then
    echo "[pdf-parity-fonts] cached: $font_path"
    return
  fi

  mkdir -p "$font_dir"
  staged_font="$(mktemp "$font_dir/.$filename.XXXXXX")"
  if ! curl --fail --silent --show-error --location --proto '=https' --retry 3 \
    --max-time 120 --max-filesize "$max_fetch_bytes" --output "$staged_font" "$font_url"; then
    rm -f "$staged_font"
    return 1
  fi
  if [ "$(wc -c < "$staged_font")" -gt "$max_fetch_bytes" ]; then
    echo "PDF parity font exceeds the fetch bound: $filename" >&2
    rm -f "$staged_font"
    return 1
  fi
  actual_sha256="$(sha256 "$staged_font")"
  if [ "$actual_sha256" != "$expected_sha256" ]; then
    echo "PDF parity font hash mismatch: $filename" >&2
    echo "  expected $expected_sha256" >&2
    echo "  actual   $actual_sha256" >&2
    rm -f "$staged_font"
    return 1
  fi
  chmod 0644 "$staged_font"
  mv "$staged_font" "$font_path"
  echo "[pdf-parity-fonts] fetched: $font_path"
}

fetch_font NotoSansCJKkr-Regular.otf \
  6bcb2a0703aa137e874fc2dffa85f6c21ba9a67fa329e81b8c801663af7e992a
fetch_font NotoSansCJKkr-Bold.otf \
  26d0c6748500a0444844280b308f5b62c7ae92ac6c6ac88148e502dd211eb52a
