[한국어](README.ko.md) · [English](README.md)

# hwp-cli

> A clean-room Rust toolkit to read, convert, render, write and AI-edit HWP 5.0 / HWPX documents with no Hancom Office or COM dependency. Runs on Linux / macOS / CI.

[![CI](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)

A Rust workspace for Korean HWP documents (`.hwp` HWP 5.0 binary, `.hwpx` OWPML/KS X 6101) with **no
external HWP library**. The CFB container, HWP record streams, OWPML XML, page layout and glyph
shaping are all implemented directly from the specification and from measurements of genuine files.
Nothing depends on Hancom Office or Windows COM automation, so it runs as-is on Linux/macOS servers
and in CI.

## Features

- **Reading and text extraction** from hwp/hwpx to plain / markdown / HTML / JSON (full IR) / CSV.
  Tables, images, headers/footers and unparsed records are preserved during parsing, and Hancom
  distribution documents (배포용문서) are decrypted and read like any other file.
- **Password-protected input** `hwp cat`, `convert` and `render` accept `--password` or the safer
  `--password-stdin` for the evidenced HWP5 and HWPX password profiles. The matching MCP tools use
  a per-call password field with no session cache. Wrong or absent passwords return one stable,
  redacted refusal.
- **Format conversion** hwp ↔ hwpx, hwp/hwpx ↔ markdown, hwp/hwpx ↔ HTML and hwp/hwpx ↔ JSON (IR),
  all through a shared document model (IR), plus one-way export to DOCX, ODT, CSV and plain text.
- **Rendering** hwp/hwpx → PNG / SVG / PDF. Stored line layout (PARA_LINE_SEG) is used when present
  and synthesized otherwise. PDF output is a single multi-page document with subset-embedded fonts,
  so text is selectable, searchable and copyable (ToUnicode CMap).
- **Writing, including the hwp binary** Both hwpx package writing and HWP 5.0 binary (CFB) writing
  are implemented. Unmodified hwp-origin documents round-trip byte-identically at the decompressed
  stream level.
- **Structured authoring** Deterministic HWP/HWPX generation from DocumentSpec v1/v2 and
  TemplateSpec/Data v1 (`compose`, `template`). Only a typed AST is allowed, with no string
  interpolation and no expression evaluation.
- **Certification and corpus gate** `certify` certifies package validation, repeated import and
  bounded native rendering, then publishes a report atomically. `corpus` generates a frozen corpus
  twice and requires the document bytes, semantics and render hashes to match.
- **AI editing** A read → edit → rewrite loop over the JSON IR. Text replacement, table cell values
  and field filling are applied while images, formatting and unparsed records are preserved.
- **Official-document authoring (공문서)** Six canonical profiles (`official`, `report`, `plan`,
  `notice`, `minutes`, `press`), eight embedded document templates (`hwp new --template`), native
  두문/결문 frames, the statutory eight-level item marks, preset table styling
  (`hwp edit --style-tables`) and a notation and structure linter (`hwp lint`, ten rules).
- **Document-level workflows** Merge several documents into one (`hwp merge`, one Section per
  input), split one into per-section or per-page-range fragments (`hwp split`), and report the
  paragraph and structural differences between two documents (`hwp compare`).
- **MCP server** A dependency-free (serde_json only) stdio MCP server exposing 22 tools to
  desktop clients, including Amazon Quick Desktop.

## Implementation status

| Area | Status |
|---|---|
| hwp/hwpx reading; text, markdown and JSON extraction | Implemented |
| hwpx writing, HWP 5.0 binary writing | Implemented |
| PNG/SVG/PDF rendering (tables, images, text boxes, shapes, foot/endnotes, page numbers, text effects) | Implemented |
| Structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) | Implemented, including merged tables |
| DocumentSpec v1/v2 and TemplateSpec v1 composition | Implemented |
| Certification (`certify`) and structured corpus gate (`corpus`) | Implemented |
| Document-level workflows (`merge`, `split`, `compare`) | Implemented; spot-checked in Hancom on 2026-08-29 (see [12-feature-gaps](docs/design/12-feature-gaps.md) GM-3/GM-4/GM-8) |
| Official-document authoring (profiles, templates, frames, lint, table styling) | Implemented |
| MCP server (22 tools) | Implemented |
| Distribution documents (배포용문서) | Read |
| HTML conversion | Structural parity with markdown; CSS mapping of character and paragraph shapes is still coarse |
| Equations | Approximated as a box plus the script |
| Charts, OLE | Not supported |
| Password-protected HWP5/HWPX | Read, convert and render with a supplied password |
| Certificate-, signature- or DRM-protected documents | Reading is refused with a specific reason |

