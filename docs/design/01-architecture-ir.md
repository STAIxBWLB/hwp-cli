[한국어](01-architecture-ir.ko.md) · [English](01-architecture-ir.md)

# The hwp-cli workspace architecture and IR, documented for a full rebuild

A Cargo workspace (`resolver = "3"`, `edition = "2024"`, `rust-version = "1.93"`). This single
document aims to make the workspace skeleton, crate boundaries, IR type hierarchy, the rationale for
the three-layer design, the lossless preservation mechanisms and the data flow reimplementable from
scratch.

---

## 1. The crate dependency graph and responsibilities

The root `Cargo.toml` binds six internal crates with `members = ["crates/*"]` and pins every external
dependency version in `[workspace.dependencies]`. Each crate only inherits, with
`version.workspace = true` and similar.

### 1.1 Dependency direction (an acyclic DAG)

```
                    hwp-model  (base IR; depends only on serde)
                   /    |    |    \        \
                  /     |    |     \        \
             hwp5   hwpx   hwp-convert   hwp-render
               \      | \       |            /
                \     |  \______ | __________/
                 \    |         \|/
                  \   +----------+ (hwpx reuses hwp-convert)
                   \  |          |
                    hwp-cli  (bin: `hwp`, depends on all five)
```

The core invariant: **`hwp-model` never depends on another internal crate.** Every crate depends on
it, so the stability of its API is the stability of the project (stated in its `lib.rs` comment).
There is no cycle. The only non-obvious edge is `hwpx → hwp-convert`, because the hwpx writer reuses
field name and command encoding (CTRL_DATA) and the OWPML type mapping from hwp-convert; since
`hwp-convert` depends only on `hwp-model`, no cycle forms.

### 1.2 Responsibilities and dependencies per crate

| Crate | Artifact | Responsibility (one sentence) | Internal deps | Main external deps |
|---|---|---|---|---|
| **hwp-model** | lib | Defines the shared **semantic IR (L1)** types for HWP and HWPX plus text extraction and unit conversion | none | `serde` (only) |
| **hwp5** | lib | HWP 5.0 binary (CFB + records) ↔ IR **reader/writer** | hwp-model | `cfb`, `flate2`, `thiserror` |
| **hwpx** | lib | HWPX (OWPML, ZIP + XML) ↔ IR **reader/writer** plus `patch` (fidelity-preserving replacement) | hwp-model, hwp-convert | `zip`, `quick-xml`, `thiserror` |
| **hwp-convert** | lib | IR ↔ markdown/JSON/HTML/ODT conversion plus editing primitives (edit, field, bookmark, gso, image, structure) | hwp-model | `serde_json`, `pulldown-cmark`, `zip` |
| **hwp-render** | lib | IR → PNG/SVG/PDF page renderer plus pixel diff | hwp-model | `tiny-skia`, `rustybuzz`, `fontdb`, `image`, `pdf-writer`, `subsetter`, `flate2` |
| **hwp-cli** | bin `hwp` | Subcommand dispatch (info, cat, convert, render, new, edit, fields, ..., mcp, dump) | all five above | `anyhow`, `clap`, `serde_json` |

Minimizing dependencies is a design norm: `hwp-model` pulls in only serde, and the other crates only
what their format or feature strictly needs. `hwp5` and `hwpx` never depend on each other (both go
through the IR). That symmetry is the heart of the hub-and-spoke structure that turns "N formats × M
outputs" into N+M adapters.

### 1.3 Internal module layers of the hwp5 crate

`hwp5` is layered from the bottom up, with **"separating scanning from interpretation"** as the norm
(`record/mod.rs`).

- `container`: MS CFB wrapping, stream enumeration and reading (`Hwp5Container::open`,
  `read_record_stream`, `body_sections`).
- `file_header`: parsing and serializing the fixed 256-byte `FileHeader` (signature, version,
  compression flags).
- `codec`: byte cursors (`ByteReader`/`ByteWriter`) and raw deflate (`compress`/`decompress`).
- `record`: the layer that **interprets no meaning at all**. `header` (the 4-byte header codec),
  `tag` (tag constants and name lookup, always preserving the raw u16), `scan` (flat stream scanning,
  `ScanMode::Tolerant`), `tree` (level-based forest reconstruction, `RecordNode::build_forest`).
- `doc_info` / `body_text`: semantically parse that `RecordNode` tree and promote it to the IR
  (`DocHeader`/`Section`).
- `read` / `write`: the top level, `read_document(path) -> ReadResult` and
  `write_document(doc, path, opts)`.

### 1.4 Internal modules of the hwpx crate

- `package`: the ZIP container (`HwpxPackage`), where `mimetype` = `application/hwp+zip` must be the
  first entry and uncompressed.
