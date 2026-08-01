[한국어](10-hwp5-structure-map.ko.md) · [English](10-hwp5-structure-map.md)

# The exhaustive HWP 5.0 structure map

A **single source of truth** for looking up **every stream, record tag, control character and
extended control id** an HWP 5.0 file can contain, and for checking with what fidelity hwp-cli
handles each. It is the catalog of "what exists and how we handle it".

## Division of roles with the other documents

| Document | Subject | Relationship to this one |
|---|---|---|
| [02-hwp5-read](02-hwp5-read.md) | **How to parse**: bit layouts, cursors, tree reconstruction algorithms | Linked from here as "details in [02] §n". This document is the *map*; 02 is the *implementation specification* |
| [03-hwp5-write](03-hwp5-write.md) | **Synthesis and writing**: CFB V3, version gating, Hancom-compatible assembly | The write column of the stream tree (§1) summarizes 03 |
| [12-feature-gaps](12-feature-gaps.md) | **Unimplemented and loss list**: what Opaque and raw-preserved records actually lose | The `Opaque` and `raw-preserved` rows of tables A and B here are 12's input |

The status classification here (§3 and §4) is the confirmed fact; document 12 evaluates how that fact
shows up as a defect in Hancom. To change a status label, change it **here first**.

### Copyright notice

Specification tables and wording are never reproduced. Tag names (`HWPTAG_*`), values and § numbers
are cited as facts, and the "payload summary" column is not a copy of the specification's description
but **a description in our own words of the layout the code actually reads**. Points where the
specification and our measurement disagree are marked ★. The specification itself is not bundled
(see [docs/README](../README.md)).

### The four status labels (shared by tables A to D)

| Label | Meaning |
|---|---|
| **semantic** | Record fields are fully interpreted into named IR structures. Rendering and editing work from the values. |
| **partial + raw** | Only the known prefix is decoded into a structure with the tail preserved, or only some fields are interpreted (as render hints) with the rest raw. |
| **raw-preserved** | Bytes are preserved wholesale without semantic interpretation, but a first-class slot exists (`raw_children`, `common_data`, `RawEntry` and so on) that a consumer such as the renderer parses separately or that is used for lossless re-emission. |
| **Opaque** | Preserved as an `OpaqueRecord` in `extras`/`id_extras`, subtree and all. No consumer interprets it; it exists purely for a lossless round-trip. |

`constant undefined (preserved through Opaque)` marks a specification record that has no constant in
`tag.rs`. The tag passes through as a raw u16, so scanning and tree reconstruction are unaffected and
it ends up preserved as Opaque.

---

## 1. The CFB storage and stream tree

An HWP 5.0 file is a Microsoft **CFB (Compound File Binary)** container. Below is the whole tree and
the read/write support matrix. "Compressed" means raw DEFLATE (no zlib header, no Adler32) applied
**only to record streams** when the FileHeader COMPRESSED bit (bit0) is set.

```
/ (root storage)
├── FileHeader                     fixed 256B, uncompressed
├── DocInfo                        document information record stream (compressed)
├── BodyText/                      storage
│   ├── Section0                   body section 0 record stream (compressed)
│   ├── Section1 ... SectionN
├── ViewText/                      distribution body (unsupported; read errors)
│   └── Section0 ...
├── BinData/                       storage
│   ├── BIN0001.png                attached binary (follows the header flag)
│   └── ...
├── \x05HwpSummaryInformation      OLE property set (uncompressed)
├── PrvText                        preview text, UTF-16LE (uncompressed)
├── PrvImage                       preview image, PNG/BMP (uncompressed)
├── DocOptions/                    storage
│   ├── _LinkDoc                   document link options
│   └── (DrmLicense, DrmRootSect, CertDrmHeader, CertDrmInfo,
│        DigitalSignature, PublicKeyInfo: optional streams, unparsed and not emitted)
├── Scripts/                       storage
│   ├── JScriptVersion             (record stream rules, so compressible)
│   └── DefaultJScript
├── XMLTemplate/                   XML template storage (unsupported; passed over)
├── DocHistory/                    document history (unsupported; passed over)
└── Bibliography/                  bibliography XML (unsupported; passed over)
```

