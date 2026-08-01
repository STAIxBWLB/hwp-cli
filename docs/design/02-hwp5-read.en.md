[한국어](02-hwp5-read.md) · [English](02-hwp5-read.en.md)

# The HWP 5.0 binary reader, specified for a rebuild

This describes the read path of `crates/hwp5/src/` in enough detail to reimplement it from scratch.
The files: `read.rs`, `container.rs`, `file_header.rs`, `codec/{reader,writer,compress}.rs`,
`record/{header,scan,tree,tag}.rs`, `doc_info.rs`, `body_text.rs`, `summary.rs`, plus the
consumer-side geometry parser `crates/hwp-render/src/shape_draw.rs` and the character classification
in `crates/hwp-model/src/paragraph.rs`.

Two design principles run through every layer. 1. **Separating scanning from interpretation**: the
`record` layer knows nothing about tag meaning and handles only `(tag, level, data)`, while semantic
parsing belongs to `doc_info` and `body_text`. 2. **Known prefix plus preserved tail**: every record
parser decodes only the known leading part into a struct and preserves the remaining bytes verbatim in
`tail: Vec<u8>`. HWP is a forward-compatible format that appends fields at the tail as versions
advance, so this rule gives both a lossless round-trip and tolerance of future versions.

---

## 1. The whole pipeline

The order in `read_document(path) -> ReadResult { document, warnings }` (`read.rs`):

1. `Hwp5Container::open(path)`: open the CFB and parse and validate the FileHeader.
2. `container.check_body_readable()`: fail immediately when encrypted or a distribution document.
3. Read `/DocInfo` (decompressed) → `scan_stream(Tolerant)` → `parse_doc_info()` → `DocHeader`.
4. For each `/BodyText/SectionN` from `body_sections()`: read (decompressed) → `scan_stream` →
   `parse_section()` → `Section`.
5. `/BinData/*` streams: when the FileHeader compression flag is set, try raw deflate and fall back to
   the original on failure → `BinStream`.
6. `/\x05HwpSummaryInformation`: `parse_summary()` (proceeding with defaults when absent or damaged).
7. Assemble `Document { meta, metadata, header, sections, bin_streams }`.

To cope with files in the wild, scanning and parsing run in **Tolerant mode**, accumulating failures
as `warnings` and preserving them opaquely (diagnosis never stops). Only writer validation and
round-trip tests use **Strict mode**.

---

## 2. The CFB container and the stream list

An HWP 5.0 file is an MS **CFB (Compound File Binary, formerly OLE2 compound document)** container.
This code delegates parsing to the `cfb` crate (`cfb::CompoundFile<File>`) and adds only a thin
wrapper (`Hwp5Container`). When reimplementing, using a library for CFB itself (512-byte sectors,
FAT/DIFAT/mini-FAT, the directory entry tree) is realistic; only the stream path conventions below
must be followed.

`list_streams()` enumerates every stream with `cfb.walk()` and returns a list of
`StreamInfo { path, size }` sorted by path. Paths are absolute and start with `/`
(`/BodyText/Section0`).

| Stream path | Contents | Compressed | Parser |
|---|---|---|---|
| `/FileHeader` | the fixed 256B header (signature, version, attributes) | no | `file_header.rs` |
| `/DocInfo` | the document information record stream (font, shape and style tables) | yes | `doc_info.rs` |
| `/BodyText/Section0..N` | the body section record streams | yes | `body_text.rs` |
| `/ViewText/Section0..N` | distribution document body (encryption related) | yes | unsupported (error) |
| `/BinData/*` | attached binaries (images such as `BIN0001.png`) | per the header flag | try and fall back |
| `/\x05HwpSummaryInformation` | the OLE property set (title, author, ...) | no | `summary.rs` |
| `/PrvText` | preview text (UTF-16LE) | no | - |
| `/PrvImage` | preview image (PNG/BMP) | no | - |
| `/Scripts/*` | JScriptVersion, DefaultJScript and so on | yes (record stream rule) | - |
| `/DocOptions/*` | document options such as `_LinkDoc` | no | - |

**Enumerating body sections** `body_sections()`: streams starting with `/BodyText/Section` are sorted
numerically by their suffix (string sorting would put `Section10` before `Section2`, so
`parse::<u32>()` before sorting is mandatory). The section count should match
`DocumentProperties.section_count`, but the reader trusts the streams that actually exist.