- `read/{mod,header,section,xml}`: parses OWPML XML with quick-xml into the IR. **The IR semantics
  are aligned with hwp5**: `hp:secPr`, `hp:ctrl(colPr)` and `hp:tbl` are represented like hwp5, as an
  extended control character (8 WCHAR) plus a `Control`.
- `write/{mod,header,section,templates}`: IR → OWPML. `mimetype` is stored, everything else deflated.
- `patch`: surgically replaces only XML text without re-serializing the package (filling template
  slots `{{name}}` at maximum fidelity).

---

## 2. The full IR type hierarchy (L1, `hwp-model`)

Modules: `document`, `header`, `paragraph`, `control`, `text`, `units`, `ids`, `opaque`. Every IR type
is `Serialize + Deserialize` (JSON round-trippable). For re-serialization stability, optional and
render-only fields use `#[serde(default, skip_serializing_if = ...)]` extensively.

### 2.1 The top level: `Document`

```rust
pub struct Document {
    pub meta: DocMeta,          // origin (format and version)
    pub metadata: Metadata,     // title, author, subject, keywords
    pub header: DocHeader,      // all id reference tables (DocInfo / header.xml)
    pub sections: Vec<Section>, // body sections
    pub bin_streams: Vec<BinStream>, // attached binaries (images and so on)
}
```

- `DocMeta { source_format: String("hwp5"|"hwpx"), source_version: String }` is used by the writer as
  the gate that decides whether to re-synthesize (§5).
- `Metadata { title/author/subject/keywords: Option<String> }` corresponds to hwp5
  `\x05HwpSummaryInformation` and hwpx `Contents/content.hpf` (OPF dc:*). All are Option with a
  default, keeping JSON round-trip compatibility. There is an `is_empty()` helper.
- `BinStream { name: String, #[serde(skip)] data: Vec<u8> }`: the bytes are excluded from default JSON
  serialization (to keep L2 from bloating). The key is the original container entry name (hwp5
  `"BIN0001.png"`, hwpx `"BinData/image1.png"`).
- `Document::resolve_bin(&BinRef) -> Option<&[u8]>`: `BinRef::Id(n)` (1-based) synthesizes the name
  `BIN{id:04X}.{ext}` from the `storage_id` and `extension` of `header.bin_data[n-1]` and matches it;
  `BinRef::ItemRef(s)` matches heuristically by name, suffix or stem.

### 2.2 `Section`

```rust
pub struct Section {
    pub paragraphs: Vec<Paragraph>,
    pub extras: Vec<OpaqueRecord>, // non-paragraph top-level records (empty in healthy files)
}
```

- `Section::section_def() -> Option<&SectionDef>` usually finds the section definition in the first
  control of the first paragraph. Section properties (paper, margins) are not a separate field but
  are expressed as an extended control (`secd`) inside a paragraph, a representation shared by hwp5
  and hwpx.

### 2.3 The paragraph and character model (`paragraph`)

An HWP body is a sequence of **UTF-16 code units (WCHAR)** in which 0 to 31 are control characters.
The single source of truth for position arithmetic is the `char_kind(code)` classification.

**Control character classification (covering all 32 codes, enforced by tests):**

| `CharKind` | WCHAR width | Codes | Meaning |
|---|---|---|---|
| `Char` | 1 | 0, 10, 13, 24-31, ≥32 | character-like (meaningful in itself): line break 10, paragraph end 13, hyphen 24, grouped space 30, fixed-width space 31 |
| `Inline` | 8 | 4-9, 19, 20 | `[code, six WCHAR of information, code]` inline controls: tab 9, field end 4 |
| `Extended` | 8 | 1-3, 11, 12, 14-18, 21-23 | extended controls pointing at a separate CTRL_HEADER record: object/table 11, header/footer 16, footnote 17, bookmark 22 |

The `ctrl_char` module holds constants for the well-known codes (`LINE_BREAK=10`, `PARA_BREAK=13`,
`OBJECT=11`, `HEADER_FOOTER=16`, `FOOTNOTE_ENDNOTE=17` and so on).

```rust
pub enum HwpChar {
    Text(char),                                   // an ordinary character (a surrogate pair is one char)
    CharCtrl(u16),                                // a 1-WCHAR character-like control
    InlineCtrl { code: u16, payload: Vec<u8> },   // 8 WCHAR; payload is the six WCHAR (12 bytes) of information
    ExtCtrl {                                      // an 8-WCHAR extended control
        code: u16,
        ctrl_id: [u8; 4],       // the forward id (for example b"secd"); stored reversed in the stream
        payload: Vec<u8>,       // the original 12 bytes of information (the first 4 are the reversed ctrl_id)
        ctrl_index: Option<u32>,// index into Paragraph::controls (None when matching fails)
    },
}
```

