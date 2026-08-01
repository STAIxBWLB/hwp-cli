[한국어](11-hwpx-structure-map.md) · [English](11-hwpx-structure-map.en.md)

# The HWPX structure map (exhaustive element catalog plus a read/write symmetry audit)

This document lets you **look up every part, namespace and XML element** an HWPX (OWPML) package can
contain, and audits element by element **how our code reads and writes it**. It serves two purposes:
1. an index so a reimplementer can find "where and how is this element handled in the code" at once,
and 2. an **audit table exposing the symmetry gaps** between what read interprets, what read discards
and what write emits, that is the list of places where the lossless round-trip breaks.

## Division of roles with documents 04 and 12

- [04-hwpx-owpml](04-hwpx-owpml.en.md) is the **subsystem design**: why we parse this way, the shape
  geometry conversion rules, the placement bit layout, the control payload byte layouts and the
  *rationale* for the round-trip contract. It explains "how it works" as a narrative.
- 11 (this document) is the **exhaustive catalog plus the symmetry audit**. It lists every element and
  tabulates the read/write handling status. Where 04 explains a rule through a representative case,
  11 makes it queryable *for every element*.
- [12-feature-gaps](12-feature-gaps.en.md) takes the "unimplemented" and "information loss" rows of
  table J in §5 as input and covers **the feature gap priorities and restoration plan**.

## The measurement principle (copyright notice)

OWPML is the KS X 6101 standard, so listing element and attribute names is unproblematic. Even so,
this document does not reproduce Hancom's schema documentation; it records **only the elements our
code actually handles**, as measured, meaning only what appears in the match arms and emitted strings
of `crates/hwpx/src/**`. OWPML elements the code never touches (for example the internals of
`hp:chart`) are mentioned by name without detail. The citation convention follows
[docs/README](../README.en.md).

---

## 1. The OPC package tree (table E)

HWPX is an OPC (Open Packaging Conventions) ZIP archive. Below is **the measured tree of a genuine
sample saved by Hancom, unzipped** (with file sizes in bytes), included here as a self-contained
snapshot rather than depending on any working directory.

```
genuine sample (unzipped) tree
├── mimetype                     19 B   application/hwp+zip  (STORED, first entry)
├── version.xml                 310 B   hv:HCFVersion
├── settings.xml                279 B   ha:HWPApplicationSetting (caret position)
├── META-INF/
│   ├── container.xml           475 B   ocf:container rootfiles (entry point)
│   ├── container.rdf           867 B   rdf:RDF package relationships
│   └── manifest.xml            134 B   odf:manifest (an empty shell)
├── Contents/
│   ├── content.hpf           1,860 B   opf:package manifest, spine and metadata
│   ├── header.xml           42,625 B   fonts, character and paragraph shapes, border fills, styles
│   └── section0.xml          3,340 B   body (paragraphs, tables, shapes)
├── Preview/
│   ├── PrvText.txt               2 B   preview text (the first ~1000 characters)
│   └── PrvImage.png          4,485 B   preview thumbnail (PNG)
└── BinData/                    (absent from this sample; present only with embedded images)
    └── imageN.{png,jpg,gif,bmp}       the original binaries referenced by objects
```

The read/write path for each part:

| Part | Role | Read path (file:line) | Write path (file:line / source) | Round-trip status |
|------|------|--------------------|----------------------------|-----------|
| `mimetype` | container magic (first entry, STORED) | `package.rs:34` verification only | `write/mod.rs:72` the `MIMETYPE` constant | constant |
| `version.xml` | format version | `version_info` semantic parsing plus **original pass-through** (`Document.hwpx_version_xml`) | re-emits the original (falling back to the `VERSION_XML` constant) | ✅ original preserved (2026-07-15, GE-β5) |
| `settings.xml` | application settings (caret) | **original pass-through** (`Document.hwpx_settings_xml`) | re-emits the original (falling back to the `SETTINGS_XML` constant) | ✅ original preserved (2026-07-15, GE-β5) |
| `META-INF/container.xml` | OCF rootfiles (entry point) | **none** | `write/mod.rs:87` the `CONTAINER_XML` constant | constant |
| `META-INF/container.rdf` | RDF package relationships | **none** | `write/mod.rs:83` the `CONTAINER_RDF` constant | constant (describes only header and section0) |
| `META-INF/manifest.xml` | ODF manifest (an empty shell) | **none** | `write/mod.rs:92` the `MANIFEST_XML` constant | constant |
| `Contents/content.hpf` | OPF package and document metadata | `read/mod.rs:90` `parse_content_meta` (title, creator, subject, keywords) | `templates.rs:28` `content_hpf()` | only the four metadata fields round-trip; spine and manifest are re-synthesized |
| `Contents/header.xml` | font, shape and style tables | `read/header.rs:99` `parse_header` | `write/header.rs:44` `write_header` | see §3.2 and §4 |
| `Contents/section{i}.xml` | body (paragraphs, tables, shapes) | `read/section.rs:52` `parse_section` | `write/section.rs:50` `write_section` | see §3.1 and §4 |
| `BinData/imageN.*` | embedded image originals | `read/mod.rs:47` → `BinStream` | `write/mod.rs:110` ← `BinCollector` (`section.rs:24`) | bytes preserved (deduplicated) |
| `Preview/PrvText.txt` | preview text | **none** | `write/mod.rs:113`, the first 1000 characters of `doc.plain_text()` | regenerated from the body |
| `Preview/PrvImage.png` | preview thumbnail | **none** | **none (lost on the IR path)** | preserved only by `patch.rs` through a raw copy |