**Deciding what is compressed** `is_record_stream(path)` (`container.rs:114`): only streams starting
with `/DocInfo`, `/BodyText/`, `/ViewText/` or `/Scripts/` are subject to the FileHeader compression
flag. FileHeader, PrvText, PrvImage, BinData and the summary information are excluded (BinData gets
its own try-and-fall-back on the read path).

**The distribution and encryption guard** `check_body_readable()`: `is_encrypted()` gives
`Hwp5Error::Encrypted` and `is_distribution()` gives `Hwp5Error::DistributionDoc`, failing clearly
before any body access.

---

## 3. The FileHeader (a fixed 256 bytes)

The `/FileHeader` stream must be exactly 256 bytes (`FILE_HEADER_SIZE`); otherwise
`BadFileHeaderSize`. It is parsed sequentially with a little-endian `ByteReader`.

| Offset | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 32 | bytes | signature | the first 17 bytes must equal `"HWP Document File"`, the rest NUL padding |
| 32 | 4 | u32 LE | version | `0xMMnnPPrr`, so `0x05000300` is 5.0.3.0 |
| 36 | 4 | u32 LE | attribute flags | see the bit table below |
| 40 | 4 | u32 LE | license flags | CCL / KOGL |
| 44 | 4 | u32 LE | EncryptVersion | the encryption version |
| 48 | 1 | u8 | KOGL country code | the KOGL supported country |
| 49 | 207 | bytes | reserved | kept as-is for the round-trip |

**Version encoding** `HwpVersion::from_u32`: `major=v>>24`, `minor=v>>16 & 0xFF`,
`build=v>>8 & 0xFF`, `revision=v & 0xFF`, displayed as `"5.0.3.0"`.

**Attribute flag bits** (the DWORD at offset 36, `file_header.rs::attr`):

| Bit | Constant | Meaning |
|---|---|---|
| 0 | COMPRESSED | compressed (raw deflate on record streams) |
| 1 | ENCRYPTED | encrypted |
| 2 | DISTRIBUTION | a distribution document (ViewText) |
| 3 | HAS_SCRIPT | scripts stored |
| 4 | DRM | DRM protected |
| 5 | HAS_XML_TEMPLATE | an XMLTemplate storage |
| 6 | HAS_HISTORY | document history management |
| 7 | HAS_SIGNATURE | a digital signature |
| 8 | CERT_ENCRYPTED | certificate encryption |
| 9 | SIGNATURE_SPARE | digital signature spare |
| 10 | CERT_DRM | certificate DRM |
| 11 | CCL | a CCL document |
| 12 | MOBILE_OPTIMIZED | mobile optimized |
| 13 | PRIVACY_SECURITY | personal information security |
| 14 | TRACK_CHANGES | change tracking |
| 15 | KOGL | KOGL copyright |
| 16 | HAS_VIDEO_CONTROL | a video control |
| 17 | HAS_TOC_FIELD | a table-of-contents field control |

Only `COMPRESSED` (bit0), `ENCRYPTED` (bit1) and `DISTRIBUTION` (bit2) actually drive branches on
read. The rest are shown by `attribute_names()` for `hwp info`.

---

## 4. Compression (raw deflate) and encoding (UTF-16LE)