- `HwpChar::wchar_width()` returns `len_utf16()` for `Text` and 1 or 8 for controls. **It is the basis
  of position arithmetic.** Miscounting an extended or inline control throws off every later offset.

```rust
pub struct Paragraph {
    pub para_shape: ParaShapeId,
    pub style: StyleId,
    pub chars: Vec<HwpChar>,
    pub char_shape_runs: Vec<(u32, CharShapeId)>, // (WCHAR start position, character shape) = PARA_CHAR_SHAPE
    pub line_segs: Vec<LineSeg>,                   // PARA_LINE_SEG (the renderer falls back when empty)
    pub controls: Vec<Control>,                    // the entities the extended controls point at
    pub header: ParaHeaderInfo,
    pub extras: Vec<OpaqueRecord>,
}
```

`Paragraph::wchar_len()` is Σ`wchar_width` (used to check against the PARA_HEADER nchars).

**`LineSeg` (one line of PARA_LINE_SEG, 36 bytes), the line layout Hancom saved. The renderer trusts
it as a first-class input:**

| Field | Type | Meaning |
|---|---|---|
| `text_start` | u32 | the line's starting text position (a WCHAR offset within the paragraph) |
| `v_pos` | i32 | the line's vertical position |
| `line_height` | i32 | line height |
| `text_height` | i32 | the height of the text portion |
| `baseline_gap` | i32 | the distance from the line's vertical position to the baseline |
| `line_spacing` | i32 | line spacing |
| `col_start` | i32 | the starting position within the column |
| `seg_width` | i32 | segment width |
| `flags` | u32 | first line of a page or column, empty segment and so on |

**`ParaHeaderInfo`** = `{ chars_flags: u8, ctrl_mask: u32, break_type: u8, instance_id: u32,
tail: Vec<u8> }`, round-trip preserving the top bit of nchars, the break kind and the per-version tail
(merged change tracking and so on).

### 2.4 The control model (`control`)

```rust
pub enum Control {
    SectionDef(SectionDef), // "secd" section definition
    Table(Table),           // "tbl " table
    Picture(Picture),       // "gso " (picture) / hp:pic image
    Generic(GenericControl),// everything else: original preserved plus collected paragraph lists
}
```

As of M1, only tables (`tbl `) and section definitions (`secd`) are semantically parsed; everything
else is preserved as `Generic`. `Control::ctrl_id() -> [u8;4]` returns the forward 4-byte id.

**`SectionDef`** = `{ data: Vec<u8> (the CTRL_HEADER payload, unparsed), page: Option<PageDef>,
extras: Vec<OpaqueRecord> }`.

**`PageDef` (PAGE_DEF, 40 bytes), the paper definition:**

| Field | Meaning |
|---|---|
| `width, height` | paper size (HwpUnit) |
| `margin_{left,right,top,bottom,header,footer}` | six margins |
| `gutter` | binding margin |
| `attr: u32` | bit0 orientation (landscape), bits 1-2 binding method |

**`Table`:**

