[한국어](04-hwpx-owpml.ko.md) · [English](04-hwpx-owpml.md)

## The HWPX/OWPML read and write subsystem (crates/hwpx)

hwp-cli's HWPX layer is a three-stage converter between **the OPC (ZIP) container, OWPML XML and the
IR (`hwp_model`)**. The IR is designed to carry **exactly the same meaning** as hwp5 (binary HWP), so
text extraction, position arithmetic and rendering take the same path for both formats. This document
covers `crates/hwpx/src/read/section.rs` and `crates/hwpx/src/write/section.rs`, describing the
conventions, byte layouts and invariants in enough detail to reimplement them from scratch.

Key files:

- `crates/hwpx/src/package.rs`: ZIP/OPC container access and mimetype validation
- `crates/hwpx/src/read/{mod,section,header,xml}.rs`: HWPX → IR
- `crates/hwpx/src/write/{mod,section,header,templates}.rs`: IR → HWPX
- `crates/hwp-model/src/{control,paragraph}.rs`: the IR type definitions

---

## 1. The HWPX ZIP (OPC) container structure

HWPX is a ZIP archive following the OPC (Open Packaging Conventions). The entries, their order and
the compression method directly affect compatibility with Hancom (HWP).

### 1.1 The entry list and write order

The order produced by `write/mod.rs::write_document_with` (leftmost first):

| # | Path | Compression | Role | Source |
|---|------|------|------|------|
| 1 | `mimetype` | **Stored (uncompressed)** | The container type magic. Must be **the first entry and uncompressed** | `templates::MIMETYPE` = `application/hwp+zip` |
| 2 | `version.xml` | Deflate | Format version (`hv:HCFVersion`) | `templates::VERSION_XML` |
| 3 | `META-INF/container.rdf` | Deflate | RDF package relationships | `templates::CONTAINER_RDF` |
| 4 | `META-INF/container.xml` | Deflate | OCF rootfiles (the entry point) | `templates::CONTAINER_XML` |
| 5 | `META-INF/manifest.xml` | Deflate | ODF manifest (an empty shell) | `templates::MANIFEST_XML` |
| 6 | `Contents/content.hpf` | Deflate | The OPF package: manifest, spine and metadata | `templates::content_hpf()` |
| 7 | `Contents/header.xml` | Deflate | Font, character shape, paragraph shape, border fill and style tables | `write/header.rs` |
| 8.. | `Contents/section0.xml`, `section1.xml`, ... | Deflate | The body (paragraphs, tables, shapes) | `write/section.rs` |
| .. | `BinData/image1.png`, ... | Deflate | Embedded images | `BinCollector` |
| .. | `Preview/PrvText.txt` | Deflate | Preview text (the first ~1000 characters) | `doc.plain_text()` |
| .. | `settings.xml` | Deflate | Application settings such as the caret position | `templates::SETTINGS_XML` |

**Invariants (never violate when reimplementing):**

- `mimetype` must be the **first local header** in the ZIP and use `CompressionMethod::Stored`. The
  OPC rule exists so the uncompressed magic sits at the front of the file. Violating it makes Hancom
  judge the file corrupt.
- In `version.xml`, `major="5" minor="1" micro="1" buildNumber="0" xmlVersion="1.5"` are **fixed
  constants** (format compatibility). Only `application` and `appVersion` carry the authoring program
  (hwp-cli, `CARGO_PKG_VERSION`).

### 1.2 The read path (`package.rs`, `read/mod.rs`)

`HwpxPackage::open(path)`:

1. Opens it with `zip::ZipArchive`.
2. Reads the `mimetype` entry and verifies it is `application/hwp+zip`, otherwise
   `HwpxError::BadMimetype`.

`read_document(path)`:

1. `Contents/header.xml` → `header::parse_header` (font and shape tables → `DocHeader`).
2. `section_entries()` sorts `Contents/section*.xml` **numerically** by the numeric suffix
   (`section0 < section1 < ... < section10`) and runs `section::parse_section` on each.
