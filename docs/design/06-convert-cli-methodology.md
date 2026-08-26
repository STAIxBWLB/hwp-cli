[한국어](06-convert-cli-methodology.ko.md) · [English](06-convert-cli-methodology.md)

# Rebuilding the hwp-cli conversion subsystem

This document describes `crates/hwp-convert` (IR editing and conversion logic), `crates/hwp-cli`
(the user interface), and the ground-truth and diagnostic methodology in enough detail to
**reimplement them from scratch**.

---

## 1. Overall architecture and conversion directions

### 1.1 Crate layers and IR-mediated conversion

Every conversion goes through a single shared document model (IR),
`hwp_model::Document{ meta, header, metadata, sections, bin_streams }`, as the **hub**. There is no
direct format-to-format conversion: it is always `read → IR → write`.

| Crate | Role |
|---|---|
| `hwp-model` | The shared IR contract: `Document`, `Paragraph{chars, char_shape_runs, controls, line_segs, header}`, `Control` (Table/Picture/SectionDef/Generic), `HwpChar` (Text/CharCtrl/InlineCtrl/ExtCtrl), `ShapeGeom`, and lossless opaque preservation |
| `hwp5` | HWP 5.0 binary (CFB + records) read/write |
| `hwpx` | HWPX (ZIP + OWPML XML) read/write |
| `hwp-convert` | IR ↔ markdown/JSON, in-memory editing primitives, field and bookmark scanning (the subject of this document) |
| `hwp-render` | IR → PNG/SVG/PDF, line layout (lineseg) synthesis, render diff |

The public API re-exported by `hwp-convert/src/lib.rs`: `replace_text`, `set_cell`, `add_rows`,
`table_dims`, `apply_meta` (edit); `create_field`, `create_hyperlink`, `list_fields`, `set_field`,
`scan_placeholders` (field); `create_bookmark`, `list_bookmarks`, `make_bokm_ctrl_data` (bookmark);
`set_char_format`, `set_para_align` (format); `insert_paragraph`, `delete_paragraph`,
`add_table_row`, `delete_table_row` (structure); `from_markdown`, `default_header`; `to_markdown`,
`to_html`, `to_odt`; `insert_image`; `to_json`, `from_json`.

### 1.2 Conversion matrix

| Conversion | Path | Fidelity |
|---|---|---|
| hwp5 → hwp5 (unmodified) | `write_hwp(preserve_layout)` | **Byte-identical round-trip** (the identity gate) |
| hwp5 → hwpx | reader → IR → hwpx writer | Semantically equivalent (shapes, fields and tables preserved) |
| hwpx → hwp5 | IR → `write_hwp_edited` (synthesis) | Semantically equivalent; gso is safely degraded |
| md → hwp5/hwpx | `from_markdown` → writer (synthesis) | A new document |
| html → hwp5/hwpx | `from_html` → writer (synthesis) | A new document, over the XHTML subset of [18](18-html-fragment-contract.md) |
| json (IR) → hwp/hwpx | `from_json` → writer | Edit round-trip |
| hwp/hwpx → md/html/json/odt | IR → serialization | Lossy, one-way |
| hwp/hwpx → docx | IR → `docx.rs` (OOXML package) | Lossy, one-way. Output only; there is no docx input |
| hwp/hwpx → csv/txt | IR → `csv.rs` or text extraction | Tables or plain text only |
| hwp/hwpx → pdf/png/svg | delegated to the render path | Render output |

**The key branch**: if `doc.meta.source_format` is `"hwp5"` and nothing was edited, the original line
layout is preserved for a byte-identical round-trip. Everything else (markdown or hwpx origin, or
edited) goes through the **synthesis path** (`edited=true`), which clears the line layout so Hancom
recomputes it.

### 1.3 CLI subcommands (`crates/hwp-cli/src/cli.rs`, clap derive)

The command surface is **generated documentation, not prose**.
`crates/hwp-cli/tests/cli_reference.rs` renders
[manual/cli-reference](../manual/cli-reference.md) and its Korean pair directly from the clap tree
and fails the build on drift, so the authoritative list of subcommands and flags lives there and is
deliberately not duplicated here. The same test gates the Korean help overlay in
`crates/hwp-cli/src/i18n.rs` against missing and stale entries.