| Stream/storage | Spec § | Read | Write (synthesis) | Compressed | Code evidence |
|---|---|---|---|---|---|
| `/FileHeader` | 3.2.1 | semantic | emitted | no | `file_header.rs:91`, `container.rs:35` |
| `/DocInfo` | 3.2.2 | semantic | emitted | yes | `doc_info.rs:18`, `write.rs:170` |
| `/BodyText/SectionN` | 3.2.3 | semantic | emitted | yes | `body_text.rs:23`, `write.rs:173` |
| `/ViewText/SectionN` | 3.2.3 | unsupported (error) | - | yes | `container.rs:101` (distribution guard) |
| `/BinData/*` | 3.2.5 | try and fall back | only referenced items included | per header flag | `read.rs`, `write.rs:190` |
| `/\x05HwpSummaryInformation` | 3.2.4 | partial + raw | synthesized | no | `summary.rs`, `write.rs:223` |
| `/PrvText` | 3.2.6 | unparsed | generated from the body | no | `write.rs:225` |
| `/PrvImage` | 3.2.7 | unparsed | only when provided in opts | no | `write.rs:226` |
| `/DocOptions/_LinkDoc` | 3.2.8 | unparsed | constant emitted | no | `write.rs:209` |
| `/DocOptions/{Drm*,CertDrm*,DigitalSignature,PublicKeyInfo}` | 3.2.8 | unparsed | **not emitted** | no | (no branch; [12](12-feature-gaps.md) GE-β3) |
| `/Scripts/*` | 3.2.9 | unparsed | sample constant emitted | yes (by rule) | `write.rs:213`, `container.rs:114` |
| `/XMLTemplate/*` | 3.2.10 | unsupported (passed over) | none | - | - |
| `/DocHistory/*` | 3.2.11 | unsupported (passed over) | none | - | - |
| `/Bibliography/*` | 3.2.12 | unsupported (passed over) | none | - | (no branch; [12](12-feature-gaps.md) GB-12) |

**Compression decision** `is_record_stream(path)` (`container.rs:114`): only streams beginning with
`/DocInfo`, `/BodyText/`, `/ViewText/` or `/Scripts/` are compressed. FileHeader, PrvText, PrvImage,
BinData and the summary information are excluded. On the read path, BinData tries and falls back per
item with `decompress(...).unwrap_or(raw)`.

**★ Write asymmetry**: the writer compresses DocInfo, BodyText and BinData with raw deflate, but
emits `/Scripts/*` as **the sample bytes lifted from an empty Hancom document (already in compressed
form)** (`write.rs:213`), writes `_LinkDoc` as 524B of zeros, and always creates the auxiliary streams
because Hancom may judge their absence as corruption (`write.rs:207`). The CFB must be **version 3
(512B sectors)**; Hancom treats V4 (4096B) as a corrupt file (`write.rs:167`, a Hancom-tested gate).

**Enumerating body sections** `body_sections()` (`container.rs:62`) sorts the numeric suffix of
`/BodyText/Section` as an integer (`parse::<u32>()` is required so that `Section10` follows
`Section2`).

---

## 2. Record header structure (summary)

A decompressed DocInfo, BodyText or Scripts stream is a sequence of records, each a 4-byte (or
8-byte) header plus a payload. Three fields are bit-packed into a single u32 LE.

```
u32 LE = tagID (low 10 bits) | level (next 10 bits) | size (top 12 bits)
If the size bit field == 0xFFF, the next u32 LE is the real size (an 8-byte header)
```

- **tagID**: 10 bits (0 to 1023). Always preserved as a raw u16, never coerced into an enum, so
  unknown tags pass through losslessly.
- **level**: 10 bits, the tree depth. Re-serializing by depth reproduces the decompressed stream
  byte for byte, which is the foundation of the lossless round-trip.
- **size**: 12 bits (0 to 4094 inline). `0xFFF` is reserved as the extension marker and cannot be
  inline.

Specification §4.1 (data record structure). For the bit masks and shifts, tolerant versus strict
scanning and stack-based tree reconstruction, see **[02] §5 (record header bit layout) and §6
(scanning and tree reconstruction)**. Evidence: `record/header.rs`, `record/scan.rs`,
`record/tree.rs`.

---

## 3. DocInfo record catalog (table A)

Every record that appears in the `/DocInfo` stream. Specification table 13 (§4.2) is the authoritative
list, and the 21 rows below correspond to it one to one. `parse_doc_info` (`doc_info.rs:18`)
interprets only **DOCUMENT_PROPERTIES and ID_MAPPINGS** at the root; the actual table entries are
classified by `parse_id_mapping_child` (`doc_info.rs:64`) as children of ID_MAPPINGS. Other roots are
Opaque-preserved in `header.extras`, and unknown ID_MAPPINGS children in `header.id_extras`.