3. Every entry starting with `BinData/` becomes a `BinStream { name, data }`.
4. `major.minor.micro.buildNumber` from `version.xml` becomes `DocMeta::source_version`.
5. `Contents/content.hpf` (OPF) goes through `parse_content_meta` to extract title, creator, subject
   and keywords (best effort; absence is not fatal).

The `section_entries` sort key:

```
n.trim_start_matches("Contents/section").trim_end_matches(".xml").parse::<u32>()
```

Entries that fail to parse sort last as `u32::MAX`.

### 1.3 The content.hpf (OPF) structure

`templates::content_hpf(section_count, bin_items, meta)` synthesizes it. Inside `<opf:package>`:

- `<opf:metadata>`: `<opf:title>`, `<opf:language>ko`, `<dc:creator>`, `<dc:subject>`,
  `<opf:meta name="keywords" content="..."/>`, plus the application marker
  `<opf:meta name="creator" content="text">hwp-cli</opf:meta>`.
- `<opf:manifest>`: `header`, `section{i}`, `settings` and the binary items (`isEmbeded="1"`).
- `<opf:spine>`: `header` and `section{i}` as `<opf:itemref linear="yes"/>`.

`BinCollector` fills the binary items' id, href and mime. `read/mod.rs::parse_content_meta` reads only
the `title`, `creator` and `subject` local names of this file plus `meta[name=keywords]` (the
application marker with `name="creator"` has the local name `meta` and is ignored).

---

## 2. Namespaces and the main elements

### 2.1 Namespaces

| Prefix | URI | Purpose |
|--------|-----|------|
| `hs` | `http://www.hancom.co.kr/hwpml/2011/section` | The section root `hs:sec` |
| `hp` | `http://www.hancom.co.kr/hwpml/2011/paragraph` | Paragraphs, runs, controls, tables and shapes (`p`, `run`, `t`, `ctrl`, `tbl`, `rect`, `pic`, `pos`, `sz`, `drawText`, ...) |
| `hc` | `http://www.hancom.co.kr/hwpml/2011/core` | Core geometry and style (`img`, `fillBrush`, `winBrush`, `gradation`, `color`, `pt0...`, `center`, `ax1`, `transMatrix`, ...) |
| `hh` | `.../2011/head` | The header.xml root |
| `ha` | `.../2011/app` | settings.xml |
| `hpf`, `dc`, `opf` | (OPF/DC) | content.hpf metadata |

The section root as emitted on write:

```xml
<hs:sec xmlns:hs=".../section" xmlns:hp=".../paragraph" xmlns:hc=".../core">...</hs:sec>
```

**Parser convention:** `read/xml.rs::attr` and every match use `local_name()` with the prefix
stripped, so `hp:p` and `p` both match the local name `p`, independent of the namespace prefix.

### 2.2 Element to IR mapping (parse_paragraph)

`parse_paragraph` consumes one `hp:p` and converts its children into the IR:

| OWPML element | IR representation | Extended/inline code | Notes |
|-----------|---------|-----------------|------|
| `hp:p` | `Paragraph` | - | `paraPrIDRef` → `para_shape`, `styleIDRef` → `style`, `pageBreak="1"` → `break_type\|=0x04`, `columnBreak="1"` → `break_type\|=0x08` |
| `hp:run` | `char_shape_runs.push((wchar_pos, id))` | - | `charPrIDRef`. At the same position the later run overwrites (handling an empty `<hp:t/>`) |
| `hp:t` | a sequence of `HwpChar::Text(c)` | - | `parse_text`; `wchar_pos += c.len_utf16()`; `GeneralRef` (`&amp;` and so on) resolved |
| `hp:tab` | `HwpChar::InlineCtrl{code:9, payload:[0;12]}` | Inline 9 | `wchar_pos += 8` |
| `hp:lineBreak` | `HwpChar::CharCtrl(10)` | Char 10 | `wchar_pos += 1` |
| `hp:secPr` | `ExtCtrl(2,"secd")` plus `Control::SectionDef` | Ext 2 | `parse_sec_pr` |
| `hp:ctrl` | (dispatched per child, §5) | - | `parse_ctrl` |
| `hp:tbl` | `ExtCtrl(11,"tbl ")` plus `Control::Table` | Ext 11 | `parse_table` |
| `hp:equation` | `ExtCtrl(11,"eqed")` plus `Generic{equation}` | Ext 11 | `parse_equation` |
| `hp:pic` | `ExtCtrl(11,"gso ")` plus `Control::Picture` | Ext 11 | `zOrder` is an attribute of the **start tag**, not of the child pos |
| `hp:rect/ellipse/line/polygon/curve/arc` | `ExtCtrl(11,ctrl_id)` plus `Generic{gso_shapes}` | Ext 11 | `collect_shape` |
| `hp:linesegarray` | `para.line_segs` | - | `parse_linesegs` |
| other objects | `ExtCtrl(11,ctrl_id)` plus `Generic{paragraph_lists}` | Ext 11 | `collect_sub_lists` (text box text) |