What belongs in this document is the part a generated reference cannot state.
`detect(path)` in `format.rs` decides by **magic bytes, not extension**: CFB
(`D0 CF 11 E0 A1 B1 1A E1`) → Hwp5, ZIP (`50 4B`) → Hwpx. A `.json` input is treated as a serialized
IR (`load_document`, `cat.rs`).

---

## 2. gso.rs: SHAPE_COMPONENT → ShapeGeom

`hwp-convert/src/gso.rs` converts hwp5's shape subtree (an `OpaqueRecord` tree) into a structured
list of `ShapeGeom`. The hwpx writer consumes it to preserve decorative shapes and text-box borders
when converting hwp → hwpx. **The byte layout is measured identically to the render parser in
`hwp-render/src/shape_draw.rs`**; a reverse dependency is not allowed, so it is kept as a copy.
Fix an offset in one and you must look at both.

### 2.1 Record tag constants and tree traversal

```
SHAPE_COMPONENT = 0x4C   SC_LINE = 0x4E   SC_RECTANGLE = 0x4F
SC_ELLIPSE = 0x50   SC_ARC = 0x51   SC_POLYGON = 0x52
SC_CURVE = 0x53   SC_CONTAINER = 0x56   MAX_DEPTH = 8
```

The entry point `shapes_from_raw(raw: &[OpaqueRecord]) -> Vec<ShapeGeom>` calls `walk`
(SHAPE_COMPONENT goes to `component`, CONTAINER recurses), then `component` (per child shape record
it calls `geometry`, recursing into nested SHAPE_COMPONENT/CONTAINER). Coordinates are HWPUNIT after
the render matrix is applied, relative to the gso box origin.

### 2.2 The 3×2 affine matrix `Mat`

```
struct Mat { a,b,c,d,e,f: f64 }   // x' = a·x + b·y + c,  y' = d·x + e·y + f
apply(x,y) = (a·x+b·y+c, d·x+e·y+f)
mul(o)     = standard 3×2 composition (self ∘ o)
```

Reading helpers: `rd_u16`, `rd_i32`, `rd_u32` (LE), `rd_f64` (8B LE), and `rd_mat(d,o)` which reads
six f64 values at `o, o+8, ..., o+40`, that is **48 bytes**, to build a Mat.

### 2.3 `parse_style`: the T·S·R matrices, borders and fill

The SHAPE_COMPONENT data layout and parsing rules:

| Step | Offset | Content |
|---|---|---|
| CHID | determines `base` | `d[0..4]==d[4..8]` → base=8 (top level, CHID×2), otherwise base=4 (member CHID×1) |
| Properties | from `base` | Common shape properties |
| translation | `base+44` | Mat `t` (48B) |
| scale/rotation count | `base+42` | `cnt = rd_u16` |
| scale and rotation pairs | `pair = base+44+48+(cnt-1)*96` | `m = t · (rd_mat(pair) · rd_mat(pair+48))`, using only the last pair |
| Border line | `bo = base+92+cnt*96` | color (u32) at bo, width (i32) at bo+4, lattr (u32) at bo+8 |
| Fill | `fo = bo+13` | ft (u32) at fo |

- Border: `lt = lattr & 0x3F`. When `lt != 0`, `border_color=color`,
  `border_width=width.max(1)`, `border_style=hwp5_line_style(lt)`. When `lt==0` there is no border
  (color 0xFFFFFFFF, width 0).
- Fill: `ft & 0x1` gives solid `fill = rd_u32(fo+4)`; `ft & 0x4` gives `parse_gradient(fo+4)`;
  bit1 (image fill) is **excluded in v1** because it needs a bin reference.
- If the data is shorter than `pair+96`, `m = t` (no scale or rotation).

`hwp5_line_style(lt)`: `2→1` (DASH), `3→2` (DOT), `4→3` (DASH_DOT), `5→4` (DASH_DOT_DOT), `6→5`
(LONG_DASH), otherwise 0 (SOLID).

### 2.4 `parse_gradient` (spec table 28)

From `fo`: `type(i16) angle(i16) cx cy spread num(i16 at fo+10)`. Only `num` in 2..=16 is allowed.
When `num>2`, an `INT32[num]` position array follows (normalized by the maximum and clamped to 0..1)
and then `COLORREF[num]`; when `num==2` the stops are evenly spaced. `radial = (gtype==1)`. Stops are
sorted by position and returned as `GradientSpec{radial, angle_deg, stops}`.