> ⚠️ **Specification grouping ≠ code comment grouping.** Specification table 13 classifies
> `MEMO_SHAPE` (BEGIN+76), `FORBIDDEN_CHAR` (+78), `TRACK_CHANGE` (+80) and `TRACK_CHANGE_AUTHOR`
> (+81) as **DocInfo records** (even though their tag values fall in the body numeric range).
> `tag.rs`, however, places them under the `// ── body (BodyText) records` comment
> (`tag.rs:53-58`) because their tag values are `BEGIN+50` or higher and they were ordered
> numerically. **The specification's table 13 governs**: these four are semantically DocInfo and
> appear in table A. Do not mistake them for body records because of where the comment sits.

| Tag id (hex) | Name (HWPTAG_*) | Value (BEGIN+n) | Spec § | Payload summary | hwp-cli status | Code evidence |
|---|---|---|---|---|---|---|
| 0x10 | DOCUMENT_PROPERTIES | +0 | 4.2.1 | section count u16 + six start numbers u16 + caret (list/para/char, u32 ×3) | semantic | `doc_info.rs:152` |
| 0x11 | ID_MAPPINGS | +1 | 4.2.2 | u32 count array (binData, fonts ×7, ...) with the real tables as children | semantic | `doc_info.rs:34` |
| 0x12 | BIN_DATA | +2 | 4.2.3 | attr u16 (kind in the low 4 bits) + link path or storage_id/extension + tail | partial + raw | `doc_info.rs:357` |
| 0x13 | FACE_NAME | +3 | 4.2.4 | attr u8 + name + [substitute font, PANOSE 10B, default font] + tail | partial + raw | `doc_info.rs:167` |
| 0x14 | BORDER_FILL | +4 | 4.2.5 | attr u16 + four sides (kind/thickness/color, 6B each) + diagonal 6B + fill u32 + [background color] + tail | partial + raw | `doc_info.rs:325` |
| 0x15 | CHAR_SHAPE | +5 | 4.2.6 | a 68B prefix (font ids ×7, width scaling, spacing, relative size, character position, base size, attr, colors ×4) + tail | partial + raw | `doc_info.rs:206` |
| 0x16 | TAB_DEF | +6 | 4.2.7 | raw preserved plus semantic parsing of the attribute u32 and tab items (position i32, kind u8, fill u8) (`tab_stops`, 2026-07-15 GC-4) | partial + raw | `doc_info.rs:112`, `parse_tab_def` |
| 0x17 | NUMBERING | +7 | 4.2.8 | raw preserved plus parsing of the seven-level format templates for rendering (`^1.` and so on) | partial + raw | `doc_info.rs:113`, `:401` |
| 0x18 | BULLET | +8 | 4.2.9 | raw preserved; only the WCHAR at offset 8 is extracted as the bullet glyph (a control character becomes `•`) | raw-preserved | `doc_info.rs:120` |
| 0x19 | PARA_SHAPE | +9 | 4.2.10 | a 42B prefix (attr1, margins, indentation, spacing, tab/numbering/border ids, offsets ×4) + tail (line spacing) | partial + raw | `doc_info.rs:260` |
| 0x1A | STYLE | +10 | 4.2.11 | name and English name + attr u8 + next style u8 + language i16 + paragraph and character shape u16 + tail | partial + raw | `doc_info.rs:302` |
| 0x1B | DOC_DATA | +11 | 4.2.12 | arbitrary document data; a root, unparsed | Opaque | `doc_info.rs:57` |
| 0x1C | DISTRIBUTE_DOC_DATA | +12 | 4.2.13 | distribution document data; a root, unparsed | Opaque | `doc_info.rs:57` |
| 0x1D | *(RESERVED)* | +13 | 4.2 table 13 | reserved; no constant in `tag.rs` | constant undefined (preserved through Opaque) | `tag.rs` (undefined) |
| 0x1E | COMPATIBLE_DOCUMENT | +14 | 4.2.14 | compatible document; a root, unparsed (the writer synthesizes it separately) | Opaque | `doc_info.rs:57` |
| 0x1F | LAYOUT_COMPATIBILITY | +15 | 4.2.15 | layout compatibility; a root, unparsed | Opaque | `doc_info.rs:57` |
| 0x20 | TRACKCHANGE | +16 | 4.2 table 13 | change tracking information; a root, unparsed | Opaque | `doc_info.rs:57` |
| 0x5C | MEMO_SHAPE | +76 | 4.2 table 13 | memo shape; an ID_MAPPINGS child, unparsed | Opaque | `doc_info.rs:148` |
| 0x5E | FORBIDDEN_CHAR | +78 | 4.2 table 13 | forbidden characters; unparsed | Opaque | `doc_info.rs:57`/`:148` |
| 0x60 | TRACK_CHANGE | +80 | 4.2 table 13 | change tracking content and shape; an ID_MAPPINGS child, unparsed | Opaque | `doc_info.rs:148` |
| 0x61 | TRACK_CHANGE_AUTHOR | +81 | 4.2 table 13 | change tracking author; an ID_MAPPINGS child, unparsed | Opaque | `doc_info.rs:148` |