### 2.3 Inserting extended control characters (`push_ext_ctrl`)

Extended objects (secd, tbl, gso and so on) appear inside the paragraph string as an **8-WCHAR
extended control character**. `push_ext_ctrl(para, wchar_pos, code, ctrl_id)`:

- pushes `HwpChar::ExtCtrl { code, ctrl_id, payload, ctrl_index }` onto `para.chars`.
- `payload` (12 bytes): the first four are the **reversed ctrl_id** (the same as the hwp5 storage
  format), the rest zero.
- `ctrl_index = Some(para.controls.len())`, pointing at the `Control` pushed next.
- `wchar_pos += 8`.

`HwpChar::wchar_width()`: `Text` is `len_utf16` (1, or 2 for a surrogate), `CharCtrl` is 1, and
`InlineCtrl`/`ExtCtrl` are 8. **It is the single basis of position arithmetic**: miscounting an
8-WCHAR control misaligns every later `char_shape_runs` and `line_segs`.

The control character classification (`hwp_model::paragraph::char_kind`, hwp5 §4.2.4):

- **Char (1 WCHAR):** 0, 10, 13, 24-31
- **Inline (8 WCHAR):** 4-9, 19, 20
- **Extended (8 WCHAR):** 1-3, 11, 12, 14-18, 21-23

---

## 3. Shape geometry (collect_shape ↔ write_shape_element)

### 3.1 Reading: `collect_shape`

`shape_kind(name)` maps the element name to a `ShapeKind`: `rect` → Rect, `ellipse` → Ellipse,
`line` → Line, `polygon` → Polygon, `curve` → Curve, `arc` → Arc.

`collect_shape` consumes the subtree and fills a `ShapeGeom`:

| Child element | Attributes read | ShapeGeom field |
|-----------|-----------|----------------|
| `hp:pos` | `horzOffset` → x, `vertOffset` → y, `treatAsChar="1"` → anchored | x, y, anchored |
| `hp:sz` | `width` → w, `height` → h | w, h |
| `hp:lineShape` | `color` → border_color (parse_color), `width` → border_width, `style` → border_style, `headStyle` → arrow_start, `tailStyle` → arrow_end | borders |
| `hc:winBrush` | `faceColor` → fill (parse_color) | fill |
| `hc:gradation` | `parse_gradation` | fill_gradient |
| `hc:pt0...ptN` | `x`, `y`, only for Polygon and Curve | points |
| `hc:center`/`ax1`/`ax2` | `x`, `y` only for Arc (in appearance order) | points (three) |
| `hp:subList` | recursive paragraphs | paragraph_lists (text inside the shape) |

**A key invariant: pt is treated differently per shape kind.**

- **The `pt0~3` of Rect, Ellipse and Arc are ignored.** They are the four bbox corners (the genuine
  format), and since the size round-trips through `hp:sz`, reading pt again would attach phantom
  points to the shape.
