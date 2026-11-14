[한국어](README.ko.md) · [English](README.md)

# agent-hwp

> A clean-room Rust toolkit to read, convert, render, write and AI-edit HWP 5.0 / HWPX documents with no Hancom Office or COM dependency. Runs on Linux / macOS / CI.

[![CI](https://github.com/STAIxBWLB/agent-hwp/actions/workflows/ci.yml/badge.svg)](https://github.com/STAIxBWLB/agent-hwp/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)

A Rust workspace for Korean HWP documents (`.hwp` HWP 5.0 binary, `.hwpx` OWPML/KS X 6101) with **no
external HWP library**. The CFB container, HWP record streams, OWPML XML, page layout and glyph
shaping are all implemented directly from the specification and from measurements of genuine files.
Nothing depends on Hancom Office or Windows COM automation, so it runs as-is on Linux/macOS servers
and in CI.

## What works and what does not

`ahwp` reads, converts, renders and writes `.hwp` and `.hwpx`, edits them in place while preserving
images, formatting and unparsed records, composes new documents from typed specs and Korean
official-document templates, and serves the same operations to AI agents over MCP. Charts and OLE
objects are not supported, equations render as an approximation, and only unmodified hwp-origin
documents round-trip byte-identically.

- **No Hancom parity claim.** The final verdict is always whether the file opens correctly in Hancom Office.

The full feature list, status table, limitations and roadmap are in
[What ahwp does and its limits](docs/manual/capabilities.md). Unimplemented features are catalogued
in [12-feature-gaps](docs/design/12-feature-gaps.md).

## Install

The command is `ahwp`. 2.0 removed the 1.x `hwp` compatibility name: replace `hwp` with `ahwp` in scripts and MCP client configurations.

```sh
# One-line script (macOS / Linux), installs to ~/.local/bin and verifies the .sha256
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh

# Homebrew (macOS / Linux); the repository is its own tap
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
brew install ahwp

# From source (Rust edition 2024, rust-version 1.93 or newer)
git clone https://github.com/STAIxBWLB/agent-hwp.git && cd agent-hwp
cargo install --path crates/hwp-cli

ahwp update            # self-update to the latest release (a brew install delegates to brew)
```

Pin a version or install directory for a build step:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --dir /usr/local/bin --tag v1.6.0
```

**No fonts are bundled.** Text extraction and conversion need none; rendering and PDF output need CJK
fonts passed with `--font-dir <dir>` or `HWP_FONT_DIR`. Pre-built binaries for every platform,
serverless and container builds (glibc 2.17 baseline), font details and rollback are in
[Install and update](docs/manual/install.md).

## Quickstart

```sh
ahwp info report.hwp                                  # format, version, properties
ahwp cat report.hwp --format markdown                 # extract text as markdown
ahwp grep "예산" report.hwp                           # search (exit 1 when nothing matches)
ahwp convert report.hwp -o report.hwpx                # hwp → hwpx
ahwp render report.hwp -o report.pdf --font-dir ./fonts   # searchable multi-page PDF
ahwp new -o out.hwpx --from notes.md                  # new document from markdown
ahwp new -o gian.hwpx --template gian-internal        # 기안문 with 두문/결문 frames
ahwp fill gian.hwpx -o final.hwpx --set "제목=예산 집행 계획"
ahwp edit form.hwp -o filled.hwp --replace "초안=>최종" --verify
ahwp mcp --root . --font-dir ./fonts                  # MCP stdio server for AI clients
```

More examples, and every command at a glance, are in [Commands at a glance](docs/manual/commands.md).

## Where to go next

| I want to | Read |
|---|---|
| See every feature, the status table, limits and roadmap | [What ahwp does and its limits](docs/manual/capabilities.md) |
| Install on another platform, in a container or serverless build, or set up fonts | [Install and update](docs/manual/install.md) |
| Look up a command or a flag | [Commands at a glance](docs/manual/commands.md) |
| Convert a document or extract text, markdown or provenance segments | [Markdown export](docs/manual/markdown-export.md) |
| Edit or fill a document, or bind form fields (workflows and conventions) | [Agent conventions](docs/manual/agent-conventions.md) |
| Author new documents from specs, write 공문 or regenerate from JSON | [Authoring documents](docs/manual/authoring.md) |
| Certify a written file or run the structured corpus | [Certification and the structured corpus](docs/manual/certification.md) |
| Use `ahwp` from an AI agent over MCP | [MCP server](docs/manual/mcp-server.md) |
| Set up a specific AI client (including Amazon Quick) or install the agent skill | [AI client integrations](docs/manual/ai-integrations.md) |
| Deploy the MCP server remotely | [Remote MCP deployment](docs/design/22-remote-mcp-deployment.md) |
| Find any other page | [The docs map](docs/README.md) |
| Contribute or build from source | [Development](docs/manual/development.md) |

## Versioning

agent-hwp follows [Semantic Versioning](https://semver.org) from 1.0.0. A breaking change to any of
the following needs a new major version:

- the `ahwp` CLI's commands, flags and exit codes;
- the MCP tools' names and arguments, over stdio and `ahwp serve`;
- the published JSON Schemas in [`schemas/`](schemas). Each schema also carries its own version in
  its file name (`edit-ops-v1`, `segment-envelope-v2`): an incompatible change ships as a new file.

Not covered: the wording of human-facing messages and `--help` text, rendered pixels, and the exact
bytes of written files.

## Contributing

Bug reports and pull requests are welcome.

- Work on a `feat/`, `fix/` or `docs/` branch and submit a PR. Do not push to main directly.
- Pass `scripts/check.sh` before opening a PR (the same gates as CI).
- Add round-trip or golden tests with new format features where possible.
- User-facing documentation is bilingual with English as the canonical side (`NAME.md`) and Korean
  as its pair (`NAME.ko.md`); update both in the same commit.
- Commit messages, PR text and release notes are written in English only.
- For specification questions, consult Hancom's official
  [HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do) (not bundled here).

## Acknowledgments

This product was developed with reference to Hancom's HWP document file format open specification,
[한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do)
(© Hancom Inc.).

The specification is copyrighted by Hancom Inc. Its open-document license permits free viewing,
copying and distribution but restricts distribution to the **unmodified original or copies thereof**,
so this repository does not bundle the specification or derivatives of it (extracted text, page
captures) and links only to the official distribution point. See [docs/README.md](docs/README.md).

Some test fixtures come from [hahnlee/hwp-rs](https://github.com/hahnlee/hwp-rs) (Apache-2.0); see
`fixtures/README.md` and the root `NOTICE`. The form-field rules of `ahwp slots --forms` and
`ahwp fill --forms` are ported from [chrisryugj/kordoc](https://github.com/chrisryugj/kordoc) (MIT);
see the root `NOTICE`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). Unless stated otherwise, code
contributed to this repository is understood to be released under the same two licenses.