### 3.1 The ID_MAPPINGS count array and language slot assignment

The ID_MAPPINGS (0x11) payload is a **u32 count array**, and the actual table entries follow as child
records. The count order (per the specification): `[binData, fonts ×7 (per language), border fill,
character shape, tab, numbering, bullet, paragraph shape, style, (memo shape, change tracking, change
tracking author, ...)]`. Indices 1 to 8 are the per-language font counts (`font_counts[0..7]`,
`doc_info.rs:42`).

**FACE_NAME language slot assignment** (`doc_info.rs:79`): a font record carries no language marker.
A `font_cursor` is kept, and when the number of fonts filled for the current slot reaches
`font_counts[cursor]` it advances to the next language slot (Korean, English, Chinese, Japanese,
other, symbol, user). The assignment is **derived from the counts**.

★ The render template parsing of NUMBERING (`parse_numbering_levels`, `doc_info.rs:401`) validates
structural consistency by checking whether each level's character shape reference is `0xFFFFFFFF`
("none" in genuine files), falling back to defaults from that level onward otherwise. It is a
render-only path reverse-engineered from genuine bytes rather than relying on the specification.

---

## 4. Body (BodyText) record catalog (table B)

Records that appear in a `/BodyText/SectionN` stream. A section root is **a sequence of PARA_HEADER
trees**, and `parse_section` (`body_text.rs:23`) warns about and Opaque-preserves any root that is not
a PARA_HEADER. Twenty-nine tag constants belong here (excluding MEMO_SHAPE, FORBIDDEN_CHAR,
TRACK_CHANGE and TRACK_CHANGE_AUTHOR, which moved to table A).

| Tag id (hex) | Name (HWPTAG_*) | Value (BEGIN+n) | Spec § | Payload summary | hwp-cli status | Code evidence |
|---|---|---|---|---|---|---|
| 0x42 | PARA_HEADER | +50 | 4.3.1 | a 22B prefix (nchars u32, ctrl_mask, paragraph and character shape, break, counts ×3, instance) + tail | partial + raw | `body_text.rs:92` |
| 0x43 | PARA_TEXT | +51 | 4.3.2 | a WCHAR array decomposed into ordinary characters, surrogates and controls (§5) | semantic | `body_text.rs:114` |
| 0x44 | PARA_CHAR_SHAPE | +52 | 4.3.3 | repeated 8B of (pos u32, charShapeId u32) | semantic | `body_text.rs:60` |
| 0x45 | PARA_LINE_SEG | +53 | 4.3.4 | 36B per line (text_start, v_pos, two heights, baseline, spacing, col, width, flags) | semantic | `body_text.rs:196` |
| 0x46 | PARA_RANGE_TAG | +54 | 4.3.5 | range tags (change tracking and so on); unparsed | Opaque | `body_text.rs:73` |
| 0x47 | CTRL_HEADER | +55 | 4.3.6 | a reversed 4B ctrl_id + the rest of the payload, dispatched per ctrl_id (table D) | partial + raw | `body_text.rs:253` |
| 0x48 | LIST_HEADER | +56 | 4.3.7 | a table cell has a 34B prefix (paragraph count, attributes, row/column/span, size, margins, borders) + tail; otherwise header_data raw | partial + raw | `body_text.rs:452`, `:585` |
| 0x49 | PAGE_DEF | +57 | 4.3.10.1.1 | 40B (paper W/H, six margins, binding margin, attr) | semantic | `body_text.rs:364` |
| 0x4A | FOOTNOTE_SHAPE | +58 | 4.3.10.1.2 | foot/endnote shape; a secd child, unparsed | Opaque | `body_text.rs:357` |
| 0x4B | PAGE_BORDER_FILL | +59 | 4.3.10.1.3 | page border and background; a secd child, unparsed | Opaque | `body_text.rs:357` |
| 0x4C | SHAPE_COMPONENT | +60 | 4.3.9.2.1 | object element (CHID, transform matrix, border and fill); the renderer parses it, the IR keeps raw | raw-preserved | `body_text.rs:608`; rendering in `shape_draw.rs` |
| 0x4D | TABLE | +61 | 4.3.9.1 | attr, rows, columns, spacing, four inner margins + ★ per-row cell counts u16 × rows + border id + tail | partial + raw | `body_text.rs:436` |
| 0x4E | SHAPE_COMPONENT_LINE | +62 | 4.3.9.2.2 | line: start and end points i32; parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x4F | SHAPE_COMPONENT_RECTANGLE | +63 | 4.3.9.2.3 | rectangle: curvature u8 + four points; parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x50 | SHAPE_COMPONENT_ELLIPSE | +64 | 4.3.9.2.4 | ellipse: attr + center and the two axis endpoints; parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x51 | SHAPE_COMPONENT_ARC | +65 | 4.3.9.2.6 | arc: arctype + center, start and end; parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x52 | SHAPE_COMPONENT_POLYGON | +66 | 4.3.9.2.5 | polygon: point count + point array; parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x53 | SHAPE_COMPONENT_CURVE | +67 | 4.3.9.2.7 | curve: point count + point array (approximated as a polyline); parsed by the renderer | raw-preserved | `shape_draw.rs` (render) |
| 0x54 | SHAPE_COMPONENT_OLE | +68 | 4.3.9.5 | OLE object; unparsed (rendering unsupported) | Opaque | `body_text.rs:617` |
| 0x55 | SHAPE_COMPONENT_PICTURE | +69 | 4.3.9.4 | picture: only the u16 at offset 71 (the BinItem id) is extracted, the rest raw-preserved | partial + raw | `body_text.rs:318` |
| 0x56 | SHAPE_COMPONENT_CONTAINER | +70 | 4.3.9.7 | grouped object; the renderer recurses into children, the IR keeps raw | raw-preserved | `shape_draw.rs` (render) |
| 0x57 | CTRL_DATA | +71 | 4.3.8 | arbitrary control data (a field name Parameter Set and so on); only the BSTR is read on demand | raw-preserved | `field.rs:189` |
| 0x58 | EQEDIT | +72 | 4.3.9.3 | equation: attr(4) + len(2) + WCHAR[len] script parsed for rendering, raw preserved | partial + raw | `body_text.rs:536` |
| 0x5A | SHAPE_COMPONENT_TEXTART | +74 | 4.3.9 (word art) | word art; unparsed | Opaque | `body_text.rs:617` |
| 0x5B | FORM_OBJECT | +75 | 4.3.9 (forms) | form object; unparsed | Opaque | `body_text.rs:617` |
| 0x5D | MEMO_LIST | +77 | 4.3 (memos) | memo list; unparsed | Opaque | `body_text.rs:617` |
| 0x5F | CHART_DATA | +79 | 4.3.9.6 | chart object; unparsed | Opaque | `body_text.rs:617` |
| 0x62 | VIDEO_DATA | +82 | 4.3.9.8 | video object; unparsed | Opaque | `body_text.rs:617` |
| 0x73 | SHAPE_COMPONENT_UNKNOWN | +99 | *(none)* | unknown object; unparsed | Opaque | `body_text.rs:617` |