### Limitations

- **Scope of lossless round-trip** Only unmodified hwp-origin documents are guaranteed byte-identical.
  Documents that were edited, or that came from hwpx/markdown, are rewritten through the writer's
  synthesis path as **semantically equivalent** (text and structure preserved). hwpx writing is always
  semantically equivalent (template-based regeneration). A fully lossless round-trip including images
  in JSON requires the `--embed-bin` path.
- **Cross-format semantic conversion** Tables, images, sections, headers/footers, text boxes, fields
  and foot/endnotes are parsed and rendered semantically, but shapes, equations, charts and OLE render
  without hwp↔hwpx record synthesis (they are preserved round-trip within the same format).
- **Fields** Existing field values can be filled and a new click-here field can be created after an
  anchor, but arbitrary field kinds cannot be created.
- **Protection scope** Password support is limited to the genuine-corpus-evidenced HWP5
  EncryptVersion 4 and HWPX ODF AES-256 profiles. Certificate encryption, signatures and DRM remain
  explicit refusals. Passwords are invocation-local and are never written to reports or receipts.
- **Render hashes** The hashes recorded by the corpus are observations for that OS and architecture,
  not a claim of cross-platform pixel equivalence.
- **No Hancom parity claim** The final verdict is always whether the file opens correctly in Hancom Office.

## Roadmap

The numbered catalog of unimplemented features, each with its code and specification evidence and a
difficulty estimate, is [docs/design/12-feature-gaps.md](docs/design/12-feature-gaps.md). That file
is the detailed source; the list below is only the shape of the work.

1. **Editor engine surface** What an embeddable editor needs and this binary does not yet expose: a
   versioned fine-grained segment envelope, render-side layout geometry for hit-testing, jpeg/webp
   raster output, and a typed JSON edit-ops channel with addressed operations and edit feedback
   (the GO series).
2. **Specification coverage** The HWP 5.0 rev1.3 body has been reconstructed as reviewable Markdown
   (§1 to §4.4) and audited against the implementation; the errata that audit produced are
   catalogued in [19 §1](docs/design/19-hwp5-spec-supplement.md). Still outstanding: the OWPML /
   KS X 6101, equation and chart specifications have not been obtained, and the bit-level
   parser/writer value comparison is only partly done.
3. **HTML fidelity** Footnote markers, merged-cell colspan/rowspan and in-cell blocks already match
   the markdown path. What remains is CSS mapping of character and paragraph shapes, columns and
   headers/footers.

Windows is not a roadmap item: ubuntu, macOS and Windows are all required CI gates today.

## Installation

### One-line script (macOS / Linux)

Installs a release binary without a Rust toolchain. After installation, `hwp update` self-updates.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

The default location is `~/.local/bin` (if it is not on PATH, the script says so). Change the
location or version with arguments or environment variables:

```sh
curl -fsSL .../install.sh | sh -s -- --dir /usr/local/bin --tag v0.17.0
HWP_INSTALL_DIR=~/bin sh scripts/install.sh
```

Archives are installed only after they match their `.sha256` asset.

### Homebrew (macOS / Linux)

The repository is its own tap (there is no separate `homebrew-*` repository).

```sh
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/hwp-cli
brew install hwp
hwp --version
```