**Write entry order** (`write/mod.rs:72-114`, leftmost first): `mimetype` (STORED) → `version.xml` →
`META-INF/container.rdf` → `container.xml` → `manifest.xml` → `Contents/content.hpf` → `header.xml` →
`section{0..}.xml` → `BinData/*` → `Preview/PrvText.txt` → `settings.xml`. `mimetype` must be the
first local header and uncompressed (an OPC rule; violating it makes Hancom judge the file corrupt).

**Read entry** (`read/mod.rs:24` `read_document`): header → sections (sorted numerically so that
`section0 < ... < section10`, `package.rs:105`) → BinData → version → content.hpf. `settings.xml`,
`Preview/*` and `META-INF/*` have no read path and never enter the IR; on an IR round-trip, write
fills them with constants or regenerates them.

---

## 2. Namespace table (table F)

The parser matches every element by `local_name()` (with the prefix stripped, see §3.3), so prefixes
matter only on the *emission* side. Emitted prefixes come from two places: the three on the body
section root (`write/section.rs:59`), and `FULL_XMLNS` (`templates.rs:25`, the full set of fifteen
declared by header and content.hpf). The package auxiliary files use their own families.

| Prefix | URI | Purpose | Files | Emits elements |
|--------|-----|------|-----------|:---:|
| `ha` | `.../hwpml/2011/app` | application settings root | settings.xml, (FULL_XMLNS) | yes (settings) |
| `hp` | `.../hwpml/2011/paragraph` | paragraphs, runs, controls, tables, shapes | section*.xml, declared in header | yes |
| `hp10` | `.../hwpml/2016/paragraph` | 2016 paragraph extensions | (FULL_XMLNS) | declaration only |
| `hs` | `.../hwpml/2011/section` | section root `hs:sec` | section*.xml | yes |
| `hc` | `.../hwpml/2011/core` | core geometry, color, matrices | section*.xml, header.xml | yes |
| `hh` | `.../hwpml/2011/head` | header root `hh:head` | header.xml | yes |
| `hhs` | `.../hwpml/2011/history` | edit history | (FULL_XMLNS) | declaration only |
| `hm` | `.../hwpml/2011/master-page` | master pages | (FULL_XMLNS) | declaration only |
| `hpf` | `.../schema/2011/hpf` | hpf package schema | content.hpf, container.xml | declaration only |
| `dc` | `http://purl.org/dc/elements/1.1/` | Dublin Core metadata | content.hpf | yes (creator, subject) |
| `opf` | `http://www.idpf.org/2007/opf/` | OPF package | content.hpf | yes |
| `ooxmlchart` | `.../hwpml/2016/ooxmlchart` | charts | (FULL_XMLNS) | declaration only |
| `hwpunitchar` | `.../hwpml/2016/HwpUnitChar` | unit characters | (FULL_XMLNS) | declaration only |
| `epub` | `http://www.idpf.org/2007/ops` | EPUB interoperability | (FULL_XMLNS) | declaration only |
| `config` | `urn:oasis:...:config:1.0` | ODF settings | settings.xml, (FULL_XMLNS) | yes (settings) |

The package family (auxiliary files only, `templates.rs`):

