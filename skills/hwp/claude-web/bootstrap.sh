#!/bin/sh
# hwp skill bootstrap for the claude.ai code-execution sandbox.
#
# The claude.ai sandbox network is registry-restricted, so this bundle ships the
# Linux x86_64 `ahwp` binary itself (bin/ahwp) instead of downloading it at runtime.
# This script installs the bundled binary into a writable location, keeps the `hwp`
# name SKILL.md's command list uses as an alias (#510 retires it at 2.0.0), and prints
# the MCP registration snippet.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
dest_dir="${HWP_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$dest_dir"
cp "$here/bin/ahwp" "$dest_dir/ahwp"
chmod +x "$dest_dir/ahwp"
# A relative symlink, so reinstalling over a bundle that shipped a `hwp` copy replaces it.
ln -sf ahwp "$dest_dir/hwp"

# Smoke check — fail loudly if the bundled binary cannot run here.
"$dest_dir/ahwp" --version >/dev/null

echo "installed: $dest_dir/ahwp (alias: $dest_dir/hwp)"
cat <<EOF

Register the MCP server (stdio) with:

  command: $dest_dir/ahwp
  args:    ["mcp", "--root", "<allowed-dir>"]

Always pass at least one --root so the file tools stay sandboxed.
EOF