Upgrade with `brew update && brew upgrade hwp`. Supported platforms are macOS (Apple Silicon and
Intel) and Linux x86_64.

### Pre-built binaries

Every [release](https://github.com/STAIxBWLB/hwp-cli/releases) attaches per-platform archives and a
`.sha256` checksum.

| Platform | Archive |
|---|---|
| Linux x86_64 | `hwp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `hwp-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `hwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `hwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Extract and put `hwp` on PATH (verify with `shasum -a 256 -c hwp-*.sha256`).

### Serverless and containers (Vercel, AWS Lambda, Docker)

The Linux archive is cross-built against a **glibc 2.17** baseline, so it runs on Amazon Linux 2 and
2023, Debian 8+, RHEL/CentOS 7+ and anything newer — including Vercel's Node runtime and AWS Lambda,
whose glibc is older than the build runner's.

Fetch it in the build step instead of committing the binary; then the pinned tag is the only thing
to bump:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh \
  | sh -s -- --tag v0.17.0 --dir ./bin
```

Run this from the build command (or a `prebuild` script) so the binary exists before the platform
collects the deployment bundle — Vercel `includeFiles`, Next.js `outputFileTracingIncludes`, or a
Docker layer. Fonts are not bundled: mount them and pass `--font-dir` (or `HWP_FONT_DIR`) for
rendering commands.

### Building from source

```sh
git clone git@github.com:STAIxBWLB/hwp-cli.git && cd hwp-cli
cargo build --release
cargo install --path crates/hwp-cli   # installs the `hwp` binary
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
hwp update            # replace itself with the latest release (checksum-verified, atomic)
hwp update --check    # report current and latest versions without replacing
hwp update --tag v0.9.0   # roll back to a specific version
```

It detects how it was installed. For the one-line script, a release archive or `cargo install`, it
replaces the running binary in place (sha256 check → temporary file in the same directory → rename,
restoring the original on failure). A Homebrew (Cellar) installation is delegated to
`brew upgrade hwp`; because brew cannot pin versions, `--tag` is refused there.

## Quickstart

```sh
# Diagnostics: format, version, properties, streams
hwp info report.hwp

# Extract body text
hwp cat report.hwp                       # plain text
hwp cat report.hwp --format markdown     # markdown
hwp cat report.hwp --format json         # full IR (JSON)
hwp cat report.hwp --format csv          # tables as CSV

# Search body text (grep semantics: exit 1 when nothing matches)
hwp grep "예산" report.hwp

# Convert (format inferred from the output extension)
hwp convert report.hwp   -o report.hwpx  # hwp → hwpx (tables, images, headers preserved)
hwp convert report.hwpx  -o report.hwp   # hwpx → hwp binary
hwp convert report.hwp   -o report.md    # images extracted to report.media/
hwp convert report.hwp   -o doc.json --embed-bin   # self-contained JSON with embedded images

# Render
hwp render report.hwp -o page.png --dpi 150
hwp render report.hwp -o report.pdf --font-dir ./fonts   # single searchable multi-page PDF

# Create a new document
hwp new -o out.hwpx --from notes.md
hwp new -o out.hwp  --from doc.json

# Structured authoring
hwp compose spec.yaml -o report.hwpx --report
hwp template report-template.yaml --data report-data.json -o report.hwpx

# Official documents (공문서)
hwp new --list-templates                              # the eight embedded templates
hwp new -o gian.hwpx --template gian-internal         # 기안문, 두문/결문 frames included
hwp slots gian.hwpx                                   # the {{slots}} left to fill
hwp fill gian.hwpx -o final.hwpx --set "제목=예산 집행 계획"
hwp lint final.hwpx --strict                          # 표기법 and structure rules

# Edit (images, formatting and unparsed records preserved)
hwp fields form.hwp                        # list fillable fields
hwp edit form.hwp -o filled.hwp \
    --replace "초안=>최종" \
    --set-cell "0:1:2=12,300원" \
    --set-field "수신처=홍길동" --verify

# Structural editing (insert/delete paragraphs, table rows/columns, cell merge/split)
hwp edit report.hwp -o out.hwp \
    --insert-para "개요=>추가 설명 문단입니다." \
    --add-row "0" --add-col "1" --merge-cells "0:1:1:2:2" --verify

# Render fidelity comparison against a Hancom reference PNG
hwp diff report.hwp --ref hancom_p1.png --page 1 --dpi 150 --font-dir ./fonts

# MCP stdio server
hwp mcp --font-dir ./fonts
```

## Command reference

The full generated reference is [docs/manual/cli-reference.md](docs/manual/cli-reference.md)
(Korean: [cli-reference.ko.md](docs/manual/cli-reference.ko.md)). Both languages are generated from the
clap definitions and a CI test enforces that they stay in sync with the code.

Help is shown in English by default and in Korean under a Korean locale; override it with
`--lang en|ko` or `HWP_LANG`. A summary:

| Command | Description |
|---|---|
| `info <file>` | Format, version, properties and stream diagnostics |
| `cat <file>` | Extract body text (plain/markdown/json/html/csv). `--with-segments` adds provenance coordinates |
| `grep <pattern> <file>` | Search paragraph text with grep semantics (non-zero exit when nothing matches) |
| `convert <input> -o <output>` | Format conversion; `--to` also targets `docx`, `odt`, `csv` and `txt`. A `.pdf` output delegates to the render path. `--strict` fails without publishing when unpreservable data is found |
| `render <input> -o <output>` | Render pages to PNG/SVG (one file per page) or PDF (single multi-page) |
| `merge <inputs...> -o <output>` | Combine two or more documents, one Section per input in argument order. `--strict` and `--loss-report` as in `convert` |
| `split <input> --out-dir <dir>` | One fragment per Section by default; `--pages` splits on page ranges estimated from the layout cache Hancom saved |
| `new -o <output>` | Create a document from markdown, from JSON IR, or from one of the eight embedded official-document templates (`--template`, `--list-templates`) |
| `compose <spec> -o <output>` | Compose DocumentSpec v1/v2 deterministically. `--dry-run` validates without writing |
| `template <template> --data <data> -o <output>` | Bounded expansion of the TemplateSpec/Data v1 typed AST |
| `edit <input> -o <output>` | Text, formatting and structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) |
| `fields` / `bookmarks` / `slots` `<file>` | List fields, bookmarks, or `{{name}}` slots |
| `fill <input> -o <output>` | Fidelity-preserving template filling (hwpx package preserved) |
| `validate <file>` | Structural validation (mimetype, required entries, XML parsing) |
| `lint <file>` | Ten official-document notation and structure rules. Advisory by default; `--strict` exits 1 on an error-severity finding and `--json` emits `hwp-lint-report-v1` |
| `certify <input> --policy <file> --report <dir>` | Certify and publish a report atomically. See [Certification v1](docs/design/16-certification-v1.md) |
| `corpus --manifest <file> --report <dir>` | Frozen structured corpus gate. See [the corpus contract](docs/design/17-structured-corpus-v1.md) |
| `diff <input> --ref <png>` | Compare a render against a Hancom reference PNG (ink, offset, pixel difference, MAE) |
| `compare <a> <b>` | Paragraph and structural differences between two documents, leaving both untouched. Exit codes follow diff(1): 0 identical, 1 differences found, 2 the run failed |
| `mcp` | Run the MCP stdio server |
| `skill export` | Export or install the bundled agent skill, including Amazon Quick profile discovery |
| `update` | Self-update (a brew installation delegates to `brew upgrade`) |
| `dump <file>` | [developer] Dump record and package structure |

Output format is usually inferred from the output file extension; `convert`/`render` also accept
`--to`/`--format`.

## Markdown export

Converts hwp/hwpx to GFM. `hwp cat --format markdown` writes to stdout only and does not extract images.

```sh
hwp convert report.hwp -o report.md                    # images extracted to report.media/
hwp convert report.hwp -o report.md --media-dir figs   # extracted to figs/ and linked as figs/...
hwp convert report.hwp -o full.md --with-header-footer --with-hidden
```

| HWP element | markdown |
|---|---|
| `개요 N` (outline) style paragraph | `#` × N heading |
| Bold / italic / strikethrough / underline / super- and subscript | `**bold**` / `*italic*` / `~~strike~~` / `<u>` / `<sup>`, `<sub>` |
| Hyperlink (%hlk) | `[text](URL)` |
| Image | `![image](<media>/imageN.png)` (extension detected from magic bytes) |
| Table without merged cells | GFM pipe table |
| Table with merged cells, nested tables or block equations | HTML `<table>` (colspan/rowspan) |
| Bullet / numbered paragraphs | `- ` / `N. ` lists (number format synthesized from the numbering definition) |
| Foot- and endnotes | `[^N]` / `[^eN]` GFM footnotes |
| Equation (eqed) | `$script$` / `$$script$$` (the HWP equation script verbatim, not LaTeX) |
| Headers/footers and hidden comments | Excluded by default; `--with-header-footer` / `--with-hidden` include them |

**Table serialization guarantee**: every `<tr>...</tr>` of an HTML table and every row of a GFM pipe
table is serialized on a single line (a nested table inside a cell is inlined on that same line).
This is pinned by tests so consumers can quote and parse row by row.

Limitations: headings are recognized only from the `개요 N` style, floating object position and
z-order are not represented, and the reverse direction (md → hwp) restores only basic constructs such
as tables and bold.

### `--with-segments` (provenance coordinates)

`hwp cat <file> --format markdown --with-segments` emits, alongside the markdown, a single-line JSON
envelope stating which source paragraph each output character range came from.

```json
{"markdown": "...", "segments": [
  {"kind": "para", "section": 0, "para": 12, "start": 345, "end": 512}
]}
```

- **Offsets are Unicode scalars (characters)**, the same as Python `str` indexing, so
  `markdown[start:end]` slices directly.
- **Coordinates are IR indices**: `section`/`para` are indices into `sections[]`/`paragraphs[]` from
  `--format json`, so they stay stable across re-decoding.
- **Sorted and non-overlapping**: segments ascend by `start` and never overlap. Output belonging to no
  paragraph remains as a gap.
- Lines produced by a table inherit the index of the paragraph containing it, and foot/endnote
  definitions belong to the referencing paragraph.
- The `markdown` field is byte-identical to the output produced without `--with-segments`.

## Structured authoring

`compose` compiles a v1 JSON/YAML specification into paragraphs, runs, styles, lists, tables,
equations, fields, headers/footers, page numbers and sections. v2 wraps that document and adds images
with accessibility descriptions, a closed SVG→PNG fallback, and HWPX native rectangular text boxes.
The v2 fallback policy is stated per target; omitting it fails as native-only.

```sh
# Validate schema, references, table spans, assets and native support without writing
hwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx --dry-run --report

# Publish the validated file atomically
hwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/report.hwpx --report
```

`template` allows only a typed `value`/`if`/`each` AST, with no string interpolation and no expression
evaluation. `reference_hwpx` surgically fills only the named text and fields of an existing package;
regenerating structure requires an explicit strict gate.

Normative schemas are in [`schemas/`](schemas/) and the contracts in
[13-document-spec-v1](docs/design/13-document-spec-v1.md),
[14-template-spec-v1](docs/design/14-template-spec-v1.md) and
[15-document-spec-v2](docs/design/15-document-spec-v2.md). Examples are in [`examples/`](examples/).

## Official-document authoring (공문서)

Korean official documents follow a fixed notation and layout. That layer is built into the binary
rather than left to a prompt.

**Profiles** (`--preset`) set the margins, numbering and paragraph shapes of a document type:
`official` (기안문·공문서), `report` (보고서), `plan` (사업계획서), `notice` (공고문),
`minutes` (회의록) and `press` (보도자료). Korean aliases (`기안문`, `보고서`, `공고`, …) normalize
onto the same six. Nested lists render the statutory eight-level item marks.

**Templates** (`--template`) go further: each brings its own profile *and* native 두문/결문 tables,
pre-filled with `{{slots}}`.

| Slug | Korean alias | Notes |
|---|---|---|
| `gian-internal` | 기안문-내부결재 | Internal approval |
| `gian-external` | 기안문-대외시행 | External dispatch |
| `gongmun-basic` | 공문서-기본 | Multi-recipient form (`{{수신자}}`) |
| `report` | 보고서 | |
| `plan` | 사업계획서 | |
| `minutes` | 회의록 | |
| `notice` | 공고문 | |
| `press` | 보도자료 | |

**Frames** fill those header and footer blocks directly: `--doc-head 기관명=…`,
`--doc-foot 발신명의=…`, `--notice-head`, `--notice-foot` and `--press-head`. Since v0.10.0 both
`--preset` and the frame flags **override** a template's own defaults instead of being refused.

**Filling** A generated document is itself a template. `hwp slots` lists what is still unfilled and
`hwp fill` replaces `{{name}}`, preserving the rest of the hwpx package byte for byte. An unmatched
slot is an error and nothing is written unless `--allow-partial` is given.

**Linting** `hwp lint` applies ten rules to `.md`, `.hwp` and `.hwpx` files (or to stdin markdown
with `-`): seven notation rules (date `2026. 8. 20.`, time, money, the `붙임:` colon and its
numbering, the closing `끝.`, punctuation), one against decorative item marks (`■ ▶ ▲ ◆ ● ※`, which
belong as `□ ○` or as nested lists), and two structural rules for statutory item marks and
Roman-numeral headings. Only the two structural rules are error severity; every notation finding is
a warning. The command is advisory and exits 0 unless `--strict` is given.

**Table styling** `hwp edit --style-tables <preset>` applies the profile's table look. Single-column
tables are skipped, and applying it twice produces byte-identical output.

## Certification and the structured corpus

`certify` runs package validation, repeated import, bounded native rendering and an optional
independent import under a frozen policy, then publishes a new directory atomically. The policy pins
font identity and forbids font substitution, macros and external references, and it fails on bounds,
collision and unresolved-field problems. The policy may also pin two optional, fail-closed evidence
checks: `preservation` ingests a `preservation-report-v1` artifact with a zero-loss budget, and
`hancom_open` ingests a `hancom-verification-receipt-v1` attestation; failed or invalid evidence
forces `overall=failed`.

`corpus` generates seven self-authored Korean documents as both HWPX and HWP, twice each, and passes
only when the document bytes, semantic statistics, page PNG hashes, render-issue hashes and font
identities all agree across the two runs.

```sh
bash scripts/fetch-corpus-fonts.sh    # fetch and SHA-256 verify the pinned OFL font (once)
hwp corpus --manifest corpus/structured-v1/manifest.json --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh` (the CI gate) runs that fetch first, so no separate setup is
needed. See [17-structured-corpus-v1](docs/design/17-structured-corpus-v1.md) for the corpus contract
and [16-certification-v1](docs/design/16-certification-v1.md) for certification.

## Regenerating a document from the JSON IR

A document can be recreated from scratch rather than edited in place.

```sh
hwp convert report.hwpx -o report.json   # hwp/hwpx → JSON IR (--embed-bin also embeds images)
hwp new --from report.json -o regen.hwpx # JSON IR → new document
```

Regeneration is verified (`crates/hwp-cli/tests/regen.rs`) by validation passing, identical `hwp cat`
output, an identical table map (count, rows/columns, merges, cell widths) and byte-identical secPr and
tabProperties slices. What still differs is by design: the line layout cache (recomputed by Hancom
Office on open), the preview image and settings.xml.

## MCP server (AI agent integration)

`hwp mcp` implements synchronous JSON-RPC 2.0 over stdio (line-delimited) using only `serde_json`,
with no tokio and no SDK. The protocol version is negotiated at `initialize`: a client
`protocolVersion` of `2025-06-18`, `2025-03-26` or `2024-11-05` is echoed back, anything else gets
the latest supported version. stdout carries the protocol; logs go to stderr.

Per-client setup (Claude Code/Desktop, Codex CLI/cloud, Kiro, Kimi, claude.ai skill upload,
and Amazon Quick Desktop) and the bundled agent skill (`hwp skill export`):
[docs/manual/ai-integrations.md](docs/manual/ai-integrations.md).
The copy-paste Windows setup, create/validate acceptance test, reusable agent instructions, and
symptom-driven recovery are in the dedicated
[Amazon Quick Desktop runbook](docs/manual/amazon-quick-desktop.md).

Amazon Quick Desktop can launch this local stdio server and expose all 22 tools. Install the
publish-safe skill into its active profile with:

```sh
hwp skill export --install amazon-quick
```

On Windows, create a dedicated exchange root under `%USERPROFILE%\AppData\LocalLow` (for example
`hwp-quick-workspace`) and pass its absolute path as the MCP `--root` (Quick arguments do not
expand environment variables). Quick starts the local MCP child at Low mandatory integrity, so
`C:\TEMP` can pass tool discovery but reject the first write. Quick's local-folder permissions do
not change that write integrity; use the dedicated runbook's creation, import JSON, and recovery
steps.

Amazon Quick Web cannot launch a local stdio process. `hwp serve` now runs the same tools over
HTTP for container deployment (`POST /mcp`, see [deployment design](docs/design/22-remote-mcp-deployment.md)),
but it is a private hop that expects a trusted edge in front of it: authenticated Streamable HTTP,
tenant isolation and artifact transfer remain future work in
[Remote MCP transport](docs/design/20-remote-mcp.md).

### Exposed tools (22)

| Tool | Required arguments | Purpose |
|---|---|---|
| `hwp_info` | `path` | Format, version, properties and stream diagnostics |
| `hwp_read` | `path` | Extract body text (`plain`/`markdown`/`json`/`html`/`csv`; header/footer, hidden and segment options). UTF-8 byte pagination (256 KiB default, 1 MiB max) |
| `hwp_grep` | `path`, `pattern` | Paragraph text search; returns `{matches, count, truncated}` — zero matches is a normal result |
| `hwp_list_fields` | `path` | List fields |
| `hwp_list_bookmarks` | `path` | List bookmarks (bokm) |
| `hwp_slots` | `path` | List `{{name}}` placeholders |
| `hwp_render` | `path` | Render pages to PNG (base64, response up to 16 MiB) or write PNG/SVG/PDF files via `output_path` (dpi 36..=600) |
| `hwp_edit` | `input`, `output` | Strict atomic editing through typed JSON operations |
| `hwp_convert` | `input`, `output` | Format conversion (`strict` defaults to true over MCP) |
| `hwp_new` | `output` | Create a document from markdown or JSON IR plus metadata |
| `hwp_compose` | `output`, `spec`/`spec_path` | Compose DocumentSpec v1/v2 through the same path as the CLI |
| `hwp_template` | `output`, `template` (+`data`) | Bounded expansion of TemplateSpec/Data v1 |
| `hwp_certify` | `input`, `policy`, `report` | Run certification and publish the report atomically |
| `hwp_fill` | `input`, `output`, `values` | Fill `{{name}}` in an hwpx template (package preserved) |
| `hwp_diff` | `input`, `ref` | Render one page and compare it to a reference PNG |
| `hwp_validate` | `path` | Structural validation, `{valid, errors, warnings}` |
| `hwp_lint` | `path` | Ten official-document notation and structure rules, as an `hwp-lint-report-v1` report |
| `hwp_merge` | `inputs`, `output` | Combine two or more documents; returns the preservation ledger |
| `hwp_split` | `input`, `out_dir` | Split per Section (or per `pages` range); returns the published fragment paths |
| `hwp_compare` | `a`, `b` | Read-only paragraph/structure diff as `hwp-compare-report-v1`. Differences never set `isError` — read `identical` |
| `hwp_put_file` | `name`, `content` | Write base64 content into the session workspace. The only way to hand an existing document to a remote deployment: tool arguments take paths, so upload first and pass the name. 512 KiB decoded |
| `hwp_get_file` | `path` | Return a workspace file as base64 (a receipt block plus an embedded resource). Refuses above 512 KiB rather than truncating. Keep intermediates in the workspace and fetch only the final artifact - the base64 lands in the client's message stream |

### Client configuration example

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts", "--root", "<repo>"]
    }
  }
}
```

`--root` (repeatable) sandboxes every file path the tools touch — inputs, outputs, nested
image/part paths, spec `base_dir`s and per-call `font_dir`s — under the given directories. The
roots also bind compose/template spec internals: image/visual assets and `reference_hwpx` packages
referenced by a spec must resolve under an allowed root. Without
any `--root` the server keeps the old unrestricted behavior and prints a one-line warning to stderr
at startup.

### The read → edit → rewrite loop

1. **Read** `hwp_read` (`format=json`) exports the whole document as IR, including tables, image
   references, formatting and unparsed records.
2. **Edit** `hwp_edit` applies replacements, cell values and field filling. Because only the IR
   changes, images, formatting and opaque records survive; only the line layout of edited paragraphs
   is invalidated and re-synthesized by the writer.
3. **Verify** `hwp_render` returns the resulting page as PNG so the agent can check the change
   visually, and `hwp_diff` compares it numerically against a Hancom reference render.

An edited hwp goes through the writer's synthesis path, which re-establishes Hancom's paragraph
invariants (line layout, the `0x0d` paragraph terminator, nchars and so on), so it opens correctly in
Hancom Office.

## Workspace layout

| Crate | Role |
|---|---|
| `hwp-model` | Shared document model (IR): the single contract every crate depends on, lossless preservation (opaque/tail), unit conversion |
| `hwp5` | HWP 5.0 binary reader/writer (CFB container, record streams, compression) |
| `hwpx` | HWPX reader/writer (ZIP package, OWPML XML) |
| `hwp-convert` | IR ↔ markdown / HTML / JSON, in-memory editing, field scanning |
| `hwp-render` | IR → PNG / SVG / PDF renderer, line layout synthesis, shaping, font subsetting and embedding, render diff |
| `hwp-cli` | The `hwp` binary (CLI and MCP server) |

## Development and testing

```sh
cargo build --all-targets
scripts/check.sh     # local CI mirror: fmt + clippy + test + structured corpus (required before a PR)
```

CI (`.github/workflows/ci.yml`) runs the platform-independent gates — `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `scripts/check-structured-corpus.sh` —
once in an ubuntu `lint` job, and runs `cargo test --workspace` on **ubuntu + macOS + windows**
(all required). The ubuntu test job installs `fonts-nanum` (glyf TTFs; the CFF `fonts-noto-cjk`
made debug-build rendering ~100x slower). The local mirror `scripts/check.sh` runs the same gates.

Tests cover byte-identical hwp5 round-trips (identity/roundtrip/synth), semantically equivalent hwpx
round-trips, IR JSON and markdown round-trips, editing and field correction, render layout, tables and
diff metrics, and structured corpus determinism. Genuine fixtures are not in the repository; when they
are absent the corresponding tests skip rather than fail.

Two checklists sit outside the automated gates because they need Hancom Office on real hardware:
[docs/hancom-verification-checklist.md](docs/hancom-verification-checklist.md) for verifying written
files, and [docs/release-readiness.md](docs/release-readiness.md) for the pre-release gate.

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
`fixtures/README.md` and the root `NOTICE`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). Unless stated otherwise, code
contributed to this repository is understood to be released under the same two licenses.