### 2.5 `geometry`: applying the matrix to local points

`p(o)` is `(rd_i32(o), rd_i32(o+4))`. Local points per tag:

| Tag | Layout | ShapeKind | round_ratio |
|---|---|---|---|
| SC_LINE | p(0) start, p(8) end (None when zero length) | Line | 0 |
| SC_RECTANGLE | `d[0]` curvature % (min 100) plus four corners p(1), p(9), p(17), p(25) | Rect | curvature |
| SC_POLYGON | n = rd_u16(0) (2..=4096), points p(4+i·8) | Polygon | 0 |
| SC_ELLIPSE | attr (u32) + center p(4) + ax1 end p(12) + ax2 end p(20) → bbox approximation | Ellipse | 0 |
| SC_CURVE | n = rd_u16(0), points p(2+i·8) (segment types ignored, approximated as a polyline) | Curve | 0 |
| SC_ARC | `d[0]` kind + center p(1) + ax1 end p(9) + ax2 end p(17), **three points preserved** | Arc | 0 |

Ellipse bbox: `rx = |a1−c|`, `ry = |a2−c|`, giving `(c.x−rx, c.y−ry)..(c.x+rx, c.y+ry)`.

Every local point goes through `s.m.apply()` before the bbox (minx/miny/maxx/maxy) is computed.
`points` are normalized **relative to the bbox origin** (x−minx, y−miny) for Line, Polygon, Curve and
Arc only; Rect and Ellipse round-trip through sz. The result is
`ShapeGeom{ kind, x=minx, y=miny, w, h, points, fill, fill_gradient, border_color, border_width,
round_ratio, border_style, arrow_start=0, arrow_end=0, anchored=false }`. Placement (anchored) is
decided by the 40B gso header and emitted by the writer as pos.

### 2.6 Arc isotropization (solving the key trap)

The matrix (rotation plus non-uniform scale) makes the center/ax1/ax2 axes **non-perpendicular
(sheared)**. Hancom's OWPML `<hp:arc>` accepts **only two perpendicular axes** and renders a
non-perpendicular pair as a pinwheel. The fix:

```
v1 = ax1 − c,  v2 = ax2 − c
r  = (|v1| + |v2|) / 2
a1 = atan2(v1),  a2 = atan2(v2)
d  = normalize(a2 − a1) → [−π, π]          // v1→v2 sweep, the short way
bis = a1 + d/2,  q = sign(d) · π/4          // ±45° around the bisector
ax1' = c + r·(cos(bis−q), sin(bis−q))
ax2' = c + r·(cos(bis+q), sin(bis+q))
```

The two axes are spread 90° apart around their bisector, **approximating a circular quarter arc**
(rotation and direction preserved; only slight ellipticity is lost). The bbox and points are
recomputed after isotropization.

### 2.7 Consumption by the hwpx writer (`hwpx/src/write/section.rs`)

`write_gso` calls `shapes_from_raw`. When there is text (a text box) it emits one rect plus the first
shape's style plus drawText; otherwise it emits `write_shape_element` per shape. **To avoid z
collisions in grouped shapes (donuts and the like)** it makes z unique as
`zorder * Z_SCALE(64) + index`, preserving relative order. `curSz` is (0,0) only for Ellipse and Arc
(the "not pre-sized" marker) and (w,h) otherwise. `fillBrush` is emitted **only when there is a
fill**, because emitting no-fill (0xFFFFFFFF) as opaque white covers what is behind it. The reader
(`collect_shape`) round-trips an arc as the three points center/ax1/ax2.

---

## 3. Fields, bookmarks, formatting and structural editing

The shared rule for every editing primitive: **change only the IR and let the writer re-establish the
invariants.** An edited paragraph clears `line_segs` (removing the stale line layout) and sets
`header.ctrl_mask=0` (the writer recomputes it from chars). hwp5 output must go through the synthesis
path (`WriteOptions.edited=true`) for Hancom to accept it.

### 3.1 Fields (`field.rs`): %hlk hyperlinks and FIELD_START/END