### 4.1 The object subtree pattern (CTRL_HEADER → SHAPE_COMPONENT → LIST_HEADER)

One extended control is a logical bundle of several records. The representative patterns:

```
CTRL_HEADER(gso )                         ← entering a drawing object, reversed ctrl_id
└── SHAPE_COMPONENT                        ← object element (transform matrix, style)
    ├── SHAPE_COMPONENT_PICTURE            ← for a picture, the BinItem id (offset 71)
    └── LIST_HEADER                        ← for a text box, entering the paragraph list
        └── PARA_HEADER ...                ← paragraphs inside the object (recursive)

CTRL_HEADER(tbl )                          ← table
├── TABLE                                  ← rows, columns and ★ per-row cell counts
├── LIST_HEADER                            ← opens cell 1
│   └── PARA_HEADER ...                    ← cell 1 paragraphs
└── LIST_HEADER ...                        ← cell 2 ... (listed as siblings; the next LIST_HEADER is the cell boundary)

CTRL_HEADER(secd)                          ← section definition
├── PAGE_DEF                               ← paper (semantic)
├── FOOTNOTE_SHAPE / PAGE_BORDER_FILL      ← Opaque
```

The reader collects paragraphs inside table cells and text boxes recursively with
`collect_paragraph_lists` (`body_text.rs:578`). GenericControl uses that flattened `paragraph_lists`
only for text extraction; **lossless re-serialization uses the original nested subtree
`raw_children`** (preventing flattening loss, `control.rs:202`).

**★ Where our measurements differ from the specification:**

- **The TABLE "Row Size" array**: contrary to the specification's wording, it measures as **the cell
  count per row, not the row height** (`row_cell_counts`, `body_text.rs:443`, `control.rs:161`).
- **The cell LIST_HEADER is 34B**: that is the length of the semantically parsed prefix
  (`parse_cell_header` fields summing to 4+4+2+2+2+2+4+4+8+2, with the rest in `header_tail` raw).
  The old "46B" note confused it with the 46B length of the object common properties in table 69;
  corrected in the 2026-07-19 exhaustive audit.