- **Only Polygon and Curve take `pt*` as geometric points.**
- **An Arc carries `center`, `ax1` and `ax2` (three points)** as `points`, the center plus two
  conjugate axes relative to the bbox. The renderer draws the arc from those three points.
- Rounded rectangle corners: the start tag's `ratio` (0 to 100%) becomes `round_ratio`.

Emission condition: `w != 0 || h != 0 || !points.is_empty()` (OR, because a horizontal or vertical
line can have one axis at zero).

### 3.2 Color conversion convention

`read/xml.rs::parse_color("#RRGGBB")` gives the COLORREF `0x00BBGGRR` (R and B swapped). `"none"` or a
failure gives `0xFFFF_FFFF`. In reverse, `write/section.rs::color_hex(c)` gives `"#RRGGBB"`, and
`templates::color_attr` maps `0xFFFF_FFFF` to `"none"`.

### 3.3 Writing: `write_shape_element`

Both hwpx-origin (`write_ir_shapes`) and hwp5-origin (`write_gso`) shapes go through this function.
The emission order follows what genuine files measure:

1. **The opening tag** `<hp:{el} id zOrder numberingType="PICTURE" textWrap="IN_FRONT_OF_TEXT"
   textFlow="BOTH_SIDES" lock dropcapstyle href groupLevel instid>` plus per-kind attributes:
   - Rect: `ratio="{round_ratio}"`
   - Ellipse: `intervalDirty="0" hasArcPr="0" arcType="NORMAL"`
   - Arc: `type="NORMAL"`
2. **`write_obj_scaffold`**: `hp:offset(0,0)`, `hp:orgSz(w,h)`, `hp:curSz(cur_w,cur_h)`, `hp:flip`,
   `hp:rotationInfo(centerX=w/2,centerY=h/2)` and `hp:renderingInfo` (identity trans, sca and
   rotMatrix).
   - **The curSz rule:** Ellipse and Arc use `(0,0)` (the "not pre-sized" marker); everything else
     uses `(w,h)`.
3. **`hp:lineShape`**: with `border_width<=0` it is `style="NONE" width="0"`, otherwise color, width,
   style (`line_style_name`), headStyle and tailStyle (`arrow_name`).
4. **`hc:fillBrush`**, emitted **only when there is a fill**:
   - `fill_gradient` Some → `hc:gradation type angle colorNum` plus `hc:color` children.
   - `fill != 0xFFFF_FFFF` → `hc:winBrush faceColor`.
   - **No fill (`0xFFFF_FFFF`) omits fillBrush entirely**: emitting it as opaque white makes a
     transparent guide shape cover the content behind it (the cause of the donut and ring diagram
     rendering bug).
5. **`hp:shadow type="NONE"`**: a required element per genuine measurement.
6. **`hp:drawText`** when there is text, see §3.5.
7. **Geometry points** (after drawText, in the genuine order):

| Kind | Emitted elements |
|------|-----------|
| Line | `hc:startPt`, `hc:endPt` (or `(0,0)` to `(w,h)` with no points) |
| Polygon/Curve | `hc:pt0 ... hc:ptN` (walking points) |
| Rect | `hc:pt0(0,0) pt1(w,0) pt2(w,h) pt3(0,h)` (the four bbox corners) |
| Ellipse | `hc:center(w/2,h/2) ax1(w,h/2) ax2(w/2,0) start1/end1/start2/end2(0,0)` |
| Arc | `hc:center/ax1/ax2` (using the three points, or the bbox approximation `center(0,0) ax1(0,h) ax2(w,0)`) |

8. **`hp:sz width height widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE"`** plus pos_xml plus
   **`hp:outMargin`**, then the closing tag.

**Invariant:** without the pt or center elements of a Rect or Ellipse, Hancom does not know the shape
outline and does not render it (a blank page). So even though the reader discards pt (§3.1), the
writer must re-synthesize it. That is the heart of the round-trip contract (§6).

### 3.4 Gradients (`parse_gradation` ↔ emission)