**Compression** (`codec/compress.rs`): compressed record streams use **raw DEFLATE with no zlib
header and no Adler32 checksum** (the same as pyhwp's `wbits=-15`). Decompression uses
`flate2::read::DeflateDecoder` (pure deflate, not the zlib wrapper) with `read_to_end`, failing as
`Hwp5Error::Decompress { stream, source }`. When reimplementing, do not mistake it for a zlib stream
and consume the first two bytes as a header: the deflate blocks start at the very first byte. BinData
can differ per record, so the read path uses `decompress(...).unwrap_or(raw)`.

**Encoding**: all text and strings are **UTF-16LE**. `ByteReader::read_wchars(n)` reads `n` u16 code
units. An HWP string (`read_hwp_string`) is **`WORD length (in code units) + UTF-16LE data`**, and the
length excludes a terminating NUL (the LPWSTR in the summary information uses a count that includes
the NUL; see §11). Decoding uses `String::from_utf16_lossy` for damage tolerance. Body text handles
surrogate pairs directly (§8).

---

## 5. The record header bit layout

A decompressed DocInfo, BodyText or Scripts stream is **a sequence of records**, each a 4-byte (or
8-byte) header plus a payload. The header bit-packs three fields into a single u32 LE
(`record/header.rs`):

```
u32 LE = tagID (low 10 bits) | level (next 10 bits) | size (top 12 bits)
```

| Bit range | Field | Mask/shift | Width |
|---|---|---|---|
| 0..10 | tagID | `v & 0x3FF` | 10 bits (0 to 1023) |
| 10..20 | level | `(v >> 10) & 0x3FF` | 10 bits (the tree depth) |
| 20..32 | size | `(v >> 20) & 0xFFF` | 12 bits (0 to 4095, the payload byte count) |

**The extended size**: when the `size` bit field is `0xFFF` (`SIZE_EXTENDED`), **the next u32 LE is
the real size**, so a header is either 4 or 8 bytes. Mind the boundary rule: `0xFFF` itself cannot be
expressed inline (that value is reserved as the extension marker), so **anything with `size >= 0xFFF`
is always written and read as the extended form**. The inline maximum is therefore `0xFFE` (4094).
Decoding pseudocode:

```
v = read_u32()
tag   = v & 0x3FF
level = (v >> 10) & 0x3FF
sf    = (v >> 20) & 0xFFF
size  = if sf == 0xFFF { read_u32() } else { sf }
payload = read_bytes(size)
```

Tags are **always preserved as a raw u16**. They are never coerced into an enum, so unknown tags pass
through without information loss. `tag::tag_name(u16)` is only a name lookup for dumps and plays no
part in parsing branches.

---

## 6. Scanning and tree reconstruction

### 6.1 The flat scan (`record/scan.rs`)

`scan_stream(data, mode) -> ScanResult { roots, warnings, record_count }`:

```
while !r.is_empty():
    at = r.pos()
    header = RecordHeader::decode(r)?      # Tolerant: on failure, warn and break
    payload = r.read_bytes(header.size)    # Tolerant: when short, preserve what remains and warn
    flat.push((header, payload))
(roots, tree_warnings) = RecordNode::build_forest(flat)
```

- **Strict**: any truncated header, short payload or tree warning returns `Err` immediately. Used for
  writer round-trip verification.
- **Tolerant**: a truncated header warns "scan aborted" then breaks; a short payload preserves the
  remaining bytes truncated, with a warning. Used only on the read path.

### 6.2 Tree reconstruction (`record/tree.rs`)

A record's `level` field is the tree depth, reconstructed with a stack (`build_forest`):

```
stack: Vec<RecordNode>   # stack[i] is the node open at depth i
for (hdr, data) in flat:
    level = hdr.level
    if level > stack.len():           # deepened without a parent (damage)
        warn; level = stack.len()      # attach to the nearest ancestor
    while stack.len() > level:         # close deeper nodes, attaching to the parent or roots
        attach(stack.pop())
    stack.push(RecordNode{tag, data, children:[]})
attach everything left on the stack
```

`RecordNode { tag: u16, data: Vec<u8>, children: Vec<RecordNode> }`. **The invariant**: for a
well-formed file, `serialize_forest` (recomputing level from depth) reproduces **the decompressed
stream byte for byte**, which is the foundation of the lossless round-trip. Damage where level jumps
non-monotonically attaches to the nearest ancestor with only a warning.

---

## 7. DocInfo parsing (`doc_info.rs`)

`parse_doc_info(roots) -> (DocHeader, warnings)` walks the root records and interprets only two
directly: `DOCUMENT_PROPERTIES` (0x10) and `ID_MAPPINGS` (0x11). Every other root is preserved
opaquely in `header.extras`.

### 7.1 Tag constants (`record/tag.rs`)

`HWPTAG_BEGIN = 0x010`. The DocInfo family is `BEGIN + n`:

| Tag | Value | Name | Tag | Value | Name |
|---|---|---|---|---|---|
| +0 | 0x10 | DOCUMENT_PROPERTIES | +7 | 0x17 | NUMBERING |
| +1 | 0x11 | ID_MAPPINGS | +8 | 0x18 | BULLET |
| +2 | 0x12 | BIN_DATA | +9 | 0x19 | PARA_SHAPE |
| +3 | 0x13 | FACE_NAME | +10 | 0x1A | STYLE |
| +4 | 0x14 | BORDER_FILL | +11 | 0x1B | DOC_DATA |
| +5 | 0x15 | CHAR_SHAPE | +14 | 0x1E | COMPATIBLE_DOCUMENT |
| +6 | 0x16 | TAB_DEF | +15 | 0x1F | LAYOUT_COMPATIBILITY |

### 7.2 DOCUMENT_PROPERTIES (0x10), 26 bytes

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | section_count |
| 2 | u16 ×6 | start_numbers (page, footnote, endnote, picture, table, equation) |
| 14 | u32 ×3 | the caret position (list_id, para_id, char_pos) |

### 7.3 ID_MAPPINGS (0x11): the count array plus child tables

The payload is a **u32 count array**, read to the end and preserved as `id_mappings_counts`. The order
(per the specification): `[binData, fonts ×7 (per language), border fill, character shape, tab,
numbering, bullet, paragraph shape, style, (memo shape, change tracking, change tracking author,
...)]`. Indices 1 to 8 are the font counts per language slot (Korean, English, Chinese, Japanese,
other, symbol, user) = `font_counts[0..7]`.

**The actual table entries are listed as children of ID_MAPPINGS.** `parse_id_mapping_child`
classifies them by tag, following the order of the count array. **FACE_NAME language slot
assignment**: a `font_cursor` advances to the next language slot when the fonts filled for the current
slot reach `font_counts[cursor]` (font records carry no language marker, so it is derived from the
counts).

Each child parser follows the prefix-plus-tail rule (pushing a default plus an opaque record on
failure):

**FACE_NAME (0x13)**, variable length:

| Offset | Type | Field | Condition |
|---|---|---|---|
| 0 | u8 | attr | bit7 = substitute font, bit6 = PANOSE, bit5 = default font |
| 1 | HWP str | name | the font name |
| ... | u8 + HWP str | alt_kind + alt_name | attr & 0x80 |
| ... | 10 bytes | panose | attr & 0x40 |
| ... | HWP str | default_name | attr & 0x20 |
| ... | tail | the rest preserved | |

**CHAR_SHAPE (0x15)**, a 68-byte prefix plus tail:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 ×7 | face_ids (font id per language) |
| 14 | u8 ×7 | ratios (width scaling) |
| 21 | i8 ×7 | spacings (letter spacing) |
| 28 | u8 ×7 | rel_sizes (relative size) |
| 35 | i8 ×7 | offsets (character position) |
| 42 | i32 | base_size (the base size in HWPUNIT) |
| 46 | u32 | attr (bold, italic, underline, outline and so on) |
| 50 | i8, i8 | shadow_gap (shadow offset x, y) |
| 52 | u32 | text_color (COLORREF) |
| 56 | u32 | underline_color |
| 60 | u32 | shade_color |
| 64 | u32 | shadow_color |
| 68 | tail | tail[0..2] = border_fill_id (5.0.2.1+) |

Note: the strikethrough bits (18 to 20) of the raw `attr` are DIFFSPEC and untrusted (`strike: false`
is fixed, preventing false strikethrough).

**PARA_SHAPE (0x19)**, a 42-byte prefix plus tail:

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | attr1 (bits 0-1 = the line spacing kind) |
| 4 | i32 | margin_left |
| 8 | i32 | margin_right |
| 12 | i32 | indent |
| 16 | i32 | spacing_top |
| 20 | i32 | spacing_bottom |
| 24 | i32 | line_spacing_old (the legacy line spacing) |
| 28 | u16 | tab_def_id |
| 30 | u16 | numbering_id |
| 32 | u16 | border_fill_id |
| 34 | u16 ×4 | border_offsets (left, right, top, bottom) |
| 42 | tail | tail = [attr2 u32, attr3 u32, line_spacing i32] |

Line spacing: when `tail.len()>=12`, `line_spacing = tail[8..12]` (5.0.2.5+); otherwise
`line_spacing_old`. The kind is `attr1 & 0x3`.

**STYLE (0x1A)**: `name (HWP str) + english_name (HWP str) + attr u8 + next_style u8 + lang_id i16 +
para_shape u16 + char_shape u16 + tail`.

**BORDER_FILL (0x14)**: `attr u16` plus four sides (left, right, top, bottom) of
`{line_type u8, width u8, color u32}` (6B each) plus the diagonal (6B) plus `fill_type u32` plus
(`bg_color u32` when `fill_type & 1`) plus tail.

**BIN_DATA (0x12)**: `attr u16`, with `kind = attr & 0xF` (0 link, 1 embedded, 2 storage). For kind 0
there are `link_abs` and `link_rel` (two HWP strings); otherwise `storage_id u16`, plus `extension` (an
HWP string) when kind is 1. Then a tail.

**TAB_DEF (0x16) and BULLET (0x18)**: preserved raw. BULLET extracts the WCHAR at offsets 8 to 10 as
the bullet character, falling back to `•` for a control character.

**NUMBERING (0x17)**: preserved raw plus parsing of the seven-level render templates
(`parse_numbering_levels`). Each level is `attr u32 + width u16 + dist u16 + charshape_ref u32
(0xFFFFFFFF in genuine files) + template (HWP str)`. When `charshape_ref != 0xFFFFFFFF`, it falls back
to defaults from that level. Example: `["^1.","^2.","^3)","^4)","(^5)","(^6)","^7"]`.

`DocHeader` fills `fonts[7][]`, `char_shapes[]`, `para_shapes[]`, `styles[]`, `border_fills[]`,
`bin_data[]`, `numberings[]` and `bullets[]` in appearance order, and each index is the reference id.

---

## 8. BodyText parsing (`body_text.rs`)

`parse_section(roots) -> (Section, warnings)`. A section root is **a sequence of PARA_HEADER trees**.
A root that is not a PARA_HEADER warns and is preserved opaquely in `section.extras`.

The BodyText family tags (`HWPTAG_BEGIN + n`):

| Tag | Value | Name | Tag | Value | Name |
|---|---|---|---|---|---|
| +50 | 0x42 | PARA_HEADER | +60 | 0x4C | SHAPE_COMPONENT |
| +51 | 0x43 | PARA_TEXT | +61 | 0x4D | TABLE |
| +52 | 0x44 | PARA_CHAR_SHAPE | +62 | 0x4E | SHAPE_COMPONENT_LINE |
| +53 | 0x45 | PARA_LINE_SEG | +63 | 0x4F | SHAPE_COMPONENT_RECTANGLE |
| +54 | 0x46 | PARA_RANGE_TAG | +64 | 0x50 | SHAPE_COMPONENT_ELLIPSE |
| +55 | 0x47 | CTRL_HEADER | +65 | 0x51 | SHAPE_COMPONENT_ARC |
| +56 | 0x48 | LIST_HEADER | +66 | 0x52 | SHAPE_COMPONENT_POLYGON |
| +57 | 0x49 | PAGE_DEF | +67 | 0x53 | SHAPE_COMPONENT_CURVE |
| +58 | 0x4A | FOOTNOTE_SHAPE | +69 | 0x55 | SHAPE_COMPONENT_PICTURE |
| +59 | 0x4B | PAGE_BORDER_FILL | +70 | 0x56 | SHAPE_COMPONENT_CONTAINER |

### 8.1 PARA_HEADER (0x42), a 22-byte prefix plus tail

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | nchars_raw (bit31 = a flag, `& 0x7FFFFFFF` = the paragraph WCHAR count) |
| 4 | u32 | ctrl_mask (a bitmask of the control kinds present in the paragraph) |
| 8 | u16 | para_shape_id |
| 10 | u8 | style_id |
| 11 | u8 | break_type (the break bits) |
| 12 | u16 | char_shape_count |
| 14 | u16 | range_tag_count |
| 16 | u16 | line_seg_count |
| 18 | u32 | instance_id |
| 22 | tail | the per-version tail (merged change tracking and so on) preserved |

`chars_flags = (nchars_raw >> 24) & 0x80`, and `nchars = nchars_raw & 0x7FFFFFFF` is used later to
verify position arithmetic.

PARA_TEXT, PARA_CHAR_SHAPE, PARA_LINE_SEG and CTRL_HEADER arrive as children of PARA_HEADER.

### 8.2 PARA_TEXT (0x43): control character classification is the basis of position arithmetic

The payload is read as an array of u16 LE (WCHAR); an odd length ignores the last byte with a warning.
Each unit `u` is consumed by these rules (`decode_para_text`):

- **`u >= 32`**: an ordinary character. A high surrogate (0xD800..0xDC00) pairs with the next unit
  (`decode_utf16`) into one char (2 WCHAR); an unpaired one gives `U+FFFD` plus a warning.
- **`u < 32`**: classified by `char_kind(u)` (§9).
  - `Char` (1 WCHAR): `HwpChar::CharCtrl(u)`, `i += 1`.
  - `Inline` or `Extended` (8 WCHAR): reads the structure `[code u, six WCHAR (12B) of information,
    closing code u]`. The closing code must equal the opening code (a mismatch warns). `i += 8`. The
    six WCHAR of information are preserved as `payload: Vec<u8>`.
    - Inline gives `HwpChar::InlineCtrl { code, payload }`.
    - Extended: the first four bytes of the payload are the **reversed ctrl_id**, so they are flipped
      and stored as `ctrl_id`, giving
      `HwpChar::ExtCtrl { code, ctrl_id, payload, ctrl_index: None }`.

A truncated 8-WCHAR control (`i + 8 > len`) warns and stops.

### 8.3 PARA_CHAR_SHAPE (0x44)

Repeating 8 bytes: `pos u32` (the starting WCHAR position within the paragraph) plus `id u32` (a
CharShapeId, using only the low 16 bits), giving `char_shape_runs: Vec<(u32, CharShapeId)>`.

### 8.4 PARA_LINE_SEG (0x45), 36 bytes per line

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | text_start (the line's starting WCHAR offset) |
| 4 | i32 | v_pos (the vertical position) |
| 8 | i32 | line_height |
| 12 | i32 | text_height |
| 16 | i32 | baseline_gap |
| 20 | i32 | line_spacing |
| 24 | i32 | col_start |
| 28 | i32 | seg_width |
| 32 | u32 | flags (first line of a page or column, empty segment and so on) |

Repeats while `remaining >= 36`. It is the first-class layout input the renderer trusts as-is.

### 8.5 CTRL_HEADER (0x47): the body of an extended control

The first four bytes of the payload are the **reversed ctrl_id** (for example the stream holds `dces`,
which flips to `secd`). Fewer than four bytes gives a `????` Generic. The flipped `ctrl_id` drives the
branch (`parse_control`):

| ctrl_id | Kind | Parser |
|---|---|---|
| `secd` | section definition | `parse_section_def`, parsing the child PAGE_DEF |
| `tbl ` | table | `parse_table` |
| `gso ` | drawing object | `parse_picture_gso` when there are no paragraphs and a PICTURE record exists, otherwise Generic |
| everything else | generic | `parse_generic`, recursively collecting paragraph lists and preserving the original children |

`rest = data[4..]` is the per-control payload. A Generic preserves the child subtree nested as-is in
`raw_children` (for lossless re-serialization) and separately collects a flattened `paragraph_lists`.

**Linking controls** `link_controls` matches the `ExtCtrl` entries in the paragraph text one to one
with `controls[]` in appearance order, filling each `ExtCtrl.ctrl_index` and warning when the text's
`ctrl_id` differs from the CTRL_HEADER's. Leftovers or shortfalls warn as well. A mismatch here throws
off every position calculation, so it is a strong verification point.

**The position invariant**: PARA_HEADER's `nchars` must equal the `wchar_len()` computed from
PARA_TEXT (per-character `wchar_width`: 1 for ordinary, 2 beyond the BMP, 8 for controls). A mismatch
signals a classification error and warns.

### 8.6 PAGE_DEF (0x49), 40 bytes

`width, height, margin_left, margin_right, margin_top, margin_bottom, margin_header, margin_footer,
gutter` (i32 ×9 in HWPUNIT) plus `attr u32`. It appears as a child of the SectionDef (`secd`).

### 8.7 TABLE (0x4D) and cells (LIST_HEADER 0x48)

A table lists **one TABLE record plus, per cell, [LIST_HEADER, PARA_HEADER...] as siblings** under
`CTRL_HEADER(tbl )`. A LIST_HEADER opens a new cell, and the PARA_HEADERs up to the next LIST_HEADER
belong to it.

**The TABLE record**:

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | attr |
| 4 | u16 | rows |
| 6 | u16 | cols |
| 8 | u16 | cell_spacing |
| 10 | u16 ×4 | inner_margins (left, right, top, bottom) |
| 18 | u16 × rows | row_cell_counts (the cell count per row) |
| 18+2·rows | u16 | border_fill_id |
| ... | tail | preserved |

**A cell LIST_HEADER** (a measured 46B prefix plus tail):

| Offset | Type | Field |
|---|---|---|
| 0 | i32 | para_count |
| 4 | u32 | list_attr |
| 8 | u16 | col |
| 10 | u16 | row |
| 12 | u16 | col_span |
| 14 | u16 | row_span |
| 16 | i32 | width (HWPUNIT) |
| 20 | i32 | height |
| 24 | u16 ×4 | margins |
| 32 | u16 | border_fill_id |
| 34 | tail | preserved |

### 8.8 The picture object gso (`parse_picture_gso`)

The object common properties (`rest`): `attr u32 + v_offset u32 + h_offset u32 + width i32 +
height i32`, with `treat_as_char = attr & 1`. Among the children it finds the
`SHAPE_COMPONENT_PICTURE` (0x55) record and reads **the u16 at offset 71, the BinItem id**, into a
`BinRef::Id`. The whole common_data is preserved so placement is lossless.

---

## 9. The control character classification (`hwp-model/src/paragraph.rs`)

The classification of codes 0 to 31 is the **single source of truth** for the reader, the writer and
text extraction. Miscounting even one 8-WCHAR control throws off every later position calculation.

| Class | Codes | WCHAR | Meaning |
|---|---|---|---|
| Char | 0, 10, 13, 24-31 | 1 | meaningful in itself (line break 10, paragraph end 13, hyphen 24, grouped space 30, fixed-width space 31, ...) |
| Inline | 4-9, 19, 20 | 8 | `[code, six WCHAR of information, code]`, self-contained (field end 4, tab 9, ...) |
| Extended | 1-3, 11-12, 14-18, 21-23 | 8 | points at a separate CTRL_HEADER (section/column 2, field start 3, object 11, footnote 17, automatic number 18, ...) |
| Char | 32+ | 1 | ordinary characters |

`ctrl_mask` (PARA_HEADER offset 4) is a hint summarizing the control kinds present in the paragraph as
bits. The reader counts controls by walking the actual PARA_TEXT, so it is not a required parsing
input and is only preserved.

---

## 10. SHAPE_COMPONENT and SC_* geometry (`hwp-render/src/shape_draw.rs`)

`body_text.rs` preserves the SHAPE_COMPONENT and SC_* subtree under a drawing object opaquely
(`raw_children`), and **geometry interpretation happens in the render consumer** (leaving the IR and
the round-trip writer unchanged). Coordinate transform: a local (authoring) space point (HWPUNIT) →
the render matrix (T·S·R) → `+origin` (HWPUNIT) → `/100` = pt.

**SHAPE_COMPONENT (0x4C)** `parse_style`: the first four bytes are the CHID. When `d[0..4]==d[4..8]`
it is a top-level object (CHID twice), so `base=8`; otherwise it is a group member, so `base=4`.
`cnt = u16 @ base+42` is the number of scale/rotation pairs. The translation matrix is
`rd_mat @ base+44` (six f64 = 48B, `[a,b,c,d,e,f]` row-major: `x'=a·x+b·y+c, y'=d·x+e·y+f`). The final
matrix is `T · (scale_last · rotation_last)`, with the last pair at `base+44+48+(cnt-1)·96`. Borders
and fill start at `base+92+cnt·96`: `color u32 + width i32 + lattr u32` (a stroke when `lattr & 0x3F`),
then the fill (`ft u32`: bit0 solid → `color`, bit2 gradient → table 28, bit1 image → BinItem).

**SC_* geometry byte layouts** (`geometry`, coordinates are i32 HWPUNIT):

| Record | Value | Layout |
|---|---|---|
| SC_LINE | 0x4E | start (x,y at 0) plus end (x,y at 8) |
| SC_RECTANGLE | 0x4F | `curvature % u8 at 0` plus four points (at 1, 9, 17, 25); curvature > 0 gives rounded corners |
| SC_ELLIPSE | 0x50 | `attr u32 at 0` plus center (at 4) plus the ax1 endpoint (at 12) plus the ax2 endpoint (at 20) |
| SC_ARC | 0x51 | `arctype u8 at 0` plus center (at 1) plus start (at 9) plus end (at 17) |
| SC_POLYGON | 0x52 | `count u16 at 0` plus the point array (at 4, stride 8B) |
| SC_CURVE | 0x53 | `count u16 at 0` plus the point array (at 2, stride 8B), approximated as a polyline |
| SC_CONTAINER | 0x56 | a group, recursing into children (`MAX_DEPTH=16`) |

Arcs and ellipses are approximated with KAPPA (0.5522847498) cubic Beziers. **Gradients (table 28)**:
`type i16, angle i16, cx i16, cy i16, spread i16, num i16 (at +10)`; when `num>2` an `i32[num]`
position array follows, then `COLORREF[num]` colors (otherwise the stops are evenly distributed).
`type==1` means radial.

---

## 11. Summary information (`summary.rs`)

`/\x05HwpSummaryInformation` is an **MS-OLEPS property set**, parsed on a best-effort basis (returning
a `Metadata` with whatever was read up to any inconsistency):

- Check the byte order `u16 @0 == 0xFFFE`.
- `section_count u32 @24`.
- The first section offset `u32 @44` (skipping the 16B FMTID: header 28 + FMTID 16 = 44).
- The section: `size u32 @sec_off`, `prop_count u32 @sec_off+4`, then a `[pid u32, offset u32]` table
  (at sec_off+8).
- Value offsets are relative to the section start. A `VT_LPWSTR` (type 31) value is
  `type u32 + count u32 (code units including the NUL) + UTF-16LE`, terminated at the NUL.

PID mapping: 0x02 title, 0x03 subject, 0x04 author, 0x05 keywords. A damaged count is clamped against
the remaining bytes (protecting against resource exhaustion).

---

## 12. The reimplementation checklist and invariants

1. **Byte cursor**: every read returns `Err(UnexpectedEof)` rather than panicking when short. Always
   little-endian.
2. **CFB**: delegating to a library is recommended. Only the stream path conventions (§2) matter.
3. **Compression**: raw deflate (no zlib header), applied only to `is_record_stream` targets and only
   when the FileHeader COMPRESSED bit is set.
4. **Record header**: 10/10/12 bits, with an 8-byte extended form when `size==0xFFF` (that is, any
   value `>=0xFFF`). Tags are preserved as a raw u16.
5. **level = tree depth** is the invariant, so re-serializing by depth reproduces the decompressed
   stream byte for byte (the foundation of the lossless round-trip).
6. **The prefix plus tail rule**: every record parser decodes only the known leading part and
   preserves the rest in `tail`. Unknown tags and records are preserved as an `OpaqueRecord`, subtree
   and all.
7. **ctrl_id is stored reversed**: the first four bytes of a CTRL_HEADER and of an ExtCtrl payload are
   flipped when interpreted (`dces` → `secd`).
8. **Position arithmetic**: PARA_HEADER `nchars == Σ wchar_width`. Controls are 8 WCHAR, characters
   beyond the BMP are 2, everything else is 1.
9. **Tolerant mode**: read is tolerant (accumulating warnings, preserving opaquely, never aborting);
   only writer verification is strict.
10. **Encoding**: consistently UTF-16LE. An HWP string is a WORD length (in code units) plus the data,
    with no terminating NUL (only the summary information LPWSTR uses a count that includes the NUL).

Related files: `crates/hwp5/src/{read,container,file_header,doc_info,body_text,summary,error}.rs`,
`crates/hwp5/src/codec/{reader,writer,compress}.rs`, `crates/hwp5/src/record/{header,scan,tree,tag}.rs`,
`crates/hwp-model/src/paragraph.rs` and `crates/hwp-render/src/shape_draw.rs`.