- **The drawing object border line information is 13B**: specification table 86 declares 11B with an
  INT16 line thickness, but measurement shows a 4B (INT32) thickness for 13B total
  (`shape_draw.rs:393,414-415`; added to the ★ list in the 2026-07-19 audit).
- **ctrl_id is stored reversed**: the first four bytes of a CTRL_HEADER/ExtCtrl payload are reversed
  (`dces` → `secd`) and flipped on read (`body_text.rs:268`).
- **CHAR_SHAPE strikethrough bits (18 to 20)**: because their meaning is DIFFSPEC they are not
  trusted, and `strike:false` is fixed (preventing false strikethrough, `doc_info.rs:249`).

---

## 5. Control character (0 to 31) classification (table C)

When consuming the WCHAR array of PARA_TEXT, **the classification of codes 0 to 31 is the basis of
all position arithmetic**. Miscounting even one 8-WCHAR control throws off every later position
calculation (`nchars == Σ wchar_width`). The single source of truth is `char_kind`
(`paragraph.rs:27`), the names live in the `ctrl_char` module (`paragraph.rs:41`), and text extraction
handling is in `text.rs:44`. The specification basis is **table 6 (control characters) in §3.2.3**
(the §4.2.4 in code comments and the §4.3.2 in earlier editions of this document are both typos,
settled in Phase 2 audit C7 and C8, 2026-07-18).

| Code | Class | WCHAR width | Well-known meaning | Text extraction |
|---|---|---|---|---|
| 0 | Char | 1 | unused/separator | discarded |
| 1 | Extended | 8 | reserved | control dispatch |
| 2 | Extended | 8 | section/column definition (secd/cold) | SectionDef/ColumnDef |
| 3 | Extended | 8 | field start (%clk and so on) | field control |
| 4 | Inline | 8 | field end | discarded (the field value boundary) |
| 5 | Inline | 8 | reserved | discarded |
| 6 | Inline | 8 | reserved | discarded |
| 7 | Inline | 8 | reserved | discarded |
| 8 | Inline | 8 | title mark | discarded |
| 9 | Inline | 8 | tab | `\t` |
| 10 | Char | 1 | line break | `\n` |
| 11 | Extended | 8 | drawing object/table (gso/tbl) | Table/Picture/Generic |
| 12 | Extended | 8 | reserved | control dispatch |
| 13 | Char | 1 | paragraph end | a paragraph boundary newline |
| 14 | Extended | 8 | reserved | control dispatch |
| 15 | Extended | 8 | hidden comment | excluded by default (`include_hidden`) |
| 16 | Extended | 8 | header/footer (head/foot) | excluded by default (`include_header_footer`) |
| 17 | Extended | 8 | foot/endnote (fn/en) | included |
| 18 | Extended | 8 | automatic number (atno) | included |
| 19 | Inline | 8 | reserved | discarded |
| 20 | Inline | 8 | reserved | discarded |
| 21 | Extended | 8 | page controls (pgnp/pghd/nwno) | included |
| 22 | Extended | 8 | bookmark/index mark (bokm) | included |
| 23 | Extended | 8 | ruby text/overlapping characters | included |
| 24 | Char | 1 | hyphen | `-` |
| 25 | Char | 1 | reserved | discarded |
| 26 | Char | 1 | reserved | discarded |
| 27 | Char | 1 | reserved | discarded |
| 28 | Char | 1 | reserved | discarded |
| 29 | Char | 1 | reserved | discarded |
| 30 | Char | 1 | grouped space | ` ` |
| 31 | Char | 1 | fixed-width space | ` ` |

Widths per class: **Char = 1 WCHAR**, **Inline = 8 WCHAR** (`[code, six WCHAR of information, code]`,
self-contained), **Extended = 8 WCHAR** (pointing at a separate CTRL_HEADER whose payload begins with
the reversed 4B ctrl_id). `ctrl_mask` (PARA_HEADER offset 4) is only a hint; the reader walks the
actual PARA_TEXT and counts the controls.

---

## 6. Extended control ctrl id catalog (table D)

Extended controls pointed at by a CTRL_HEADER (the Extended codes in §5), looked up by their
**forward ctrl_id**. The hwp5 reader semantically parses only `secd`, `tbl ` and `gso ` (pictures) in
`parse_control` (`body_text.rs:253`), collecting paragraph lists as a GenericControl for the rest.
hwpx maps OWPML elements to the same ctrl_id and code in `section.rs`, so both formats mean the same
thing in the IR.

### 6.1 Structure, object and section controls

