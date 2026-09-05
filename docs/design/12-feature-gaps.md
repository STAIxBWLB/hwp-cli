[한국어](12-feature-gaps.ko.md) · [English](12-feature-gaps.md)

# Feature gap catalog and a difficulty/dependency roadmap

This document is the single catalog of **what hwp-cli still cannot do**. Where the format maps
(10 and 11) state as fact "what exists and how we handle it", document 12 evaluates **how that
handling shows up as a defect in Hancom testing, in synthesis and in rendering**, and attaches a
difficulty, a value and dependencies to each gap so that restoration can be prioritized.

## 0. Where this document sits

### 0.1 Division of roles with the other documents

| Document | Role | Relationship to 12 |
|---|---|---|
| [07-hangul-compat-rules](07-hangul-compat-rules.md) §F | The **investigation narrative of unresolved issues** found in Hancom (F1 text-box drop, F2 page overflow) | 12 **inherits by link**: it summarizes and points rather than restating the narrative (see §7 GG) |
| [00-overview](00-overview.md) §5 | A **summary snapshot** of the current state | 12 expands that snapshot item by item |
| [10-hwp5-structure-map](10-hwp5-structure-map.md) §8 | The list of hwp5 records that are **unparsed (Opaque) or raw-preserved** | The **underlying data** for 12 §2 and §3 (what lossless preservation actually loses) |
| [11-hwpx-structure-map](11-hwpx-structure-map.md) §5 | The hwpx read/write **symmetry matrix** (unimplemented, information loss, round-trip asymmetry) | The **underlying data** for 12 §2, §4 and §5 |
| [08-external-research](08-external-research.md) | External evidence: standards, open source and an **ecosystem feature comparison** | The demand and precedent evidence behind §10 GJ and the §16 roadmap |
| **12 (this document)** | **The single catalog of every feature gap, plus the roadmap** | - |

The status labels (Opaque, raw-preserved, skip and so on) are defined authoritatively in **10 and
11**. To change a label, change it there first; 12 follows. Specification § numbers and tag names are
cited as facts; wording is never reproduced (see the [README](../README.md)).

### 0.2 ID convention

- A gap id has the form `series-number` (`GA-1`, `GB-6`). The series:
  - **GA to GG** (first edition): input gate / object types / layout and typesetting / equations /
    the conversion matrix / fields and forms / render precision
  - **GH to GM** (added in the 2026-07-08 re-sweep): export loss / import limits / unsupported
    formats and legacy / editing primitives / text extraction options / CLI commands and workflows
  - **GN** (added in the 2026-08-20 kordoc comparison): the official-document authoring layer
  - **GO** (added in the 2026-08-22 hwp-editor comparison): the GUI-editor engine surface
- Items inherited from 07 §F carry their original number as well: `GG-1 (=07§F1)`.
- Within GE, the special class that loses information **only in an hwpx → hwpx round-trip** is split
  out as `GE-α` (§5.2), and the class where **ancillary data is replaced by constants or regenerated
  when rewriting through the IR** is `GE-β` (§5.3).

### 0.3 The "unimplemented versus lossless preservation" distinction (this document's core criterion)

The same record can be a gap or not **depending on which path you look from**. The single criterion:

> **Opaque preservation is not a gap in a round-trip. It is a gap only in synthesis (cross-format
> conversion) and in rendering.**

- An hwp5 `OpaqueRecord` (the whole subtree preserved; see the status table in
  [10](10-hwp5-structure-map.md) §0) **loses no bytes** in an `hwp5 → hwp5` round-trip, so it is
  not a record-level gap on that path. Since 2026-08-14, native HWP rewrites copy the immutable source
  CFB and patch only planned streams, preserving ancillary streams, storages and untouched binary
  payloads. Package-surgical HWPX editing landed the same day; see the third #90 entry in
  the resolution history (§0.5).
- To **synthesize** that same record as `hwp5 → hwpx`, its meaning would have to be interpreted and
  rewritten as OWPML; that knowledge is missing, so it is **dropped**, making it a gap on the
  synthesis path.
- For the renderer to draw that object (a chart, OLE and so on) it must interpret the payload, which
  it cannot, leaving **a blank**, so it is a gap on the render path.

Every item therefore states its **affected paths** (read / round-trip / synthesis / render). When the
current behavior is `Opaque preservation` and round-trip is not among the affected paths, that is not
a defect but **designed losslessness**.

### 0.4 Item schema

Each gap is described in the following table form.

| Column | Meaning |
|---|---|
| **ID** | `series-number`; items inherited from 07 also carry the original number |
| **Symptom** | The defect a user or reimplementer observes |
| **Code evidence** | `file:line`, verified by comparing the actual file |
| **Spec/format evidence** | An HWP 5.0 § or an OWPML element name |
| **Current behavior** | `refused` / `Opaque preserved (lossless round-trip)` / `dropped (lost)` / `approximated` |
| **Affected paths** | Which of `read`, `round-trip`, `synthesis` (cross-format) or `render` shows the gap |
| **Difficulty** | `S` = data structures only / `M` = needs ground truth / `L` = needs repeated Hancom testing |

The `crates/` prefix is omitted (`hwp5/src/write.rs` means `crates/hwp5/src/write.rs`).

> **Registration history**: first edition (GA to GG) → the 2026-07-08 re-sweep (GH to GM added, GE-α
> and GE-β split out) → the **2026-07-19 exhaustive specification audit** (a three-way comparison of
> the reconstructed specification markdown §3, §4.2, §4.3 and §4.4 against the code and this catalog;
> the 2026-07-19 coverage audit), which added GB-13 to 15, GD-4, GE-9 to 13, GE-α9, GE-β7 to β8, GF-4 to 5 and
> GG-21 to 24, strengthened the write-side evidence for GE-4/5/6, widened the scope of GG-15 and
> GG-19, and sharpened the wording of GB-8 and GF-2 → the **2026-08-20 kordoc comparison**, which
> added the GN series (official-document authoring layer) and GM-10 (PII redaction) and recorded
> the disposition of GJ-1 import.

### 0.5 Resolution history

Resolved items are not deleted from the catalog; the row keeps a ✅ and a date (what used to be a gap
is itself knowledge).