An HWP field is `FIELD_START` (ExtCtrl, **code 3**, ctrl_id), then the display text, then `FIELD_END`
(InlineCtrl, **code 4**). Control ids: `%clk` (click-here), `%fmu` (formula), `%hlk` (hyperlink),
plus `%mmg`, `%dte`, `%ddt`, `%xrf`, `%bmk`, `%pat`, `%smr`, `%usr`, `%unk`. `is_field_ctrl_id`
recognizes that set. `owpml_field_type` and `field_ctrl_id_from_owpml` map both ways to the OWPML
types (CLICK_HERE, FORMULA, HYPERLINK and so on).

**Reading**: `list_fields` walks the body, table cells and text boxes recursively. For each
FIELD_START it finds the control by `ctrl_index` and collects `field_meta` (name and command) and
`field_value` (the text between START and FIELD_END).

**Byte layouts**:

- Field command data: `attr(4) etc(1) len(2, WCHAR count) WCHAR[len] id(4) trailing(4)`, parsed back
  by `parse_command`.
- The CTRL_DATA (click-here name) Parameter Set: `setid(2) count(2 i16)` followed by items.
  `first_bstr` extracts the first BSTR; each item is `id(2) type(2)`, and PIT_BSTR (1) is
  `UINT32 len + WCHAR[len]`. Type sizes: 0 (null) = 0, 2/6 = 1, 3/7 = 2, 4/5/8/9 = 4; anything
  unknown aborts safely.

**Creation (measured from genuine files, mandatory)**:

- `rev_payload(ctrl_id)`: 12B whose first 4B are the reversed ctrl_id (the reader parses it reversed).
- `field_end_payload`: 12B whose first **3B are the reversed ctrl_id without `%`**, with p[3]=0. For
  example `%hlk` END is `6b 6c 68 00`. **If the END payload is all zeros, Hancom cannot pair START
  with END, the field stays unfinished and clicking does nothing** (confirmed on the fourth round of
  ground truth).
- `make_field_command_data`: `%hlk` = (attr `0x0000a800`, etc 0), `%fmu` = (0, `0x08`), others =
  (0,0). **The id must be non-zero**: with id=0 Hancom treats %hlk as plain text. `field_instance_id`
  assigns a deterministic non-zero id from the FNV-1a 32-bit hash of the command (mapping 0 to 1).
- `hlk_command(url)`: backslash-escape only `\ ; :`, then append `;1;0;0;` (for example
  `http\://...;1;0;0;`).
- The hyperlink display text needs a **blue (0x00FF0000) underlined** character shape obtained
  through `hyperlink_char_shape`; without it Hancom neither recognizes nor displays a link (confirmed
  in Hancom). `apply_run_style` applies it to the display range `[iw+8, iw+8+display width)` and
  restores the original shape afterwards.

`create_field` and `create_hyperlink` locate the anchor with `find_match`, insert the control and
field chars, fix runs with `adjust_runs`, and relink ExtCtrl to controls in appearance order with
`relink_ctrl_index`.

### 3.2 Bookmarks (`bookmark.rs`)

A bookmark is not a field but a `bokm` control, so `list_fields` does not see it. The character is
`ExtCtrl{ code: 22 (BOOKMARK), ctrl_id: b"bokm" }`, **a single point marker** with no START/END pair,
and the control is `Generic{ ctrl_id: bokm, data: [], raw_children: [CTRL_DATA(name)] }`.

The CTRL_DATA layout (which must be **byte-identical** to genuine files):
`setid=0x021b(2) count=1(2) id=0x40000000(4) type=1 BSTR(2) len(2) WCHAR[len]`. The constant prefix
in `make_bokm_ctrl_data` is `[0x1b,0x02, 0x01,0x00, 0x00,0x00,0x00,0x40, 0x01,0x00]` followed by len
and WCHAR. `decode_bokm_name` reads the name from offset 12. It is compared with `assert_eq!` against
the 24B of "책갈피테스트" in the genuine `bookmark.hwp`.

### 3.3 Formatting (`format.rs`)