| ctrl id | Name | Spec § | hwp-cli status | hwpx element | Code evidence |
|---|---|---|---|---|---|
| `secd` | section definition | 4.3.10.1 | partial + raw (PAGE_DEF semantic, the rest raw/Opaque) | `hp:secPr` | `body_text.rs:338`, `section.rs:136` |
| `cold` | column definition | 4.3.10.2 | partial (ColumnDef, for rendering) | `hp:ctrl > hp:colPr` | `body_text.rs:555`, `section.rs:377` |
| `tbl ` | table | 4.3.9.1 | semantic (TABLE plus cells) | `hp:tbl` | `body_text.rs:380`, `section.rs:691` |
| `gso ` | drawing object (common) | 4.3.9 | picture = partial (Picture), otherwise raw (rendered) | `hp:pic` / `hp:rect`, `ellipse`, `line`, `arc`, `polygon`, `curve` | `body_text.rs:309`, `section.rs:178`, `995` |
| `eqed` | equation | 4.3.9.3 | partial (script, typeset for rendering) | `hp:equation` | `body_text.rs:517`, `section.rs:1130` |
| `head` | header | 4.3.10.3 | Generic (paragraphs collected, 8B payload) | `hp:ctrl > hp:header` | `section.rs:399`, `588` |
| `foot` | footer | 4.3.10.3 | Generic (paragraphs collected, 8B payload) | `hp:ctrl > hp:footer` | `section.rs:399`, `588` |
| `fn  ` | footnote | 4.3.10.4 | Generic (paragraphs collected) | `hp:ctrl > hp:footNote` | `section.rs:589` |
| `en  ` | endnote | 4.3.10.4 | Generic (paragraphs collected) | `hp:ctrl > hp:endNote` | `section.rs:590` |
| `atno` | automatic number | 4.3.10.5 | Generic (12B payload synthesized) | `hp:ctrl > hp:autoNum` | `section.rs:465`, `593` |
| `nwno` | new number | 4.3.10.6 | Generic (6B payload synthesized) | `hp:ctrl > hp:newNum` | `section.rs:475`, `596` |
| `pghd` | page hiding | 4.3.10.7 | Generic (4B bitmap payload synthesized) | `hp:ctrl > hp:pageHiding` | `section.rs:446`, `595` |
| `pgnp` | page number position | 4.3.10.9 | Generic (12B payload synthesized) | `hp:ctrl > hp:pageNum` | `section.rs:415`, `594` |
| `bokm` | bookmark | 4.3.10.11 | Generic (name in CTRL_DATA) | `hp:ctrl > hp:bookmark` | `section.rs:562` |

*Odd/even adjustment (§4.3.10.8), index marks (§4.3.10.10), overlapping characters (§4.3.10.12) and
ruby text (§4.3.10.13) pass through as Generic without separate semantic parsing. For an unknown
ctrl_id, `section.rs:597` uses the first four bytes of the element name as the ctrl_id and emits code
21.*

### 6.2 Field controls (field start, §4.3.10.15)

A field is `FIELD_START` (character code 3, Extended), then the display text, then `FIELD_END`
(code 4). The kind is distinguished by ctrl_id and maps both ways to the hwpx `fieldBegin type`
attribute. All twelve kinds (`field.rs:37`, `56`, `91`):

| ctrl id | Name | OWPML `fieldBegin type` | Code evidence |
|---|---|---|---|
| `%clk` | click here | `CLICK_HERE` | `field.rs:57`, `92` |
| `%fmu` | formula | `FORMULA` | `field.rs:58`, `93` |
| `%hlk` | hyperlink | `HYPERLINK` | `field.rs:59`, `94` |
| `%mmg` | mail merge | `MAIL_MERGE` | `field.rs:60`, `95` |
| `%dte` | date | `DATE` | `field.rs:61`, `96` |
| `%ddt` | document date | `DOCUMENT_DATE` | `field.rs:62`, `97` |
| `%xrf` | cross reference | `CROSS_REF` | `field.rs:63`, `98` |
| `%bmk` | bookmark (field) | `BOOKMARK` | `field.rs:64`, `99` |
| `%pat` | file path | `PATH` | `field.rs:65`, `100` |
| `%smr` | document summary | `SUMMARY` | `field.rs:66`, `101` |
| `%usr` | user information | `USER_INFO` | `field.rs:67`, `102` |
| `%unk` | unknown | `UNKNOWN` | `field.rs:68`, `103` |