Reading: `type` (LINEAR gives linear; anything else, RADIAL, CIRCLE and so on, is approximated as
radial), `angle`, and the `hc:color value` children as evenly spaced stops. Fewer than two colors
gives `None`.
Writing: `hc:gradation type="{LINEAR|RADIAL}" angle colorNum` plus one `hc:color value` per stop.

### 3.5 Text inside a shape (`drawText`)

`write_draw_text` emits `<hp:drawText lastWidth="{width}" name="" editable="0"><hp:subList
vertAlign="CENTER">paragraphs</hp:subList><hp:textMargin left/right/top/bottom="283"/></hp:drawText>`.
Every `paragraph_lists` entry is merged into one subList (a v1 approximation of multi-column text
boxes). Shape text paragraphs **always emit a linesegarray** regardless of `preserve_linesegs`
(measured on genuine files: Hancom always stores line layout for text box paragraphs).

---

## 4. Floating and inline placement (`hp:pos`)

`hp:pos` decides whether an object flows like a character (inline) or floats, along with its reference
and offsets.

### 4.1 Attribute to code mapping

| Attribute | Value → code | Read function |
|------|-----------|-----------|
| `treatAsChar` | `"1"` → inline (anchored) | (direct) |
| `vertRelTo` | PAPER=0, PAGE=1, PARA=2 | `vert_rel_to_code` |
| `horzRelTo` | PAPER=0, PAGE=1, COLUMN=2, PARA=3 | `horz_rel_to_code` |
| `vertAlign`/`horzAlign` | TOP/LEFT=0, CENTER=1, BOTTOM/RIGHT=2 | `align_code` |
| `vertOffset`/`horzOffset` | i32 (HWPUNIT) | `attr_offset_i32` |
| `affectLSpacing`, `flowWithText`, `holdAnchorAndSO` | `"1"` → bool | (direct) |

**The negative offset rule:** hwpx stores negatives as **unsigned two's complement decimal** (for
example `-77` becomes `"4294967219"`). `attr_offset_i32` parses as `i64` then reinterprets `as i32`;
parsing directly as `i32` would fail on the range, so this is required.

### 4.2 GsoPlacement ↔ the hwp5 common property attr

An hwpx table or shape reads `<hp:pos>`, `<hp:sz>`, `<hp:outMargin>` and `zOrder` into a
`GsoPlacement` and synthesizes the hwp5 CTRL_HEADER common property `attr(u32)` (`synth_attr`).
Without reading them, the writer overwrites with the floating constant and an inline table drops out
of the text flow.

The `GsoPlacement::synth_attr` bit layout (the upper 16 bits are the observed constant `0x082a`):

| Bit | Field |
|------|------|
| bit0 | `treat_as_char` |
| bit2 | `affect_line_spacing` |
| bits 3-4 | `vert_rel_to` |
| bits 5-7 | `vert_align` |
| bits 8-9 | `horz_rel_to` |
| bits 10-12 | `horz_align` |
| bit13 | `flow_with_text` |

A measured genuine example: an inline table with `treatAsChar=1, vertRelTo=PARA(2),
horzRelTo=PARA(3), flowWithText=1` gives `0x082a_2311`.

### 4.3 Reconstructing pos for an hwp5-origin gso (`gso_pos_xml`)

`parse_gso_header(data)` (20B or more): `attr@0(u32)`, `voff@4`, `hoff@8`, `w@12`, `h@16`,
`zorder@20` (0 when the length is under 24). `gso_pos_xml(attr, voff, hoff)` extracts the bits back:

- `treat=attr&1`, `vrel=(attr>>3)&3`, `valign=(attr>>5)&7`, `hrel=(attr>>8)&3`,
  `halign=(attr>>10)&7`.
- **Floating (treat=0)** gives `flowWithText=0 allowOverlap=1`, and **inline (treat=1)** gives
  `flowWithText=1 allowOverlap=0` (measured on genuine files: a floating object with flow=1 makes
  Hancom fail to place many shapes, giving a blank page).

### 4.4 pos for hwpx-origin shapes (`write_ir_shapes`)

