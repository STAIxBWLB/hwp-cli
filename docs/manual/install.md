[한국어](install.ko.md) · [English](install.md)

# Install and update

Every way to install `ahwp`, the font setup rendering needs, and how to update or roll back. The README carries the short path; this page has the detail, including serverless and container builds.

## Install ahwp

The command is `ahwp`. 2.0 removed the 1.x `hwp` compatibility name: replace `hwp` with `ahwp` in scripts and MCP client configurations.

### One-line script (macOS / Linux)

Installs a release binary without a Rust toolchain. After installation, `ahwp update` self-updates.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh
```

The default location is `~/.local/bin` (if it is not on PATH, the script says so). Change the
location or version with arguments or environment variables:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --dir /usr/local/bin --tag v1.6.0
HWP_INSTALL_DIR=~/bin sh scripts/install.sh
```

Archives are installed only after they match their `.sha256` asset.

### Homebrew (macOS / Linux)

The repository is its own tap (there is no separate `homebrew-*` repository).

```sh
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
brew install ahwp
ahwp --version
```

Upgrade with `brew update && brew upgrade ahwp`. A machine still on the old `hwp` formula moves
to `ahwp` on its next `brew update` through `formula_renames.json`, or with
`brew install ahwp`. Supported platforms are macOS (Apple Silicon and
Intel) and Linux x86_64.

### Pre-built binaries

Every [release](https://github.com/STAIxBWLB/agent-hwp/releases) attaches per-platform archives and a
`.sha256` checksum.

| Platform | Archive |
|---|---|
| Linux x86_64 | `ahwp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `ahwp-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `ahwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `ahwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `ahwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Extract and put `ahwp` on PATH (verify with `shasum -a 256 -c ahwp-*.sha256`).

### Serverless and containers (Vercel, AWS Lambda, Docker)

The Linux archive is cross-built against a **glibc 2.17** baseline, so it runs on Amazon Linux 2 and
2023, Debian 8+, RHEL/CentOS 7+ and anything newer — including Vercel's Node runtime and AWS Lambda,
whose glibc is older than the build runner's.

Fetch it in the build step instead of committing the binary; then the pinned tag is the only thing
to bump:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --tag v1.6.0 --dir ./bin
```

Run this from the build command (or a `prebuild` script) so the binary exists before the platform
collects the deployment bundle — Vercel `includeFiles`, Next.js `outputFileTracingIncludes`, or a
Docker layer. Fonts are not bundled: mount them and pass `--font-dir` (or `HWP_FONT_DIR`) for
rendering commands.

### Building from source

```sh
git clone git@github.com:STAIxBWLB/agent-hwp.git && cd agent-hwp
cargo build --release
cargo install --path crates/hwp-cli   # installs the `ahwp` binary
```

Requires Rust edition 2024 and `rust-version = 1.93` or newer.

### Fonts

**No fonts are bundled with the repository** (`fonts/` is gitignored). Text extraction and conversion
work without fonts; CJK glyphs are needed only for rendering, PDF output and hwp binary writing
(preview image).

| Used by | How fonts are specified |
|---|---|
| `render` / `diff` / `mcp` | `--font-dir <dir>` (repeatable) |
| `convert` / tests | `HWP_FONT_DIR` environment variable (falls back to the project `fonts/`) |

The structured corpus gate never uses system fonts; it uses only hash-pinned OFL fonts. The font
bytes are not committed. `scripts/fetch-corpus-fonts.sh` downloads them from the manifest's pinned
URL and verifies each against its SHA-256.

### Updating

```sh
ahwp update            # replace itself with the latest release (checksum-verified, atomic)
ahwp update --check    # report current and latest versions without replacing
ahwp update --tag v1.6.2   # roll back to a specific version (v1.6.0 or later)
```

It detects how it was installed. For the one-line script, a release archive or `cargo install`, it
replaces the running binary in place (sha256 check → temporary file in the same directory → rename,
restoring the original on failure). A Homebrew (Cellar) installation is delegated to
`brew upgrade ahwp`; because brew cannot pin versions, `--tag` is refused there.