**hwp-cli status**: every kind is parsed on demand (reading name, command and value; the IR is
unchanged, `field.rs:110`). Creation and editing support `%clk` (click here), `%hlk` (hyperlink) and
`%bmk`/`bokm` (bookmark). hwpx reads them as `hp:fieldBegin` (with the child
`hp:parameters > stringParam name="Command"`) plus `hp:fieldEnd` (`section.rs:516`). ★ Measured on
genuine files: `%hlk` needs a **non-zero** command record id for Hancom to recognize a hyperlink, and
the FIELD_END payload must carry the reversed 3B ctrl_id (without `%`) for the pair to close
(`field.rs:423`, `476`).

---

## 7. Specification § to code index

Cross-references by major topic. "Responsible code" is the entry function for that topic.

| Spec § | Topic | Responsible code (file:function) | This document |
|---|---|---|---|
| 3.1 to 3.2 | File and storage structure | `container.rs:list_streams`, `body_sections`, `is_record_stream` | §1 |
| 3.2.1 / 4.1 | File recognition information / record structure | `file_header.rs:parse`, `record/header.rs` | §1, §2 |
| 4.1 | Record header bit packing | `record/header.rs`, `record/scan.rs`, `record/tree.rs` | §2, [02] §5, §6 |
| 4.2.1 | Document properties | `doc_info.rs:parse_document_properties` | table A |
| 4.2.2 | Id mapping header | `doc_info.rs:parse_doc_info` (the ID_MAPPINGS branch) | table A §3.1 |
| 4.2.3 to 4.2.11 | DocInfo table entries | `doc_info.rs:parse_id_mapping_child` | table A |
| 4.2.6 | Character shape (★ strikethrough bits) | `doc_info.rs:parse_char_shape` | table A |
| 4.3.1 to 4.3.4 | Paragraph header, text, character shape, layout | `body_text.rs:parse_paragraph` | table B |
| 4.3.2 | Control character classification | `paragraph.rs:char_kind`, `text.rs:extract_into` | §5 |
| 4.3.6 | Control header (reversed ctrl_id) | `body_text.rs:parse_control` | §4.1, §6 |
| 4.3.9.1 | Table object (★ per-row cell counts) | `body_text.rs:parse_table` | table B, §4.1 |
| 4.3.9.2.* | Drawing object geometry | `hwp-render/src/shape_draw.rs` | table B, [02] §10 |
| 4.3.9.3 | Equation object | `body_text.rs:parse_eqed`, `find_eqedit_script` | table B, §6.1 |
| 4.3.9.4 | Picture object (BinItem id) | `body_text.rs:parse_picture_gso` | table B |
| 4.3.10.1.1 | Paper setup | `body_text.rs:parse_page_def` | table B |
| 4.3.10.2 to 4.3.10.11 | Non-object controls (columns, headers, footnotes, page, bookmarks) | `section.rs` (hwpx synthesis), `body_text.rs:parse_generic` | §6.1 |
| 4.3.10.15 | Fields | `hwp-convert/src/field.rs`, `section.rs:parse_ctrl` | §6.2 |

---

## 8. Completeness summary (input to gap document 12)

**Rows marked `Opaque` in tables A and B** (no interpreting consumer; round-trip preservation only):

- DocInfo: `DOC_DATA`, `DISTRIBUTE_DOC_DATA`, `COMPATIBLE_DOCUMENT`, `LAYOUT_COMPATIBILITY`,
  `TRACKCHANGE`, `MEMO_SHAPE`, `FORBIDDEN_CHAR`, `TRACK_CHANGE`, `TRACK_CHANGE_AUTHOR`
- Body: `PARA_RANGE_TAG`, `FOOTNOTE_SHAPE`, `PAGE_BORDER_FILL`, `SHAPE_COMPONENT_OLE`,
  `SHAPE_COMPONENT_TEXTART`, `FORM_OBJECT`, `MEMO_LIST`, `CHART_DATA`, `VIDEO_DATA`,
  `SHAPE_COMPONENT_UNKNOWN`

**Rows marked `raw-preserved`** (preserved without interpretation but with a consumer or re-emission
slot):

- DocInfo: `TAB_DEF`, `BULLET`
- Body: `SHAPE_COMPONENT`, `SHAPE_COMPONENT_LINE`, `RECTANGLE`, `ELLIPSE`, `ARC`, `POLYGON`, `CURVE`
  and `SHAPE_COMPONENT_CONTAINER` (all parsed geometrically by the renderer), plus `CTRL_DATA`

**`constant undefined`**: `RESERVED` (0x1D / BEGIN+13), which has no constant in `tag.rs` and is
preserved through Opaque.

These are the subject of document 12 (feature gaps). `Opaque` means entirely uninterpreted (yet
guaranteed to round-trip losslessly), while `raw-preserved` means the render consumer parses it
partially but it never reaches the IR as a semantic field.
