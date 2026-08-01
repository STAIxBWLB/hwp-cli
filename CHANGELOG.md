# Changelog

Release notes are written in English only. This file is the source of truth, and
`scripts/release_notes.sh <version>` extracts the section for a version verbatim as the GitHub
Release body.

The workspace `Cargo.toml` `[workspace.package] version` is the single source for version numbers.

---

## [Unreleased]

**Added**

- DOCX export (GJ-1): `convert --to docx` writes OOXML from the IR (`hwp-convert::docx`) —
  paragraphs with Heading styles, run properties (font, size, color, shade, letter-spacing,
  super/subscript), para alignment/spacing/indents, tables with gridSpan/vMerge and nesting,
  embedded images, hyperlinks, numbering lists, footnotes/endnotes, and page setup from
  SectionDef. Equations fall back to script text. DOCX input stays open (L-tier).

---

## [0.6.0]

**Added**

- HTML fragment round-trip (contract: docs/design/18). A new `from_html` importer parses a
  well-formed XHTML subset (table colspan/rowspan, cell blocks, data-URI/relative-path images,
  inline marks, lists) into the IR. Contract violations are hard errors.
- HTML export alignment: `convert --to html` now emits merged cells as colspan/rowspan (GH-4),
  preserves nested tables/images inside cells (GH-5), and renders footnotes/endnotes as `<sup>`
  anchors with a trailing definitions section (GH-3). The output is XHTML that `from_html`
  reads back.
- Mixed md+HTML part files: HTML table blocks can be embedded in markdown body text and are
  parsed by `new`/`convert`. Inline `<u>`, `<sup>` and `<sub>` round-trip through the IR.
- Template + part fill: `hwp fill --set name=@part.md` (or a `parts` map in `--data`) replaces
  the `{{name}}` anchor paragraph with the part file's blocks, composing large documents
  part-by-part (limited to hwp-cli-generated documents in the default palette family).
- SVG images in parts: `<img src="*.svg">` is embedded via closed-subset validation and
  deterministic PNG rasterization. The validation/rasterization implementation was extracted
  into `hwp-convert::svg` and is now shared with DocumentSpec v2.
- A `parts` argument on the MCP `hwp_fill` tool, exposing part grafting over MCP.
- ODT export GH-3/4/5: footnotes/endnotes as `<text:note>`, merged cells as
  number-columns/rows-spanned with covered-table-cell, and nested tables/images preserved
  inside cells. All export paths (md/html/odt) are now covered.
- HTML style round-trip (contract v2, docs/design/18 section 8): the html export carries
  char and para shapes as `.cs{n}`/`.ps{n}` CSS rules (fragments stay self-contained with a
  leading `<style>`) and `from_html` restores them. Tags stay authoritative for marks,
  default-palette shapes are reused (dedup), and unknown fonts are appended on restore.
- Edit primitives: `edit --add-table "anchor=>rows-json"` (table insertion), `--set-para`
  (line spacing, indent, margins, paragraph spacing), `--set-page` (paper size, margins,
  orientation), and `--delete-image/--delete-table/--delete-field/--delete-bookmark`
  (object deletion with anchor-char and FIELD_END surgery).
- CLI workflow: convert accepts multiple inputs with `--out-dir` (batch) and `-` for
  stdin input / stdout output of text formats. New `hwp grep <pattern> <file>` (recursive
  paragraph search, exit code 1 on no match).
- Extraction formats: `cat --format csv` / `convert --to csv` (tables to CSV, RFC 4180)
  and `convert --to txt` with `.txt` inference.

---

## [0.5.0]

**Added**

- A structured authoring path: `compose` and `template` deterministically generate HWP/HWPX from
  DocumentSpec v1/v2 and TemplateSpec/Data v1. Only a typed AST is allowed, with no string
  interpolation and no expression evaluation.
- Native-only certification (`certify`): pins font identity, forbids font substitution, macros and
  external references, renders every page, fails on bounds, collision and unresolved fields, and
  publishes the report atomically.
- A structured corpus gate (`corpus`): generates seven self-authored Korean documents as HWPX and HWP
  twice each and passes only when document bytes, semantic statistics, page PNG hashes, render-issue
  hashes and font identities all agree.
- Published JSON Schemas (`schemas/`), examples (`examples/`) and design documents 13 to 17.
- Three new MCP tools (`hwp_compose`, `hwp_template`, `hwp_certify`), for 15 in total.
- Localized CLI help: English by default, Korean under a Korean locale, and overridable with
  `--lang <en|ko>` or `HWP_LANG`. The CLI reference is generated in both languages.

**Fixed**

- Restored the Windows build. File identity and link counts no longer use the nightly-only
  `windows_by_handle` API; they are read from a handle via `GetFileInformationByHandle`. An identity
  that cannot be read never compares equal, so the TOCTOU recheck fails closed.
- Pass an explicit impersonation token when computing inherited DACLs on Windows. With a NULL token
  the call returned `ERROR_NO_TOKEN` and every publish failed.
- Open the staged file for write before fsync, because `FlushFileBuffers` requires write access on
  Windows. With a read-only handle every surgical hwpx publish failed with `ERROR_ACCESS_DENIED`.
- Pin `eol=lf` for the hash-pinned inputs under `examples/` so golden reports still match on a Windows
  checkout.

**Changed**

- Corpus fonts are fetched rather than committed. `scripts/fetch-corpus-fonts.sh` downloads them from
  the manifest's pinned URL and verifies each against its SHA-256, accepting only `https` from the
  pinned host and only relative in-corpus destinations.
- User-facing documentation is now a Korean original plus an English pair (`NAME.md` / `NAME.en.md`).
- Release notes are now bilingual and driven by `CHANGELOG.md`; a release fails without a section for
  that version.

---

## [0.4.1]

- Page numbers are rendered in both HWP and HWPX output (#30).
- Added Korean official-document presets (gian, report) to the markdown import path (#28).
- Fixed □/○ paragraph spacing and list marker (-, ·) visibility in gaejosik documents (#27).
- Renamed the Hancom verification output directory to `hwp-verification` (#29).

---

## [0.4.0]

- Added `hwp update` self-updating and a one-line installation script that does not need Homebrew (#25).

---

## [0.3.0]

- Homebrew installation is supported: the repository is its own tap and the formula is updated
  automatically on release (#24).
- Added hwpx equation emission and `hp:script` entity resolution (#23).
- Added `--font-dir` to `convert` so PDF conversion can select external fonts from the CLI (#22).
- The CLI reference is generated from the clap definitions, with a drift gate (#20).