`CharFormat{ bold/italic/underline/strike: Option<bool>, size_pt, color }`. `set_char_format` calls
`restyle_range` for each matching range, using each run's existing shape as the base and toggling
only the requested bits (preserving partial formatting). `apply_format` bits: bold `1<<1`, italic
`1<<0`, underline bits 2 to 3 (`1<<2` is under the character), strike bits 18 to 20 (`1<<18`) plus the
strike flag, `base_size = pt×100` (min 100), and text_color (COLORREF 0x00BBGGRR).

`set_para_align` writes the alignment into bits 2 to 4 of the para_shape `attr1` (0 justify, 1 left,
2 right, 3 center, 4 distribute, 5 divide). `find_or_insert` and `find_or_insert_para` append to the
header tables without duplicates; appending is safe because **the writer derives the ID_MAPPINGS
count from `.len()`**. `normalize_runs` re-establishes the run invariants (sorted, duplicates at the
same position removed, adjacent identical ids merged, first run at pos 0).

### 3.4 Structural editing (`structure.rs`)

- `insert_paragraph(anchor, text, before)` inherits the anchor paragraph's para_shape, style and
  first char_shape. `make_paragraph` appends `PARA_BREAK(0x0d)` to chars and sets
  `char_shape_runs=[(0,cs)]`.
- `delete_paragraph` **preserves SectionDef paragraphs and keeps at least one paragraph per section**
  (preventing an empty section).
- `add_table_row` clones the last row's cell structure (with content cleared), increments `rows` and
  pushes to `row_cell_counts`.
- `delete_table_row` uses `cells.retain`, renumbers rows above, decrements `rows` and removes from
  `row_cell_counts`. The last row cannot be deleted.

### 3.5 In-memory editing primitives (`edit.rs`)

- `replace_text(from,to,all)` matches **only within a contiguous run of Text characters** (it breaks
  at control boundaries, which preserves formatting and structure). When `to` contains `from` (for
  example "한라대학교" → "제주한라대학교") the inserted text would match again, causing an **infinite
  loop**; `start = char_idx + to_chars` prevents it.
- `find_match` concatenates contiguous Text segments in `chars`, searches for `from`, and returns
  `(chars index, WCHAR offset)`.
- `adjust_runs(runs, p, lo, ln)` moves run boundaries for a replacement at p with old length lo and
  new length ln (boundaries inside the range are removed and later boundaries shift by
  `delta = ln − lo`).
- `set_cell` uses the cell's first paragraph as a formatting template and replaces only the content.
- `add_rows` **checks the u16 range** (refusing rather than corrupting by truncation) and clones
  `clean_template_row` (the last row that fills every column and has no merges). **A cloned paragraph
  gets a unique non-zero `instance_id` (max+1 within the table) and the nchars bit31 marker
  (`chars_flags |= 0x80`, the last-paragraph marker)**: the hwp5 edit path does not reassign these in
  the writer, so failing to set them here breaks object links.
- `blank_para_like` preserves the template's para_shape, style, first char_shape and header, and sets
  `chars_flags |= 0x80`.

### 3.6 Image insertion (`image.rs`)

`insert_image(anchor, path, size)` reads the pixel size with `image_pixel_size` (PNG IHDR at 16, GIF
LSD at 6, BMP at 18, JPEG SOF markers, all dependency-free header parsing) and computes the display
size with `display_size` (Natural scales down proportionally when it exceeds the body width, using
`px·7200/96` HWPUNIT; Mm uses `mm·283.46457`). After the anchor it inserts
`ExtCtrl{ code:11, ctrl_id: b"gso " }` plus `Control::Picture{ bin_ref: ItemRef(name), extras: [] }`
and adds a `BinStream`. **extras is empty** because the writer synthesizes the hwp5 shape records
(SHAPE_COMPONENT plus picture).

---

## 4. markdown → hwpx (`from_markdown.rs`)

`from_markdown(md)` feeds `pulldown_cmark` events (ENABLE_TABLES) into a `Builder` that constructs
the IR: headings become "개요 N" styles, bold and italic become character shapes, GFM tables become
Table, list items get a "• " prefix, and line breaks become CharCtrl(10).

### 4.1 `default_header`: the minimum matching an empty Hancom document

- **Fonts**: "함초롬바탕" in every LANG slot (`attr=0x01` TTF, `default_name="HCR Batang"`).
  `emit_face_name` automatically ORs the 0x20 bit (the default name used for substitution).
