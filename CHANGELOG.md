# Changelog

Release notes are written in English only. This file is the source of truth, and
`scripts/release_notes.sh <version>` extracts the section for a version verbatim as the GitHub
Release body.

The workspace `Cargo.toml` `[workspace.package] version` is the single source for version numbers.

---

## [Unreleased]

## [0.8.4]

**Fixed**

- Amazon Quick Desktop Windows guidance now uses a dedicated
  `%USERPROFILE%\AppData\LocalLow\hwp-quick-workspace` MCP root. Quick starts local MCP children at
  Low mandatory integrity, so discovery can succeed under a normal Medium-integrity `C:\TEMP`
  root while the first atomic document write still fails with `Access is denied (os error 5)`.

## [0.8.3]

**Fixed**

- CI no longer re-runs platform-independent gates on every OS, nor the whole test matrix on
  every tag: `ci.yml` runs fmt, clippy, and the structured-corpus gate once in an ubuntu `lint`
  job and only `cargo test --workspace` in the 3-OS `test` matrix, and `release.yml` verifies the
  tagged commit's already-green CI via the check-runs API instead of re-running it. The ubuntu
  test step also drops from ~15 minutes to ~2: render tests now resolve glyf-outline Nanum TTFs
  instead of CFF Noto CJK fonts, and the dev profile builds ttf-parser/rustybuzz/tiny-skia with
  opt-level 2 (release binaries unaffected).

## [0.8.2]

**Added**

- A bilingual, copy-paste Amazon Quick Desktop runbook now covers Windows binary verification,
  connector import, skill and agent setup, an end-to-end create/validate smoke test, daily file
  staging, and symptom-driven recovery for quoting, stale tool IDs, auto-disable, rendering, and
  sandbox path failures.

**Fixed**

- Amazon Quick Desktop on Windows: keep canonical MCP paths in verbatim form for sandbox
  authorization, then use ordinary drive/UNC spelling for filesystem I/O only when every component
  has equivalent Win32 semantics. Paths with trailing dots/spaces, reserved device names, or other
  verbatim-only semantics remain verbatim or fail closed. This removes the verbatim-path failure
  without weakening root containment. Also document separate JSON arguments and recovery from the
  `Access is denied` handshake loop that causes Quick to auto-disable the connector.

## [0.8.1]

**Added**

- Amazon Quick Desktop integration: `hwp skill export --install amazon-quick` installs the
  publish-safe bundled skill into the active Quick profile, with explicit profile ID or absolute
  path override, registry validation, and symlink-safe profile-relative writes.
- Amazon Quick documentation now covers Desktop stdio setup and all 16 tools, agent/skill
  publishing, troubleshooting, and the Quick Web limitation. The future authenticated Streamable
  HTTP, tenant isolation, and artifact model remain design-only in `docs/design/20-remote-mcp.md`
  and issue #52.

**Fixed**

- Release workflow: `update-formula` no longer pushes the Homebrew formula commit directly to
  main — branch protection rejects the Actions token (first seen on the v0.8.0 tag). The job
  now opens a `brew/formula-vX.Y.Z` PR and enables squash auto-merge; when the repository has
  no auto-merge or required checks never run for a token-created PR, the PR stays open for a
  human merge. `scripts/update_formula.sh X.Y.Z` remains the local recovery path.

---

## [0.8.0]

**Added**