`ShapeGeom` has no relTo, so it is approximated: when `anchored`, the inline convention
`(treat=1, vertRelTo=PARA, horzRelTo=COLUMN)`; otherwise absolute coordinates
`(treat=0, PAPER, PAPER)`. Offsets come from `s.y` and `s.x`. With no z-order available, it is assigned
increasing by shape index (`i`).

---

## 5. Controls under hp:ctrl (parse_ctrl ↔ the writer arms)

A control inside `hp:ctrl` maps to an hwp5 ctrl_id plus a control character code, and its payload is
synthesized here. The writer drops a GenericControl with an empty payload.

| OWPML | ctrl_id | Code | Payload | Builder / inverse |
|-------|---------|------|----------|---------|
| `colPr` | `cold` | 2 | none | - |
| `header`/`footer` | `head`/`foot` | 16 | 8B: `apply(u32)` + `id(u32)` | `head_foot_data` |
| `footNote`/`endNote` | `fn  `/`en  ` | 17 | none | - |
| `autoNum` | `atno` | 18 | 12B: `0,4,0` (u32 ×3) | `build_atno` |
| `pageNum` | `pgnp` | 21 | 12B: `props(u32)` + 6B of 0 + `sideChar(u16)` | `build_pgnp` ↔ `page_num_pos_name` |
| `pageHiding` | `pghd` | 21 | a 4B bitmap | `build_pghd` |
| `newNum` | `nwno` | 21 | 6B: `0(u32)` + `num(u16)` | `build_nwno` |
| `fieldBegin` | (type → id) | 3 (Ext) | CTRL_DATA (0x0057) | §5.2 |
| `fieldEnd` | - | 4 (Inline) | the matching start's reversed 3B ctrl_id | §5.2 |
| `bookmark` | `bokm` | 22 (Ext) | the name in CTRL_DATA (0x0057) | the `bookmark` module |

### 5.1 Payload byte layouts (confirmed by measurement)

- **head_foot_data (8B):** `apply(u32 LE)` + `id(u32 LE)`. apply: BOTH=0, EVEN=1, ODD=2. Measured,
  `<hp:header id="2" applyPageType="BOTH">` gives `00000000 02000000`.
- **build_pgnp (12B):** `props(u32) = format | (position<<8)` + `6B of 0` + `sideChar(u16)`.
  Positions: NONE=0, TOP_LEFT=1 ... BOTTOM_RIGHT=6, OUTSIDE_TOP=7, OUTSIDE_BOTTOM=8, INSIDE_TOP=9,
  INSIDE_BOTTOM=10. Only DIGIT=0 is mapped for format. Measured, `pos=BOTTOM_CENTER, sideChar='-'`
  gives `00 05 00 00 00 00 00 00 00 00 2d 00`.
- **build_pghd (4B):** a bitmap of `bit0 hideHeader, 1 hideFooter, 2 hideMasterPage, 3 hideBorder,
  4 hideFill, 5 hidePageNum`. Measured: a cover page is `0x21` and a table of contents `0x20`.
- **build_atno (12B):** `0, 4, 0` (u32 ×3, the measured standard).
- **build_nwno (6B):** `0(u32, kind = PAGE)` + `num(u16)`. Measured, `num=1` gives `00000000 0100`.

### 5.2 The field round-trip (fieldBegin/fieldEnd)

Reading (`parse_ctrl`):

- `fieldBegin`: `type` → ctrl_id (`field_ctrl_id_from_owpml`), `name` → the name CTRL_DATA (record tag
  `0x0057`, `make_field_ctrl_data`), and the child `hp:parameters > stringParam[name=Command]` →
  `make_field_command_data` (including a non-zero id, which Hancom needs to recognize `%hlk` as a
  hyperlink). The result is `ExtCtrl(3, ctrl_id)`.
