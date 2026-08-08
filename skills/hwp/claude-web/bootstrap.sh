#!/bin/sh
# hwp skill bootstrap for the claude.ai code-execution sandbox.
#
# The claude.ai sandbox network is registry-restricted, so this bundle ships the
# Linux x86_64 `hwp` binary itself (bin/hwp) instead of downloading it at runtime.
# This script installs the bundled binary into a writable location and prints the
# MCP registration snippet.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
dest_dir="${HWP_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$dest_dir"
cp "$here/bin/hwp" "$dest_dir/hwp"
chmod +x "$dest_dir/hwp"

# Smoke check — fail loudly if the bundled binary cannot run here.
"$dest_dir/hwp" --version >/dev/null

echo "installed: $dest_dir/hwp"
cat <<EOF

Register the MCP server (stdio) with:

  command: $dest_dir/hwp
  args:    ["mcp", "--root", "<allowed-dir>"]

Always pass at least one --root so the file tools stay sandboxed.
EOF