| Field | Type | Meaning |
|---|---|---|
| `common_data` | Vec<u8> | the original CTRL_HEADER object common properties (populated for hwp5-origin) |
| `placement` | Option<GsoPlacement> | placement information for hwpx-origin (None for hwp5-origin) |
| `attr` | u32 | table attributes |
| `rows, cols` | u16 | row and column counts |
| `cell_spacing` | u16 | cell spacing |
| `inner_margins` | [u16;4] | inner margins, left/right/top/bottom |
| `row_cell_counts` | Vec<u16> | cell count per row (measured: the specification's "Row Size" is a cell count) |
| `border_fill` | BorderFillId | table border and background |
| `table_tail` | Vec<u8> | the rest of the TABLE record |
| `cells` | Vec<Cell> | cell list (LIST_HEADER appearance order, that is row-major) |
| `extras` | Vec<OpaqueRecord> | unparsed children |

**`Cell`** = `{ list_attr: u32, col/row/col_span/row_span: u16, width/height: HwpUnit,
margins: [u16;4], border_fill: BorderFillId, header_tail: Vec<u8>, paragraphs: Vec<Paragraph> }`. A
cell recursively contains `Paragraph` again.

**`Picture`** = `{ common_data: Vec<u8>, width/height: HwpUnit, treat_as_char: bool (as a character
versus floating), z_order: u32, vert_offset/horz_offset: i32, bin_ref: BinRef,
extras: Vec<OpaqueRecord> }`.

**`BinRef`** = `Id(BinDataId)` (hwp5, 1-based) or `ItemRef(String)` (the hwpx manifest
`binaryItemIDRef`).

**`GsoPlacement`** holds placement information read from hwpx `<hp:pos>`, `<hp:sz>`, `<hp:outMargin>`
and zOrder in order to synthesize the hwp5 CTRL_HEADER 40-byte common properties. Fields:
`treat_as_char, affect_line_spacing, flow_with_text, hold_anchor: bool`,
`vert_rel_to/horz_rel_to/vert_align/horz_align: u8`,
`vert_offset/horz_offset/z_order/width/height: i32`, `out_margins: [u16;4]`. The key method
`synth_attr() -> u32` composes the bits:

```
0x082a_0000
 | treat_as_char        // bit0
 | affect_line_spacing<<2
 | (vert_rel_to&3)<<3    // bits 3-4
 | (vert_align&7)<<5     // bits 5-7
 | (horz_rel_to&3)<<8    // bits 8-9
 | (horz_align&7)<<10    // bits 10-12
 | flow_with_text<<13    // bit13
```

The upper 16 bits `0x082a` are an observed constant (widthRelTo and heightRelTo = ABSOLUTE and so on).
Losing this information makes the writer overwrite an inline table as a floating object, dropping it
out of the text flow, so a regression test pins the genuine value (`0x082a2311` and similar).

**`GenericControl` (the lossless container for uninterpreted controls):**

| Field | Meaning |
|---|---|
| `ctrl_id: [u8;4]` | the forward id (b"gso ", b"head" and so on) |
| `data: Vec<u8>` | the CTRL_HEADER payload |
| `paragraph_lists: Vec<ParagraphList>` | paragraph lists per LIST_HEADER, collected recursively **for text extraction only** |
| `extras: Vec<OpaqueRecord>` | unparsed children |
| `raw_children: Vec<OpaqueRecord>` | the original hwp5 CTRL_HEADER subtree (nesting included), **for lossless re-serialization**. When present, emission uses this tree; paragraph_lists and extras are extraction-only (preventing gso and similar nesting from being flattened) |
| `gso_shapes: Vec<ShapeGeom>` | hwpx drawing object geometry and style, **render-only** (populated by the hwpx reader) |
| `equation: Option<Equation>` | equations, **render-only** |

`ParagraphList` = `{ header_data: Vec<u8>, paragraphs: Vec<Paragraph> }`.

**Render-only drawing object types:**

- `ShapeKind` = `Rect | Ellipse | Line | Polygon | Curve | Arc`.
- `ShapeGeom { kind, x/y/w/h: i32 (bounding box in HWPUNIT), points: Vec<(i32,i32)>,
  fill: u32 (COLORREF), fill_gradient: Option<GradientSpec>, border_color: u32, border_width: i32,
  round_ratio: u8, border_style: u8 (0 solid to 5 long dash), arrow_start/arrow_end: u8,
  anchored: bool }`.
- `GradientSpec { radial: bool, angle_deg: f32, stops: Vec<(f32 position 0..1, u32 COLORREF)> }`.
- `Equation { script: String, width/height: i32, inline: bool, x/y: i32 }`, which the renderer
  approximates as a box plus the script text.

### 2.5 The header (reference table) model (`header`)

`LANG_COUNT = 7` (Korean, English, Chinese, Japanese, other, symbol, user).

```rust
pub struct DocHeader {
    pub properties: DocumentProperties,
    pub fonts: [Vec<FaceName>; LANG_COUNT], // fonts per language slot
    pub bin_data: Vec<BinDataItem>,
    pub border_fills: Vec<BorderFill>,      // references are 1-based by convention
    pub char_shapes: Vec<CharShape>,
    pub tab_defs: Vec<RawEntry>,
    pub numberings: Vec<RawEntry>,
    pub bullets: Vec<RawEntry>,
    pub bullet_chars: Vec<char>,            // render-only, parallel to bullets
    pub numbering_levels: Vec<Vec<NumLevel>>,// render-only, parallel to numberings
    pub para_shapes: Vec<ParaShape>,
    pub styles: Vec<Style>,
    pub id_mappings_counts: Vec<u32>,       // the original ID_MAPPINGS count array (per-version length preserved)
    pub id_extras: Vec<OpaqueRecord>,       // unparsed ID_MAPPINGS children
    pub extras: Vec<OpaqueRecord>,          // unparsed DocInfo roots (DOC_DATA, compatibility settings)
}
```

- **`DocumentProperties`** (DOCUMENT_PROPERTIES, 26 bytes) = `{ section_count: u16,
  start_numbers: [u16;6] (page, footnote, endnote, picture, table, equation), caret: (u32,u32,u32) }`.
- **`FaceName`** = `{ attr: u8, name: String, alt_kind: Option<u8>, alt_name: Option<String>,
  panose: Option<[u8;10]> (attr bit6), default_name: Option<String> (attr bit5),
  type_info: Option<String> (for the OWPML round-trip), tail: Vec<u8> }`.
- **`CharShape`**: the character shape, with per-language-slot arrays (`face_ids: [u16;7]`,
  `ratios`/`rel_sizes: [u8;7]`, `spacings`/`offsets: [i8;7]`), `base_size: i32` (10pt = 1000),
  `attr: u32` (effect bits), `strike: bool` (a semantic flag), four colors
  (`text_color`/`underline_color`/`shade_color`/`shadow_color: u32`, COLORREF 0x00BBGGRR),
  `shadow_gap: (i8,i8)`, `border_fill_id: u16` and `tail: Vec<u8>`. Accessors: `is_bold` (bit1),
  `is_italic` (bit0), `underline_kind` (bits 2-3), `has_outline` (8-10), `has_shadow` (11-12),
  `is_emboss` (13), `is_engrave` (14), `is_superscript` (15), `is_subscript` (16),
  `char_offset(lang)`. **A caution on strikethrough:** raw bits 18 to 20 are DIFFSPEC (the
  specification disagrees with reality), so they are not trusted and only the separate `strike` flag
  decides. The HWP5 reader always sets false, and only the HWPX reader sets true for a visible
  `<hp:strikeout>`. `attr` is preserved (with no effect on the byte round-trip).
- **`ParaShape`**: the paragraph shape, with `attr1: u32`, `indent`, `margin_left`/`right`,
  `spacing_top`/`bottom`, `line_spacing_old`, `tab_def_id`/`numbering_id`/`border_fill_id: u16`,
  `border_offsets: [i16;4]`, `line_spacing_type: u8` (0 percent, 1 fixed, 2 margin, 3 minimum),
  `line_spacing: i32` and `tail`. Accessors: `alignment()` (attr1 bits 2-4: 0 justify to 5 divide),
  `head_type()` (bits 23-24: 0 none, 1 outline, 2 number, 3 bullet), `head_level()` (bits 25-27: 1-7).
- **`NumLevel`** = `{ start: u32, fmt: NumFmt, template: String ("^N" is the level-N number slot) }`.
  `NumFmt` = `Digit | HangulSyllable | HangulJamo | CircledDigit | LatinUpper | LatinLower |
  RomanUpper | RomanLower`.
- **`Style`** = `{ name, english_name: String, attr/next_style: u8, lang_id: i16,
  para_shape: ParaShapeId, char_shape: CharShapeId, tail }`.
- **`BinDataItem`** = `{ attr: u16, link_abs/link_rel: Option<String>, storage_id: Option<u16>,
  extension: Option<String>, tail }`. `kind()` is attr & 0xF (0 link, 1 embedding, 2 storage).
- **`BorderLine`** = `{ line_type: u8 (0 none, 1 solid, ...), width: u8 (an index into the thickness
  table), color: u32 }`. `width_mm()` looks up the 16-step mm table (0.1 to 5.0) and `is_visible()`.
- **`BorderFill`** = `{ attr: u16, sides: [BorderLine;4] (left, right, top, bottom),
  diagonal: BorderLine, fill_type: u32, bg_color: Option<u32>, tail }`. `visible_bg()` excludes
  0xFFFFFFFF (none).
- **`RawEntry`** = `{ data: Vec<u8>, children: Vec<OpaqueRecord> }`, the raw preserved form of an id
  table entry before semantic parsing (tab_defs, numberings, bullets).

### 2.6 Id newtypes (`ids`): type-safe indices

`#[serde(transparent)]` newtypes generated by the `id_type!` macro: `CharShapeId(u16)`,
`ParaShapeId(u16)`, `StyleId(u16)`, `BorderFillId(u16)` (**1-based by convention**),
`FaceNameId(u16)`, `BinDataId(u16)`. They prevent mixing up id kinds at compile time.

### 2.7 Units (`units`)

`HwpUnit(pub i32)` is 1/7200 inch (`#[serde(transparent)]`), with `PER_PT=100` and `PER_INCH=7200`.
**1pt is exactly 100 HWPUNIT**, so pt conversion is lossless. Conversions: `to_pt()`, `to_mm()`,
`to_px(dpi)`. Layout arithmetic is done in these integer units.

### 2.8 Text extraction (`text`)

`TextOptions { include_header_footer: bool, include_hidden: bool }`.
`Document::plain_text[_with]` walks sections then paragraphs. The inclusion policy is based on
**the character code of the extended control** (more stable than the ctrl_id): tables and objects (11)
and footnotes (17) are included, while headers and footers (16) and hidden comments (15) are excluded
by default. Tables use tabs between cells and newlines between rows (similar to hwp5txt), and
`Generic` walks its paragraph_lists.

---

## 3. Why the IR has three layers (L0 / L1 / L2)

As stated in `hwp-model/lib.rs`:

- **L0 (per-format lossless representation)**: the representation closest to each format's bytes. For
  hwp5 that is a forest of `RecordNode { tag: u16, data: Vec<u8>, children }` (a level-based tree);
  for hwpx it is XML text and `HwpxPackage` entries. **It interprets no meaning**: the `record` module
  handles only `(tag, level, data)` and always preserves the tag as a raw u16 (no coercion into an
  enum, so a new tag cannot break it).
- **L1 (the semantic IR)**: this `hwp-model` crate, a format-neutral semantic model of
  `Document`, `Section`, `Paragraph`, `Control`, `DocHeader` and so on.
- **L2 (derived representations)**: markdown/JSON/HTML/ODT (`hwp-convert`) and PNG/SVG/PDF
  (`hwp-render`), outputs where loss is acceptable.

**Why three layers:**

1. **HWP 5.0 and HWPX (OWPML) are almost isomorphic semantically.** L1 is therefore not a "lowest
   common denominator" but a faithful transfer of **the HWP semantic model itself**: even the hwpx
   reader normalizes `secPr`, `colPr` and `tbl` into hwp5-style extended controls (8 WCHAR), so
   position arithmetic and text extraction run through **code shared by both formats**. That is the
   heart of the decision to adopt one format's model as canonical instead of abstracting a common
   ancestor.
2. **Without separating L0 from L1, a lossless round-trip is impossible.** Even when parsing (or
   scanning) fails or hits an unsupported record, the data is carried in L0 form (`OpaqueRecord`,
   `RawEntry`, `raw_children`, `tail`) and emitted byte for byte when saving back to the same format.
   "Separating scanning from interpretation" is what makes this possible.
3. **Separating L1 from L2** reduces N formats × M outputs to N+M adapters (hub and spoke). Every
   conversion, edit and render consumes only L1, so a new input format needs one reader and a new
   output needs one converter or renderer.

---

## 4. `OpaqueRecord` and the lossless round-trip design

### 4.1 The core type

```rust
pub struct OpaqueRecord {
    pub tag: u16,
    pub data: Vec<u8>,          // serialized as a hex string (readable snapshots)
    pub children: Vec<OpaqueRecord>, // the whole subtree preserved
}
```

The lossless strategy: **an unknown record is never discarded but carried in raw form, subtree and
all.** Saving to the same format emits it as-is; a cross-format conversion drops it but counts a
warning (`DROP:`), which `--strict` (§5.3) turns into a failure.

### 4.2 The hex_bytes serde module

`OpaqueRecord::data` and every other raw byte field (`Picture::common_data`, `SectionDef::data`,
`*::tail`, `HwpChar::*::payload` and so on) serialize as a **hex string** via
`#[serde(with = "hex_bytes")]`, for readable JSON and insta snapshots. Deserialization validates an
even length and then parses two characters at a time with `u8::from_str_radix(_,16)`. Thanks to this,
the L2 JSON (§5.4) round-trips binaries without loss.

### 4.3 The "known prefix plus tail" rule

The second axis of losslessness is partial parsing. Each record type **structures only the leading
part whose meaning is known, and preserves the version-specific remainder wholesale in
`tail: Vec<u8>`** (`CharShape`, `ParaShape`, `Style`, `FaceName`, `BorderFill`, `BinDataItem` and
`ParaHeaderInfo` all do this). The writer mirrors the parser exactly: "re-emit the prefix, then append
the tail". That is why a simple control document read from hwp5 round-trips **byte-identically** at
the decompressed stream level.

The lossless mechanisms per layer:

| Location | Mechanism | What is preserved |
|---|---|---|
| record header | the `RecordHeader{tag,level,size}` codec | the raw u16 tag and the extended size (the 0xFFF marker) |
| tree structure | `RecordNode::build_forest` | level == depth, so the tree round-trips by recomputing depth |
| unsupported records | `OpaqueRecord` | tag, data (hex) and the children subtree |
| the tail of known records | `*.tail` | version-specific extra fields |
| extended control characters | `HwpChar::ExtCtrl.payload` | the 12 bytes of six WCHAR of information (including the reversed ctrl_id) |
| nested hwp5 objects | `GenericControl::raw_children` | the entire CTRL_HEADER subtree (preventing flattening) |
| ID_MAPPINGS counts | `id_mappings_counts` | the per-version array length (checked against the derived value on write) |
| hwpx fonts | `FaceName::type_info` | the OWPML typeInfo element verbatim |

`RecordNode::build_forest` is corruption-tolerant: when a level jumps without a parent it attaches to
the nearest ancestor and accumulates a warning (`ScanMode::Tolerant`). The ID_MAPPINGS counts are
derived from the table lengths (never synchronized by hand), preserving only the tail when the
original has extra per-version counts.

### 4.4 Separating render-only fields from round-trip fields

So as not to break losslessness, every derived field used only for rendering (`bullet_chars`,
`numbering_levels`, `gso_shapes`, `equation`, `strike`, `NumLevel::template`) is declared
`#[serde(default, skip_serializing_if=...)]`, so it is **never written into the binary** and is
omitted from JSON when empty. For example `CharShape::strike` is only a semantic flag; the original
`attr` bits are preserved and the byte round-trip is unaffected.

---

## 5. Data flow

### 5.1 Format detection (the hub entrance)

`hwp-cli/format.rs` decides by **magic bytes, not extension**: CFB `D0CF11E0A1B11AE1` → `Hwp5`,
ZIP `PK` (504B) → `Hwpx`. `commands::cat::load_document(path)` is the canonical dispatch:

- the extension `.json` → `hwp_convert::from_json` (deserializing L2 JSON into the IR)
- CFB → `hwp5::read_document`
- ZIP → `hwpx::read_document`

All three return a `Document` (L1) and stream warnings to stderr. Every command afterwards consumes
only L1.

### 5.2 read → IR (L0 → L1)

**hwp5 (`read::read_document`):**

1. `Hwp5Container::open` opens the CFB and runs `check_body_readable`.
2. The `/DocInfo` stream is read (raw deflate decompressed), then `scan_stream(_, Tolerant)` produces
   a `RecordNode` forest (L0), then `parse_doc_info` produces the `DocHeader`.
3. Each `/BodyText/SectionN` from `body_sections()` goes through the same scan and then
   `parse_section` into a `Section`.
4. `/BinData/*` streams (decompressed with a try-and-fall-back when the compression flag is set)
   become `Vec<BinStream>`.
5. `\x05HwpSummaryInformation` goes through `parse_summary` into `Metadata` (best effort).
6. The result is `Document{ meta:{source_format:"hwp5", source_version: version}, metadata, header,
   sections, bin_streams }`.

**hwpx (`read::read_document`):**

1. `HwpxPackage::open`.
2. `Contents/header.xml` → `header::parse_header` → `DocHeader`.
3. `section_entries()` (`Contents/sectionN.xml`) → `section::parse_section` → `Section`, updating
   `properties.section_count`.
4. `BinData/*` → `BinStream`.
5. major.minor.micro.buildNumber from `version.xml` becomes source_version, and
   `Contents/content.hpf` (OPF) becomes `Metadata`.

The IR the two readers produce is **semantically aligned** (hwpx also represents secd, colPr and tbl
as extended controls), so no later logic distinguishes the format.

### 5.3 IR → write (L1 → L0 / file)

`commands::convert::run` branches on the output extension (or `--to`; `write_by_ext` is shared):

| Target | Path |
|---|---|
| `.hwp` | `write_hwp[_edited/_structural]` → `hwp5::write_document` |
| `.hwpx` | `hwpx::write::write_document_with(preserve_linesegs)` |
| `.md` | `hwp_convert::to_markdown` |
| `.html` | `hwp_convert::to_html` |
| `.odt` | `hwp_convert::to_odt` |
| `.json` | `hwp_convert::to_json(pretty, embed_bin)` |
| `.pdf` | delegated to the render path → `hwp_render::render_document_pdf` |

**The writer's re-synthesis gate (a core invariant):** `hwp5::write_document` splits handling by
`doc.meta.source_format`:

- **hwp5-origin and unmodified** emits byte-identically through the mirrored "known prefix plus tail".
- **hwpx or markdown origin, or edited (`edited`)** re-establishes the paragraph invariants: the
  paragraph end `0x0d` character, nchars bit31 (the last paragraph), omitting PARA_TEXT for empty
  paragraphs, and emitting line layout. `synthesize_pictures` (synthesizing hwpx and markdown images
  into hwp5 SHAPE_COMPONENT records) and `degrade_hwpx_gso` (re-synthesizing gso_shapes into hwp5 gso
  records) must run, or `strip_unwritable_pictures` drops them.

**Line layout (lineseg) handling, avoiding Hancom's "tampering" warning:** Hancom raises a security
warning when the line layout cache disagrees with the content, so by default the line layout is
**removed** and Hancom recomputes it on open (`preserve_linesegs=false` by default). The branches in
`write_hwp_impl`:

- unmodified hwp5 or `--preserve-layout`: the original line layout as-is.
- hwpx-origin or edited hwp5 (which has line layout): `clear_linesegs` (recursively including nested
  table cells and headers) so Hancom recomputes.
- sources with no line layout, such as markdown: `hwp_render::lineseg::synthesize_linesegs` computes
  it by font shaping and fills the IR (the HCR fonts are required).
- edited but otherwise unmodified hwp5 (`write_hwp_edited`) is **surgical**: only the edited
  paragraphs have their line layout emptied (count=0) so only they are recomputed, while unedited
  paragraphs keep the original (clearing everything inflates empty paragraphs in table cells and
  produces blank space, as measured).

When saving hwp, page 1 is rendered at 48dpi and included as PrvImage. `--strict` exits abnormally
when there is a `DROP:` warning (meaningful only for structure-preserving hwp and hwpx targets).

### 5.4 IR ↔ JSON (L1 ↔ L2, a lossless round-trip)

`hwp_convert::to_json(doc, pretty, embed_bin)`: with `embed_bin=false` it is plain serde (image bytes
excluded, because `BinStream::data` is `#[serde(skip)]`). With `embed_bin=true` the bytes go into
`bin_streams[].data_b64` as base64, producing **self-contained JSON**. `from_json` separates and
decodes `data_b64` to restore the images too. Tests enforce "identical IR apart from images" and
"completely identical when embedded".

### 5.5 IR → render (L1 → pixels/vectors)

The `hwp-render/lib.rs` pipeline: **IR → layout (`layout::layout_document`, LineSegLayouter) →
`display::DisplayList` → a backend (png tiny-skia / svg / pdf)**. All three backends consume the same
DisplayList.

- `build_display_list(doc, opts)`: create the `FontStore`, load `opts.font_dirs`, run
  `layout_document(doc, &mut store, &mut warnings)`, then merge the font resolution report.
- `render_document` → `png::render_png(list, dpi)` → `RenderOutput{ pages: Vec<Pixmap>, report }`.
- `render_document_svg` → `svg::render_svg` → `SvgOutput{ pages: Vec<String>, report }`.
- `render_document_pdf(doc, opts, pages)` selects pages (1-based) then `pdf::render_pdf` (embedding
  fonts and searchable text) → `PdfOutput{ data, report }`.
- `RenderOptions{ dpi: f32 (96 by default), font_dirs }`. With no fonts specified,
  `resolve_font_dirs` loads the HCR fonts from `HWP_FONT_DIR` (or `fonts/`); without them the system
  fonts substitute and fidelity drops sharply.
- `layout` trusts the line layout Hancom saved as a first-class input when `line_segs` is present and
  shapes through the fallback path when it is empty. `diff::compare` measures pixel and offset error
  against a Hancom reference PNG (`hwp diff`).

### 5.6 The whole pipeline in brief

```
file (hwp5/hwpx/json)
   │  detect (magic bytes) → load_document
   ▼
[L0] RecordNode forest / OWPML XML / JSON
   │  parse_doc_info, parse_section / parse_header, parse_section / from_json
   ▼
[L1] Document ──────────────┬───────────────┬──────────────┐
   │ hwp5::write_document   │ hwpx::write   │ hwp_convert   │ hwp_render
   ▼ (the re-synthesis gate) ▼               ▼ to_md/html/    ▼ layout → DisplayList
 .hwp                     .hwpx            odt/json         → png/svg/pdf
```

The editing flow (`hwp edit`) loads L1, transforms it with hwp-convert's editing primitives
(`replace_text`, `set_cell`, `set_field`, `create_bookmark`/`hyperlink`, `insert_image`,
`insert_paragraph`, `add`/`delete_table_row`, `set_char_format`, `set_para_align`, `apply_meta`) and
saves with `write_hwp_edited` (surgical) or `write_hwp_structural` (synthesis forced). Filling an hwpx
template (`hwp fill`) bypasses L1 entirely and replaces only XML text through `hwpx::patch` (maximum
fidelity).

---

## 6. Invariants to keep when rebuilding

1. **`hwp-model` depends on nothing internal beyond serde**, the root of the acyclic DAG.
2. **`char_kind` is the single source of truth for WCHAR width classification**: extended and inline
   are 8, everything else 1. Break this and every position offset breaks.
3. **Record tags are preserved as raw u16** (never coerced into an enum), and the tree round-trips
   through level == depth.
4. **The "known prefix plus tail" symmetry**: reader and writer mirror each other. Anything unknown is
   preserved wholesale as `OpaqueRecord`, `RawEntry` or `raw_children`.
5. **Render-only fields are isolated from the binary and JSON round-trips with
   `skip_serializing_if`**, keeping the lossless invariant.
6. **Line layout is removed when editing or converting across formats so Hancom recomputes it**;
   inconsistency produces a "tampering" warning. Only unmodified hwp5 keeps the original.
7. **The writer's re-synthesis gate is `source_format`**: unmodified hwp5-origin is byte-identical,
   everything else and every edit re-establishes the paragraph invariants and synthesizes object
   records.
8. **BorderFillId is 1-based**, `BinRef::Id` is also 1-based (`bin_data[id-1]`), and `HwpUnit` is
   1/7200 inch with 1pt = 100 units (lossless).