| Prefix | URI | Files |
|--------|-----|-----------|
| `hv` | `.../hwpml/2011/version` | version.xml (root `hv:HCFVersion`) |
| `ocf` | `urn:oasis:...:container` | container.xml (root `ocf:container`) |
| `rdf` | `http://www.w3.org/1999/02/22-rdf-syntax-ns#` | container.rdf |
| `ns0` (pkg#) | `.../hwpml/2016/meta/pkg#` | container.rdf (`hasPart`, the `HeaderFile`/`SectionFile`/`Document` types) |
| `odf` | `urn:oasis:...:manifest:1.0` | manifest.xml (root `odf:manifest`) |

**Audit point:** `FULL_XMLNS` declares fifteen namespaces but only `hp`, `hs`, `hc`, `hh`, `dc` and
`opf` (plus `ha` and `config` in settings) actually emit elements. `hp10`, `hhs`, `hm`, `hpf`,
`ooxmlchart`, `hwpunitchar` and `epub` are **declared but emit nothing**, meaning we generate no 2016
extensions, history, master pages or charts (see §5, unimplemented).

---

## 3. The read element catalog

### 3.1 section.xml (table G)

Four handling states:

- **semantic**: attributes fully interpreted into IR fields.
- **partial**: only some attributes interpreted (the rest ignored); the cell notes what was left out.
- **fallback preserved**: an unsupported element is wrapped in a `GenericControl`, recursively
  preserving only its text (subList).
- **skip**: the subtree is consumed and discarded (information loss).

Evidence is the actual line numbers in `read/section.rs`. The tables below contain **every element
arm** of each parser function (the exhaustive match-arm comparison is at the end of the document).

#### parse_section / parse_paragraph (`hp:p` and its children)

| Element (local name) | Parent | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|---|
| `p` | `hs:sec`/`tc`/`subList` | paraPrIDRef, styleIDRef, pageBreak, columnBreak | `Paragraph` (para_shape, style, break_type) | semantic | :59, :78-88 |
| `run` | `p` | charPrIDRef | `char_shape_runs` (overwriting at the same pos) | semantic | :99 |
| `t` | `p` | (text, entities, lineBreak) | a sequence of `HwpChar::Text` | semantic | :117 → `parse_text` :243 |
| `tab` | `p` | - | `InlineCtrl{9}` (8 WCHAR) | semantic | :122 |
| `lineBreak` | `p` | - | `CharCtrl(10)` | semantic | :132 |
| `secPr` | `p` | (children pagePr, margin) | `ExtCtrl(2,secd)` plus `SectionDef` | partial (grid, note and border ignored) | :136 → `parse_sec_pr` :312 |
| `ctrl` | `p` | (dispatched per child) | - | recursive | :149 → `parse_ctrl` :504 |
| `tbl` | `p` | rowCnt, colCnt, cellSpacing, pageBreak, repeatHeader, noAdjust, borderFillIDRef, zOrder | `ExtCtrl(11,tbl )` plus `Table` | semantic | :154 → `parse_table` :691 |
| `equation` | `p` | script plus sz and pos | `ExtCtrl(11,eqed)` plus `Generic{equation}` | partial (the script verbatim only) | :159 → `parse_equation` :1130 |
| `linesegarray` | `p` | lineseg* | `para.line_segs` | semantic | :173 → `parse_linesegs` :1255 |
| `pic` | `p` | zOrder (on the start tag) plus children | `ExtCtrl(11,gso )` plus `Picture` | semantic | :178 → `parse_picture` :889 |
| `rect`/`ellipse`/`line`/`polygon`/`curve`/`arc` | `p` | (shape geometry) | `ExtCtrl(11,ctrl_id)` plus `Generic{gso_shapes}` | semantic | :191 → `collect_shape` :995 (`shape_kind` :981) |
| *other objects* (container, textart and so on) | `p` | subList only | `ExtCtrl(11,ctrl_id)` plus `Generic{paragraph_lists}` | fallback preserved | :191 → `collect_sub_lists` :933 |

#### parse_text (inside `hp:t`)

| Event | What is read | IR target | Status | Evidence |
|---|---|---|---|---|
| `Text` | the string (counted in UTF-16) | `HwpChar::Text` | semantic | :251 |
| `GeneralRef` | `&amp; &lt; &gt; &quot; &apos;` and numeric references | `HwpChar::Text` | semantic | :262 |
| `lineBreak` | - | `CharCtrl(10)` | semantic | :281 |

#### parse_sec_pr (children of `hp:secPr`)

| Element | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|
| `pagePr` | width, height, landscape | `PageDef` (size, attr bit0) | partial | :335 |
| `margin` | left, right, top, bottom, header, footer, gutter | `PageDef` margins | semantic | :344 |
| *others* (grid, startNum, visibility, footNotePr, endNotePr, pageBorderFill, lineNumberShape) | - | - | **skip (ignored)** | :353 `_ => {}` |

#### parse_ctrl (children of `hp:ctrl`)

| Element | ctrl_id/code | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|---|
| `fieldBegin` | (type → id) / 3 | type, name, the Command child | `Generic` plus CTRL_DATA (0x0057) | semantic | :516 |
| `fieldEnd` | - / 4 | (LIFO matching) | `InlineCtrl(4)` with the reversed ctrl_id | semantic | :550 |
| `bookmark` | `bokm` / 22 | name | `Generic` plus the name in CTRL_DATA | semantic | :562 |
| `colPr` | `cold` / 2 | type, layout, colCount, sameSz, sameGap | `Generic{column_def}` | partial (colSz and colLine uncollected) | :586 → `parse_col_pr` :377 |
| `header` | `head` / 16 | applyPageType, id plus subList | `Generic` plus an 8B payload | semantic | :587 → `head_foot_data` :399 |
| `footer` | `foot` / 16 | applyPageType, id plus subList | `Generic` plus an 8B payload | semantic | :588 |
| `footNote` | `fn  ` / 17 | (subList) | `Generic` | partial (no payload) | :589 |
| `endNote` | `en  ` / 17 | (subList) | `Generic` | partial | :590 |
| `autoNum` | `atno` / 18 | - | `Generic` plus a 12B constant | partial (standard value) | :593 → `build_atno` :465 |
| `pageNum` | `pgnp` / 21 | pos, sideChar | `Generic` plus 12B | partial (format DIGIT only) | :594 → `build_pgnp` :415 |
| `pageHiding` | `pghd` / 21 | hideHeader, hideFooter, hideMasterPage, hideBorder, hideFill, hidePageNum | `Generic` plus a 4B bitmap | semantic | :595 → `build_pghd` :446 |
| `newNum` | `nwno` / 21 | num | `Generic` plus 6B | partial (kind fixed to PAGE) | :596 → `build_nwno` :475 |
| *other ctrl children* | (id) / 21 | subList only | `Generic` | fallback preserved | :597 `other` |
| `stringParam[name=Command]` | - | text | the field command | semantic | `read_field_command` :643 |

#### parse_table (children of `hp:tbl`) / parse_cell (children of `hp:tc`)

| Element | Parent | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|---|
| `tc` | `tbl` | header, borderFillIDRef | `Cell` | recursive | :738 → `parse_cell` :802 |
| `tr` | `tbl` | - | (a container; the row is recovered from cellAddr) | **skip (ignored)** | :742 |
| `inMargin` | `tbl` | left, right, top, bottom | `Table.inner_margins` | semantic | :750 |
| `pos` | `tbl` | treatAsChar, relTo, align, offset, flow and so on | `GsoPlacement` | semantic | :760 |
| `sz` | `tbl` | width, height | `GsoPlacement` | semantic | :772 |
| `outMargin` | `tbl` | left, right, top, bottom | `GsoPlacement.out_margins` | semantic | :776 |
| *other tbl children* | `tbl` | - | - | **skip (subtree consumed)** | :743 `_ => skip_subtree` |
| `cellAddr` | `tc` | colAddr, rowAddr | `Cell.col`/`row` | semantic | :829 |
| `cellSpan` | `tc` | colSpan, rowSpan | `Cell.col_span`/`row_span` | semantic | :833 |
| `cellSz` | `tc` | width, height | `Cell.width`/`height` | semantic | :837 |
| `cellMargin` | `tc` | left, right, top, bottom | `Cell.margins` | semantic | :841 |
| `subList` | `tc` | vertAlign | `Cell.list_attr` bits 5-6 | partial (vertAlign only) | :849 |
| `p` | `tc` | (paragraphs) | `Cell.paragraphs` | recursive | :861 |
| *other tc children* | `tc` | - | - | **skip (ignored)** | :864 `_ => {}` |

#### parse_picture (children of `hp:pic`) / collect_shape (shape children) / parse_equation / parse_gradation / parse_linesegs

| Element | Parent | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|---|
| `sz` | `pic` | width, height | `Picture.width`/`height` | semantic | :897 |
| `pos` | `pic` | treatAsChar, vertOffset, horzOffset | `Picture` (treat, offsets) | partial (relTo uncollected) | :901 |
| `img` | `pic` | binaryItemIDRef | `Picture.bin_ref` | semantic | :909 |
| *other pic children* (imgRect, imgClip, imgDim, renderingInfo, image effects) | `pic` | - | - | **skip (ignored)** | :916 `_ => {}` |
| `pos` | shape | horzOffset, vertOffset, treatAsChar | `ShapeGeom.x`/`y`/`anchored` | semantic | :1014 |
| `sz` | shape | width, height | `ShapeGeom.w`/`h` | semantic | :1021 |
| `lineShape` | shape | color, width, style, headStyle, tailStyle | border fields | semantic | :1025 |
| `winBrush` | shape (fillBrush) | faceColor | `ShapeGeom.fill` | semantic | :1040 |
| `pt0...ptN` | Polygon/Curve | x, y | `ShapeGeom.points` | semantic | :1047 |
| `center`/`ax1`/`ax2` | Arc | x, y | `ShapeGeom.points` (three points) | semantic | :1054 |
| `gradation` | shape (fillBrush) | type, angle, colors | `fill_gradient` | partial (angle approximated) | :1085 → `parse_gradation` :1217 |
| `subList` | shape | (paragraphs) | `paragraph_lists` | recursive | :1068 |
| *other shape children* (shadow, outMargin, renderingInfo, and the pt of Rect/Ellipse/Arc) | shape | - | - | **skip (ignored)** | :1059 `_ => {}` |
| `script` | equation | text | `Equation.script` | semantic | :1145 |
| `sz` | equation | width, height | `Equation.width`/`height` | semantic | :1147 |
| `pos` | equation | treatAsChar, offset | `Equation.inline`/`x`/`y` | semantic | :1150 |
| `color` | gradation | value | stops | semantic | :1228 |
| `lineseg` | linesegarray | textpos, vertpos, vertsize, textheight, baseline, spacing, horzpos, horzsize, flags | `LineSeg` | semantic | :1258 |

### 3.2 header.xml (table H)

`parse_header` accumulates into context variables (`current_char`, `current_para`, `current_border`,
`current_numbering`) in a single streaming loop. Evidence is line numbers in `read/header.rs`.

| Element (local name) | Parent context | Attributes read | IR target | Status | Evidence |
|---|---|---|---|---|---|
| `fontface` | refList | lang | the `current_lang` slot (seven languages) | semantic | :125 (`lang_slot` :58) |
| `font` | fontface | face | `fonts[slot]` `FaceName` | semantic | :132 |
| `typeInfo` | font | (all attributes verbatim) | `FaceName.type_info` | fallback preserved (as an attribute string) | :440 |
| `charPr` | charProperties | height, textColor, shadeColor, useFontSpace, useKerning, borderFillIDRef | `CharShape` | semantic | :140 |
| `fontRef` | charPr | hangul...user | `CharShape.face_ids` | semantic | :166 |
| `ratio` | charPr | per language | `CharShape.ratios` | semantic | :178 |
| `spacing` | charPr | per language | `CharShape.spacings` | semantic | :178 |
| `relSz` | charPr | per language | `CharShape.rel_sizes` | semantic | :178 |
| `offset` | charPr | per language | `CharShape.offsets` | semantic | :178 |
| `bold` | charPr | - | attr bit1 | semantic | :194 |
| `italic` | charPr | - | attr bit0 | semantic | :199 |
| `underline` | charPr | type, shape, color | attr bits 2-3, underline_shape, underline_color | semantic | :204 |
| `strikeout` | charPr | shape | attr bit18, strike | partial (NONE and 3D are not strikethrough) | :218 |
| `supscript` | charPr | - | attr bit15 | semantic (write symmetric since 2026-07-15) | :234 |
| `subscript` | charPr | - | attr bit16 | semantic (write symmetric) | :239 |
| `shadow` | charPr | type, color, offsetX, offsetY | attr bit11, shadow_color and gap | semantic (write symmetric) | :245 |
| `outline` | charPr | type | attr bit8 | partial (presence only; write symmetric as SOLID/NONE) | :259 |
| `emboss` | charPr | - | attr bit13 | semantic (write symmetric) | :266 |
| `engrave` | charPr | - | attr bit14 | semantic (write symmetric) | :271 |
| `paraPr` | paraProperties | snapToGrid, condense, fontLineHeight, tabPrIDRef | `ParaShape.attr1`/`tab_def_id` | semantic | :276 |
| `align` | paraPr | horizontal | attr1 bits 2-4 | semantic | :301 (`alignment_code` :87) |
| `heading` | paraPr | type, level, idRef | attr1 bits 23-27, numbering_id | semantic | :309 |
| `intent`/`left`/`right`/`prev`/`next` | paraPr > margin | value (in units of ×2) | `ParaShape` margins | semantic | :356 |
| `margin` (End) | paraPr | - | `para_margin_done` (only the first branch is taken) | control | :515 |
| `lineSpacing` | paraPr | type, value | line_spacing_type, line_spacing(_old) | semantic | :375 |
| `breakSetting` | paraPr | breakLatinWord, breakNonLatinWord, widowOrphan, keepWithNext, keepLines, pageBreakBefore | several attr1 bits | semantic | :404 |
| `border` | paraPr | borderFillIDRef | `ParaShape.border_fill_id` | semantic | :432 |
| `numbering` | numberings | - | `current_numbering` | semantic | :326 |
| `paraHead` | numbering | level, start, numFormat plus text | `NumLevel` (fmt, template) | semantic | :333 (`num_fmt` :72) |
| `bullet` | (refList) | char | `bullet_chars` | semantic | :350 |
| `borderFill` | borderFills | - | `current_border` | semantic | :454 |
| `slash`/`backSlash` | borderFill | type | attr bit2/bit5 | partial (presence only) | :464 |
| `leftBorder`/`rightBorder`/`topBorder`/`bottomBorder` | borderFill | type, width, color | `BorderFill.sides` | semantic | :475 (`parse_border_line` :49) |
| `diagonal` | borderFill | type, width, color | `BorderFill.diagonal` | semantic | :486 |
| `winBrush` | borderFill (fillBrush) | faceColor | `BorderFill.bg_color`, fill_type bit0 | semantic | :491 |
| `style` | styles | name, engName, paraPrIDRef, charPrIDRef, nextStyleIDRef, langID | `Style` | semantic | :499 |
| *others* (beginNum, compatibleDocument, docOption, linkinfo, autoSpacing, ...) | - | - | - | **skip (ignored)** | :510 `_ => {}` |

### 3.3 The local-name matching policy and its implications

**Policy:** all matching is by `e.local_name()` with the namespace prefix stripped (`read/xml.rs:6`
`attr`, and every `match e.local_name().as_ref()` in the two parsers above). So `hp:p` and `p` both
match the local name `p`, regardless of which prefix is used or redefined.

**Implication 1 (robustness):** the parser does not break when prefixes differ between documents
(even if a genuine file redeclares a different prefix instead of `hp:`).

**Implication 2 (collision risk):** elements with the same local name under different prefixes cannot
be distinguished. In practice `hc:winBrush` (shape fill) and `hh:winBrush` (border fill background)
share the local name `winBrush` but live in different parser contexts (collect_shape versus
parse_header) and do not collide. `sz` and `pos` are likewise interpreted only locally within the tbl,
pic, shape and equation contexts.

**Two fates for unmatched elements:**

- **fallback preserved**: the `_ =>` arm of `parse_paragraph` (`section.rs:191`) and the `other` arm
  of `parse_ctrl` (`:597`) wrap an unsupported element in a `GenericControl` (using the first four
  bytes of the original local name as the ctrl_id) and recursively collect its `subList` paragraphs.
  **The text survives**, but object-specific properties (chart data, OLE and so on) are discarded. On
  write, with neither gso_shapes nor paragraph_lists, it is dropped (§4, §5).
- **skip (information loss)**: uninteresting elements are discarded with `_ => {}` (ignoring the
  event) or `skip_subtree` (consuming the whole subtree). Every row marked **skip** in tables G and H
  is in this category. Nothing remains in the IR, so on a round-trip write either re-synthesizes a
  constant or the element simply disappears.

---

## 4. The write emission catalog (table I)

`write/section.rs` emits **89** unique elements and `write/header.rs` emits **51** (measured by grep,
including prefixes). Rather than listing them individually, they are grouped by family. Elements read
cannot produce (constants) are marked *constant*.

### 4.1 write/section.rs (89)

| Family | Emitted elements | Source function |
|---|---|---|
| section root | `hs:sec` | `write_section` :59 |
| paragraphs, runs, text | `hp:p`, `hp:run`, `hp:t`, `hp:tab`, `hp:lineBreak` | `write_paragraph` :116, `flush_text` :410 |
| section definition (mostly constants) | `hp:secPr`, `hp:grid`, `hp:startNum`, `hp:visibility`, `hp:lineNumberShape`, `hp:pagePr`, `hp:margin`, `hp:footNotePr`, `hp:endNotePr`, `hp:autoNumFormat`, `hp:noteLine`, `hp:noteSpacing`, `hp:numbering`, `hp:placement`, `hp:pageBorderFill` | `write_default_sec_pr` :450 |
| columns | `hp:colPr` (plus an `hp:ctrl` wrapper) | `write_col_ctrl` :473 |
| header and footer | `hp:header`/`hp:footer` (emitted by local name), `hp:subList` | `write_header_footer` :500 |
| page controls | `hp:pageNum`, `hp:pageHiding`, `hp:newNum`, `hp:autoNum` | `write_paragraph` arms :292-344 |
| fields and bookmarks | `hp:fieldBegin`, `hp:fieldEnd`, `hp:parameters`, `hp:stringParam`, `hp:bookmark` | :256-291 |
| tables | `hp:tbl`, `hp:tr`, `hp:tc`, `hp:cellAddr`, `hp:cellSpan`, `hp:cellSz`, `hp:cellMargin`, `hp:inMargin`, `hp:outMargin`, `hp:sz`, `hp:pos` | `write_table` :972 |
| shape common scaffolding (constants) | `hp:offset`, `hp:orgSz`, `hp:curSz`, `hp:flip`, `hp:rotationInfo`, `hp:renderingInfo`, `hc:transMatrix`, `hc:scaMatrix`, `hc:rotMatrix` | `write_obj_scaffold` :609 |
| shape elements | `hp:rect`, `hp:ellipse`, `hp:line`, `hp:polygon`, `hp:curve`, `hp:arc`, `hp:connectLine` (for counting) | `write_shape_element` :692 |
| shape style and fill | `hp:lineShape`, `hc:fillBrush`, `hc:winBrush`, `hc:gradation`, `hc:color`, `hp:shadow` | :741-781 |
| shape geometry points | `hc:startPt`, `hc:endPt`, `hc:pt`/`hc:pt0..3`, `hc:center`, `hc:ax1`, `hc:ax2`, `hc:start1`, `hc:end1`, `hc:start2`, `hc:end2` | :787-839 |
| text box text | `hp:drawText`, `hp:subList`, `hp:textMargin` | `write_draw_text` :622 |
| pictures | `hp:pic`, `hc:img`, `hp:imgRect`, `hp:imgClip`, `hp:imgDim` | `write_picture` :1060 |
| line layout (optional) | `hp:linesegarray`, `hp:lineseg` | :381 |

### 4.2 write/header.rs (51)

| Family | Emitted elements | Source function |
|---|---|---|
| root and structure | `hh:head`, `hh:beginNum`, `hh:refList`, `hh:compatibleDocument`, `hh:layoutCompatibility`, `hh:docOption`, `hh:linkinfo` | `write_header` :44 |
| fonts | `hh:fontfaces`, `hh:fontface`, `hh:font`, `hh:typeInfo` | `write_fontfaces` :76 |
| border fills | `hh:borderFills`, `hh:borderFill`, `hh:slash`, `hh:backSlash`, `hh:leftBorder`/`rightBorder`/`topBorder`/`bottomBorder`, `hh:diagonal`, `hc:fillBrush`, `hc:winBrush` | `write_border_fills` :130 |
| character shapes | `hh:charProperties`, `hh:charPr`, `hh:fontRef`, `hh:ratio`, `hh:spacing`, `hh:relSz`, `hh:offset`, `hh:italic`, `hh:bold`, `hh:underline`, `hh:strikeout`, `hh:outline`, `hh:shadow`, `hh:emboss`, `hh:engrave`, `hh:supscript`, `hh:subscript` (all IR-driven since 2026-07-15) | `write_char_properties` :184 |
| tabs | `hh:tabProperties`, `hh:tabPr` | `write_tab_properties` :263 |
| numbering and bullets | `hh:numberings`, `hh:numbering`, `hh:paraHead`, `hh:bullets`, `hh:bullet` (added by PR #8; previously bullet definitions were lost on write) | `write_numberings`, `write_bullets` |
| paragraph shapes | `hh:paraProperties`, `hh:paraPr`, `hh:align`, `hh:heading`, `hh:breakSetting`, `hh:autoSpacing`, `hh:margin`, `hc:intent`/`left`/`right`/`prev`/`next`, `hh:lineSpacing`, `hh:border` | `write_para_properties` :291 |
| styles | `hh:styles`, `hh:style` | `write_styles` :346 |

**Audit point:** many elements in `write_default_sec_pr` are **fixed constant templates** unrelated to
the IR (footnote, endnote and page border constants). They exist to make a "valid document", not to
preserve a round-trip. Since 2026-07-15, `write_numberings` emits from the IR when `numbering_levels`
exists, falling back to the old `^{level}.` constant only when it does not (the hwp5 path).

---

## 5. The read/write symmetry matrix (table J)

Where the lossless round-trip breaks, audited in three categories. This table is the input to gap
document 12.

### (a) Emitted by write only, unparsed by read: write re-synthesizes on a round-trip

Read discards them (the skips in §3), so they are absent from the IR. Write recomputes them from sz
and coordinates, or fills constants.

| Element | write emits | read | Round-trip effect | Evidence |
|---|---|---|---|---|
| `hp:offset`, `hp:orgSz`, `hp:flip`, `hp:rotationInfo` | constants (0,0)/(w,h)/angle 0 | skip | rotation and flip not preserved (always 0) | `write/section.rs:609` |
| `hp:renderingInfo` plus `hc:transMatrix`/`scaMatrix`/`rotMatrix` | identity matrix constants | skip | the transform matrix is regenerated as identity | :612 |
| `hp:curSz` | Ellipse and Arc (0,0), otherwise (w,h) | skip | the measured genuine value is re-synthesized | :736 |
| `hc:pt0~3` (Rect), `hc:center`/`ax1`/`ax2`/`start*`/`end*` (Ellipse) | bbox recomputed from sz | **ignored** (the pt of Rect and Ellipse) | re-synthesized from sz (avoiding duplication) | :805, :813 |
| `hp:shadow type="NONE"` | constant | skip | shape shadow is a constant | :781 |
| `hp:imgRect`, `hp:imgClip`, `hp:imgDim` | bbox constants | skip (parse_picture) | image cropping and dimensions re-synthesized | :1079 |
| `hp:drawText` > `hp:textMargin` | the constant margin 283 | only the text (subList) is collected | text box margins become a constant | :622 |
| lineShape `headfill`, `tailfill`, `headSz`, `tailSz`, `endCap`, `outlineStyle`, `alpha` | constants | only color, width, style, head and tail | arrow size and tail become constants | :748 |
| `hp:pageBorderFill`, `hp:footNotePr`, `hp:endNotePr`, `hp:grid`, `hp:startNum`, `hp:visibility`, `hp:lineNumberShape` | re-emits the original when present, otherwise section constants | **original XML pass-through** (`secpr_raw_children`) | ✅ **resolved (2026-07-15, GC-5)**: the original and its order are preserved in an hwpx round-trip (not semantic parsing) | `parse_sec_pr` ↔ `write_default_sec_pr` |
| `hh:beginNum`, `hh:compatibleDocument`, `hh:docOption`, `hh:autoSpacing` | constants | skip (header) | compatibility and document options become constants | `write/header.rs:51,71` |

### (b) Interpreted by read but constant, approximated or unemitted by write: read but not written back to hwpx

The value exists in the IR (and survives to hwp5), but the hwpx writer flattens it to a constant or an
approximation, so it is **lost in an hwpx → hwpx round-trip**.

| Element/attribute | read interprets | write | Round-trip effect (hwpx → hwpx) | Evidence |
|---|---|---|---|---|
| charPr `shadow` | attr bit11 plus color and offset | ✅ IR-driven (`DROP` plus color and offset) | **resolved (2026-07-15)** | read `header.rs:245` ↔ `write_char_properties` |
| charPr `outline` | attr bit8 | ✅ presence-driven `SOLID`/`NONE` | **resolved (2026-07-15)** | `:259` ↔ likewise |
| charPr `emboss`/`engrave` | attr bit13/14 | ✅ emitted only when set | **resolved (2026-07-15)** | `:266`, `:271` ↔ likewise |
| charPr `supscript`/`subscript` | attr bit15/16 | ✅ emitted only when set | **resolved (2026-07-15)** | `:234`, `:239` ↔ likewise |
| `hh:underline shape` | type, **shape** and color interpreted (new IR `underline_shape`) | ✅ driven by `underline_shape` (0 = SOLID) | **resolved (2026-07-15)** | `:204` ↔ likewise |
| colPr `colSz`/`colLine` (per-column widths and separators) | uncollected (equal width assumed) | emits the values but has no per-column width | unequal columns and separators lost | `parse_col_pr :377` ↔ `write_col_ctrl :473` |
| `hc:gradation angle`, center, step | angle only (approximated in radians) | rounded angle plus constant centerX/Y and step | gradient center and steps approximated | `parse_gradation :1217` ↔ `:764` |
| `hp:pagePr landscape` | attr bit0 | re-emitted by default_sec_pr | preserved (along with the other secPr constants) | `:340` ↔ `:453` |
| numbering `paraHead` format | template, start and numFormat collected | ✅ driven by `numbering_levels` (falling back to the old constant) | **resolved (2026-07-15)**, which also fixed itemCnt being flattened for multiple numbering definitions | `:333` ↔ `write_numberings` |
| tab `tabPr` (positions and leaders) | `tabPr`/`tabItem` parsed semantically into the IR `TabDef` | ✅ emitted from `tab_stops` (falling back to the old constant) | **resolved (2026-07-15, GC-4)** | `read/header.rs` ↔ `write_tab_properties` |
| paraPr `heading` (paragraph-to-numbering link) | attr1 bits 23-27 plus numbering_id | ✅ OUTLINE/NUMBER/BULLET re-emitted | **resolved (2026-07-15, second round)**, see [12](12-feature-gaps.en.md) GE-α8. PR #8 (07-18) additionally fixed the OUTLINE idRef off-by-one observed in Hancom (genuine idRef=0) and tolerated non-contiguous definition ids | `:309` ↔ `write_para_properties` |

### (c) Missing on both sides (unimplemented): neither read nor write handles it semantically

| Object/element | Current handling | Evidence |
|---|---|---|
| `hp:chart` (ooxmlchart) | read fallback (no text) → write DROP | namespace declared only (§2) |
| OLE objects (`hp:ole` and so on) | read fallback → write DROP | `collect_sub_lists` :933 → DROP `write/section.rs:364` |
| `hp:video` and media | read fallback → write DROP | likewise |
| `hp:container` (grouped objects) | read fallback (subList text only) → write DROP | :191 → :364 |
| `hp:textart` | read fallback (text only) → write DROP | likewise |
| `hp:formObject` (form objects) | read fallback → write DROP | likewise |
| `hp:compose`/`hp:dutmal` (overlapping characters, ruby text) | read fallback → write DROP | likewise |
| Index marks (idxm), odd/even adjustment (pgct), hidden comments (tcmt), arriving as Generic from hwp5 | read fallback (synthesizing a 4B ctrl_id, `read/section.rs:704-710`) → write DROP | added in the 2026-07-19 audit: tcmt drops the whole paragraph list and therefore loses content ([12](12-feature-gaps.en.md) GF-5), and pgct has no confirmed hwpx element name at all |
| Master pages (`hm:` master-page) | absent from both read and write | namespace declared only |
| Edit history (`hhs:`) | absent from both read and write | namespace declared only |

**A nuance for category (c):** `container`, `textart`, `formObject` and `compose` are **not entirely
ignored** by read; `collect_sub_lists` places their child `subList` paragraphs into
`GenericControl.paragraph_lists`, so *the text does reach the IR*. On write, however, that Generic has
no gso_shapes and no known ctrl_id, so it ends at
`Control::Generic(g) => warnings.push("DROP...")` (`write/section.rs:364`) and the collected text
disappears with it. `chart`, `ole` and `video` have no text at all and are lost completely.

---

## 6. OWPML enumeration to hwp5 code conversion table

The mappings that actually exist in the code. A read function (OWPML string → code) pairs with a write
function (code → OWPML string). Evidence is the corresponding function in `read/*.rs` and `write/*.rs`.

### Placement and reference (section)

| Axis | OWPML value → code | read | write (inverse) |
|---|---|---|---|
| `vertRelTo` | PAPER=0, PAGE=1, PARA=2 | `vert_rel_to_code` :663 | `vert_rel_to_name` :565 |
| `horzRelTo` | PAPER=0, PAGE=1, COLUMN=2, PARA=3 | `horz_rel_to_code` :672 | `horz_rel_to_name` :572 |
| `vertAlign` | TOP=0, CENTER=1, BOTTOM=2 | `align_code` :682 | `vert_align_name` :580 |
| `horzAlign` | LEFT=0, CENTER=1, RIGHT=2 | `align_code` :682 | `horz_align_name` :587 |

### Lines and borders

| Axis | OWPML value → code | read | write (inverse) |
|---|---|---|---|
| shape line style (lineShape `style`) | SOLID=0, DASH=1, DOT=2, DASH_DOT=3, DASH_DOT_DOT=4, LONG_DASH=5 | `line_style_code` :1195 | `line_style_name` :594 |
| arrowheads (head/tailStyle) | NORMAL/NONE/"" = 0, otherwise 1 | `arrow_code` :1207 | `arrow_name` :604 |
| border line (borderFill `type`) | NONE=0, SOLID=1, DASH=2, DOT=3, DASH_DOT=4, DASH_DOT_DOT=5, LONG_DASH=6, CIRCLE=7, DOUBLE_SLIM=8, SLIM_THICK=9, THICK_SLIM=10, SLIM_THICK_SLIM=11 | `line_type_code` `header.rs:17` | `line_type_name` `write/header.rs:16` |
| border thickness | the nearest index in a 16-step mm table (0.1 to 5.0mm) | `width_index` `header.rs:36` | `width_mm_attr` `write/header.rs:34` |

### Characters and paragraphs (header)

| Axis | OWPML value → code | read | write (inverse) |
|---|---|---|---|
| alignment (align `horizontal`) | JUSTIFY=0, LEFT=1, RIGHT=2, CENTER=3, DISTRIBUTE=4, DISTRIBUTE_SPACE=5 | `alignment_code` :87 | `write_para_properties` :298 |
| line spacing kind (lineSpacing `type`) | PERCENT=0, FIXED=1, BETWEEN_LINES=2, AT_LEAST=3 | :380 | :307 |
| underline (underline `type`) | NONE=0, BOTTOM=1, TOP=3 | :207 | :239 |
| paragraph head (heading `type`) | NONE=0, OUTLINE=1, NUMBER=2, BULLET=3 | :311 | (heading constant emitted) |
| number format (`numFormat`) | DIGIT / HANGUL_SYLLABLE / HANGUL_JAMO / CIRCLED_DIGIT / LATIN_UPPER and LOWER / ROMAN_UPPER and LOWER | `num_fmt` :72 | (constant) |
| language slot (fontface `lang`) | HANGUL=0, LATIN=1, HANJA=2, JAPANESE=3, OTHER=4, SYMBOL=5, USER=6 | `lang_slot` :58 | `LANG_NAMES` `write/header.rs:12` |

### Control payloads (section, `hp:ctrl`)

| Axis | OWPML value → code | read | write (inverse) |
|---|---|---|---|
| column kind (colPr `type`) | NEWSPAPER=0, BALANCED=1, PARALLEL=2 | `parse_col_pr` :377 | `write_col_ctrl` :473 |
| column direction (colPr `layout`) | LEFT=0, RIGHT=1, MIRROR=2 | `parse_col_pr` :383 | `write_col_ctrl` :482 |
| header/footer application (`applyPageType`) | BOTH=0, EVEN=1, ODD=2 | `head_foot_data` :400 | `write_header_footer` (the BOTH constant) :516 |
| page number position (pageNum `pos`) | NONE=0, TOP_LEFT=1 ... BOTTOM_RIGHT=6, OUTSIDE_TOP=7, OUTSIDE_BOTTOM=8, INSIDE_TOP=9, INSIDE_BOTTOM=10 | `build_pgnp` :415 | `page_num_pos_name` :654 |
| table page break (tbl `pageBreak`) | NONE=0, TABLE=1, CELL=2 | `parse_table` :700 | (write fixes CELL) :1002 |
| gradient (gradation `type`) | LINEAR = linear, otherwise approximated as radial | `parse_gradation` :1223 | `write_shape_element` (LINEAR/RADIAL) :765 |
| color (`#RRGGBB` ↔ COLORREF) | `#RRGGBB` → `0x00BBGGRR` (R and B swapped) | `parse_color` `xml.rs:36` | `color_hex` `section.rs:555` / `color_attr` `templates.rs:101` |

---

## Appendix: the exhaustive match-arm comparison

While writing this document, **every** element matching arm in `read/section.rs` and `read/header.rs`
was compared one to one against the tables in §3.

- **read/section.rs**: element handling arms (by local name) verified exhaustively per parser:
  `parse_paragraph` 11 arms (plus 1 fallback), `parse_text` 3, `parse_sec_pr` 2 (plus skip),
  `parse_ctrl` 12 (plus other), `parse_table` 6 (plus skip_subtree), `parse_cell` 5 (plus skip),
  `parse_picture` 3 (plus skip), `collect_shape` 8 (plus skip), `parse_equation` 3,
  `parse_gradation` 1, `parse_linesegs` 1, `read_field_command` 1. Nothing is missing from table G
  (the entity constants `amp`, `lt`, `gt`, `quot` and `apos` are a detail of text interpretation and
  are folded into the parse_text row of §3.1).
- **read/header.rs**: 35 Start/Empty dispatch arms plus 7 End dispatches (fontface, margin, charPr,
  borderFill, paraPr, numbering, paraHead) plus Text (the paraHead template), all verified. Nothing is
  missing from table H.

### Summary of the input to gap document 12

**Unimplemented (missing on both sides):** chart, ole, video, container, textart, formObject,
compose/dutmal, master pages (hm master-page) and edit history (hhs). For container, textart,
formObject and compose, read preserves only the text as a fallback, which write then drops.

**Skip (information loss): consumed or ignored, leaving no trace in the IR:**

- section: the grid, startNum, visibility, footNotePr, endNotePr, pageBorderFill and lineNumberShape
  of `hp:secPr` (`:353`); unknown children of `hp:tbl` (`:743` skip_subtree) and `hp:tr` (`:742`);
  unknown children of `hp:tc` (`:864`); the imgRect, imgClip, imgDim, renderingInfo and image effects
  of `hp:pic` (`:916`); the shadow, outMargin and renderingInfo of shapes plus the pt of Rect,
  Ellipse and Arc (`:1059`).
- header: unmatched elements such as beginNum, compatibleDocument, docOption, linkinfo and
  autoSpacing (`:510`).

**Interpreted by read but lost by write (category (b)):** the shadow, outline, emboss, engrave,
supscript and subscript of charPr, underline shape, colPr per-column widths and separators, gradient
center and step, numbering formats, and tab definitions.
