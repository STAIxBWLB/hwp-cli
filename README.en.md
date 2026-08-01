[한국어](README.md) · [English](README.en.md)

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

- **Reading and text extraction** from hwp/hwpx to plain / markdown / HTML / JSON (full IR). Tables,
  images, headers/footers and unparsed records are preserved during parsing.
- **Format conversion** hwp ↔ hwpx, hwp/hwpx ↔ markdown, hwp/hwpx ↔ JSON (IR), all through a shared
  document model (IR).
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
- **MCP server** A dependency-free (serde_json only) stdio MCP server exposing 15 tools.

## Implementation status

| Area | Status |
|---|---|
| hwp/hwpx reading; text, markdown and JSON extraction | Implemented |
| hwpx writing, HWP 5.0 binary writing | Implemented |
| PNG/SVG/PDF rendering (tables, images, text boxes, shapes, foot/endnotes, page numbers, text effects) | Implemented |
| Structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) | Implemented, including merged tables |
| DocumentSpec v1/v2 and TemplateSpec v1 composition | Implemented |
| Certification (`certify`) and structured corpus gate (`corpus`) | Implemented |
| MCP server (15 tools) | Implemented |
| HTML conversion | Works, but lower fidelity than markdown (roadmap) |
| Equations | Approximated as a box plus the script |
| Charts, OLE | Not supported |
| Encrypted / distribution (DRM) documents | Reading is refused |

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
- **Render hashes** The hashes recorded by the corpus are observations for that OS and architecture,
  not a claim of cross-platform pixel equivalence.
- **No Hancom parity claim** The final verdict is always whether the file opens correctly in Hancom Office.

## Roadmap

Detailed items live in [TODO.md](TODO.md) (Korean) / [TODO.en.md](TODO.en.md); the catalog of
unimplemented features is [docs/design/12-feature-gaps.md](docs/design/12-feature-gaps.md).

1. **Specification re-documentation (in progress)** Reconstruct the HWP 5.0 specification PDF as
   reviewable Markdown. The rev1.3 body is done (§1 to §4.4); the OWPML/KS X 6101, equation, chart and
   distribution-document specifications still need to be obtained.
2. **Spec-driven review of the implementation (in progress)** Compare parser, writer and IR values
   bit by bit against the reconstructed specification. A coverage audit registered 17 new gaps, of
   which 4 content-loss items are the first to fix.
3. **HTML conversion upgrade** Carry the footnote markers, merged-cell colspan/rowspan and in-cell
   blocks already solved on the markdown path over to HTML, and improve CSS mapping of character and
   paragraph shapes.
4. **Windows** Platform defects in the compile and publish paths are being worked through. In CI,
   ubuntu and macOS are required gates and Windows is informational (non-blocking).
5. **Distribution (DRM) documents** A candidate to start once the specification is available.

## Installation

### One-line script (macOS / Linux)

Installs a release binary without a Rust toolchain. After installation, `hwp update` self-updates.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

The default location is `~/.local/bin` (if it is not on PATH, the script says so). Change the
location or version with arguments or environment variables:

```sh
curl -fsSL .../install.sh | sh -s -- --dir /usr/local/bin --tag v0.4.1
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
| macOS Apple Silicon | `hwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `hwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Extract and put `hwp` on PATH (verify with `shasum -a 256 -c hwp-*.sha256`).

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
hwp update --tag v0.4.0   # roll back to a specific version
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

The full generated reference is [docs/manual/cli-reference.en.md](docs/manual/cli-reference.en.md)
(Korean: [cli-reference.md](docs/manual/cli-reference.md)). Both languages are generated from the
clap definitions and a CI test enforces that they stay in sync with the code.

Help is shown in English by default and in Korean under a Korean locale; override it with
`--lang en|ko` or `HWP_LANG`. A summary:

| Command | Description |
|---|---|
| `info <file>` | Format, version, properties and stream diagnostics |
| `cat <file>` | Extract body text (plain/markdown/json). `--with-segments` adds provenance coordinates |
| `convert <input> -o <output>` | Format conversion. A `.pdf` output delegates to the render path. `--strict` fails without publishing when unpreservable data is found |
| `render <input> -o <output>` | Render pages to PNG/SVG (one file per page) or PDF (single multi-page) |
| `new -o <output>` | Create a document from markdown or JSON IR |
| `compose <spec> -o <output>` | Compose DocumentSpec v1/v2 deterministically. `--dry-run` validates without writing |
| `template <template> --data <data> -o <output>` | Bounded expansion of the TemplateSpec/Data v1 typed AST |
| `edit <input> -o <output>` | Text, formatting and structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) |
| `fields` / `bookmarks` / `slots` `<file>` | List fields, bookmarks, or `{{name}}` slots |
| `fill <input> -o <output>` | Fidelity-preserving template filling (hwpx package preserved) |
| `validate <file>` | Structural validation (mimetype, required entries, XML parsing) |
| `certify <input> --policy <file> --report <dir>` | Certify and publish a report atomically. See [Certification v1](docs/design/16-certification-v1.en.md) |
| `corpus --manifest <file> --report <dir>` | Frozen structured corpus gate. See [the corpus contract](docs/design/17-structured-corpus-v1.en.md) |
| `diff <input> --ref <png>` | Compare a render against a Hancom reference PNG (ink, offset, pixel difference, MAE) |
| `mcp` | Run the MCP stdio server |
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
[13-document-spec-v1](docs/design/13-document-spec-v1.en.md),
[14-template-spec-v1](docs/design/14-template-spec-v1.en.md) and
[15-document-spec-v2](docs/design/15-document-spec-v2.en.md). Examples are in [`examples/`](examples/).

## Certification and the structured corpus

`certify` runs package validation, repeated import, bounded native rendering and an optional
independent import under a frozen policy, then publishes a new directory atomically. The policy pins
font identity and forbids font substitution, macros and external references, and it fails on bounds,
collision and unresolved-field problems.

`corpus` generates seven self-authored Korean documents as both HWPX and HWP, twice each, and passes
only when the document bytes, semantic statistics, page PNG hashes, render-issue hashes and font
identities all agree across the two runs.

```sh
bash scripts/fetch-corpus-fonts.sh    # fetch and SHA-256 verify the pinned OFL font (once)
hwp corpus --manifest corpus/structured-v1/manifest.json --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh` (the CI gate) runs that fetch first, so no separate setup is
needed. See [17-structured-corpus-v1](docs/design/17-structured-corpus-v1.en.md) for the corpus contract
and [16-certification-v1](docs/design/16-certification-v1.en.md) for certification.

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
with no tokio and no SDK (protocol version `2024-11-05`). stdout carries the protocol; logs go to stderr.

### Exposed tools (15)

| Tool | Required arguments | Purpose |
|---|---|---|
| `hwp_info` | `path` | Format, version, properties and stream diagnostics |
| `hwp_read` | `path` | Extract body text; `json` returns the full IR. UTF-8 byte pagination (256 KiB default, 1 MiB max) |
| `hwp_list_fields` | `path` | List fields |
| `hwp_list_bookmarks` | `path` | List bookmarks (bokm) |
| `hwp_slots` | `path` | List `{{name}}` placeholders |
| `hwp_render` | `path` | Render one page to PNG (dpi 36..=600, response up to 16 MiB) |
| `hwp_edit` | `input`, `output` | Strict atomic editing through typed JSON operations |
| `hwp_convert` | `input`, `output` | Format conversion (`strict` defaults to true over MCP) |
| `hwp_new` | `output` | Create a document from markdown or JSON IR plus metadata |
| `hwp_compose` | `output`, `spec`/`spec_path` | Compose DocumentSpec v1/v2 through the same path as the CLI |
| `hwp_template` | `output`, `template` (+`data`) | Bounded expansion of TemplateSpec/Data v1 |
| `hwp_certify` | `input`, `policy`, `report` | Run certification and publish the report atomically |
| `hwp_fill` | `input`, `output`, `values` | Fill `{{name}}` in an hwpx template (package preserved) |
| `hwp_diff` | `input`, `ref` | Render one page and compare it to a reference PNG |
| `hwp_validate` | `path` | Structural validation, `{valid, errors, warnings}` |

### Client configuration example

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts"]
    }
  }
}
```

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

CI (`.github/workflows/ci.yml`) installs `fonts-noto-cjk` on ubuntu and then runs
`cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
`cargo test --workspace` → `scripts/check-structured-corpus.sh` on **ubuntu + macOS** (required) and
**windows** (informational, non-blocking). The local mirror `scripts/check.sh` runs the same gates.

Tests cover byte-identical hwp5 round-trips (identity/roundtrip/synth), semantically equivalent hwpx
round-trips, IR JSON and markdown round-trips, editing and field correction, render layout, tables and
diff metrics, and structured corpus determinism. Genuine fixtures are not in the repository; when they
are absent the corresponding tests skip rather than fail.

## Contributing

Bug reports and pull requests are welcome.

- Work on a `feat/`, `fix/` or `docs/` branch and submit a PR. Do not push to main directly.
- Pass `scripts/check.sh` before opening a PR (the same gates as CI).
- Add round-trip or golden tests with new format features where possible.
- User-facing documentation is a Korean original plus an English pair (`NAME.md` / `NAME.en.md`);
  update both in the same commit.
- For specification questions, consult Hancom's official
  [HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do) (not bundled here).

## Acknowledgments

This product was developed with reference to Hancom's HWP document file format open specification,
[한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do)
(© Hancom Inc.).

The specification is copyrighted by Hancom Inc. Its open-document license permits free viewing,
copying and distribution but restricts distribution to the **unmodified original or copies thereof**,
so this repository does not bundle the specification or derivatives of it (extracted text, page
captures) and links only to the official distribution point. See [docs/README.en.md](docs/README.en.md).

Some test fixtures come from [hahnlee/hwp-rs](https://github.com/hahnlee/hwp-rs) (Apache-2.0); see
`fixtures/README.md` and the root `NOTICE`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). Unless stated otherwise, code
contributed to this repository is understood to be released under the same two licenses.