- `fieldEnd`: `matching_field_start` scans `para.chars` back to front LIFO (handling nested fields) to
  find the matching FIELD_START (code 3) ctrl_id, and `field_end_payload` builds the reversed 3B
  payload (without `%`). The result is `InlineCtrl(4)`. If it were all zeros, Hancom could not pair
  the field and the hyperlink would not respond to clicks.

Writing (`write_paragraph`):

- When a Generic has a field ctrl_id, it emits `<hp:fieldBegin id type name editable dirty zorder
  fieldid metaTag>` plus, when a Command exists, `<hp:parameters><hp:stringParam name="Command">...`.
  The id is stored in `current_field_id`.
- On an `InlineCtrl(4)` it closes with `<hp:fieldEnd beginIDRef="{fid}" fieldid="{fid}"/>`.

---

## 6. The table and cell round-trip

### 6.1 Tables (`parse_table` ↔ `write_table`)

Attributes read: `pageBreak` (NONE=0, TABLE=1, CELL=2 → `attr` bits 0-1), `repeatHeader="1"` → `attr`
bit2, `noAdjust="1"` → `attr` bit3, plus `rowCnt`, `colCnt`, `cellSpacing` and `borderFillIDRef`.
Children: `hp:tc` → cells, `hp:inMargin` → `inner_margins[left,right,top,bottom]`, and `hp:pos`,
`hp:sz` and `hp:outMargin` → `GsoPlacement`. After the loop, `row_cell_counts` is reconstructed from
the cells' rows.

**Invariant:** leaving `attr`'s pageBreak at 0 means "do not split", so a table that does not fit in
the remaining space is pushed wholesale to the next page (the table-of-contents box separation bug).

Writing: `col_w` and `row_h` are estimated as the maximum cell width and height, giving `total_w` and
`total_h`. Then `hp:tbl` (with the fixed attributes `pageBreak="CELL" repeatHeader="1"`) plus `hp:sz`
plus `hp:pos` (inline `treatAsChar="1" vertRelTo="PARA"`) plus `hp:inMargin`. Cells are grouped by row
(`BTreeMap<u16,Vec<&Cell>>`) into `hp:tr` > `hp:tc`.

### 6.2 Cells (`parse_cell`)

`hp:tc`: `header="1"` → `list_attr` bit18 (a header cell, repeated on each page) and
`borderFillIDRef`. Children: `cellAddr` (colAddr/rowAddr → col/row), `cellSpan` (colSpan/rowSpan),
`cellSz` (width/height), `cellMargin` (left/right/top/bottom → margins), `subList vertAlign`
(TOP=0, CENTER=1, BOTTOM=2 → `list_attr` bits 5-6) and `p` → paragraphs.

**Invariant:** not reading subList vertAlign leaves it 0 (TOP), so cell content bunches at the top,
and when the cell height exceeds the content the empty area below splits onto the next page (a blank
page). Genuine cells use CENTER (`0x20`).

---

## 7. The write_paragraph run state machine and the round-trip contract

### 7.1 The run state machine

`write_paragraph` opens `<hp:p id paraPrIDRef styleIDRef pageBreak columnBreak merged="0">` and walks
`para.chars`, switching `<hp:run charPrIDRef>` at **character shape boundaries** (the `open_run!`
macro flushes text and closes then reopens the run when the shape changes). `shape_id_at(para,
wchar_pos)` gives the effective char_shape at a position.

Character handling:

- `Text(c)` goes into `text_buf`, flushed as `<hp:t xml:space="preserve">`.
- `CharCtrl`: 10 → `<hp:lineBreak/>`, 24 → `'-'`, 30 → NBSP, 31 → a space.
- `InlineCtrl`: 9 → `<hp:tab/>`, 4 → fieldEnd.
- `ExtCtrl`: looks up `para.controls` by `ctrl_index` and emits per kind (SectionDef, cold,
  head/foot, Table, Picture, fields, bokm, pgnp/pghd/nwno/atno, gso_shapes, gso; anything else warns
  `DROP`).

When the first paragraph has no SectionDef, `inject_secpr` injects `write_default_sec_pr` (A4 by
default) plus `write_col_ctrl`. An empty paragraph is guaranteed one
`<hp:run charPrIDRef><hp:t/></hp:run>`.