- **Ten char_shapes**: 0 body, 1 bold, 2 italic, 3 bold+italic, 4 to 9 H1 to H6 (ratios 1.8, 1.5,
  1.3, 1.2, 1.1, 1.1). **`shade_color=0xFFFFFFFF` (none)**: the default of 0 makes Hancom draw an
  opaque black shade behind every character (the "black bars" from the 14th round of testing).
  `shadow_gap=(10,10)`, `shadow_color=0x00C0C0C0`.
- **para_shapes**: `attr1=0x180` (bit7 Korean line breaking plus bit8 line grid),
  `line_spacing_old=160`, `border_fill_id=2`, all measured from good samples. Index 0 default,
  1 heading (left plus spacing), 2 body (bottom spacing).
- **tab_defs**: Hancom's three default automatic left/center/right tabs (8B each: attribute u32 0/1/2
  plus count 0 plus reserved). **Leaving it empty creates a dangling reference and is judged as
  corruption.**
- **border_fills**: 1 and 2 are borderless, 3 is a 0.12mm solid line.

### 4.2 `inject_section_controls`: injecting section and column definitions

Before the first paragraph, insert a `SectionDef` (PageDef A4: width 59528, height 84186, margins and
so on) and a `cold` (column definition) control. Existing references shift: `ctrl_index += 2`,
`char_shape_runs pos += 16`, `line_segs.text_start += 16`. The `secd` and `cold` ExtCtrl (code 2,
reversed ctrl_id payload) are inserted at the front of chars. **`header.break_type = 0x03`** (bit0
section break plus bit1 column break) is the value Hancom always uses on a section's first paragraph;
0 breaks header-control consistency and reads as corruption. (The hwp5 round-trip path does not go
through this function, so the byte-identity gate is unaffected.)

### 4.3 `table_paragraph`: GFM tables

A `tbl ` ExtCtrl (code 11) anchor plus `Control::Table`. Cell margins `[510,510,141,141]`,
border_fill 3, col_w = BODY_WIDTH(42520)/cols. **Even an empty cell needs one paragraph and one
char_shape run** (nparas ≥ 1): a cell with nparas=0 is corruption to Hancom and crashes pyhwp too.
Missing cells in short rows are filled with empty paragraphs.

The full markdown → hwpx flow: the `new` and `convert` commands build the IR with `from_markdown` and
emit through `hwpx::write_document` (the synthesis path, `preserve_linesegs:false`). The reverse,
IR → markdown, is `to_markdown` in `markdown.rs` (outline style → heading, char_shape run → `**`/`*`,
table → GFM).

---

## 5. Write path branching (`commands/convert.rs`)

`write_hwp_impl(doc, output, preserve_layout, edited)` has three branches:

1. **`!synthesize || preserve_layout`** (unmodified hwp5): keep the original line layout, giving byte
   identity.
2. **`has_source_linesegs`** (hwpx origin or edited hwp5): `clear_linesegs` removes line layout from
   every paragraph (recursing into table cells and headers) so Hancom recomputes from paragraph and
   character shapes.
3. **Sources with no line layout** (markdown): synthesize by font shaping with
   `hwp_render::lineseg::synthesize_linesegs`, which **requires the HCR fonts** (`HWP_FONT_DIR` or
   `fonts/`).

`write_hwp_edited` preserves the original line layout when the source is hwp5 (with count=0 only for
edited paragraphs) and synthesizes otherwise. `write_hwp_structural` **forces synthesis for every
source** so that the invariants of inserted paragraphs and rows (0x0d, the last-paragraph bit, counts)
are applied. `--strict` makes `bail_on_strict` count `DROP:`-prefixed warnings and exit abnormally.

The `fill` command has two paths: (1) **byte-preserving replacement** of hwpx `{{name}}`
(`hwpx::patch::fill_placeholders`, preserving the preview and BinData), and (2) IR table filling
(`add_rows` plus `set_cell`) when `--data` contains a `tables` array of objects. If "tables" is an
array of strings it routes to the plain fill (to avoid misinterpretation, `has_tables` accepts **only
an array of objects**).

---

## 6. The ground-truth methodology

### 6.1 The corpus must never be committed