- MCP server: `--root <dir>` (repeatable) sandboxes every path-typed tool argument — reads,
  writes, nested `insert_image`/`seal`/`parts` paths, compose/template `base_dir`, per-call
  `font_dir`, and the certify report directory. Roots are canonicalized at startup (a missing or
  unreadable root fails fast); write guards reject `..` components and close the
  symlink-overwrite escape. With no `--root` the server is unrestricted as before and prints a
  one-line stderr warning at startup. (#50)
- MCP protocol negotiation: `initialize` echoes the client's `protocolVersion` when it is one of
  `2025-06-18` / `2025-03-26` / `2024-11-05`, and otherwise replies with the latest supported
  version. (#50)
- Compose/template asset hardening: the MCP `--root` sandbox now also binds spec-internal asset
  references. DocumentSpec v1/v2 image and visual assets fail with
  `asset_snapshot_outside_roots`, and TemplateSpec `reference_hwpx` packages fail with
  `reference_outside_roots`, unless the resolved file sits under at least one root — so a spec
  cannot reach files outside the sandbox even when `base_dir` itself is attacker-influenced.
  The binding is verified against the opened file handle rather than the request pathname,
  closing the rename-swap race between the path check and the open. CLI and corpus callers
  pass no roots and behave exactly as before. (#53)
- MCP `hwp_edit` typed-operation parity: the seven edits that existed only as CLI string flags
  are now structured JSON arguments — `add_table` (`[{anchor, rows: [[...]]}]`), `set_para`
  (`[{pattern, line_spacing_pct | line_spacing_pt, indent_mm, left_mm, right_mm, top_mm,
  bottom_mm}]`), `set_page` (a single object: `width_mm`/`height_mm`/`margin_*_mm`/
  `orientation`), `delete_image` (`[{anchor}]`), `delete_table` (`[{index | anchor}]`, exactly
  one of the two), and `delete_field`/`delete_bookmark` (`[{name}]`). They run through the same
  strict, atomic, re-read-verified edit path as the CLI with identical applied/unapplied
  semantics. (#50)
- Edit `--verify` (and therefore MCP `hwp_edit`) now accepts `add_table` and image deletion:
  the semantic canonicalizer gained the exact HWPX writer projections for freshly synthesized
  tables (page-break/repeat-header attr and the inline default placement) and for bin streams
  no control references anymore (the HWPX writer only embeds referenced streams), instead of
  failing the re-read comparison. (#50)
- MCP read/convert/render parity + new `hwp_grep` (16 tools total) (#50): `hwp_read` gains
  `html`/`csv` formats, `with_header_footer`/`with_hidden`, and `with_segments` (markdown-only;
  segments are filtered to the paginated window but keep absolute offsets). `hwp_convert` gains
  explicit `to`, `font_dir`, `media_dir` and header/footer/hidden options — and PDF conversion
  now actually receives the configured font directories instead of an empty list. `hwp_render`
  gains `format` (png/svg/pdf), a `pages` range spec (mutually exclusive with the legacy `page`),
  and `output_path` to write PNG/SVG/PDF files and return metadata instead of base64 (the escape
  hatch from the 16 MiB response cap; single-page base64 PNG stays the default). The new
  `hwp_grep` tool searches paragraph text and returns `{matches, count, truncated}` — zero
  matches is a normal result.
- First-class agent skill (#51): the canonical `skills/hwp/SKILL.md` (command quick reference,
  MCP server usage, safety rules) is committed and embedded in the binary;
  `hwp skill export [-o DIR]` materializes it, and `--install claude-code|codex` writes it to
  `~/.claude/skills/hwp/` or `~/.codex/skills/hwp/`. The file is English-only by design — it
  is consumed by agents, and one canonical language avoids bilingual double-maintenance.
- Release packaging + AI integration docs (#51): every tag now also publishes
  `hwp-skill-claude-web.zip` (+ `.sha256`) — SKILL.md, a bootstrap script and the bundled
  Linux x86_64 binary, because the claude.ai sandbox network is registry-restricted and cannot
  download the binary at runtime — and `docs/manual/ai-integrations.md` (English canonical,
  Korean pair) documents per-client setup: Claude Code/Desktop, Codex CLI/cloud, Kiro/Kimi,
  claude.ai skill upload, and Amazon Quick Suite (convert to docx/pdf and upload; remote HTTP
  MCP tracked in #52).

**Fixed**

- Markdown/HTML image references are now bound to the MCP `--root` sandbox (#56, the gap left
  over from #53): `hwp_new` markdown input and `hwp_fill` part files fail closed when a
  referenced image resolves outside every root — whether by absolute path or a `../` escape —
  instead of following the reference and embedding the file. The check shares the
  canonicalized startup roots with the tool-argument guards, runs before any read, and its
  error does not leak the resolved path. CLI callers and root-less servers pass no roots and
  keep the previous degrade-to-alt-text behavior exactly.
- Markdown export: footnotes referenced inside an HTML-fallback table emitted their definition
  as a `<div class="hwp-footnote">` block, which the importer contract rejects — the footnote
  body was dropped in the `cat --format markdown` → `new --from` round-trip. Definitions are now
  always emitted as GFM footnote syntax (`[^N]: body`), and the importer reattaches `fnref`
  markers inside HTML fragments to those definition bodies as real footnote/endnote anchors, so
  the round-trip preserves the note (and no longer leaks a dangling `#fn-N` hyperlink). (#47)

---

## [0.7.1]

**Fixed**

- DOCX export: Word opened the result in Compatibility Mode and reported accessibility as
  unavailable. The package carried no `word/settings.xml`, so Word fell back to the 2007 content
  model; the part is now emitted with `compatibilityMode` 15 (Word 2013+). Found by the first
  real-hardware check of the writer on Word 16.111.2 for macOS — the structural gates could not
  see it. That same session confirmed the v0.7.0 output otherwise opens correctly: no repair
  dialog, correct heading sizes, correct colspan/vMerge tables.

---

## [0.7.0]

**Added**

- DOCX export (GJ-1): `convert --to docx` writes OOXML from the IR (`hwp-convert::docx`) —
  paragraphs with Heading styles, run properties (font, size, color, shade, letter-spacing,
  super/subscript), para alignment/spacing/indents, tables with gridSpan/vMerge and nesting,
  embedded images, hyperlinks, numbering lists, footnotes/endnotes, and page setup from
  SectionDef. Equations fall back to script text. DOCX input stays open (L-tier).

**Fixed**

- DOCX export: Word rejected or misrendered documents produced by the first GJ-1 writer.
  `w:pPr` and `w:rPr` children are now emitted in `CT_PPr`/`CT_RPr` schema order, and line
  spacing is merged with before/after into the single `w:spacing` the schema allows (it was
  emitted twice). Hyphens, non-breaking spaces, line breaks and tabs no longer escape their
  run. Every `abstractNum` defines all nine levels so a `numPr` cannot reference an undefined
  `w:ilvl` (HWP's 10th level is dropped — `w:ilvl` above 8 is invalid). A cell that is both
  colspan and rowspan no longer emits one `vMerge` cell per covered column, which made covered
  rows overflow the table grid; `w:tcW` accounts for the span, and a cell ending in a nested
  table gets the trailing `w:p` the schema requires. C0 control characters are dropped instead
  of producing an unparseable part.

**Verification**

- The DOCX unit tests now validate the emitted package structurally — property order and
  cardinality, run containment, per-row grid coverage against the `w:gridCol` count, and
  `numId`/`ilvl` resolution against `numbering.xml` — rather than asserting on substrings.

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