### 7.2 Splitting shape runs (SHAPE_RUN_LIMIT)

Hancom **renders only about the first 21 shapes in a run and discards the rest** (confirmed in
Hancom). `SHAPE_RUN_LIMIT=12`, and the `shape_break!` macro opens a new run with the same char_shape
when run_shapes hits the limit before emitting a shape. `count_shape_tags` counts `<hp:rect `,
`<hp:ellipse `, `<hp:line ` (with the trailing space to distinguish `lineShape`, `lineseg` and
`lineBreak`), `arc`, `polygon`, `curve`, `pic` and `connectLine ` in the emitted XML.

### 7.3 Making z-order unique (Z_SCALE)

When a grouped shape (multiple shapes in one gso, for example a donut as grey plus a white hole)
shares the gso z-order, the collision makes Hancom draw only one. `Z_SCALE=64` assigns
`zorder*Z_SCALE + i`, preserving relative order while making each unique.

### 7.4 Preserving the linesegarray

Body paragraphs emit `<hp:linesegarray>` only when `preserve_linesegs` (false by default) is set.
Line layout inconsistent with the content makes Hancom raise a "tampering" security warning, so it is
removed by default and Hancom recomputes it. **Only an unmodified round-trip sets it true.** Text
inside a shape is always preserved, as in §3.5.

### 7.5 The round-trip contract in brief

| Item | What the reader discards | What the writer re-synthesizes | Rationale |
|------|-------------------|----------------------|------|
| Rect/Ellipse/Arc pt | pt0~3 (the bbox corners) | pt0~3, center and axes recomputed from sz | duplicated pt creates phantom points |
| Shape size | - | `hp:sz` (w,h) | the single source of size |
| curSz | - | Ellipse and Arc (0,0), otherwise (w,h) | measured on genuine files |
| No fill (0xFFFFFFFF) | - | **omits** fillBrush | keeps it transparent |
| linesegarray (body) | kept, but | not emitted by default | avoids the tampering warning |
| Field pairing | - | beginIDRef and fieldEnd linked LIFO | makes hyperlinks work |
| gso common attr | the individual `<hp:pos>` attributes | the attr(u32) bits composed | preserves inline versus floating |
| z-order | absent from hwpx ShapeGeom | the order index, or gso*Z_SCALE+i | overlap order |

**Unsupported controls** are dropped with a `DROP:` warning (some text boxes, gso that failed to
parse and so on). Warnings propagate as a `Vec<String>` up to `read_document` and `write_document`.

---

## 8. Reimplementation checklist (summary)

1. **Container:** mimetype (stored, first entry) → version.xml → META-INF/* → content.hpf →
   header.xml → section*.xml → BinData/* → Preview → settings. On read, validate mimetype and sort
   sections numerically.
2. **Position arithmetic:** every object is inserted into the paragraph as an 8-WCHAR extended control
   character (`push_ext_ctrl`), counted exactly by `wchar_width`. char_shape_runs and line_segs align
   to that coordinate system.
3. **Shapes:** collect_shape reads pos, sz, lineShape, winBrush and gradation, plus pt for Polygon and
   Curve and center/axes for Arc. The pt of Rect, Ellipse and Arc is ignored but re-synthesized by the
   writer. fillBrush is emitted only when there is a fill, and curSz is (0,0) for Ellipse and Arc.
4. **Placement:** treatAsChar, vertRelTo, horzRelTo, vertAlign, horzAlign and offsets map to the
   GsoPlacement bits (the 0x082a constant plus the low bits). Negative offsets are parsed as u32 two's
   complement.
5. **Control payloads:** head_foot (8B), pgnp (12B), pghd (4B), atno (12B) and nwno (6B) in exactly
   that LE layout. Fields use CTRL_DATA (0x0057) with LIFO pairing.
6. **Warning propagation:** collect drops and unparsed items into `warnings` so losslessness can be
   diagnosed.