`.gitignore` excludes `/corpus` (an external HWP_CORPUS_DIR), `/fonts/`, `/fixtures/golden/*.png`,
`/fixtures/hwp5/` and `/fixtures/hwpx/` (test documents), and `/docs/*.pdf|*.hwp|*.png|spec.txt`
(Hancom copyrighted material). **Only the recipe (README) is committed.** `fixtures/hwp5/*.hwp` are
fetched from hahnlee/hwp-rs (Apache-2.0) and placed manually; when absent the related tests skip
automatically (`skip_if_no_fixtures` decides by the presence of `fixtures/hwpx/minimal.hwpx`).

### 6.2 Comparing against genuine bytes

Ground truth is **a pair of the original .hwp and the hwpx exported by Hancom**. Our generated bytes
must be **exactly the same** as genuine Hancom output for Hancom to accept them. Measured genuine hex
is embedded as constants in the code and compared by unit tests:

- `gso.rs`: `LINE_SC` (a decorative-line SHAPE_COMPONENT of 252B, CHID `$lin`×2, scale 496.08/0.04,
  border width 32) plus `LINE_GEOM` (SC_LINE from (0,0) to (100,100)) correspond exactly to Hancom's
  exported `<hp:line>` with curSz 49608×4 and width=32.
- `bookmark.rs`: `FIXTURE_CTRL_DATA` 24B, `assert_eq!(make_bokm_ctrl_data("책갈피테스트"), ...)`.
- `field.rs`: `make_field_command_data(b"%hlk", ...)` with attr `0x0000a800`, len 37 (matching the
  genuine work_report %hlk), id ≠ 0, and END payload `6b 6c 68`.

Tag values are written as **literals (0x4c, 0x4e)** in tests so that a wrong constant is caught.

### 6.3 Separating synthesis from round-trip

Two test grades are kept distinct:

- **Byte-identical round-trip** (the identity gate): unmodified hwp5 read then write. Every fixture
  must be byte-identical to the original. See `hwp5/tests/identity.rs` and `roundtrip.rs`.
- **Semantic equivalence** (synthesis): markdown or hwpx origin and edited documents are verified only
  for text and structure preservation (layout recomputation is allowed). The round-trip flagship test
  in `cli.rs` checks identical `cat` text, no `DROP` warning, and field preservation.

Flagship tests in `cli.rs`: `변환_글상자_텍스트_필드_보존` (the work_report text box "나눔글꼴" and
%hlk "설치하기" survive), `변환_장식_도형_보존` (annual_report shape drops ≤ 8 and at least 20 distinct
zOrder values), and `변환_완전_왕복_hwp_hwpx_hwp` (text and fields preserved both ways).

---

## 7. Diagnostic techniques

### 7.1 Injecting our elements into a ground-truth document

`tools/gen_verification_set.sh` generates 11 files under `~/Documents/hwp-verification/` for the user
to **open directly in Hancom Office** and judge acceptance. The method: generate base.hwp from base.md
(which contains the anchors "제목" and "여기에"), then **inject our elements** with
`edit --create-bookmark` and `--create-hyperlink`, and emit both hwp and hwpx. Each file passes its own
re-read gate first (`check`: `cat` prints content without warnings), so that **no broken file is
handed to the user**. The A series covers the full pipeline on real documents; the B series is minimal
per-feature files for isolating a cause on failure.

### 7.2 Placement diagnosis

Hancom draws overlapping shapes by z-order. We used to flatten everything to `zOrder="0"`, giving an
undefined order and a blank cover page. The diagnosis: the `변환_장식_도형_보존` test counts the set of
distinct `zOrder="` values in section0.xml and requires **at least 20** (preventing a regression back
to all zeros). Grouped shapes (donuts) are scaled by `Z_SCALE=64` and offset by index to stay unique.
`textWrap=IN_FRONT_OF_TEXT` (measured on genuine files); TOP_AND_BOTTOM fails to place many shapes and
yields a blank page.

### 7.3 Three-way comparison