- **2026-08-14 (issue #90 preservation gate)**: native writers gained the closed-enum,
  content-free `hwp-preservation-report-v1` ledger. Source-free authoring and same-format writes now
  fail atomically on writer omissions or unexpected container/package loss; cross-format strict
  conversion also compares semantic assets, controls, relationships and metadata. This resolves
  silent publication but did not itself repair either native writer; the next resolution entries
  record the HWP repair and then the HWPX repair.

- **2026-08-14 (issue #90 source-preserving HWP writer)**: same-format HWP `convert`, `edit` and IR
  `fill` now write against an immutable input snapshot. A no-op is an exact file copy; edits derive a
  stream mutation plan, retain untouched CFB entries and BinData byte-for-byte, preserve unchanged
  BodyText subtrees, and synthesize line layout only for changed or inserted paragraphs. Private
  complex-document acceptance passed no-op, metadata, text, paragraph, table-cell and image cases in
  Hancom with no corruption warning. Package-surgical HWPX editing followed in the next entry.

- **2026-08-14 (issue #90 package-surgical HWPX editing and cross-format loss detection)**:
  the hwpx reader retains verbatim XML for opaque run-level controls
  (`GenericControl.hwpx_raw_xml`, e.g. `hp:container`) and the writer re-emits it, so a full
  HWPX rewrite no longer drops them. Non-regenerated package entries (original META-INF
  overrides, DocOptions, `Contents/memoExtended.xml`, extra previews) ride
  `Document.hwpx_extra_entries`, and unreferenced BinData entries pass through into the
  regenerated content.hpf manifest instead of being dropped. Every same-format HWPX `edit`
  now goes through `hwpx::patch::rewrite_document_staged`, which reserializes only the
  dirty content entries (header.xml / content.hpf / section*.xml, decided by before/after
  IR comparison) and raw-copies every other ZIP entry byte-for-byte; inserted images append
  as new BinData entries with the original OPF manifest ids preserved. Cross-format
  conversion now inventories package assets the target format cannot represent (HWPX extra
  entries on the way to HWP; the hwp5 XMLTemplate/DocHistory slots on the way to HWPX) as
  typed `hwp-preservation-report-v1` events, so `--strict` fails closed on such loss and
  `hwp convert --loss-report <PATH>` writes the JSON verdict either way. Two latent writer
  fidelity bugs were fixed en route (inline `hp:pic` zOrder, no-border lineShape color).
  Private complex-document acceptance (36 package entries, 24 BinData including 7 WMF, 5
  `hp:container`): metadata, text, paragraph, table-cell and image HWPX edits keep the
  entry set identical with all opaque entries byte-identical (media 24→24, 25 after an
  image insert; containers 5→5), `hwp validate` is clean, non-strict hwp→hwpx preserves all
  24 media (previously 9), strict conversion fails closed in both directions, and all eight
  outputs opened in Hancom with no corruption or repair dialog. Still open in #90 (PR 7+):
  the pagination/font gate and certification.

- **2026-08-15 (issue #90 PR 6 — vector images and nested containers)**: HWPX
  `hp:container` children are parsed into `gso_shapes` + `GenericControl.container_box`
  and render at the container origin (nested containers flattened with accumulated
  offsets); the verbatim raw XML stays the reserialization source of truth. WMF picture
  binaries are interpreted by a bounded pure-Rust renderer (`hwp-render/src/wmf.rs`) at
  layout time — window/DC state, pen/brush/font objects, polygons/polylines with
  even-odd or winding fills, DIB blits (1-bpp mask + color transparency pairs, pattern
  brushes approximated as density-blended solids), and CP949 `ExtTextOut` text — so all
  three backends render them instead of emitting magenta placeholders. Out-of-subset
  records are typed bounded-skips (`wmf_unsupported_record_omitted`), malformed streams
  fall back to the placeholder (`wmf_parse_invalid_placeholder`); neither counts as
  parity success. Private complex-document parity: `unsupported_control_omitted` and
  `image_decode_placeholder` eliminated (unsupported 8→0), figure-page raster error
  improved (e.g. worst page 0.43→0.38 bad-pixel), WMF text became searchable; the
  remaining deltas are the PR 7 pagination/font work.
  hwpx→hwp conversion does not yet flag settings.xml/version.xml/preview slot loss with
  typed events. And content.hpf regeneration keeps only modeled manifest items/metadata,
  so unmodeled manifest items survive as orphan package entries (raw-copied but unlisted).

- **2026-08-15 (issue #90 PR 7a — over-height CELL row pagination)**: the renderer now
  paginates a CELL-policy table row taller than the remaining page capacity instead of
  clipping it. An unflagged `v_pos` reset packs softly into the remaining CELL fragment
  capacity, declared row height beyond the cached content is preserved as row-level blank
  continuation fragments, row spans pack span-aware (a span that can move as a unit stays
  together on a fresh page; one intersecting an internally fragmented row or too tall for a
  fresh page surfaces as typed `table_cell_fragmentation_incomplete`), and page-bottom
  slivers are contained. The full behavior contract is in
  [21-pdf-parity §6](21-pdf-parity.md).

- **2026-08-15 (issue #90 PR 7b — font identity gate)**: the render report now carries
  hash-only per-font requested/resolved identities, requested weight state, face index and
  `font_resolution_complete`. The v2 parity `fonts` gate hardens accordingly: substitution-free
  rendering, complete resolution, and every resolved face's byte hash pinned by the manifest —
  missing identity evidence fails closed. Certification deduplicates visible duplicate font
  rows. This closes the font half of the remaining PR 7 delta noted in the PR 6 entry; the
  pagination half is PR 7a above.

- **2026-08-16 (issue #90 PR 8a — certification evidence checks)**: certification gained two
  optional, fail-closed evidence checks, additive to the closed schemas. `preservation`
  ingests a `hwp-preservation-report-v1` artifact and enforces a zero-loss budget;
  `hancom_open` ingests a `hancom-verification-receipt-v1` attestation (new schema) that the
  edited outputs opened in Hancom without corruption or repair. Failed or malformed evidence
  forces `overall=failed`; absent checks keep the previous verdict shape.

- **2026-08-16 (issue #90 PR 8b, in flight — manifest-declared parity gate exclusions)**:
  the v2 parity manifest accepts an optional `gate_exclusions` array (unique gate names, at
  most eight) so a local private profile can declare a gate measured-but-not-blocking.
  Measurement and `passed_gates`/`failed_gates` reporting are untouched; eligibility is
  computed over `blocking_failed_gates` (failed minus excluded), and each scorecard and the
  scoreboard echo `excluded_gates` + `blocking_failed_gates` as required fields — no hidden
  relaxation. With no exclusion declared the schemas keep the legacy strict rule, so the
  public CI profile is unchanged. Contract details in
  [21-pdf-parity §4.3](21-pdf-parity.md).

- **2026-08-14 (issue #77 positioned, counted row/column insertion)**: `--add-row` grew
  from append-only to `TABLE[:AT[:COUNT[:TEMPLATE_ROW]]]` and `--add-col` to
  `TABLE[:AT[:COUNT]]` (`AT` omitted or `end` appends; MCP `add_row`/`add_col` gained the
  matching optional `at`/`count`/`template_row` fields). Insertion builds and validates
  the logical grid first, extends `row_span`/`col_span` of merges crossing the boundary,
  never creates a cell under a covering span, and projects styles for new 1x1 cells from
  the visible cell at `TEMPLATE_ROW` (merged and vertically covered rows can donate
  styles; text is never cloned). Without an explicit template, append keeps the legacy
  clean-row resolver and positioned insertion uses the nearest row at or before the
  boundary. Column insertion keeps the total-width redistribution policy, new paragraph
  instance IDs are issued above the document maximum, and every failure mode (bounds,
  u16 overflow, invariant violation) publishes nothing. Tables inside opaque HWPX
  containers stay fail-closed.

- **2026-08-15 (issue #78 deep table cloning)**: `--clone-table
  "SOURCE_TABLE=>ANCHOR[=>blank|keep]"` (MCP `clone_table` with
  `source_table`/`anchor`/`text_mode`) deep-copies the source table — geometry,
  merge topology, widths/heights, borders, fills, and paragraph/character
  styles — and inserts the clone after the anchor paragraph. `blank` (default)
  keeps one empty styled paragraph per logical cell and drops all source text
  and content controls (fields, bookmarks, hyperlinks, images, equations,
  nested content). `keep` clones nested tables and pictures as well, remapping
  every paragraph instance ID above the document maximum and patching gso
  object identities (the preserved common-data ID at offset 32, or the
  placement z-order that drives the writer's ID synthesis) so the clone shares
  no mutable ID with the source; binary assets are reused in place. Keep mode
  aborts atomically on opaque `Generic` controls (fields, equations, text
  boxes) whose raw identity bytes the model cannot remap — never a silent drop.
  Anchor matching is top-level only (same semantics as `--add-table`), and
  every failure mode publishes nothing.

- **2026-07-15**: GA-5 (version gate), GE-α1 to α5 and α7 (character effects, underline shape and
  numbering format in the hwpx round-trip), GE-β4 (summary information fields), GH-1 and GH-2
  (markdown/HTML links and images), GL-1 (extraction options exposed on the CLI). Implemented in
  parallel by Opus 4.8; all 236 tests passed with an E2E smoke check (links, images, media directory,
  validate).
- **2026-07-15 (second round, from the first Hancom feedback)**: testing found C6 numbering not
  displayed, C8 missing dates and C9 missing subject. **GE-α8** (re-emitting the paragraph-to-numbering
  heading) was resolved, C8 added **PID 0x14, the Korean date string** to summary information
  (measured on 40 genuine files as derived from the creation time in KST, the source of Hancom's
  "date" display), and C9 aligned content.hpf metadata fully with the genuine format (subject and
  keyword meta formats, CreatedDate/ModifiedDate in ISO, date in Korean), **which also resolved the
  hwpx date emission gap**. The FILETIME conversion utility is shared in `hwp-model/src/units.rs`.
  All 247 tests passed. **★ The Hancom gate passed (2026-07-15)**: the first round found C1 to C5 and
  C7 (character effects) correct and C6, C8, C9 defective; after the second round of fixes, re-testing
  confirmed **C6 numbering, C8 dates and C9 subject and date all correct**. Every item in this
  paragraph is confirmed in Hancom.
- **2026-07-15 (third round, the low-cost batch)**: implemented **GC-4** (tab definitions: a new IR
  TabDef, hwp5 §4.2.7 semantic parsing alongside raw, hwpx tabPr round-trip; rendering still
  outstanding), **GC-5** (pass-through of unparsed secPr children), **GC-8 and GC-9** (negative
  indent rendering, splitting a paragraph background across pages), **GE-β5** (pass-through of
  settings.xml and version.xml; hp:switch outstanding) and **GM-7** (`edit --seal`, floating in front
  of text). All 260 tests passed, clippy clean. ~~⚠ GM-7 (D1/D2 seals) and GC-4 (D3 user tabs) await
  Hancom testing~~ → **all confirmed in Hancom (2026-07-18)**.
- **2026-07-18 (fourth round, the Phase 2 specification audit)**: a full audit against the user's
  reconstructed specification markdown (15 confirmed, 4 refuted adversarially) produced fixes for
  **C15** (raw 0x09 tabs, the cause of the A11 hang; a triple defense of the tab = InlineCtrl(9)
  invariant), **in-t tab emission** (A12: a bare tab is ignored at zero width; the type and leader
  mapping was derived by reverse-engineering 91 genuine tabs), **C9** (table common properties
  44B → 46B) and **C10** (removing an incorrect holdAnchor on page-break placeholders). The other 11
  were corrected as comment, design-document or specification errors (the specification half is now
  catalogued in [19 §1](19-hwp5-spec-supplement.md)). After
  four rounds of Hancom testing, **D1, D2 and D3 all pass**: seals and user tabs are confirmed. All
  268 tests passed.
- **2026-07-18 (PR #8, an external contribution, comparison audit complete)**: high-fidelity markdown
  export **resolved GH-3, GH-4, GH-5, GH-6 and GH-8 on the markdown path** (footnote `[^N]` markers,
  merged-cell HTML fallback, in-cell blocks, `- `/`N. ` lists, `$..$` equations and character-effect
  spans) plus `--media-dir` and extended convert text options. Incidental repairs: **fixing an
  OUTLINE heading idRef off-by-one observed in Hancom** (genuine files use idRef=0), tolerating
  non-contiguous and duplicate numbering and bullet definition ids, and **adding `hh:bullets` on
  write** (previously bullet definitions were silently lost in hwpx writing, an unregistered gap);
  list logic moved from hwp-render to hwp-model (tidying the hub and spokes). The comparison audit
  noted that, being export-only, **GI (import) is unchanged, deepening the markdown round-trip
  asymmetry** (raising the priority of GI-1 and GI-2), that the html and odt paths remain, and that
  GH-5 has no dedicated nested-table test.
- **2026-07-19 (the GI batch)**: **GI-1 and GI-2 resolved**: from_markdown reconstructs
  strikethrough, footnotes (`[^N]`/`[^eN]`) and ordered/nested lists into the IR, **closing the
  markdown round-trip** with the #8 exporter (including preserved start values, verified E2E). hwpx
  write gained `footNote`/`endNote` emission (previously DROP) and hwp5 gained footnote and
  NUMBERING/BULLET synthesis. Two pre-existing defects were also resolved: a dangling numbering idRef
  in the approval corpus hwp → hwpx, **registered as GE-7 and root-fixed the same day** (normalizing
  the ±1 boundary between hwp5 read and write, reverting the temporary phantom defense, and locking
  a boundary round-trip test), and a 0-based correction to the C6 verification assertion. The
  verification set was 25/0 (H1 and H2 added), all 297 tests passed. **★ Confirmed in Hancom
  (2026-07-19, three H rounds)**: H1 (hwpx) passed fully on the first attempt. H2 (hwp5) exposed and
  resolved two more defects: 1. the real BULLET layout is 25B with the character at offset 12 (table
  42 is a typo; compared against five genuine records, see [07](07-hangul-compat-rules.md) **B7**),
  and 2. strikethrough is write-only bit18 (unreliable on read because of change-tracking pollution,
  see **B8**; bit18 alone was proven to render in Hancom). Finally, synthesized footnotes, numbered
  and bulleted lists and strikethrough are all confirmed in Hancom on both the hwp5 and hwpx paths.
- **2026-07-19 (finishing GI)**: **GI-3 and GI-4 resolved**: markdown image embedding (base_dir
  relative paths, inline Picture reusing the validated insert_image path, byte round-trip with the #8
  media extraction) and inline code formatting (HCR Dotum plus a light grey shade, with multi-font
  table wiring verified). **The whole GI series is closed.** The set was 26/0 (I1 added), all 303
  tests passed. Outstanding: backticks are not restored on re-export (explicitly out of scope).
- **2026-07-19 (GC-2 and GC-3)**: after a preliminary investigation (an exhaustive sweep of 236
  genuine files) confirmed the layout, closed the "refutation" history in 08 and re-evaluated the
  value, the implementation landed: **blocking cross-conversion loss** (raw alongside, extras kept as
  the identity source of truth, a three-tier per-source single emission) plus **new page border
  rendering** (matching the ground truth on all four sides, independently cross-verified by two
  agents). The set was 27/0 (J1, three layers: validate, inherited XML, render ink 0.95+). All 305
  tests passed. Outstanding: hwpx read enrichment (borders when rendering hwpx directly), and
  EVEN/ODD and body-relative positioning (no genuine sample). **⚠ J1 awaited Hancom testing** (hwp5
  raw → hwpx pageBorderFill emitting real properties is a new emission shape).
- **2026-07-30 (GG-13)**: page number rendering resolved: document start number, PAGE `nwno` restart,
  `pghd` hiding, `pgnp` positions 1 to 10 (with inside/outside odd-even mirroring), decorations and
  supported number formats, plus dynamic substitution of PAGE `atno` in the body, header and footer,
  all implemented in the shared DisplayList stage. PNG, SVG and PDF use the same result. GE-4 (`pgnp
  formatType` fixed to DIGIT in HWPX conversion) remains; GG-16 (header/footer kind selection) was resolved in PR 9.
- **2026-08-13 (PDF parity PR 2, [issue #79](https://github.com/STAIxBWLB/hwp-cli/issues/79))**:
  PR #81 implements renderer-side table page splitting. A table crossing `body_bottom` follows
  `Table.attr` bits 0-1 (pageBreak NONE/TABLE/CELL) and bit2 (repeatHeader) plus `Cell.list_attr`
  bit18 (header cell): NONE pushes the table wholesale to the next page, TABLE/CELL split at row
  boundaries with header rows redrawn on continuation pages, and `treat_as_char` tables never split
  (GE-8's "one character" rule). Boundaries that cross row-spanning cells are excluded. Splits and
  oversized indivisible row bands are reported (`TableSplitAcrossPages` info,
  `TableRowTooTallClipped`). Also fixed
  the stale-`para_top` anchor bug for page-spanning paragraphs. New model accessors
  `Table::page_break_policy/repeat_header`, `Cell::is_header/vert_align`. Cell-internal splitting
  (pageBreak=CELL) is approximated as row-boundary splitting. This is implementation status, not a
  Hancom parity certification. Outstanding: the Hancom verification round (repeated-header ground
  truth) and multi-column-aware table splitting.

---

## 1. GA: the input gate (what is refused outright)

The front line: protection states that either require a supported credential path or are
**deliberately refused** before body parsing. Resolved entries remain here as historical evidence;
open entries still block the whole pipeline when they appear in real documents.

| ID | Symptom | Code evidence | Spec/format | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GA-1 | ~~Password-encrypted HWP5/HWPX is refused before the normal reader~~ | `crates/hwp5/src/read.rs` and `read/password.rs`; `crates/hwpx/src/read/password.rs`; `crates/hwp-cli/src/commands/cat.rs` | HWP5 §3.2.1 bit1 plus genuine-file profile evidence; HWPX ODF encryption metadata plus genuine-file profile evidence | ✅ **resolved 2026-08-26**: a supplied password unlocks the evidenced HWP5 EncryptVersion 4 and HWPX ODF AES-256 profiles for `cat`, `convert`, `render` and matching MCP calls. Wrong/absent passwords share one typed refusal. Verified with seven content-free receipts and direct Hancom comparison | read, convert, render | **L** ★ |
| GA-2 | ~~Distribution (ViewText) documents are refused; even with body content in `/ViewText/Section*`, access is blocked beforehand~~ | `hwp5/src/distdoc.rs` (256B `DISTRIBUTE_DOC_DATA` deobfuscation, SHA-1 derived AES-128-ECB key, raw inflate), `read.rs` (feeds the plaintext into the same `scan_stream`/`parse_section` pair as `/BodyText`) | §3.2.1 bit2, §3.2.3 ViewText; algorithm reconstructed from pyhwp `hwp5/distdoc.py`, not from the Hancom specification | ✅ **resolved 2026-08-20**: `cat`, `convert` and `render` accept distribution documents. Verified against all 11 genuine distribution documents in the corpus (`crates/hwp5/tests/distdoc_corpus.rs`) | read | **M** ★ |
| GA-3 | ~~DRM and certificate-secured documents have no dedicated refusal path~~ | `crates/hwp5/src/file_header.rs` (`is_drm`, `is_cert_encrypted`, `is_cert_drm`, ordered `check_body_readable_after_password`) and `crates/hwp5/src/error.rs` | §3.2.1 bit4, bit8, bit10 | ✅ **resolved 2026-08-20**: dedicated typed refusals with remedy hints. The bit-to-real-world-condition mapping is covered by synthesized headers, not a genuine protected file | read | **S** |
| GA-4 | ~~Digitally signed documents can fall into downstream parsing~~ | `crates/hwp5/src/file_header.rs` (`has_signature`, ordered refusal; signature-spare deliberately excluded) and `crates/hwp5/src/error.rs` | §3.2.1 bit7, bit9; §3.2.8 signature streams | ✅ **resolved 2026-08-20**: bit7 has a dedicated typed refusal; bit9 remains metadata-only. The mapping is not verified against a genuine signed file | read | **S** |
| GA-5 | **Versions passed silently unchecked**: parse checked only the signature, letting 5.1.x and future versions through. Synthesis uses 5.1.x sample constants, so per-version record length differences beyond PARA_HEADER 24/22B are not gated | `hwp5/src/file_header.rs:91-115` (no version check), `write.rs:113` (only one 5.0.3.2 branch), `:1072-1089` (defaults to 5.1.0.1 on parse failure) | §3.2.1 version field | ✅ **resolved (2026-07-15)**: major ≠ 5 is refused with `UnsupportedVersion`, all 5.x allowed | read, round-trip | S |

**GA lesson:** GA-1 required genuine files and crypto reverse engineering (L) and closed on
2026-08-26 with evidence-bounded password support. Its last defect was not in the cipher at all: a
key is derived from the password's **CP949** bytes, not its UTF-8 bytes, so every non-ASCII password
was refused while ASCII ones worked. ASCII is byte-identical in both encodings, which is exactly why
a corpus needs a fixture whose credential is not ASCII ([07 H1](07-hangul-compat-rules.md)). The
HWPX half is measured; the HWP5 half applies the same candidate list on the strength of sharing the
assumption, and stays inference until a genuine HWP5 document with a non-ASCII password is measured. GA-3 and GA-4 closed earlier as explicit local
refusals; their header branches remain honestly marked as unverified against genuine certificate,
DRM or signed files. GA-2 was the other decryption case and is also closed. ★ It was M rather than L because
Hancom's official
「한글문서파일형식\_배포용문서\_revision1.2」 publishes the entire decryption algorithm
(the 256B DISTRIBUTE_DOC_DATA record, the random array, the SHA1-derived key and AES-128 ECB), and
pyhwp has implemented it since 2014 ([08](08-external-research.md), the ecosystem comparison).
GA-5 was a one-line version comparison, fixed immediately.

---

## 2. GB: object types (records and elements exist, but their meaning is unparsed)

The largest series: objects whose record or element **exists and can be scanned and round-tripped**,
but whose payload is not interpreted semantically, leaving a blank in synthesis and rendering. The
key point is **the difference per format**:

- **hwp5** preserves the whole subtree as an `OpaqueRecord`, so `hwp5 → hwp5` is lossless (see the
  Opaque list in [10](10-hwp5-structure-map.md) §8).
- **hwpx read** falls back to `GenericControl`, discarding the object's own properties and keeping
  **only the child subList text** in the IR ([11](11-hwpx-structure-map.md) §3.3).
- **hwpx write** finally emits `DROP` when that Generic is neither a known ctrl_id nor a gso_shape
  and carries no retained raw XML, **losing even the text**. On the same-format path this no longer
  fires: since 2026-08-14 (#90) the reader retains the control's verbatim XML
  (`GenericControl.hwpx_raw_xml`) and the writer re-emits it unchanged.

The same object therefore behaves differently per path: "hwp5 round-trip lossless / hwpx round-trip
lost / synthesis lost / render blank" (GB-6 excepted on the hwpx round-trip since 2026-08-14, #90).

| ID | Object (hwp5 tag / hwpx element) | Code evidence | Spec | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GB-1 | **Charts** (`CHART_DATA` 0x5F / `hp:chart` ooxmlchart) | hwp5 `body_text.rs:617` (Opaque), hwpx unimplemented `write/section.rs:364` (DROP), [11](11-hwpx-structure-map.md) §5(c) | §4.3.9.6 | hwp5 Opaque preserved / hwpx dropped (no text either, that is a total loss) | round-trip (hwpx only), synthesis, render | L; hwpx generation **M** ★ |
| GB-2 | **OLE objects** (`SHAPE_COMPONENT_OLE` 0x54 / `hp:ole`) | hwp5 `body_text.rs:617`, hwpx `write/section.rs:364`, [10](10-hwp5-structure-map.md) table B | §4.3.9.5 | hwp5 Opaque preserved / hwpx dropped | round-trip (hwpx only), synthesis, render | L |
| GB-3 | **Video** (`VIDEO_DATA` 0x62 / `hp:video`) | hwp5 `body_text.rs:617`, hwpx `write/section.rs:364` | §4.3.9.8 | hwp5 Opaque preserved / hwpx dropped | round-trip (hwpx only), synthesis, render | L |
| GB-4 | **Word art** (`SHAPE_COMPONENT_TEXTART` 0x5A / `hp:textart`) | hwp5 `body_text.rs:617`, hwpx `read/section.rs:191` (text fallback) → `write/section.rs:364` (DROP) | §4.3.9 (word art) | hwp5 Opaque preserved / hwpx falls back to text then drops | round-trip (hwpx only), synthesis, render | M |
| GB-5 | **Form objects** (`FORM_OBJECT` 0x5B / `hp:formObject`) | hwp5 `body_text.rs:617`, hwpx `read/section.rs:191` → `:364` | §4.3.9 (forms) | hwp5 Opaque preserved / hwpx text only then dropped | round-trip (hwpx only), synthesis, render | M |
| GB-6 | **Grouped objects** (`SHAPE_COMPONENT_CONTAINER` 0x56 / `hp:container`): hwp5 raw-preserves and renders it (recursing into children); hwpx carries the verbatim XML through same-format rewrites (2026-08-14, #90) and — since 2026-08-15 (#90 PR 6a) — parses container children into `gso_shapes` + `GenericControl.container_box`, so containers now **render** in both formats (child shapes at the container origin, nested containers flattened with accumulated offsets, container text laid out in the container box). Remaining loss: hwp→hwpx synthesis still flattens groups into sibling shapes, and hwpx→hwp5 keeps the typed `OpaqueControlUnrepresentable` failure | hwp5 rendering in `hwp-render/src/shape_draw.rs` ([10](10-hwp5-structure-map.md) §8 raw-preserved), hwpx `read/section.rs` (`collect_container`), render arm in `hwp-render/src/layout.rs` | §4.3.9.7 | hwp5 raw-preserved (renders) / hwpx raw-XML carried (round-trip lossless) and rendered | synthesis | M |
| GB-7 | **Memos** (`MEMO_LIST` 0x5D in the body plus `MEMO_SHAPE` 0x5C in DocInfo / no `hp:` emission in hwpx) | hwp5 `body_text.rs:617`, `doc_info.rs:148` (Opaque); hwpx declares only the namespace ([11](11-hwpx-structure-map.md) §2) | §4.3 (memos), §4.2 table 13 | hwp5 Opaque preserved / hwpx unimplemented | round-trip (hwpx only), synthesis, render | M |
| GB-8 | **Change tracking and edit history** (`TRACKCHANGE` 0x20, `TRACK_CHANGE` 0x60, `TRACK_CHANGE_AUTHOR` 0x61, `PARA_RANGE_TAG` 0x46 / hwpx `hhs:` history). PARA_RANGE_TAG's purpose in the specification also covers **range marking such as highlighting and proofreading marks** (§4.3.5), not only change tracking | hwp5 `doc_info.rs:148`, `body_text.rs:73` (Opaque); hwpx unimplemented ([11](11-hwpx-structure-map.md) §5(c)) | §4.2 table 13, §4.3.5 | hwp5 Opaque preserved / hwpx unimplemented | round-trip (hwpx only), synthesis | L |
| GB-9 | **Arbitrary document and distribution data** (`DOC_DATA` 0x1B, `DISTRIBUTE_DOC_DATA` 0x1C, `COMPATIBLE_DOCUMENT` 0x1E, `LAYOUT_COMPATIBILITY` 0x1F) | hwp5 `doc_info.rs:57` (Opaque), though the writer **synthesizes** COMPATIBLE and LAYOUT separately ([07](07-hangul-compat-rules.md) A4) | §4.2.12 to §4.2.15 | hwp5 Opaque preserved (plus synthesis handling) / hwpx unimplemented | synthesis (partly resolved) | L |
| GB-10 | **Master pages** (hwpx `hm:` master-page; no hwp5 counterpart) | absent in both hwpx read and write ([11](11-hwpx-structure-map.md) §2, §5(c)) | OWPML master-page | unimplemented | round-trip, synthesis, render | M |
| GB-11 | **Unknown objects and forbidden characters** (`SHAPE_COMPONENT_UNKNOWN` 0x73, `FORBIDDEN_CHAR` 0x5E) | hwp5 `body_text.rs:617`, `doc_info.rs:57` (Opaque) | §4.2 table 13 | hwp5 Opaque preserved / hwpx unimplemented | round-trip (hwpx only) | L |
| GB-12 | **The Bibliography storage is not captured**: read does not lift it into the IR and write does not emit it, so it is **lost when rewriting through the IR** (identity round-trips are unaffected) | no read/write branch in hwp5 ([10](10-hwp5-structure-map.md) §1 tree; registered 2026-07-08) | §3.2.12 Bibliography (stored as XML) | dropped (rewrite) | rewrite | S |
| GB-13 | ~~**Captions are entirely unparsed**~~: captions for tables, pictures, shapes and OLE (tables 71 to 73) appear in no IR field, parser or hwpx element (a grep for `caption` across three crates returns nothing). The hwp5 round-trip is lossless because they are buried in common_data raw, but caption text such as "표 1" is blank in synthesis and rendering | no caption branch in hwp5 (`body_text.rs` has no LIST_HEADER caption test), no element names in hwpx read/write | §4.3.9 tables 71 to 73 | ✅ **resolved (2026-08-14, PR 9)**: `Caption` IR on Table/Picture/GenericControl (side, direction, gap, width, last_width, paragraphs); hwp5 discriminates caption LIST_HEADERs per pyhwp `TableCaption`/`GShapeObjectCaption` without adding them to text-box lists and re-synthesizes them with saturating HWPUNIT values; hwpx `<hp:caption>` round-trips for tables, pictures, and generic shapes; extraction preserves visual caption order; the renderer keeps captions on split tables and renders generic/unsupported-GSO captions by side and gap. Verification items: listflags upper bits not round-tripped, spec table 71/72 length inconsistency followed table 72 | synthesis, render | M |
| GB-14 | **NUMBERING start numbers and extended levels are unparsed**: paragraph head information (tables 39 and 40) is read and discarded (`_attr`, `_width`, `_dist`), and the global and per-level start numbers are hardcoded to `start: 1` at every level. Levels 8 to 10 extension fields do not even exist as a concept in the IR (fixed at 7 levels) | `hwp5/src/doc_info.rs:452-479` (`read_level`, `start: 1`) | §4.2.8 | approximated (start fixed at 1) | read (dump/json), synthesis | M (the record's trailing layout must be compared against genuine files) |
| GB-15 | **Image and check bullets are unparsed**: only one bullet character is extracted from BULLET, while the image-bullet flag and id, image information (contrast, brightness, effects) and check characters are preserved only as raw | `hwp5/src/doc_info.rs:125-138` | §4.2.9 | hwp5 raw-preserved / dropped in synthesis and render | synthesis, render | S to M (rare in practice, lowest priority) |

**GB lesson:** looking only at hwp5 → hwp5 round-trips, the entire GB series appears "lossless" and
the gap is invisible (exactly the trap in §0.3). The defects appear only in **hwpx round-trips,
cross-format synthesis and rendering**. GB-6 (grouping) is especially subtle: hwp5 even renders it,
and on the hwpx side it used to vanish entirely; since 2026-08-14 (#90) its raw XML survives a
same-format rewrite, and since 2026-08-15 (#90 PR 6a) hwpx containers render too, so the loss is
now confined to synthesis. Restoring most of this series requires **reverse
engineering the payload from a genuine file containing that object** (M/L), so obtaining ground truth
comes first ([00](00-overview.md) §4). ★ The exception is **the hwpx path of GB-1**: in HWPX a
chart is not OLE but an **OOXML DrawingML `chartSpace` XML part** (`Chart/chartN.xml` plus a manifest
entry plus `hp:chart chartIDRef`), so it can be generated and interpreted with the existing hwpx
writing infrastructure (precedent: kordoc v3.16, see [08](08-external-research.md)).

---

## 3. GC: layout and typesetting

The document opens and the text is visible, but **typesetting properties** (direction, borders,
footnote shape, tabs, columns, indentation) are unapplied or approximated. Each is either hwp5 Opaque
(lossless round-trip), hwpx skip (lost in round-trip), or ignored by the renderer.

| ID | Symptom | Code evidence | Spec | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GC-1 | **Vertical writing unsupported**: direction is always emitted as horizontal | hwpx `write/header.rs:335` (`textDir="LTR"` constant), `write/section.rs:460` (`textDirection="HORIZONTAL"` constant) | OWPML `secPr@textDirection`, `paraPr@textDir` | approximated (horizontal) | synthesis, render | M |
| GC-2 | **Page borders and backgrounds unapplied**. Redefined by the 2026-07-19 investigation: the layout (14B) is confirmed by all 714 genuine records and our round-trip was already correct (closing the "refutation" history in 08). The real gaps were 1. loss in hwp5 → hwpx cross-conversion (replaced by constants) and 2. no rendering at all. hwpx ↔ hwpx was already lossless through the GC-5 pass-through | Ground truth: 제안요청서_11.19.hwp (BOTH = id7 with real borders, BF#7 = four solid black sides at 0.4mm) | §4.3.10.1.3 (table 135's length declaration is a typo; TODO §1.4) | ✅ **resolved and confirmed in Hancom (2026-07-19)**: raw alongside (extras kept as the identity source of truth) plus a three-tier per-source single emission (hwpx original → hwp5 raw interpretation → constants), and new page border rendering (matching the ground truth on all four sides, doubly and independently verified). J1 testing: borders shown on all 34 pages with no corruption. Outstanding: not shown when rendering hwpx directly (read enrichment to follow), and EVEN/ODD and body-relative positioning (no genuine sample) | synthesis (hwp5 → hwpx), render | S to M |
| GC-3 | **Footnote and endnote shape unapplied**. Redefined by the 2026-07-19 investigation: the layout (28B, with a 4B separator length) is confirmed across all 476 genuine records. **The corpus contains zero custom attr values and the current hardcoded rendering already matches every genuine file**, so deferring the rendering is justified. The real gap is only the loss in hwp5 → hwpx cross-conversion | All five distinct genuine values are the default form | §4.3.10.1.2 (the separator length's data type is a typo; TODO §1.4) | ✅ **resolved (2026-07-19)**: footnote and endnote shape kept as raw alongside and emitted to hwpx (28B interpreted). Rendering remains deferred (the current hardcoding matches every genuine file) | synthesis (hwp5 → hwpx) | S |
| GC-4 | **Tab definitions lost** (user tab positions and leader characters) | New IR `TabDef`/`TabItem`, hwp5 `parse_tab_def` (§4.2.7, raw kept alongside so identity is unchanged), hwpx `tabPr`/`tabItem` round-trip. ★ The first round of Hancom testing showed a naked tabItem **hangs Hancom**, corrected to the genuine `hp:switch` structure ([07](07-hangul-compat-rules.md) **A11**) | §4.2.7 `TAB_DEF` / `hh:tabPr` | ✅ **resolved and confirmed in Hancom (2026-07-18, fourth round)**: two defects found in Hancom were resolved by bisection and ground-truth comparison, the raw 0x09 hang ([07](07-hangul-compat-rules.md) **A11**) and a bare tab being ignored at zero width (**A12**, in-t emission with derived attributes). ~~Rendering remains outstanding~~ ✅ **resolved 2026-08-19**: Phase 1 FIDL-03, PR #115 applies the tab positions and leader characters at render time (`crates/hwp-render/src/tab.rs`, `tab_stops` and `next_tab`, with the raw `TAB_DEF` bytes as the fallback); checked in Hancom in the Phase 1 round (`tab-leader.hwpx` draws the leader and the number ends at the stop) | round-trip (hwpx only), render | S |
| GC-5 | **Section properties skipped** (grid, startNum, visibility, lineNumberShape) | `parse_sec_pr` in hwpx now **passes unparsed children through as raw XML** (`secpr_raw_children` plus a pagePr sentinel), and write re-emits the original (falling back to the previous constants when absent) | OWPML `secPr` children | ✅ **resolved (2026-07-15, third round)**: preservation of the original rather than semantic parsing | round-trip (hwpx only), synthesis | S |
| GC-6 | **Multi-column text boxes unsupported**: linked and multi-column text boxes are approximated as a single column | `hwp-render/src/layout.rs:864` (`v1 single column, multi-column in the hwp5 arm unsupported`), `:788` | §4.3.10.2 column definitions | approximated (single column) | render | S |
| GC-7 | **Odd/even adjustment unparsed**: passes through as Generic without semantic parsing | hwpx `read/section.rs:597` (unknown ctrl → code 21 Generic), [10](10-hwp5-structure-map.md) §6.1 footnote | §4.3.10.8 | Generic preserved (unparsed) | synthesis, render | S |
| GC-8 | **Negative indent (hanging indent) ignored in rendering** | Both the body and cell paths in `hwp-render/src/layout.rs` now allow negatives (clamping only at the boundary), test `내어쓰기_첫줄이_왼쪽` | §4.2.10 paragraph shape indentation | ✅ **resolved (2026-07-15, third round)** | render | S |
| GC-9 | **A paragraph background spanning pages was omitted** | The background is split into per-page Rect fragments, test `페이지_걸친_문단배경_조각` | §4.2.5 borders and backgrounds | ✅ **resolved (2026-07-15, third round)** | render | S |

**GC lesson:** GC-2 and GC-3 (page borders and footnote shape) are **frequent in official documents**,
so their value is high. In all of them hwp5 already preserves the information losslessly (Opaque), so
**the data is there**; what is blocked is "interpreting that payload semantically and emitting it to
hwpx and the renderer", which unlocks once ground truth fixes the record layout (M). GC-4 to GC-9 are
mostly local data-structure or render fixes (S).

---

## 4. GD: equations

Most equations render through the mini-TeX typesetter ([05](05-rendering.md), after commit
`ff4184b`), but the following constructs are still approximated or untypeset. The evidence is the
**known-unsupported list** stated in the typesetter's header comment.

| ID | Symptom | Code evidence | Spec | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GD-1 | **Matrices untypeset**: the column alignment character `&` is treated as a space rather than typeset | `hwp-render/src/equation.rs:10` (stated unsupported), `:59` (`'&' => ... column alignment (matrix), treated as a space in v1`) | §4.3.9.3 equation script | approximated (treated as a space) | render | M |
| GD-2 | **Large-operator limits unplaced**: the `sum` and `int` symbols appear but their lower and upper limits are not attached to the operator | `hwp-render/src/equation.rs:10`, `:216` (`sum` → ∑), `:217` (`int` → ∫) | §4.3.9.3 | approximated (placed as scripts) | render | M |
| GD-3 | **Complex delimiters unsupported** (auto-sizing brackets and so on) | `hwp-render/src/equation.rs:10` (`복잡 구분자`) | §4.3.9.3 | approximated | render | M |
| GD-4 | **EQEDIT's own properties unparsed**: the equation record's (table 105) font size (HWPUNIT), color, baseline, version information and font name are not held by the IR `Equation` (only script, size and position). Rendering approximates the font size by working back from the object box, which can differ from the value specified in the source | `hwp-model/src/control.rs:270` (`Equation` structure), `hwp5/src/body_text.rs:592` (`find_eqedit_script`, script only) | §4.3.9.3 table 105 | approximated (back-computed) | render, synthesis | S to M |

**GD lesson:** all three need typesetting metrics matched against **genuine equation ground truth**,
so they are M. The round-trip itself preserves the script verbatim as raw
([10](10-hwp5-structure-map.md) table B `EQEDIT`), so the gap is **confined to the render path**.
rhwp, an implementation in the same language (Rust), already typesets `MATRIX`, `PMATRIX`, `BMATRIX`
and `DMATRIX` and can serve as a reference ([08](08-external-research.md)).

---

## 5. GE: the conversion matrix (loss by direction)

Loss that appears only in cross-format **synthesis**, not in round-trips. Two classes: (§5.1)
deliberate degradation or constant substitution during synthesis, and (§5.2) `GE-α`, round-trip
asymmetry where the value survives to hwp5 but is lost **only in hwpx writing**.

### 5.1 GE: loss by synthesis direction

| ID | Symptom | Code evidence | Spec | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GE-1 | **Deliberate shape degradation for hwpx → hwp5**: a text box hoists its text into the body and the shape wrapper is omitted; purely decorative shapes are dropped (lossless gso re-synthesis is not available) | `hwp5/src/write.rs:467` (`degrade_hwpx_gso`), `:510` (warning) | §4.3.9 objects | dropped (safe degradation) | synthesis (hwpx → hwp5) | L |
| GE-2 | ~~**A picture is dropped when its binary is missing**: if the stream referenced by bin_ref cannot be found, the picture is omitted~~ | `hwp5/src/write.rs` (`bin_stream_index`, mirroring `Document::resolve_bin`: `BinRef::Id` through the `header.bin_data` storage id, `BinRef::ItemRef` by trailing-component match against the `BIN_DATA` table) | §4.3.9.4 pictures | ~~dropped (lost)~~ ✅ **resolved 2026-08-19**: Phase 1 FIDL-02, PR #115 resolves the referenced stream instead of omitting the picture, with the reproduction cases pinned as regression tests in `write.rs`; checked in Hancom in the Phase 1 round (the document shows the referenced image, not a fallback) | synthesis | S |
| GE-3 | **colPr per-column widths and separators uncollected**: assumed equal width with no separator, losing unequal columns | `hwpx/src/read/section.rs:375` (`colSz and colLine children uncollected in v1`), `:392` | §4.3.10.2 / `hp:colPr` | dropped → constants | synthesis, render | S |
| GE-4 | **pgnp page number format fixed to DIGIT**: only Arabic numerals are mapped, losing other formats (circled, Roman, Hangul and so on). **Write has the symmetric defect**: it ignores the format field of hwp5-origin `g.data` and fixes `formatType="DIGIT"` (strengthened in the 2026-07-19 audit) | read `hwpx/src/read/section.rs:429` (`build_pgnp:415`) plus write `hwpx/src/write/section.rs:307-325` | §4.3.10.9 / `hp:pageNum` | approximated (DIGIT) | synthesis (both ways) | S |
| GE-5 | **nwno new-number kind fixed to PAGE**: only the number value is taken and the kind is fixed to PAGE. **Write has the symmetric defect**: it discards `g.data[0..4]` (the kind) and fixes `numType="PAGE"` (strengthened in the 2026-07-19 audit) | read `hwpx/src/read/section.rs:473` (`build_nwno`) plus write `hwpx/src/write/section.rs:343-352` | §4.3.10.6 / `hp:newNum` | approximated (fixed kind) | synthesis (both ways) | S |
| GE-6 | **atno auto-number payload is a constant**: synthesized as the standard 12B constant. **Write has the symmetric defect**: it never reads `g.data` and fixes `<hp:autoNum numType="PAGE"/>`, so even a picture, table or equation number in the hwp5 original (table 143) is recorded as a page number (strengthened in the 2026-07-19 audit) | read `hwpx/src/read/section.rs:465` (`build_atno`) plus write `hwpx/src/write/section.rs:353-358` | §4.3.10.5 / `hp:autoNum` | approximated (constant) | synthesis (both ways) | S |
| GE-7 | **Two conventions for numbering ids**: the IR is 0-based but only hwp5 read lifted the on-disk 1-based value as-is (PR #8 normalized hwpx only), causing dangling idRef and off-by-one in hwp5 → hwpx | hwp5 `doc_info.rs parse_para_shape` (head 2/3 −1 normalization) ↔ `write.rs emit_para_shape` (+1 restoration), convention comment in `hwp-model/header.rs` | - | ✅ **resolved (2026-07-19)**: ±1 normalized at the boundary (locked by a boundary round-trip test). The byte-level roundtrip gate is safe because head 2/3 is measurably absent from every fixture. Side effect: hwp5-origin numbered lists now point at the correct definition (previously an off-by-one fallback) | synthesis (hwp5 → hwpx) | S |
| GE-8 | **Page-spanning tables fail to split (hwp → hwpx)**: a long table that should span pages does not split in the converted file and runs off the bottom of the page (overlapping page numbers, clipped rows). Found in the exhaustive 34-page J1 Hancom review (pages 4 and 6); this was the first Hancom test of a converted page-spanning table (the earlier A5/A6 tables fit on one page) | **Cause confirmed by triangulating ground truth**: pageBreak is innocent (the original attr=2=CELL is consistent). The real culprit was **dropping `linesegarray` from table cells entirely**. Hancom needs per-line vertical positions to split a cell at a page boundary (genuine originals and Hancom's own conversions both keep 100% of cell line layout; only ours had none). The fix: force emission of line layout for table cell paragraphs (following the text-box precedent) plus emitting pageBreak, repeatHeader and noAdjust from the IR attr (constants only as a synthesis fallback) | Spec tables 75/76 (bits 0-1 splitting; "do not split" is a typo), OWPML `hp:tbl@pageBreak` | ❌ **the first fix failed in Hancom (2026-07-19)**: the linesegarray hypothesis was rejected (emitting it still did not split, because Hancom recomputes typesetting and the cache is not a necessary condition; emitting faithful line layout and attr is still correct against genuine files and was kept). treatAsChar=1 was also innocent by measurement. → **The real culprits were confirmed by direct comparison with table-splitting ground truth (2026-07-19)**: 1. **emitting treatAsChar fixed to 1** (the original mixes per table, and page-spanning tables use 0, that is floating; a table treated as a character is "one character" and cannot split, so it runs through) and 2. **recomputing sz height by summing rows** (ignoring the original's common-property value, inflating the problem tables more than twofold). pageBreak was ultimately innocent. Fixed on 2026-07-19: the TABLE object's common properties (table 69) are inherited into the IR as `GsoPlacement` and emitted as hwpx `hp:pos`/`hp:sz` (constants and recomputation only as a synthesis fallback). **Exhaustive ground-truth comparison matched 33/33 tables** (treatAsChar and sz, against the user's Hancom-saved file). hwp5 identity is unaffected (common_data raw is kept). ✅ **Confirmed in Hancom (2026-07-19)**: tables on pages 4 and 6 split correctly, closing three rounds (linesegarray rejected → direct ground-truth comparison → inheritance fix) | synthesis (hwp5 → hwpx), Hancom rendering | M |
| GE-9 | **Floating picture placement lost (hwp5 → hwpx)**: `parse_picture_gso` fills z_order and the vertical and horizontal offsets with **hardcoded 0**, and hwpx write emits those values straight into a floating `<hp:pos>`, so floating pictures pile up at the top left (offset 0) regardless of their original position. This is the asymmetry left behind when GE-8 fixed only TABLE with `GsoPlacement` inheritance; the same solution applies | `hwp5/src/body_text.rs:309-336` (`z_order: 0, vert_offset: 0, horz_offset: 0`) ↔ `hwpx/src/write/section.rs:1563` (emits those values as-is) | §4.3.9 table 69 | approximated (fixed at 0) | synthesis (hwp5 → hwpx) | S (reuse GsoPlacement) |
| GE-10 | **The tail of the object common properties is unparsed**: instance_id, keep-with-next and **the object description (accessibility alternative text)** have no IR field for any object type. The hwp5 round-trip is lossless through common_data raw, but the description is always dropped in hwpx synthesis | `hwp-model/src/control.rs:462` (`GsoPlacement` stops at 32B), hwpx `write/section.rs:1558,1566` (no desc attribute emitted) | §4.3.9 table 69 | dropped (synthesis) | synthesis (hwp5 → hwpx) | S to M |
| GE-11 | **FACE_NAME ancillary information dropped in synthesis**: substitute fonts (actually used in the render fallback), the ten PANOSE fields and the default font are never referenced by hwpx `write_fontfaces`, so hwp5 → hwpx conversions lose substitute font information. The root cause is the hardcoded `type_info: None` on the hwp5 source | hwp5 `doc_info.rs:174-211` (parsed, used for rendering at `fonts.rs:118`) ↔ hwpx `write/header.rs` (unreferenced), `doc_info.rs:208` (`type_info: None`) | §4.2.4 tables 20 to 22 | dropped (synthesis) | synthesis (hwp5 → hwpx) | M (the PANOSE → typeInfo mapping must be confirmed) |
| GE-12 | **Document start numbers unparsed on hwpx read**: write emits `<hh:beginNum>` but read has no parsing for it (a full grep returns nothing), so page, footnote, endnote, picture, table and equation start numbers from an hwpx source never reach the IR | write `hwpx/src/write/header.rs:53-59` ↔ read absent | §4.2.1 / `hh:beginNum` | dropped (read) | round-trip (hwpx → hwpx), synthesis (hwpx → hwp5) | S (the same pattern as GE-3) |
| GE-13 | **Style kind (PARA/CHAR) ignored both ways in hwpx**: read never reads the `type` attribute (attr is always 0) and write fixes `type="PARA"`, so **character styles are always recorded as paragraph styles** and are lost even in an hwpx → hwpx round-trip | read `hwpx/src/read/header.rs:589-597` ↔ write `hwpx/src/write/header.rs:527-538` | §4.2.11 table 48 / `hh:style@type` | approximated (fixed to PARA) | round-trip (hwpx → hwpx), synthesis | S |
| GE-14 | **Equations (eqed) dropped when writing hwpx**: read lifts `<hp:equation>` (and the hwp5 EQEDIT script) into an IR `Equation`, but there was no writer arm, so it fell into the generic fallback and an equation vanished merely by editing or converting an hwpx | write `hwpx/src/write/section.rs` (`write_equation`) ↔ read `hwpx/src/read/section.rs` (`parse_equation`), hwp5 `body_text.rs` (`parse_eqed`) | §4.3.9.3 / `hp:equation` | ✅ **resolved (2026-07-27)**: `<hp:equation>` directly under the run plus `hp:sz`, `hp:pos` and `hp:script` emission (locked by `수식_hwpx_왕복`). ⚠ The equation-specific attributes (version, baseLine, baseUnit, font) are **standard estimates without ground truth** and await Hancom confirmation | round-trip (hwpx → hwpx), synthesis (hwp5 → hwpx) | S |
| GE-15 | **hp:script entities and CDATA lost on read**: `read_element_text` ignored `Event::GeneralRef` and `CData`, so special characters in an equation script such as `x &lt; y` disappeared on read (the `hp:t` parser had this handling; only this path lacked it) | `hwpx/src/read/section.rs` (`resolve_entity`, a shared helper with `parse_text`) | XML 1.0 §4.6 predefined entities | ✅ **resolved (2026-07-27)**: the five references plus numeric references are resolved and CDATA sections collected, pairing with the writer's `esc()` | round-trip (hwpx → hwpx), extraction | S |
| GE-16 | **Attribute-value entities double-escaped (hwpx read)**: `attr()` lifted quick-xml's raw value as-is, so the writer's `esc()` wrapped it again and `&amp;` grew with every round-trip. Bookmark, field and style names and the equation `script` attribute were all affected | `hwpx/src/read/xml.rs` (`attr`, unescape applied) | XML 1.0 §4.6 | ✅ **resolved (2026-07-27)**: entities are resolved as soon as an attribute value is read (an unresolvable reference keeps the original text). Found by adversarial review (codex) | round-trip (hwpx → hwpx), synthesis | S |
| GE-17 | **XML non-characters (U+FFFE, U+FFFF) emitted**: `esc()` filtered only C0 control characters and let non-characters through, producing a package that is not well-formed, which makes parsers and Hancom **reject the whole file** | `hwpx/src/write/templates.rs` (`esc`) | XML 1.0 §2.2 Char range | ✅ **resolved (2026-07-27)**: removed like C0 (`esc_금지문자_제거`). Found by adversarial review (codex) | all hwpx writing | S |

> **2026-07-27 (GE-14 to GE-17)**: the diagnosis in external contribution PR #7 (missing writer arms
> for footnotes/endnotes and equations) was partly pre-resolved: footnotes and endnotes landed in
> `e433462` (a measured version), leaving only the equation emission arm and entity resolution to
> implement separately. **Adversarial review (codex, compared against Hancom's official OWPML model
> source)** produced four further fixes: 1. `lineMode="0"` → the enumerated value `CHAR`
> (`enumdef.h` `g_EquationLineList` = LINE|CHAR, default CHAR), 2. pass-through of non-default
> attributes and common children for hwpx-origin equations (`Equation::raw_attrs` and `raw_props`;
> previously zOrder, textWrap, baseUnit, text color, equation font and PAGE-relative placement were
> all rewritten to defaults), 3. restoring the placement of hwp5-origin floating equations from the
> gso common header (reusing `gso_pos_xml`, isomorphic to pictures in GE-9), and 4. GE-16 and GE-17.
> Equations were also included in the per-run object limit count (undercounting means losing objects).
>
> **Outstanding**: the equation-specific constants on the `<hp:equation>` synthesis path (version,
> baseLine, baseUnit, font) are still **standard estimates without ground truth** (the corpus has no
> hwpx containing an equation). If equations display incorrectly in Hancom (`L1_수식.hwpx`,
> `docs/hancom-verification-checklist.md` §L), replace them with the properties of a genuine saved
> file. That read `trim()`s `hp:script` (not preserving leading and trailing spaces) is also
> unresolved, but has no typesetting impact.

> **2026-07-19 (the GK batch)**: GK-1 and GK-2 implemented. After confirming the five storage rules
> unanimously by measuring all 1,816 merged tables in genuine files, four primitives plus four CLI
> flags plus invariant gates landed. Genuine merged-table regression, round-trip and set 30/0 (K1 to
> K3 added). All 322 tests passed. **✅ The K series is confirmed in Hancom (2026-07-19)**: the
> merge and column manipulation primitives are complete.

### 5.2 GE-α: hwpx round-trip asymmetry (read interprets it; only hwpx write loses it)

A special class. Read **interprets these properties correctly** into the IR, so they survive to
`hwp5`. The hwpx writer, however, flattens them to constants or omits them, so they disappear **only
in an `hwpx → hwpx` round-trip** ([11](11-hwpx-structure-map.md) §5(b)). The shared cause is local
constant-ification in `write/header.rs`, which is why each can be **restored independently by editing
one file**.

| ID | Property | Code evidence (read ↔ write) | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|
| GE-α1 | Character **shadow** (charPr shadow) | read `hwpx/src/read/header.rs:245` ↔ write `write_char_properties` | ✅ **resolved (2026-07-15)**: emitted from the IR | round-trip (hwpx → hwpx), synthesis | S |
| GE-α2 | Character **outline** (charPr outline) | read `read/header.rs:259` ↔ write likewise | ✅ **resolved (2026-07-15)** | round-trip (hwpx → hwpx) | S |
| GE-α3 | **Emboss and engrave** | read `read/header.rs:266,271` ↔ write likewise | ✅ **resolved (2026-07-15)** | round-trip (hwpx → hwpx) | S |
| GE-α4 | **Superscript and subscript** | read `read/header.rs:234,239` ↔ write likewise | ✅ **resolved (2026-07-15)** | round-trip (hwpx → hwpx) | S |
| GE-α5 | **Underline shape** | read `read/header.rs:204` (new IR `underline_shape`) ↔ write likewise | ✅ **resolved (2026-07-15)** | round-trip (hwpx → hwpx) | S |
| GE-α6 | **Gradient center and step** | read `read/section.rs` (`parse_gradation`, angle plus centerX/centerY and step) ↔ write `write/section.rs` (`write_shape_element`, emitting `gr.center_x`, `gr.center_y` and `gr.step` from the IR) | ~~approximated (constant center and steps)~~ ✅ **resolved 2026-08-19**: Phase 1 FIDL-01, PR #115 made both sides IR-driven; checked in Hancom in the Phase 1 round (2026-08-19). Status owned by [11](11-hwpx-structure-map.md) §(b), corrected first | round-trip (hwpx → hwpx), render | M |
| GE-α7 | **Numbering format** (numbering paraHead) | read `read/header.rs:333` ↔ write `write_numberings` | ✅ **resolved (2026-07-15)**: driven by `numbering_levels`, also fixing itemCnt for multiple numbering definitions | round-trip (hwpx → hwpx) | S |
| GE-α8 | **Paragraph-to-numbering link** (paraPr heading): read interpreted it (attr1 bits 23 to 27 plus numbering_id) but write fixed `type="NONE"` | read `read/header.rs:309` ↔ write `write_para_properties` | ✅ **resolved (2026-07-15, second round)**: OUTLINE/NUMBER/BULLET re-emitted; the defect was found in Hancom (C6) | round-trip (hwpx → hwpx), synthesis | S |
| GE-α9 | **Header/footer applyPageType**: read interprets BOTH/EVEN/ODD correctly (hwp5-origin also preserves the applied side in the raw 8B) but write emits the constant `applyPageType="BOTH"`, so odd/even headers and footers are all recorded as "both" in hwpx round-trips and synthesis. This is **damage to the hwpx file itself** (the same symptom appears when opened in Hancom), which is more serious than GG-16 (ignored while rendering). Book-style odd/even headers are frequent in official documents and papers. Found in the 2026-07-19 exhaustive audit | read `read/section.rs:506-517` (`head_foot_data`) ↔ write `write/section.rs:863-901` (`:879` constant) | ✅ **resolved (2026-08-14 doc catch-up; code fixed in 5db1c6a)**: write emits the preserved apply side via `header_footer_apply_page` (`write/section.rs:907-918`) | round-trip (hwpx → hwpx), synthesis | S |

> **Residual sub-gap (related to α5):** among underline shapes, **WAVE** has no mapping in the
> reader's `line_type_code` and is downgraded to SOLID; dotted, double and the rest round-trip
> correctly. Found while building the C-series Hancom test set (2026-07-15).

### 5.3 GE-β: ancillary data lost when rewriting through the IR (added 2026-07-08)

Another special class: **ancillary streams and metadata** that are not body records never reach the
IR, so **rewriting through the IR** (read → IR → write) replaces them with constants or regenerates
them. Unlike the "Opaque lossless" case in §0.3, these are not even Opaque-preserved, so they are
**lost even when rewriting to the same format** (an unmodified identity re-serialization is a byte
copy and unaffected). Note that PrvText (the preview **text**) is regenerated from the body every
time and is therefore not a stale gap; the gaps are the items below.

| ID | Target | Code evidence | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|
| GE-β1 | **The preview image (PrvImage / Preview/PrvImage.png)**: ✅ HWPX raw pass-through resolved 2026-08-14; HWP read still does not capture the source preview and its writer uses only an explicitly rendered option | hwp5 `write.rs` (only when opts provide it), hwpx `read/mod.rs` + `write/mod.rs`, `patch.rs` | HWPX preserved; HWP regenerated or omitted | HWP rewrite | S (if the renderer is reused) |
| GE-β2 | **Scripts (macros)**: the original JScript is discarded and replaced with the constant from an empty Hancom document | hwp5 `write.rs:213-221` (sample byte constants), hwpx `patch.rs:4` | dropped → constant | rewrite | S |
| GE-β3 | **DocOptions ancillary streams**: `_LinkDoc` is a 524B zero constant and the six DRM and signature streams are not emitted | `write.rs:208-210`, [10](10-hwp5-structure-map.md) §1 | dropped/constant | rewrite | M |
| GE-β4 | **Summary information fields lost**: creation and modification time, last saved by, description | `summary.rs`, `write.rs`, `hwp-model/src/document.rs`, hwpx `templates.rs` | ✅ **resolved (2026-07-15)**: Metadata gained description, last_saved_by, create_time and modify_time (raw FILETIME u64) with a read/write round-trip. Print time and statistics remain (defaults emitted) | rewrite | S |
| GE-β5 | **hwpx settings.xml and version.xml replaced by constants** | `Document.hwpx_settings_xml`/`hwpx_version_xml` pass through the original (falling back to the previous constants), including the JSON round-trip | ✅ **resolved (2026-07-15, third round)**: `hp:switch` (inside a section) remains | rewrite | S |
| GE-β6 | **Embedded fonts**: `isEmbedded="0"` is hardcoded, losing the font BinData and the hwp5 typeInfo | hwpx `write/header.rs:84,98,105`, `read/header.rs:132-135`, hwp5 `doc_info.rs:201` (`type_info: None`) | dropped (flag and binary) | rewrite, render | M |
| GE-β7 | **The XMLTemplate storage is silently lost**: FileHeader bit5 is decoded for display only, and `/XMLTemplate` is absent from both the read whitelist and the write storage sequence, so it is dropped **without a warning** when rewriting through the IR (affecting e-government forms and other XML schema-bound documents). Found in the 2026-07-19 exhaustive audit | `hwp5/src/read.rs:20-84` (whitelist), `write.rs:168-228` (missing from the creation sequence), `file_header.rs:64,166` (bit only) | §3.2.10 | dropped (rewrite, silent) | rewrite | S (a raw pass-through slot) |
| GE-β8 | **The DocHistory storage is silently lost**: document history (locked version, date, author, description, DiffML, the most recent copy) disappears entirely when rewriting, **without a warning**. The eight §4.4 records live in a separate tag space with no constants and no parser at all. Found in the 2026-07-19 exhaustive audit | `/DocHistory` absent from `read.rs` and `write.rs` (grep returns nothing), the §4.4 tag space absent from `tag.rs`, `file_header.rs:65,167` (bit only) | §3.2.11, §4.4 | dropped (rewrite, silent) | rewrite | S (preservation) / M (interpreting the content; the LASTDOCDATA flag values are undocumented and need measurement) |

**GE lesson:** GE-1 (shape degradation) shares a root with 07 §F1 (no lossless gso re-synthesis) and
is therefore L. By contrast **the whole GE-α series is low-cost and solvable with data structures
alone**, without ground truth: read already interprets them, so write only needs to emit the
corresponding element. They are local edits to `write/header.rs`, independent of anything in GA to GD
or GG (an "immediately actionable" node in the §16 dependency graph). Note that **GE-β already has a
bypass in the "fidelity-preserving fill" (`patch.rs`)**: for hwpx it preserves the whole package and
replaces only text, so GE-β matters only on the IR path needed for structural editing. The root fix
is to add "ancillary stream pass-through" slots to the IR (mostly S).

---

## 6. GF: fields and forms

All twelve field kinds are parsed on demand ([10](10-hwp5-structure-map.md) §6.2), but there are
gaps in creation and interpretation coverage.

| ID | Symptom | Code evidence | Spec | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GF-1 | **Unknown fields fall back to %unk**: unmapped field kinds and OWPML types are flattened to `%unk`/`UNKNOWN` | `hwp-convert/src/field.rs:69` (`_ => "UNKNOWN"`), `:87` (`_ => *b"%unk"`), `:104` | §4.3.10.15 / `fieldBegin@type` | approximated (fallback) | round-trip, synthesis | S |
| GF-2 | **Index marks, ruby text and overlapping characters unparsed**: preserved only as Generic without semantic parsing (marker-like controls with no paragraph list; hidden comments were split out as GF-5 because they lose content, 2026-07-19) | hwpx `read/section.rs:597` (unknown ctrl → code 21 Generic), [10](10-hwp5-structure-map.md) §6.1 footnote | §4.3.10.10, §4.3.10.12, §4.3.10.13 | Generic preserved (unparsed) | synthesis, render | M |
| GF-3 | **Constraints on creating new fields**: only existing names can be filled, and no new field can be created. Editing can create only `%clk`, `%hlk` and `%bmk`/`bokm` | `hwp-convert/src/field.rs` (limited creation kinds), [README](../README.md) scope and limitations | §4.3.10.15 | unimplemented (creation) | editing | M |
| GF-4 | **22 of 34 field kinds unrecognized, so a field is dropped entirely in hwp5 → hwpx**: `is_field_ctrl_id` recognizes only 12 of the 34 kinds in specification table 128. The 19 change-tracking fields (`FIELD_REVISION_*`), memo (`%%me`), personal-information security (`%cpr`) and table of contents (`%toc`) match no write arm and hit the catch-all DROP, **losing the whole field** (a higher loss grade than GF-1's "kind flattening"). hwp5 → hwp5 is lossless because the hwp5 crate does not use `is_field_ctrl_id`. Found in the 2026-07-19 exhaustive audit | `hwp-convert/src/field.rs:37-53` ↔ the gate's use at `hwpx/src/write/section.rs:271`, catch-all `:386-391` | §4.3.10.15 table 128 | dropped (synthesis) | synthesis (hwp5 → hwpx) | S (extend the recognition list plus the fieldBegin type mapping) |
| GF-5 | **Hidden comment (tcmt) body lost (hwp5 → hwpx)**: the only control in the GF-2 group that has a paragraph list. hwp5 read carries even the body paragraphs into the IR through its fallback, but hwpx write has no dedicated arm and hits the catch-all DROP, so **text content, not a marker, disappears** (it can be emitted following the fn/en arm precedent). Found in the 2026-07-19 exhaustive audit | hwp5 `body_text.rs:625-676` (`collect_paragraph_lists` fallback) ↔ hwpx `write/section.rs:386-391` (catch-all DROP) | §4.3.10.14 | dropped (content) | synthesis (hwp5 → hwpx) | S |

**GF lesson:** GF-1 has a fallback so the file does not break, but the kind information is flattened
(S). The overlap and ruby cases in GF-2 border on the GB-10 series (control character 23) and need
ground truth for semantic rendering (M).

---

## 7. GG: render precision

### 7.1 Inherited from 07 §F

The unresolved issues that 07 §F treats as an **investigation narrative** are inherited here as
catalog items. **07 remains the source of truth for the narrative**; only a summary and a link appear
here (the no-restatement principle, §0.1).

| ID | Symptom | Code evidence | Status and direction | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|---|
| GG-1 (=07§F1) | **Text box drop**: in a round-tripped hwp the text-box frame itself is lost (the text survives by being hoisted into the body) | `hwp5/src/write.rs:467` (`degrade_hwpx_gso`) | Inherited from [07 §F1](07-hangul-compat-rules.md). A root fix requires **property fidelity** on the 239B SHAPE_COMPONENT | dropped (safe degradation) | synthesis (hwpx → hwp5) | L |
| GG-2 (=07§F2) | **Page overflow**: synthesized multi-page documents overflow vertically (markdown is defended by the content_h reset) | `hwp-render/src/lineseg.rs` (`synthesize_linesegs`) | Inherited from [07 §F2](07-hangul-compat-rules.md). Line layout property fidelity is the leading cause | approximated | render, synthesis | L |
| GG-3 (=U2) | **Justified alignment approximated**: justify (0), distribute (4) and divide (5) were treated identically, spaces first, never stretching the last line | `hwp-render/src/layout.rs` (`align_line`/`justify_line`), [05](05-rendering.md) §1.4 | 0 keeps spaces-first without last-line stretch; 4 (distribute) adds the trailing gap and stretches the last line; 5 (divide) stretches the last line without the trailing gap | ✅ **resolved (2026-08-14, PR 7)** — the 4/5 last-line semantics await Hancom confirmation | render | M |
| GG-4 (=U4) | **Letter spacing approximated**: `spacing_pt = size_pt × spacings[lang]/100` was applied in pt floats | `hwp-render/src/shape.rs` (`letter_spacing_pt`), [05](05-rendering.md) §3.2 | spacing is now computed in the HWPUNIT integer domain with half-up rounding (including the relative-size/sup-sub chain), then converted to pt | ✅ **resolved (2026-08-14, PR 7)** — whether the trailing 자간 of a line counts awaits Hancom confirmation | render | M |

**On U1 and U3:** 00 §5 names only "U2 (justify) and U4 (letter spacing)". `U1` and `U3` are defined
nowhere in the docs or the git history, so following the no-guessing principle they are **deliberately
excluded**. If the U series is ever confirmed as a complete U1 to U4 enumeration, they will be added
to this table.

**GG lesson:** GG-1 and GG-2 follow 07 §F's overarching hypothesis ("sufficient property fidelity
resolves them naturally"), so obtaining ground truth plus repeated Hancom testing (L) is the only
route. GG-3 and GG-4 are local to rendering but need pixel comparison against genuine renders (M).

### 7.2 Render property gaps (added in the 2026-07-08 re-sweep)

Properties that exist in the IR (or are preserved in raw) but are not applied by the renderer,
confirmed by an exhaustive re-sweep of `crates/hwp-render/`. The affected path is **render** for all
of them unless noted.

| ID | Symptom | Code evidence | Current behavior | Difficulty |
|---|---|---|---|---|
| GG-5 | **Cell border line type ignored**: `BorderLine.line_type` (dotted, double) was unapplied, so every cell border rendered as a single solid line | `hwp-render/src/border.rs` (`border_strokes`), `layout.rs` (`draw_table_rows`) | ✅ **resolved (2026-08-13, PR 5)**: dash family (codes 2 to 7) renders through `Stroke.dash`, the double family (8 to 11) as offset parallel strokes (visual weight split approximate, pending the Hancom round); `Item::Line` deleted, all border emits on `Item::Path` | S |
| GG-6 | **Paragraph border line type ignored**: the same root as GG-5 on a different path | `layout.rs` (`draw_para_bg_slice`) | ✅ **resolved (2026-08-13, PR 5)**: same `border_strokes` helper as GG-5 | S |
| GG-7 | **Cell and paragraph background hatch and gradient ignored**: `BorderFill` modeled only a solid `bg_color` (hatch/gradient lived in tail raw) | `hwp-model/src/header.rs` (`hatch`/`gradient`), `hwp-render/src/display.rs` (`Fill::Hatch`), `layout.rs` (`bg_fill_item`) | ✅ **resolved (2026-08-14, PR 8)**: hwp5 tail parse (hatch color/style, gradation block) and hwpx `hc:gradation` round-trip; cells, paragraphs and character backgrounds emit `Fill::Gradient`/`Fill::Hatch` via `Item::Path` in all three backends (png segments, svg `<pattern>`, pdf flattened lines). Hatch spacing/width are approximations; hwpx cannot express the hatch style (color only) | M |
| GG-8 | **Emphasis dots not rendered**: the `CharShape.attr` bits 21 to 24 were preserved but there was neither an accessor nor rendering | `hwp-model/src/header.rs` (`emphasis_kind`), `hwp-render/src/layout.rs` (`push_run`) | ✅ **resolved (2026-08-13, PR 6)**: all 13 kinds (DOT_ABOVE to DOT_BELOW, hwplib `EmphasisSort` order) render per glyph; hwpx `symMark` round-trips. The 3/4 TILDE/CARON order conflict between spec rev1.2 table 35 and hwplib awaits Hancom visual confirmation | S |
| GG-9 | **Underline shape (double, dotted, wave) and above-character underline not rendered**: only kind==1 (below) was recognized, with no accessor for the shape bits (4 to 7) | `hwp-model/src/header.rs` (`underline_shape_code`), `hwp-render/src/border.rs` (`decor_strokes`) | ✅ **resolved (2026-08-13, PR 6)**: kind==3 (above) renders, and the 0-based shape codes (solid to double-wave, hwplib `BorderType2`) apply — dash family via `Stroke.dash`, double/weighted as offset strokes, wave as cubic paths. Above-underline y and wave constants are placeholders for the Hancom round | S |
| GG-10 | **Strikethrough shape ignored**: double strikethrough and the rest were unapplied, fixed to a single solid line | `hwp-render/src/shape.rs` (`strike_shape`), `border.rs` (`decor_strokes`) | ✅ **resolved (2026-08-13, PR 6)**: bits 26 to 29 (B8 observation-based) drive the same decor stroke table as GG-9; hwpx strikeout shape round-trips. 3D codes degrade to solid | S |
| GG-11 | **Character shadow offset ignored**: `CharShape.shadow_gap` was unused and a fixed diagonal offset (0.05 to 0.06em) was applied | `hwp-model/src/header.rs` (`shadow_gap`), `hwp-render` `png.rs`/`pdf.rs`/`svg.rs` | ✅ **resolved (2026-08-13, PR 6)**: all three backends offset by `size_pt * gap/100` per axis; the (0,0) gap keeps the legacy 0.06em fallback pending Hancom confirmation | S |
| GG-12 | **Outline numbering is partial**: default head_type 1 markers render, but custom outline definitions/restarts and the sequence beyond the verified 14 Hangul markers are not modeled or oracle-verified | `hwp-model/src/list.rs` (`ListState::marker_for_render`), `hwp-render/src/layout.rs` | approximated (default seven-level format; empty paragraphs and text-box scope covered) | M |
| GG-13 | ~~**Page numbers not rendered**: no page counter; pgnp and atno controls were counted as skipped and never rendered~~ | `hwp-render/src/page_number.rs`, `layout.rs` (`PageNumberState`), `shape.rs` (`shape_range_page`) | ✅ **resolved (2026-07-30)**: start, restart and hiding; pgnp position, decoration and supported formats; dynamic PAGE atno substitution. Unsupported formats fall back to decimal with a warning; GE-4 remains separate; GG-16 was resolved in PR 9 | M |
| GG-14 | ~~**Endnote placement approximated**: rendered like a footnote at the bottom of the anchor page rather than at the end of the document or section (a "position" problem distinct from GC-3's "shape")~~ | `hwp-render/src/footnote.rs:35-72`, `layout.rs` (per-kind split at note collection, section-end flush) | ✅ **resolved (2026-08-14, PR 9)**: layout splits page notes by `NoteKind` — footnotes keep the per-page bottom flush (reservation is now footnote-only), endnotes accumulate section-wide and render as a closing block after the last body content (new page if it does not fit) | M |
| GG-15 | **Image rotation, cropping (imgClip), flipping, brightness/contrast and picture effects not rendered**: `Item::Image` had no transform fields and picture effects (tables 108 to 116) were unparsed | `hwp-model/src/control.rs` (`Picture`), `hwp-render/src/display.rs` (`Item::Image`), `layout.rs` (Picture emit) | ⚠ **partially resolved (2026-08-14, PR 8)**: rotation/flip/crop/brightness/contrast parsed (hwp5 record offsets + hwpx attrs) and rendered in all three backends (png Transform + pre-crop, pdf matrix + clip, svg transform + clipPath). Residual: picture effects are parsed and reported as the typed `picture_effects_unsupported` warning, not rendered; hwp5 flip bits unlocated; brightness/contrast pixel map is a linear approximation | M |
| GG-16 | ~~**Odd/even/first-page header and footer distinction ignored**: the first head/foot found is repeated on every page (distinct from GC-7's section EVEN_ADJUST)~~ | `layout.rs` (`head_foot_apply`, `select_furniture`) | ✅ **resolved (2026-08-14, PR 9)**: all head/foot controls are collected with their apply value (data bits 0-1: BOTH/EVEN/ODD) and the renderer selects by printed page parity (fallback BOTH, then first entry — single-header sections unchanged). FIRST-page furniture has no source-format representation and remains an approximation | S |
| GG-17 | **Column separators not rendered**: `ColumnDef.divider` was dropped by both readers and unused by the renderer | `hwp-model/src/control.rs`, `hwpx/src/read/section.rs` (`colLine`), `hwp-render/src/layout.rs` | ⚠ **partially resolved (2026-08-13, PR 5)**: hwpx read/write of `hp:colLine` plus rendering of the divider between column bands. Deferred: the hwp5 coldef divider parse (byte offsets unconfirmed — every local fixture carries the same single-column COLDEF; `TODO(GG-17)` in `hwp5/src/body_text.rs`) and hwpx → hwp5 synthesis of the divider | S |
| GG-18 | **Line spacing model approximated (synthesis only)**: decided by attr1 & 0x3, treating fixed (1) and minimum (3) identically and misreading margin-only (2) as a ratio. Real files use the cached lineseg and are unaffected | `hwp-render/src/lineseg.rs` (`line_advance_hu`, `compute_linesegs`), `hwp-model/src/header.rs` (`line_spacing_type`) | ✅ **resolved (2026-08-14, PR 7)**: version-aware `line_spacing`/`line_spacing_type`; ratio `base*v/100`, fixed exact `v/2` (clamp removed), margin-only `base + v/2`, minimum `max(base, v/2)` (per-line natural height approximated by `base`, flagged for the Hancom round) | M |
| GG-19 | **Many ParaShape attribute bits unsupported (synthesis only)**, such as prohibition handling, widow/orphan protection and single-line input: only greedy line breaking exists. Widened in the 2026-07-19 audit: attr1's line-break basis, minimum space, keep with next, paragraph protection, page break before, **vertical alignment**, border connection, margin ignoring and paragraph tail shape (table 44), plus attr2's automatic Korean/English and Korean/number spacing adjustment (table 45), all have no accessor and only round-trip as raw | `lineseg.rs:301-333`, `hwp5/src/doc_info.rs:269-319` (only three attr1 accessors) | approximated (synthesis path) | M |
| GG-20 | **Inline control character widths ignored**: fixed-width spaces, hyphens, grouped spaces and the like were not reflected in width computation | `hwp-render/src/shape.rs` (piece loop, `fw_space_run`), `layout.rs` and `lineseg.rs` (wrap guards) | ✅ **resolved (2026-08-14, PR 7)**: HYPHEN shapes a real `-`; NB_SPACE keeps an identifiable U+00A0 source, shapes with the ordinary space advance, and suppresses both adjacent wrap points; FW_SPACE gets a fixed 1em advance | S |
| GG-21 | **Shape line type and arrowheads unapplied on the hwp5 direct render path**: the `dash_pattern` and `arrowheads` functions were called only on the hwpx ShapeGeom path; the hwp5 raw path was fixed to `Stroke::solid` and never emitted arrowheads | `hwp-render/src/shape_draw.rs` (`hwp5_line_style` + `draw_component` SC_LINE arm) | ✅ **resolved (2026-08-13, PR 5)**: the raw path maps the line attribute through `hwp5_line_style` into `dash_pattern` and emits `arrowheads` (head/tail bits flattened to presence booleans, matching the hwpx path). Arrowhead **shape and size** remain flattened to a fixed triangle on both paths | S |
| GG-22 | **Character-level borders and backgrounds not rendered**: `CharShape.border_fill_id` was fully parsed and round-tripped but the renderer never referenced it | `hwp-model` CharShape ↔ `hwp-render/src/layout.rs` (`push_run`) | ✅ **resolved (2026-08-13, PR 6)**: per-run background Rect (emitted before the glyphs) plus four-side borders via `border_rectangle_items`, reusing the paragraph background logic. Box metrics (y - 0.80em to y + 0.25em) are placeholders for the Hancom round | S to M |
| GG-23 | **Ellipse-to-arc conversion unparsed**: only 28B of the 60B ellipse record (table 96) were read, so a converted ellipse always rendered complete | `hwp-render/src/shape_draw.rs` (SC_ELLIPSE, `ellipse_arc_path`) | ✅ **resolved (2026-08-14, PR 8)**: the arc flag (attr bit1), start/end pairs and arc kind (bits 2 to 9) are read; arcs/pies/chords render via the axis-vector generalization of `arc_path`. Kind value mapping (0/1/2 = arc/pie/chord) and sweep direction await Hancom ground truth | M |
| GG-24 | **Diagonal border line type and BORDER_FILL effect bits not rendered**: the diagonal itself has rendered since the `diagonal_dirs` pass, but its `line_type` was dropped and the 3D/shadow effect bits in attr are only raw-preserved | `hwp-model` BorderFill.diagonal ↔ `hwp-render/src/border.rs` | ⚠ **partially resolved (2026-08-13, PR 5)**: the diagonal now honors `line_type` through `border_strokes` (same helper as GG-5). The 3D and shadow effect bits remain raw-preserved | S (rare) |

> Confirmed **not** to be gaps in the re-sweep (to prevent misreporting): width scaling (x_scale),
> the on/off of emboss, engrave, outline and character shadow, cell vertical alignment, cell margins,
> automatic row height, superscript and subscript, character shading and underline color are all
> rendered. GE-α1 to α3 are hwpx **write round-trip** gaps, not missing rendering.

---

## 8. GH: export loss (markdown/HTML/ODT) (added in the 2026-07-08 re-sweep)

What is lost when writing the IR to a text format. The subjects are
`hwp-convert/src/{markdown,html,odt}.rs`.

| ID | Symptom | Code evidence | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|
| GH-1 | **Hyperlink URLs dropped (markdown/HTML)** | `markdown.rs`, `html.rs`, `field.rs` (new `hyperlink_url` helper) | ✅ **resolved (2026-07-15)**: markdown `[text](URL)`, html `<a href>`, with a markdown round-trip preservation test | export | S |
| GH-2 | **Images dropped (markdown/HTML)** | `markdown.rs` (`MarkdownOptions.media_dir`), `html.rs`, `image.rs` (new `image_kind` helper) | ✅ **resolved (2026-07-15)**: html embeds a data URI, and `convert` to .md extracts a `<stem>.media/` sidecar (cat stdout unchanged) | export | S |
| GH-3 | **Foot- and endnotes absorbed inline without markers (markdown/HTML/ODT alike)**: `[^n]` and `<text:note>` unused | `markdown.rs`, `html.rs`, `odt.rs:181-199` | ✅ **markdown resolved (2026-07-18)**: `[^N]`/`[^eN]` markers in the body plus definitions at the end (GFM footnotes). ✅ **html resolved (2026-08-01)**: `<sup id="fnref-N">` anchors plus a `<section class="footnotes">` definitions section (presentational only). ✅ **odt resolved (2026-08-01)**: `<text:note>` (citation + body). All export paths resolved | export | S |
| GH-4 | **Merged cells flattened**: col_span and row_span are unreflected in any output (no colspan/rowspan or columns-spanned emission) | `markdown.rs`, `html.rs`, `odt.rs:203-243` | ✅ **markdown resolved (2026-07-18)**: with merged cells present, falls back to an HTML `<table>` (colspan/rowspan); a GFM table is kept only for unmerged tables. ✅ **html resolved (2026-08-01)**: occupancy-grid colspan/rowspan emission, reconstructed by from_html (contract 18). **Real-machine confirmed (P1)**: merged cells from a part-composed document render correctly in Hancom Office. ✅ **odt resolved (2026-08-01)**: number-columns/rows-spanned + covered-table-cell. All export paths resolved | export | S |
| GH-5 | **In-cell blocks (nested tables, images) dropped**: a cell takes only inline text and discards the block buffer | `odt.rs:215` (blk discarded), `markdown.rs`, `html.rs` | ✅ **markdown resolved (2026-07-18)**: falls back to an HTML table when a nested table or block equation is detected, preserving cell fragments in appearance order and referencing images safely. ✅ **html resolved (2026-08-01)**: the discarded cell block buffer is gone; nested tables and images are preserved (with a dedicated nested-table unit test). ✅ **odt resolved (2026-08-01)**: cell blocks are preserved. All export paths resolved. ⚠ The markdown path still has no dedicated unit test for "an actual nested table inside a cell" (only indirect verification through equations and images) | export | M |
| GH-6 | **Lists flattened (markdown)**: only headings were recognized; bullet and numbered paragraphs were not restored to `- `/`1. ` syntax | `markdown.rs` plus `hwp-model/src/list.rs` (moved from render, now the source of truth) | ✅ **resolved (2026-07-18)**: `- `/`N. ` lists with indentation based on the parent marker width, per-definition number counters with per-section restart, and number format synthesis (non-digit engine marks export as digit ordinals so the md round trip stays clean, #134) | export | S |
| GH-7 | **ODT page layout not reproduced**: margins, columns and header position are omitted (stated in the module comment) | `odt.rs:3-5` | approximated (omitted) | export | M |
| GH-8 | **Equations and character effects dropped (markdown)**: the eqed script was not emitted and underline, strikethrough and sub/superscript were flattened | `markdown.rs` | ✅ **resolved (2026-07-18)**: inline `$..$` and block `$$..$$` equations (the HWP script verbatim) plus `<u>`, `~~`, `<sup>` and `<sub>` spans | export | S |

## 9. GI: import limits (markdown/JSON) (added in the 2026-07-08 re-sweep)

| ID | Symptom | Code evidence | Current behavior | Paths | Difficulty |
|---|---|---|---|---|---|
| GI-1 | **GFM extensions unparsed** (strikethrough, footnotes) | `from_markdown.rs`: STRIKETHROUGH and FOOTNOTES enabled, strikethrough becomes a strike run, and `[^N]`/`[^eN]` are reconstructed as footnote/endnote controls (symmetric with the #8 export structure) | ✅ **resolved and confirmed in Hancom (2026-07-19)**: md → IR → md, hwpx and hwp5 round-trips are closed and H1 and H2 passed in Hancom (footnotes and strikethrough included). Task lists (TASKLISTS) are deliberately excluded, having no IR counterpart | import | S |
| GI-2 | **Ordered and nested lists flattened** | `from_markdown.rs`: ordered becomes a NUMBER heading plus a numbering definition (start preserved), bullets become BULLET, and nesting uses head_level. The IR numbering reference convention is established as 0-based | ✅ **resolved (2026-07-19)**: the round-trip is closed (start=3 is preserved too). Outstanding: saving to hwp5 resets start to 1 (encoding start into the NUMBERING bytes is future work), and lists inside footnotes are excluded in v1 | import | S |
| GI-3 | **Markdown images `![alt](url)` dropped** | `from_markdown_with(MarkdownImportOptions{base_dir})` embeds local paths (an inline Picture plus BinStream, at natural size, shrunk to the body width), and warns with an alt fallback for remote or missing files. Byte round-trip with the #8 media extraction | ✅ **resolved (2026-07-19)**: reuses the validated insert_image path (low Hancom risk) | import | S |
| GI-4 | **Inline code formatting lost** | HCR Dotum (font table index 1, seven slots) plus a light grey shaded CharShape run. Multi-font wiring verified consistent in both writers | ✅ **resolved (2026-07-19)**: outstanding, backticks are not restored on markdown re-export (font-based, so undetectable; out of scope) | import | S |
| GI-5 | **from_json image bytes are conditional**: without `--embed-bin` the bin `data` is skipped and therefore lost | `hwp-convert/src/lib.rs:39,68-96` | partial (conditional) | import | S |

## 10. GJ: unsupported formats and legacy (added in the 2026-07-08 re-sweep)

Gaps along the input and output format axis. For demand and precedent, see the ecosystem comparison
in [08](08-external-research.md).

| ID | Symptom | Evidence | Current behavior | Difficulty |
|---|---|---|---|---|
| GJ-1 | **No DOCX input or output**: the most common interoperability request. Demand is high enough that Microsoft ships an official batch converter (HwpConverter plus BATCHHWPCONV), yet OSS HWP → DOCX is open territory | `hwp-convert/src/docx.rs` | ✅ **output resolved (2026-08-01)**: `convert --to docx` (`hwp-convert::docx` — paragraphs/styles, tables with gridSpan/vMerge + nesting, images, hyperlinks, numbering, footnotes/endnotes, equations as script fallback). Input stays open (L-tier full round-trip) | M (output) / L (input) |
| GJ-2 | **No HWPML (.hml) input or output**: Hancom's official specification (HWPML rev1.2 Part II) and a KS standard exist, with kordoc as an implementation precedent | grep finds nothing; hwpml appears only as a namespace URI | unimplemented | M |
| GJ-3 | **HWP 3.x legacy silently refused**: no `V3.00` signature detection, giving a generic "signature mismatch" error. The official specification (3.0 rev1.2 Part I) exists, with rhwp, kordoc and LibreOffice hwpfilter as precedents | `hwp-cli/src/format.rs:22-38` (CFB and ZIP only) | silently refused | detection S / parsing M to L |
| GJ-4 | **No RTF input or output** | grep finds nothing | unimplemented | M |
| GJ-5 | **No table-to-CSV extraction**: no path to pull tables out as data (quantitative demand evidence is unverified; see the caveat in [08]) | grep finds nothing | ✅ **resolved (2026-08-01)**: `cat --format csv` + `convert --to csv` (`hwp-convert::csv`, RFC 4180) | S |
| GJ-6 | **`.txt` extension inference fails**: `convert -o out.txt` errors, and plain text is only `cat` to stdout | `hwp-cli/src/commands/convert.rs:195-213` (no txt arm) | ✅ **resolved (2026-08-01)**: `ConvertFormat::Txt` + `.txt` inference; also writable to stdout (`-`) | S |
| GJ-7 | **No reverse input from HTML/ODT/PDF**: input is hwp5, hwpx, json and markdown only (four output-only formats) | `hwp-cli/src/commands/cat.rs:18-44` | **HTML partially resolved (2026-08-01)**: contract XHTML subset input (`from_html`, including the markdown-mixed path; docs/design/18). ODT/PDF input remains unimplemented | partial | S (HTML) / L (ODT, PDF) |
| GJ-8 | **HWPX distribution documents**: unsupported by every implementation (H2Orestart #42 open). Whether the official HWP5 distribution specification covers the HWPX variant is unverified | [08](08-external-research.md), open question | unimplemented | L |

## 11. GK: missing editing primitives (added in the 2026-07-08 re-sweep)

Operations absent from the `edit`, `structure` and `format` series. All were confirmed absent by grep;
the evidence is `hwp-convert/src/{edit,structure,format}.rs` and `hwp-cli/src/main.rs:113-165` (every
Edit flag).

| ID | Symptom | Notes | Difficulty |
|---|---|---|---|
| GK-1 | **Cell merge and split**: `merge_cells` (refusing partial overlap) and `split_cell` (A5 to A7 standard empty cells) plus the CLI `--merge-cells` and `--split-cell` | `edit.rs` (a recursive locator, indices matching set-cell). The specification is the **five rules measured across all 1,816 genuine merged tables** (merged-away cells not stored, row-major order, area tiling, row_cell_counts, region size), with an invariant gate after each operation | ✅ **resolved and confirmed in Hancom (2026-07-19)**: K1 (hwpx) and K2 (hwp5) display merges with no corruption |
| GK-2 | **Column add and delete**: `add_col` (preserving total width by redistributing evenly, inheriting the #9 policy) and `delete_table_column`, supporting merged tables (expanding and shrinking spans) with an insert position, plus the CLI `--add-col "table\|table:position"` and `--delete-col` | `edit.rs`: regression across nine genuine merged tables (zero invariant violations) plus the tbl9 total-width preservation test (#9) | ✅ **resolved and confirmed in Hancom (2026-07-19)**: K3 column structure verified |
| GK-3 | **No new table insertion**: from_markdown creates tables, but there is no anchor-based insertion primitive | - | ✅ **resolved (2026-08-01)**: `edit --add-table "anchor=>rows-json"` (`edit::add_table` with the invariant gate) | S |
| GK-4 | **Paragraph shape editing limited to alignment**: no change to line spacing, indentation, left/right margins or paragraph spacing | `format.rs:211-245` (attr1 alignment bits only) | ✅ **resolved (2026-08-01)**: `edit --set-para "find=>key:value"` (line-spacing/indent/left/right/top/bottom, `format::set_para_props`) | S |
| GK-5 | **No header or footer editing**: only inclusion or exclusion during extraction | `text.rs:62-66` | M |
| GK-6 | **No page setup change**: margins, paper and orientation (PageDef is injected as constants only on `new`) | `from_markdown.rs:562-573` | ✅ **resolved (2026-08-01)**: `edit --set-page "key:value"` (dimensions in mm + orientation, `format::set_page_def`) | S |
| GK-7 | **No named style application or creation**: only direct shape manipulation, with no editing of a "Heading 1" style link | all of `format.rs` | M |
| GK-8 | **No object deletion**: images, fields, tables and bookmarks cannot be deleted (only insertion, and paragraph and row deletion, which is asymmetric) | `edit.rs`, `field.rs`, `image.rs` | ✅ **resolved (2026-08-01)**: `edit --delete-image/--delete-table(n|anchor)/--delete-field/--delete-bookmark` (`edit::delete_object` — control + anchor char + FIELD_END surgery) | S |
| GK-9 | **add-row and delete-row table indexing disagreed with set-cell** (top level only versus recursive depth-first), a latent bug | the old nth_table in `structure.rs` | ✅ **resolved (2026-07-18)**: unified on the recursive locator, and add-row also refuses merges and picks a clean template row automatically | S |

## 12. GL: text extraction options (added in the 2026-07-08 re-sweep)

| ID | Symptom | Code evidence | Difficulty |
|---|---|---|---|
| GL-1 | **TextOptions (header and hidden comment toggles) not exposed on the CLI** → ✅ **resolved (2026-07-15)**: `cat --with-header-footer` and `--with-hidden` added (applying to plain and markdown). PR #8 (2026-07-18) extended it to `convert -o *.md` | `hwp-model/src/text.rs` ↔ `main.rs`, `commands/{cat,convert}.rs` | S |
| GL-2 | **Foot- and endnotes cannot be separated or excluded**: always included in the body, with no way to extract only or omit them | `text.rs:62-66` (`_ => true`) | S |
| GL-3 | **No table exclusion or page/section range extraction**: everything or nothing | `text.rs:20-40` | S |

## 13. GM: CLI commands and workflows (added in the 2026-07-08 re-sweep)

What is missing relative to the full subcommand list (`main.rs`: info, cat, convert, render, new,
diff, edit, fields, bookmarks, slots, fill, validate, mcp, dump). Demand evidence is the ecosystem
comparison in [08](08-external-research.md).

| ID | Symptom | Demand and precedent | Difficulty |
|---|---|---|---|
| GM-1 | **No batch, glob or directory processing**: every command takes a single file argument | MS BATCHHWPCONV and headless H2Orestart demonstrate batch demand | ✅ **resolved (2026-08-01)**: convert accepts multiple inputs + `--out-dir` (files named `<stem>.<ext>`) | S |
| GM-2 | **Weak stdin input and stdout piping**: convert and edit require an output file and do not accept `-` (only cat writes to stdout) | Unix CLI convention | ✅ **resolved (2026-08-01)**: convert accepts `-` as input (stdin staged) and as output (text formats to stdout) | S |
| GM-3 | **No document merging**: combining several hwp files into one | A full chapter of the pyhwpx cookbook (merging 33 files into 99 pages); the current solution is Windows COM only and unstable | ✅ **spot-checked in Hancom (2026-08-29, HWP 12.30.0 build 6446, macOS 26.6.2 build 25G83)**: `hwp merge` output opened with no corruption, tampering or repair dialog on three artifacts — two HWP5-input merges to `.hwp` (8 recorded preservation-loss events) and to `.hwpx` (152 events), plus one mixed HWP5+HWPX merge to `.hwp` (16 events). Hancom accepted the files despite those recorded losses; this is not a lossless claim, and the observation is bounded to that exact Hancom build, that exact OS build and those three artifacts — not a general or Windows claim | M |
| GM-4 | **No document splitting or page extraction**: render `--pages` is for images | The pyhwpx cookbook (splitting 100 pages into one file each) | ✅ **spot-checked in Hancom (2026-08-29, HWP 12.30.0 build 6446, macOS 26.6.2 build 25G83)**: `hwp split` produced two fragments from the GM-3 merge output with zero recorded preservation-loss events; both fragments opened in Hancom with no corruption, tampering or repair dialog. Bounded to that exact Hancom/OS build and those two fragments | M |
| GM-5 | **No text search (grep) command**: only edit `--replace` exists | - | ✅ **resolved (2026-08-01)**: `hwp grep <pattern> <file>` (recursive paragraph search, exit code 1 on no match, `--ignore-case`) | S |
| GM-6 | **No bulk metadata editing or dumping**: `--set-meta` is local to new and edit | - | ✅ **already covered (corrected 2026-08-01)**: `hwp info` dumps metadata JSON and `edit --set-meta` edits title/author/subject/keywords | S |
| GM-7 | **Automatic seal and signature stamping**: `edit --seal "anchor=>image@size"` implemented (a floating Picture in front of text, keeping the anchor text, default 20mm) | `hwp-convert/src/image.rs insert_seal` | ✅ **confirmed in Hancom (2026-07-16, D1 and D2 passed)**: settled after three rounds of testing, hwpx uses `IN_FRONT_OF_TEXT` plus `allowOverlap=1` plus an offset, and hwp5 uses attr `0x04aa4310` (in front of text, PARA-relative, body restriction lifted; compared against the §4.3.9.1 bit table) |
| GM-8 | **No document content comparison**: `diff` is render-pixel only, with no text or structure comparison | kordoc compare_documents precedent | ✅ **confirmed (2026-08-29)**: `hwp compare` reports paragraph- and structure-level differences between two documents and leaves both untouched (SHA-256 unchanged before/after the run), exiting 1 when differences exist; read-only, so no Hancom open applies | M |
| GM-9 | **AI integration differs by client environment**: Amazon Quick Desktop can install the publish-safe skill into the active profile and run the local stdio connector, but Quick Web cannot launch `hwp mcp` or share desktop paths. A hosted, authenticated Streamable HTTP service with tenant-isolated artifacts remains unimplemented | Desktop profile discovery and installation are ✅ **resolved (2026-08-09)** by `skill export --install amazon-quick`; Web remains open in [20-remote-mcp](20-remote-mcp.md) and [issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52) | Desktop S / Web L |
| GM-10 | **No PII redaction**: Korean official documents routinely carry resident registration numbers, phone numbers, account and card numbers, but there is no command that detects and masks them; the editing pipeline can only replace strings it is given literally | kordoc `src/redact.ts` precedent (seven detectors, format-preserving masking that keeps digit count and separators, a hit report that never contains raw PII). Decision 2026-08-20: a core `hwp redact` command on the source-preserving edit pipeline, not an external concern; candidate, not scheduled | M |

## 14. GN: official-document authoring layer (added in the 2026-08-20 kordoc comparison)

Korean official documents (공문서) have a statutory form: an eight-level item-mark sequence,
notation rules for dates, times, money and attachments, and fixed document frames. Phase 2.2 now
ships six canonical profiles, verified eight-level numbering and the documented margin family;
the regulation layer continues to ship inside the binary as a bundled skill. The remaining gaps are
notation lint, document frames, template shortcuts, table styling and editing parity. The comparison
that produced this series is kordoc v4.9.0 (MIT), see [08](08-external-research.md); the spec and
phase breakdown are [issue #121](https://github.com/STAIxBWLB/hwp-cli/issues/121).

| ID | Symptom | Notes and precedent | Difficulty |
|---|---|---|---|
| GN-1 | ✅ **resolved 2026-08-22 (phase 2.2)** — all canonical profiles render the statutory `1. / 가. / 1) / 가) / (1) / (가) / ① / ㉮` ladder through visible depth 8; authored depth 9+ fails closed | HWPX writes the `CircledHangul` mapping directly. HWP5 uses the safe, direct encoding observed in genuine Hancom Office, not an unsupported literal/raw-byte claim. Hancom observations confirmed post-`하` continuation at levels 2, 6 and 8, source order and visible marks in 14/14 HWP/HWPX profile artifacts | M |
| GN-2 | ✅ **resolved 2026-08-22 (phase 2.2)** — canonical official profiles, one per document type: `official`, `report`, `plan`, `notice`, `minutes`, `press` | Canonical aliases and Korean names normalize before writer selection. All profiles use 20/10/20/20 page margins; body, header/footer and page-number defaults were observed in genuine Hancom Office across 14 artifacts. **Amended 2026-08-22**: the seventh profile `gaejosik` was retired against the 한국공공언어진흥원 공문서 작성법 길라잡이 (2025-01-08), which places 개조식 as a sentence-ending style available inside 내부결재 bodies rather than a document class. Its emitted header was identical to `notice` and its Malgun Gothic choice was never sourced; 개조식 lives in the skill's regulation reference §6 어투 | S |
| GN-3 | ✅ **resolved 2026-08-23 (phase 2.3)** — ~~**No notation check**: `hwp validate` is structural and has no ruleset for the regulation's notation~~: `hwp lint` ships ten rules over `.md`/`.hwp`/`.hwpx` and stdin markdown (seven notation rules for date, time, money, the `붙임:` colon and its numbering, the closing `끝.` and punctuation; one against decorative item marks; two structural rules for statutory item marks and Roman-numeral headings) | `crates/hwp-convert/src/lint.rs`; advisory by default, `--strict` exits 1 on an error-severity finding, `--json` emits `hwp-lint-report-v1`, and only the two structural rules are error severity. Exposed as the MCP tool `hwp_lint` | S |
| GN-4 | ✅ **resolved 2026-08-24 (phase 2.4)** — ~~**No document frames**~~: 기안문 두문 (기관명, 수신, 경유) and 결문 (발신명의, 결재/협조, 시행/접수, contact block), the 공고문 head and foot, and the 보도자료 머리박스 are generated via `hwp new --doc-head`/`--doc-foot`/`--notice-head`/`--notice-foot`/`--press-head`, reachable over the CLI and MCP (`hwp_new`) | Every frame is a table wrapped in one anchor paragraph, sharing `table_paragraph()`'s shape (D-02, D-10). Hancom acceptance procedure (07 PROC) recorded for this phase's outputs | M |
| GN-5 | ✅ **resolved 2026-08-24 (phase 2.4)** — ~~**No document templates**~~: `hwp new --template <slug-or-alias>` embeds all eight document skeletons directly (`--list-templates` to enumerate), and `minutes.md` reflects the nine statutory elements (공공기록물 관리에 관한 법률 시행령 제18조, D-19) | Embedded markdown skeletons resolved through `commands::skill::template_file`, feeding the existing `hwp fill` path; mutually exclusive with `--from` and every frame flag (D-05) | S |
| GN-6 | ✅ **resolved 2026-08-24 (phase 2.4)** — ~~**Generated tables are unstyled**~~: every GFM table gets header-row shading, bold, centring and content-proportional column widths at import time (D-07), and `hwp edit --style-tables`/MCP `hwp_edit.style_tables` applies the same styling to existing files, byte-stable on reapplication (D-08) | Content-driven styling (`hwp-convert::style_table`/`style_tables`), Hangul-aware `display_width`; the workspace skill's `style_pass.py` and its corpus evidence were the source | S |
| GN-7 | ✅ **resolved 2026-08-20 (phase 2.1)** — ~~**The regulation layer is not shipped**~~: the official-document guide, the regulation reference and the templates now ship in the bundled skill (`hwp skill export`) as a directory tree, English canonical with Korean mirrors, guarded by the drift and EN/KO parity gates | Living checklist and subcommand-grain parity matrix in [23-hwpx-skill-absorption](23-hwpx-skill-absorption.md) (issue #121); capabilities still native-side land as GN-1 to GN-6 and GN-8 in phases 2.2-2.5 | S |
| GN-8 | ✅ **resolved 2026-08-26** — ~~**Editing parity with the retired skill unproven**: the workspace skill filled slots that span runs~~: `hwp fill` now coalesces a `{{slot}}` that inline formatting split across text runs, and `hwp slots` and `hwp fill` are pinned to agree by `crates/hwp-cli/tests/slot_fill_agreement.rs` | The filled value inherits the first run's character shape. A slot still cannot cross a line break or a paragraph boundary, which is deliberate ([skills#35](https://github.com/STAIxBWLB/skills/issues/35)) | S |
| GN-9 | ✅ **resolved 2026-08-22 (phase 2.2)** — the unsourced 30/15/20/15 preset tuple was replaced with top/bottom/left/right 20/10/20/20 mm for every canonical profile | [23-hwpx-skill-absorption](23-hwpx-skill-absorption.md) §3 records the D-14 source boundary. 14/14 genuine Hancom HWP/HWPX observations verified the profile layouts. Per-side CLI/MCP margin overrides remain caller-controlled, not statutory claims | S |

GN-1, GN-2 and GN-9 are resolved in phase 2.2; GN-7 was resolved in phase 2.1; GN-4 (frames),
GN-5 (template shortcut) and GN-6 (table styling) are resolved in phase 2.4, GN-3 (lint) in phase
2.3, and GN-8 (editing parity) on 2026-08-26. The GN series is closed.

## 15. GO: the GUI-editor engine surface (added in the 2026-08-22 hwp-editor comparison)

The embeddable GUI editor lives in a separate repository (`hwp-editor`, UI and thin adapters
only) and delegates every document operation to the `hwp` binary. The 2026-08-22 repo-collection
comparison (`refs/hwp/.planning/codebase/HWPCLI-COMPARISON.md` §6 — private working notes, not
tracked here) plus an audit of that repo's
engine adapter (pinned to hwp-cli v0.8.7) surfaced the engine-surface gaps below; all six are
scheduled in milestone 2 (Phases 5-7 and 9, requirement ids EDT-01..07). The same triage excluded
OCR (ONNX runtime vs. the single-binary constraint), an LLM layer (lives in MCP clients), COM
automation (intentional exclusion) and capsule provenance (rhwp-specific; `certify` covers the
quality-policy niche) — reasons recorded in the milestone's `.planning/REQUIREMENTS.md`, which is
local planning state rather than tracked documentation. Chart/OLE read+render
joins the milestone-4 catalog (with GB-1); GA-1 was pulled into milestone 2 (Phase 8).

| ID | Symptom | Notes and precedent | Difficulty |
|---|---|---|---|
| GO-1 | **Segment envelope is paragraph-only**: `hwp cat --with-segments` emits `kind: "para"` segments with section/para indices and nothing else — no run/table/cell/image/field kinds, no stable ids, no style summary, no schema | hwp-editor `packages/core/src/segments.ts` parses exactly this shape; a versioned v2 envelope (`schemas/segment-envelope-v2.schema.json`, flag-gated) keeps v0.8.x output as default for one release | S |
| GO-2 | **No render-side geometry**: the renderer emits pixels/SVG/PDF but no machine-readable bounding boxes, so an editor cannot map a click to a segment (hit-testing), draw a selection overlay or scroll-sync | `hwp render --layout-json <path>` writing per-page boxes keyed by segment id (`schemas/render-layout-v1.schema.json`); boxes come from the existing display-list/layout pass | M |
| GO-3 | **Raster formats are PNG/SVG only**: hwp-editor's `PageImageFormat` includes jpeg/webp and its server rejects them at `cli-engine.ts:366` | `--format jpeg|webp` on the existing raster path | S |
| GO-4 | **Edit ops are separator-delimited argv**: values containing `=>`, `=` or `:` are ambiguous (hwp-editor `ops.ts` documents the caveat), and free-text anchors collide on duplicated text | `hwp edit --ops <file.json>` applying a typed ops array (`schemas/edit-ops-v1.schema.json`) covering the 28 existing op kinds | S |
| GO-5 | **No paragraph/run addressing**: table ops already address by numeric `table:row:col` coordinates, but everything outside a table targets text anchors only, so run-range character formatting, paragraph move, list indent/outdent and paragraph style cannot be expressed | Segment-id / `section:para` / run-range addressing on the GO-4 channel; proven on documents where the same text appears twice | M |
| GO-6 | **No structured edit feedback**: `hwp edit` reports applied counts and names unapplied requests on stderr and fails closed without `--allow-partial`, but there is no machine-readable report, no changed-segment ids and no way to preview an edit without writing | `hwp edit --report <path>` (applied/failed ops + changed segment ids) and `--dry-run` | S |

GO-2 and GO-5 are M because layout coordinates and run addressing must be checked against genuine
files; the rest are data-structure work on existing paths. Writer-touching items (GO-4..GO-6
output paths) end with the Hancom acceptance procedure (07 PROC) per the milestone-2 phases.

## 16. Roadmap: difficulty × value plus the dependency graph

### 16.1 The difficulty × value matrix

**Value** is based on frequency in real documents plus practical demand (the ecosystem comparison in
[08](08-external-research.md)).

| | **Difficulty S** (data structures only) | **Difficulty M** (needs ground truth) | **Difficulty L** (repeated Hancom testing) |
|---|---|---|---|
| **High value** (frequent) | ✅ the whole official-document GN series is resolved: ~~GN-2, GN-3, GN-5, GN-6, GN-7, GN-8~~ (official presets, notation lint, document templates, table styling, bundled regulation layer, editing parity — 2026-08-20 to 2026-08-26). GC-4 and GC-5 (tabs, section properties), GC-8 and GC-9 (hanging indent, paragraph background). ✅ resolved 2026-07-15: ~~GE-α1 to α5 and α7, GH-1 and GH-2, GL-1, GA-5, GE-β4~~ / ✅ resolved 2026-07-18 (markdown): ~~GH-3, GH-4, GH-5, GH-6, GH-8~~ | **GN-1, GN-4** (statutory eight-level marks, document frames — writer-visible, Hancom acceptance), GG-3 and GG-4 (justify, letter spacing), GF-2 (index marks, overlap), ~~GA-2 ★~~ (reading distribution documents, resolved 2026-08-20), ~~GJ-1 output~~ (DOCX export resolved 2026-08-01), **GK-1** (cell merge), **GK-2** (column deletion; addition resolved on 07-19). ✅ GC-2 and GC-3 resolved on 07-19 (J1 awaiting Hancom) | GG-1 and GG-2 (text box drop, overflow) |
| **Medium value** | GC-6 (multi-column text boxes), GE-2 to GE-6 (picture drop, columns, number synthesis), GF-1 (%unk), **GB-12** (bibliography), **GE-β1, β2, β5** (preview, scripts, settings), ~~GG-16~~ (local rendering, resolved in PR 9 alongside GG-5, GG-6, GG-8 to GG-11, GG-17, GG-20, GG-21, GG-22 from 2026-08-13/14), ~~GH-3, GH-4, GH-5~~ (footnote markers, merged cells, in-cell blocks; markdown resolved 2026-07-18, html and odt 2026-08-01), ~~GJ-5 and GJ-6~~ (csv, txt, resolved 08-01). ✅ the whole GI series and GE-7 resolved on 07-19. ~~GK-3, GK-4, GK-6, GK-8~~, ~~GM-1, GM-2, GM-5~~, ~~GM-3, GM-4, GM-8~~ (merge, split, compare — shipped in v0.13.0 and spot-checked in Hancom on 2026-08-29; see the rows above), **GM-6** (corrected to already-covered), GM-7 (sealing, resolved 07-16) — ✅ resolved in the 2026-08-01 batch | GB-4 to GB-7 and GB-10 (word art, forms, grouping, memos, master pages), GC-1 (vertical writing), GD-1 to GD-3 (equations; rhwp precedent), GE-α6 (gradients), GF-3 (field creation), **GB-1 hwpx chart generation ★** (chartSpace; kordoc precedent), **GJ-2 and GJ-3** (hml, HWP 3.x; specifications public), **GG-7, GG-12 to GG-15, GG-18, GG-19** (render pixel comparison), **GE-β3 and β6** (DocOptions, embedded fonts), **GH-7** (ODT layout), **GK-5 and GK-7** (header editing, styles), **GM-10** (PII redaction; kordoc precedent) | GB-2 and GB-3 (OLE, video), **GJ-1 full round-trip** (DOCX import; 2026-08-20 disposition: stays out of scope and is revisited only after every other roadmap item is complete — the same call covers PDF, XLS and XLSX import, with kordoc as the covering tool meanwhile), **GM-9 Web** (authenticated hosted MCP, tenant isolation and operations; Desktop is resolved) |
| **Low value** (rare) | **GI-5** (embed-bin), **GL-2 and GL-3** (extraction granularity) | **GJ-4** (rtf) | GB-8, GB-9, GB-11 (change tracking and so on), **GJ-7** (reverse input), **GJ-8** (HWPX distribution) |

GA-1, GA-3 and GA-4 are removed from the open-value matrix: password input closed on 2026-08-26,
and the certificate/DRM/signature usability gaps closed as explicit refusals on 2026-08-20.

**The 2026-08-22 GO editor series** sits at **GO-1, GO-3, GO-4, GO-6 = S, GO-2 and GO-5 = M**, all
with a concrete consumer (the separate hwp-editor repo), and is scheduled as milestone 2
(Phases 5-9). It is listed here rather than in the cells because the cells predate the series;
the same decision pulled GA-1 into milestone 2.

**How to read it:** the top left (S, high) has **the best return**. Beyond GE-α (character effect
round-trips), **GH-1 and GH-2** (markdown/HTML links and images, reusing the ODT embedding pattern)
and **GL-1** (just a clap flag) were the new entry points. ★ marks the 2026-07-08 re-evaluation:
**GA-2 (distribution) dropped from L to M because the decryption algorithm is public** (and was
resolved on 2026-08-20), and
**the hwpx path of GB-1 (charts) dropped from L to M because it is an OOXML chartSpace**. The 2026-08-20 addition is the **GN official-document series**: six of its eight items are S, because
the regulation layer is mostly presets, text rules and shipped documents rather than format work; only
GN-1 (a new numbering format in the IR and both formats) and GN-4 (frames the writer emits) need ground
truth and Hancom acceptance. The bottom right (L, low) is the lowest priority.

### 16.2 The dependency graph

```
[obtain ground truth]  ──precedes──▶  GB-1 to 7 (object rendering)  ──needs──▶  10/11 record structure interpretation
   │                       GC-2/GC-3 (page borders, footnote shape) ── FOOTNOTE_SHAPE/PAGE_BORDER_FILL semantics
   │                       GD-1 to 3 (equation typesetting)  ── genuine equation metrics
   │                       GG-1/GG-2 (property fidelity) ── repeated Hancom testing (07 §F)
   │                       GG-7/GG-12 to 15/GG-18/19 ── pixel comparison with genuine renders
   │
[official specification exists, no reverse engineering needed] ──▶ GA-2 ✅ (distribution decryption, 배포용문서 rev1.2)
   │                              GJ-2 (HWPML, spec Part II)   GJ-3 (HWP 3.x, spec Part I)
   │                              (but specification-versus-real-file mismatches exist, so corpus verification is separate)
   │
[independent, immediately actionable] ──▶ GE-α6     (gradient center and step; α1 to α5, α7 and α8 ✅ resolved 2026-07-15)
                    GE-2       (local to write.rs, turning the picture drop warning into recovery)
                    GA-3/GA-4  (refusal messages; the GA-5 version gate is ✅ resolved)
                    GC-4 render (apply tab positions and leaders in hwp-render/tab.rs; the round-trip is ✅ resolved)
                    (✅ resolved: GH-1/GH-2, GL-1, GC-4/5/8/9, GE-β5; GM-7 implemented and awaiting Hancom)
   │
[highest demand] ──▶ GJ-1 (DOCX output) ──quality first──▶ GH-1/GH-2/GH-4 (links, images and merged cells
                                               become the base data for the DOCX mapping)
   │
[concrete web consumer] ──▶ GM-9 Web ──precedes──▶ protocol-core extraction + artifact model
                                      ──then──▶ OAuth resource server + Streamable HTTP + tenant operations
   │
[regulation layer, mostly independent] ──▶ GN-7 (ship the rules, the contract and the templates)
                    ──precedes──▶ GN-1/GN-2 (marks and presets)  ──precedes──▶ GN-4/GN-5/GN-6
                                        │                                        (frames, templates, table styling)
                                        └──▶ GN-3 (notation lint; needs the rules, not the engine)
                    ──finally──▶ GN-8 (editing parity, then the old skill is retired)
```

**Dependency rules in brief:**

- **GB object rendering** requires the record and element structure interpretation of 10 and 11 first
  (currently Opaque or fallback, so the semantic fields are absent from the IR). Most also require
  **obtaining ground truth first** ([00](00-overview.md) §4).
- **GC-2 and GC-3** (page borders, footnote shape): hwp5 already preserves the information as Opaque,
  so this is a three-step path of "fix the record layout with ground truth → promote to semantic IR
  fields → emit to hwpx and the renderer".
- **The whole GE-α series** is an **independent node depending on nothing**, since read already
  interprets it; the shortest path is adding the corresponding emission on write.
- **GG-1 and GG-2** share a root with the unresolved items in 07 §F (property fidelity), so
  **repeated Hancom testing plus ground truth** are jointly prerequisite.
- **The GN series** depends on the engine only through GN-1 (a numbering format the IR and both
  formats must carry) and GN-4 (paragraphs and tables the writer emits). GN-7 comes first because
  the rules it ships are the specification the rest is checked against, and GN-8 comes last because
  it is the proof that nothing the retired skill did was lost.

### 16.3 Items gated on ground truth (needing genuine files and Hancom testing)

The following can only start once **a genuine Hancom file is available**, per the ground-truth
methodology in [00](00-overview.md) §4 (no guessed typesetting). The rest (especially GE-α, GH, GL,
local GC and local rendering) can proceed with data structures and rendering alone.

- **GB-1 to GB-7, GB-10**: charts, OLE, video, word art, forms, grouping, memos, master pages, each
  needing a genuine file containing that object
- **GC-1, GC-2, GC-3**: vertical writing, page borders, footnote shape, each needing a genuine file
  using that typesetting
- **GD-1 to GD-3**: genuine equations containing matrices, large operators and complex delimiters
- **GG-1, GG-2**: repeated Hancom testing as narrated in 07 §F
- **GG-13 to GG-15, GG-19**: settled by pixel comparison with genuine renders (GG-12 resolved in PR 3, GG-18 in PR 7, GG-7/GG-23 in PR 8, GG-14/GG-16 in PR 9)
- **GJ-2, GJ-3**: can start from the official specification, but because
  specification-versus-real-file mismatches are known ([08](08-external-research.md): column
  definitions 14 versus 16B), genuine corpus verification must run alongside. GA-2 took exactly this
  route and is resolved: the published algorithm was implemented, then checked against all 11
  genuine distribution documents
- **Three deferred verdicts (from the 2026-07-19 exhaustive audit; whether they are gaps at all is
  unconfirmed, so they must not be registered before measurement)**: 1. the current code reading the
  first field of an arc (SC_ARC) as 1B versus the UINT32 declared in specification table 101
  (`shape_draw.rs:571-576`), needing a byte comparison against genuine arc shapes; 2. drawing object
  hflip/vflip (table 83 attr bits 0/1) unread, possibly already implied as a negative scale in the
  render matrix, needing a genuine flipped shape to compare; 3. the current code deriving the
  ParaShape line spacing kind only from the legacy attr1 bits 0-1 (`doc_info.rs:297`) versus table 46
  attr3 (5.0.2.5+, which includes "minimum"), needing a genuine file with "minimum" line spacing to
  check whether Hancom also writes attr1 in sync on newer versions

Two pieces of verification infrastructure would lower the cost of this whole section, both surfaced by
the 2026-08-20 comparison ([08](08-external-research.md)) and neither scheduled: a **Hancom COM
automation harness** on Windows that renders and re-extracts through real Hancom (kordoc
`bench/hangul-com-pdf.ps1`), which would make the acceptance procedure repeatable instead of manual;
and a **public-corpus collector** for in-the-wild government documents feeding the existing
`HWP_CORPUS_DIR` soak tests (kordoc `bench/collect-korea-kr.mjs`). The collector is compatible with
the data policy — the corpus is fetched locally and never committed.

---

**Summary:** the first edition's low-cost, high-value entry points (GE-α character effects, GH-1 and
GH-2 links and images, GL-1 extraction options, GA-5 the version gate, GE-β4 summary information)
were **all resolved on 2026-07-15** (§0.5). The next entry points are **GC-8 and GC-9** (hanging
indent and paragraph background, S) and **GE-β5 and GM-7** (settings pass-through and sealing, S).
The high-value, high-difficulty frontal approach was **GC-2 and GC-3** (page borders and footnote
shape, frequent in official documents) plus **GA-2** (reading distribution documents), all three of
which are now resolved. The former largest demand, **GJ-1** DOCX output, was resolved on 2026-08-01
(export only; input remains L-tier) — the next large item was the GM-3/4/8 family, spot-checked in Hancom on 2026-08-29; the milestone now moves to the release gate.