Three viewpoints are crossed: (1) **our reader** (cat text, fields JSON), (2) **our renderer** (PNG,
PDF), and (3) **Hancom's export** (reference PNG, hwpx). `hwp diff <in> --ref <Hancom PNG> --dpi <n>`
compares our render to the Hancom reference by pixels and profile: `ink_ratio` (ink coverage, that is
completeness), `dx/dy` (position offset), `bad_pixel_pct` (pixel difference, that is glyphs and
antialiasing), and MAE. Fonts must be pinned to the same set as Hancom (`HWP_FONT_DIR`) for the same
line breaking. The `golden` test (`HWP_GOLDEN=1`) compares `*.ref.png` automatically. The `validate`
command has an exit-code contract (mimetype, required entries, XML parsing; `valid:false` exits 1).

---

## 8. Practical traps (confirmed by testing in Hancom)

| Symptom | Cause | Fix |
|---|---|---|
| **Black bars** immediately on open | char_shape `shade_color=0`, an opaque black shade in Hancom | `shade_color=0xFFFFFFFF` (the "none" marker) |
| Hyperlink **does not follow on click** | FIELD_END payload all zeros, so START and END never pair | END payload's first 3B are the reversed ctrl_id without `%` |
| %hlk **treated as plain text** (only blue) | field command id = 0 | Non-zero id from the FNV-1a hash, attr `0x0000a800` |
| Hyperlink **not recognized** | No character shape on the display text | Apply a blue (0x00FF0000) underlined char_shape |
| Table cell **corruption dialog** | A cell with nparas=0 (an empty `\| \|`) | One paragraph plus one char_shape run even in empty cells |
| Inserted field **broken** | ctrl_index does not match the appearance order of controls | Call `relink_ctrl_index` |
| **Tampering verdict** after editing | Stale line layout inconsistent with the content | `line_segs.clear()` on edited paragraphs plus the synthesis path |
| **Broken object links** after cloning a row | Duplicate instance_id in the cloned paragraph | Assign a unique max+1 within the table |
| Table record **broken** | Row count truncated to u16, desynchronizing cells and counts | Refuse when the remaining capacity is exceeded |
| Replacement **infinite loop** | `to` contains `from`, so the insert matches again | `start = char_idx + to_chars` |
| Arc renders as a **pinwheel** | The matrix made the two axes non-perpendicular | Isotropize to ±45° around the bisector |
| Guide shapes become **opaque white discs** | No-fill (0xFFFFFFFF) emitted as a white fillBrush | Emit fillBrush only when there is a fill |
| Donuts and grouped shapes **not rendered** | z collision between multiple shapes in one gso | Make z unique as `z*Z_SCALE+index` |
| Dangling reference corruption | Empty tab_defs | Three default tabs |
| Markdown paragraph corruption | break_type=0 on a section's first paragraph | `break_type=0x03` |

**Safe gso degradation for hwpx → hwp**: testing in Hancom judged re-synthesized round-trip gso as
corrupt, so a text box preserves only its text in the body (the shape wrapper is omitted). The shape
itself does not survive the round-trip, but text and fields do and the file is valid. With `--strict`,
dropping a decorative shape in that direction exits abnormally.

---

## 9. Rebuild checklist

1. Fix the `hwp_model` IR contract (Document, Paragraph, HwpChar, Control, ShapeGeom).
2. `format.rs::detect` (magic bytes) then `load_document` (hwp5/hwpx/json).
3. `gso.rs`: tag constants → `walk`/`component`/`geometry` → `parse_style` (the T·S·R matrices and
   the offset table) → arc isotropization → the genuine LINE_SC 252B test.
4. `field.rs` and `bookmark.rs`: FIELD_START(3)/END(4)/BOOKMARK(22), rev_payload and end_payload,
   the command and CTRL_DATA bytes, and comparison tests against genuine files.
5. `format.rs`, `structure.rs`, `edit.rs`: run bits, alignment, table rows, replacement,
   adjust_runs, instance_id and nchars bit31.
6. `from_markdown.rs`: default_header (shade_color, attr1, tab_defs), inject_section_controls
   (break_type 0x03) and table_paragraph (nparas ≥ 1).
7. Write branching (`write_hwp_impl`, three paths) with synthesis and unmodified kept separate.
8. The CLI (clap) plus the `cli.rs` integration tests (exit codes, flagship round-trips), and the
   `cli_reference.rs` drift gate that generates the manual and pins the Korean help overlay.
9. The ground-truth gates (gitignore, genuine hex constants, identity versus synthesis) and the
   diagnostics (injection, zOrder, the three-way diff).
