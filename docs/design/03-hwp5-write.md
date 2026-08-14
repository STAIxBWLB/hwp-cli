[한국어](03-hwp5-write.ko.md) · [English](03-hwp5-write.md)

# The HWP5 binary writer and Hancom-compatible synthesis, specified for a rebuild

This document describes the whole subsystem that serializes the IR (`hwp_model::Document`) into an
HWP 5.0 binary that Hancom Office opens without a corruption or tampering verdict, in enough detail to
reimplement it from scratch. The core implementation is `crates/hwp5/src/write.rs` (2,049 lines), and
the invariants are pinned by `tests/roundtrip.rs`, `tests/identity.rs` and `tests/synth.rs`. Every
constant, byte offset and length here is **ground truth measured from genuine Hancom-saved files**
(hello_world 5.1.0.1, 가나다 5.1.1.0, work_report 5.0.2.4, halla 5.1.1.0, annual_report) **and
confirmed through the Hancom gate**; none of it can be derived from the specification alone (these are
values that lenient parsers such as pyhwp accept but Hancom itself refuses).

---

## 0. The design principle: a mirrored "prefix plus tail"

Every record is parsed and emitted by the rule **`{known prefix as struct fields} + {the remaining
bytes verbatim as a tail}`**. HWP is a forward-compatible format that appends fields to a record's
tail as versions advance, so:

- The parser (`doc_info.rs`, `body_text.rs`) preserves the uninterpreted tail in `tail` with
  `r.take_rest().to_vec()`.
- Each `emit_*` in the writer (`write.rs`) is **a byte-for-byte mirror of the parser**. When `tail` is
  non-empty (an hwp5 original round-trip) it is appended as-is; when empty (hwpx or markdown
  synthesis) the default tail for the declared version's layout is filled in.

Thanks to that symmetry, an hwp5 document with only simple controls round-trips **byte-identically at
the decompressed stream level** (finally proven by `identity.rs` and `roundtrip.rs`). Structures that
get flattened, such as gso, are guaranteed semantic equivalence.

Converting between the record tree and the flat stream is `record/tree.rs`'s job. Because it
recomputes **level = tree depth** on serialization, parse → tree → re-serialize is byte-identical
(`RecordNode::serialize_forest`).

---

## 1. The container: CFB V3 (512B sectors) is mandatory

HWP 5.0 is an MS CFB (Compound File Binary) container. **It must be created as version 3 (512-byte
sectors).** Writing the default V4 (4096B sectors) makes Hancom refuse it as "a corrupt file"
(measured through the Hancom gate).

```rust
let mut cfb = cfb::CompoundFile::create_with_version(cfb::Version::V3, file)?;
```

The stream write order and contents (`write_document`, write.rs:161-229):

| CFB path | Contents | Compression |
|---|---|---|
| `/FileHeader` | the fixed 256B header | none |
| `/DocInfo` | the DocInfo record forest | raw deflate |
| `/BodyText/Section{i}` | the per-section body record forest | raw deflate |
| `/BinData/BIN{id:04X}.{ext}` | only images referenced by the BIN_DATA table | raw deflate |
| `/DocOptions/_LinkDoc` | `[0u8; 524]` | none |
| `/Scripts/JScriptVersion` | 13B of sample raw bytes | (treated as a record stream, but the sample is written as uncompressed bytes) |
| `/Scripts/DefaultJScript` | 16B of sample raw bytes | likewise |
| `/\u{5}HwpSummaryInformation` | the OLE property set (§9) | none |
| `/PrvText` | preview UTF-16LE (~1000 characters) | none |
| `/PrvImage` | PNG (only when opts.prv_image is present) | none |

**Compression** is **raw deflate** with no zlib header (pyhwp's `wbits=-15`, `flate2::DeflateEncoder`).
Which streams are compressed is decided by `container::is_record_stream` (prefixes `/DocInfo`,
`/BodyText/`, `/ViewText/`, `/Scripts/`).

For `BinData`, only items in `header.bin_data` that have a `storage_id` are included, named
`BIN{id:04X}.{ext}`. Streams the table does not reference are dropped with a warning (following the
hwp5 naming rule).

---

## 2. FileHeader: EncryptVersion=4 is mandatory

The layout (a fixed 256B, `file_header.rs`):

| Offset | Size | Field | Value on synthesis |
|---|---|---|---|
| 0 | 32 | the signature `"HWP Document File"` plus NUL padding | fixed |
| 32 | 4 | the version DWORD `0xMMnnPPrr` | parsed from `source_version` |
| 36 | 4 | attribute flags | `0x1` (bit0 = compressed) |
| 40 | 4 | license (CCL / KOGL) | `0` |
| 44 | 4 | **EncryptVersion** | **`4`** |
| 48 | 1 | KOGL supported country | `0` |
| 49 | 207 | reserved | `0` |

**EncryptVersion=4 is decisive.** Even for an unencrypted document, modern Hancom (2010+, Hangul 7.0+)
always writes EncryptVersion=4. All six fixtures in fixtures/hwp5 have the encryption bit (bit1) of
the attribute flags at 0 yet encver=4. **Writing 0 makes Hancom refuse it as corrupt or tampered.**
Attribute flag bit0 (compressed) is always set, and `roundtrip.rs` and `synth.rs` assert that
`is_compressed()` holds.

Version encoding: `major<<24 | minor<<16 | build<<8 | revision`. `parse_version` parses
`source_version` (for example `"5.1.0.1"`) and falls back to **5.1.0.1** (`HwpVersion{5,1,0,1}`) on
failure. That default is the baseline for every version gate below.

---

## 3. Separating the synthesis path (source ≠ hwp5) from the unmodified round-trip (identity)

The most important architectural decision in this subsystem. **The two paths must never mix.**

### Computing the gate flags (write.rs:53-57)

```
synth_pictures = meta.source_format != "hwp5" || has_synthesizable_picture(doc)
synth_gso      = meta.source_format != "hwp5" || has_synthesizable_gso(doc)
synthesize     = synth_pictures || synth_gso || opts.edited
```

- `has_synthesizable_picture`: true when a `Picture` has empty `extras` (that is, it holds no hwp5
  shape records). **A picture newly inserted by editing** must be synthesized even for an hwp5 origin,
  or strip drops it.
- `has_synthesizable_gso`: true when a control has `ctrl_id != "gso "` yet carries `gso_shapes`
  (a structured shape created by the hwpx reader).
- `opts.edited`: an hwp5 original was read, modified and is being written back. Picture
  re-synthesis (`synthesize_pictures`) is **not** done (the shape records already exist), but the
  paragraph invariants and line layout must be re-established.

### What happens when synthesize = true (the synthesis path)

1. `synthesize_pictures` (§7 picture synthesis), `degrade_hwpx_gso` (§8 text box degradation) and
   `strip_unwritable_pictures` (dropping controls that cannot be synthesized).
2. The last-paragraph-per-list flag, `set_last_para_flag` (§6).
3. `ensure_para_shape_defaults` (correcting missing ParaShape baselines: `line_spacing_old` 0 → 160,
   `border_fill_id` 0 → 2).
4. `break_type |= 0x03` on the first paragraph of each section.
5. Inside `emit_paragraph`: the paragraph end 0x0d, char_shape dedup and omitting PARA_TEXT for empty
   paragraphs (§6).
6. `assign_instance_ids`: when a PARA_HEADER instance_id is 0, a unique non-zero id is assigned
   (from 0x10000001). **Hancom treats instance_id=0 as abnormal and judges the file corrupt** (every
   sample is non-zero).
7. In `emit_doc_info`: injecting COMPATIBLE_DOCUMENT (§5), injecting TAB_DEF and NUMBERING defaults
   (§4, a safety net), and `max(1)` on the DOCUMENT_PROPERTIES start numbers.

### When synthesize = false (the identity path)

- The document is **not even cloned** (when `needs_normalize(doc)` is also false). The original IR is
  emitted as-is.
- Every `tail`, `instance_id` (including 0) and `chars_flags` is preserved exactly, giving byte
  identity.
- COMPATIBLE_DOCUMENT is already preserved in `header.extras`, so it is not re-injected.
- PARA_LINE_SEG is emitted only when `preserve_linesegs=true` (the byte-identity gate). When false it
  is omitted with seg_count=0 so Hancom recomputes it (preventing a line layout cache inconsistent
  with edited content, which triggers Hancom's "tampering" warning).

**A note on orthogonality:** `synthesize` (correcting invariants) and `preserve_linesegs` (emitting
line layout) are separate axes. The emission condition is
`emit_lineseg = synthesize || preserve_linesegs`.

---

## 4. Record length version gating

Hancom refuses a file as corrupt or tampered when the declared version and the actual record lengths
disagree. Only synthesized paragraphs (with an empty `tail`) get the per-version standard length
filled in; an original round-trip preserves it exactly through `tail`.

### PARA_SHAPE (0x19): 54B → 58B

The 5.1.0.1 layout is **58B**. When `tail` is empty after the 42B prefix, the next 16B are filled
(`emit_para_shape`, write.rs:1436):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | attr1 |
| 4-28 | 24 | margin_left, margin_right, indent, spacing_top, spacing_bottom, line_spacing_old (i32 each) |
| 28 | 2 | tab_def_id |
| 30 | 2 | numbering_id |
| 32 | 2 | border_fill_id |
| 34 | 8 | border_offsets[4] (u16) |
| 42 | 4 | attribute 2 = 0 |
| 46 | 4 | attribute 3 = 0 |
| 50 | 4 | line spacing = `line_spacing>0 ? line_spacing : 160` |
| 54 | 4 | **the trailing 4B = 0** |

**Omitting the trailing 4B (offsets 54 to 58) gives 54B and makes Hancom raise an integrity
warning.** `synth.rs` asserts `record_sizes(&di, 0x19).all(== 58)`. CHAR_SHAPE is gated the same way
and must be **74B**.

### PARA_HEADER (0x42): 22B → 24B

The prefix is 22B. When the declared version is **5.0.3.2 or newer**, a UINT16 (=0) for "merged for
change tracking" is appended, giving **24B** (specification table 58). The gate is
`add_tracking_tail = source_version >= 0x05_00_03_02` (write.rs:113). Pre-5.0.3.2 (work_report
5.0.2.4) is correct at 22B, so the gate is false there. `synth.rs` asserts
`record_sizes(&bt, 0x42).all(== 24)` (synthesis declares 5.1.0.1).

### ID_MAPPINGS (0x11): 15, 16 or 18 counts

The count array length must agree with the declared version (specification tables 15 and 16). It is
**derived from the actual table lengths** and never synchronized by hand (write.rs:1166-1204):

| Index | Count | Introduced in |
|---|---|---|
| 0 | bin_data | the base (15 entries) |
| 1-7 | fonts (seven language slots) | |
| 8 | border_fills | |
| 9 | char_shapes | |
| 10 | tab_defs | |
| 11 | numberings | |
| 12 | bullets | |
| 13 | para_shapes | |
| 14 | styles | |
| 15 | memo shape | 5.0.2.1+ (16 entries) |
| 16 | change tracking | 5.0.3.2+ (18 entries) |
| 17 | change tracking author | 5.0.3.2+ |

```
version_target = if ver >= 0x05_00_03_02 { 18 }
                 else if ver >= 0x05_00_02_01 { 16 } else { 15 }
target = max(original count length, version_target, derived count length)
```

**Unconditionally padding to 18 inflates a 5.0.2.x document (16 entries), so the version and layout
disagree and it is judged corrupt** (demonstrated by work_report). The count emission loop and the
child record emission loop reference **the same Vec** (`tab_defs_owned`, `numberings_owned`), so the
counts and the actual item counts always agree (an invariant).

### Other tail gates

- **BORDER_FILL**: with an empty tail, fill with pattern color (u32=0) plus pattern kind
  (u32=0xFFFFFFFF) plus extra property size (u32=0) plus transparency (u8=0) (color and transparency
  only when fill_type & 1).
- **CHAR_SHAPE**: with an empty tail, fill border_fill_id (u16, `max(2)`) plus strikethrough color
  (u32=0). **shade_color must not be 0** (0xFFFFFFFF means "none"). At 0, Hancom draws an opaque black
  shade behind every character cell, giving "black bars" (asserted in `synth.rs`).
- **STYLE**: with an empty tail, a lock u16=0.
- **LIST_HEADER (cell)**: with an empty tail, text width (i32 = the cell width) plus 8B reserved,
  giving 46B.
- **TABLE**: with an empty table_tail, a zone property size u16=0 (5.0.1.0+).

---

## 5. The COMPATIBLE_DOCUMENT subtree (mandatory for 5.1.x)

Every genuine 5.1.x file (가나다 5.1.1.0, hello_world 5.1.0.1) has this subtree. **Omitting it makes
Hancom refuse the file as corrupt or tampered.** Older versions (work_report 5.0.2.4) are exempt. The
injection condition is `source_format != "hwp5"` **and** not already present in `header.extras`
(write.rs:1264). An hwp5 original round-trip preserves it in `extras`, so it is not re-injected.

The tree (tag values relative to `HWPTAG_BEGIN=0x10`):

```
COMPATIBLE_DOCUMENT (0x1E)          data = [0u8; 4]  (target program 0)
├─ LAYOUT_COMPATIBILITY (0x1F)      data = [0u8; 20]
└─ TRACKCHANGE (0x20)               data = [0u8; 1032], except data[0]=0x38
```

`TRACKCHANGE` is **1032B** with only the first byte 0x38 (measured on samples) and the rest zero.
`synth.rs` finds 0x1E in DocInfo and asserts that 0x1F and 0x20 are its children.

---

## 6. Paragraph rules (the synthesis path invariants)

The rules `emit_paragraph` (write.rs:1507) enforces so that output is isomorphic to genuine Hancom
paragraphs (compared exhaustively against all 188 paragraphs of 가나다). These five defects together
were the root cause of the "corrupt even at lowered security" warning.

### (a) Every paragraph ends with the paragraph terminator 0x0d

When `synthesize`, a `CharCtrl(13)` is pushed when the last char is not already one. `synth.rs`
asserts the last u16 of PARA_TEXT == 13.

### (b) An empty paragraph omits the PARA_TEXT record, with nchars=1

Measured on genuine files: an empty paragraph or cell has `nchars=1` (an implicit paragraph
terminator) plus PARA_CHAR_SHAPE and PARA_LINE_SEG, but **no PARA_TEXT record**.

```rust
let char_count = if para.chars.is_empty() { 1 } else { para.wchar_len() };
// ...
if char_count > 1 { /* emit PARA_TEXT */ }
```

The synthesis path appends 0x0d to every paragraph, so an empty paragraph becomes `chars=[0x0d]`
(char_count=1); emitting that as `PARA_TEXT=[0x0d]` makes **Hancom refuse it with "file corrupt plus
empty body"** (the cause of corruption in every table with empty cells: title boxes, tables of
contents, section headers). pyhwp leniently accepts an empty PARA_TEXT, so 23 rounds failed to detect
it; only byte comparison against genuine files caught it. `빈_문단은_para_text_없음` in `synth.rs`
prevents a regression.

`nchars` has the character count in the low 31 bits, and `wchar_len()` sums `Text → len_utf16`,
`CharCtrl → 1` and `Inline/ExtCtrl → 8` (a 6-WCHAR payload plus the code on each side).

### (c) nchars bit31 marks only the last paragraph of a list

The top bit of `nchars` (0x80000000) marks "the last paragraph of a list (section, table cell or text
box)". `set_last_para_flag` sets `chars_flags |= 0x80` on **only the last paragraph** of each list and
clears the rest (recursing into table cells and text boxes). When emitting PARA_HEADER,
`nchars = char_count | (chars_flags << 24)`.

**Setting it on every paragraph makes Hancom treat the first paragraph as the last and ignore the
rest** (the multi-paragraph "everything after the first is invisible" symptom). `synth.rs` asserts
bit31 is set for a single paragraph.

### (d) Merging consecutive identical PARA_CHAR_SHAPE runs (dedup)

```rust
p.char_shape_runs.dedup_by(|(_, b), (_, a)| a == b);
```

Duplicate runs are judged corrupt. `synth.rs` asserts a single paragraph has run count 1
(char_shape_cnt=1).

### (e) break_type = 0x03 on a section's first paragraph

The first paragraph of each section gets `break_type |= 0x03` (section and column break). `synth.rs`
asserts PARA_HEADER offset 11 == 0x03.

### (f) ctrl_mask covers only extended and inline controls

`ctrl_mask` keeps the original value when present, and otherwise is computed by ORing `1<<code` from
the `InlineCtrl` and `ExtCtrl` codes in `chars`. **Character-like controls (paragraph end 13, line
break 10 and so on) are excluded**: setting them makes Hancom judge the file corrupt because "a
control that ctrl_mask claims exists is not actually there".

### The PARA_HEADER 22B prefix layout

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | nchars (bit31 = last paragraph, low 31 = character count) |
| 4 | 4 | ctrl_mask |
| 8 | 2 | para_shape id |
| 10 | 1 | style id |
| 11 | 1 | break_type |
| 12 | 2 | char_shape run count |
| 14 | 2 | range_tag count (the number of PARA_RANGE_TAG) |
| 16 | 2 | line_seg count (the real count when `emit_lineseg`, otherwise 0) |
| 18 | 4 | instance_id |
| 22 | (2) | merged for change tracking (the 5.0.3.2+ tail) |

Child record order: PARA_TEXT (only when char_count > 1) → PARA_CHAR_SHAPE → PARA_LINE_SEG (when
emitted) → extras → the controls (CTRL_HEADER).

PARA_TEXT encoding (`emit_para_text`): `Text` → UTF-16LE, `CharCtrl` → u16, and `Inline/ExtCtrl` →
`[code(u16), payload 12B (zero-padded when short), code(u16)]` (8 WCHAR in total).

---

## 7. Picture synthesis (hwpx or markdown images → hwp5 shape records)

An hwpx `<hp:pic>` is read as an IR `Picture` with empty `extras`. hwp5 stores a picture as a
`gso CTRL_HEADER → SHAPE_COMPONENT → SHAPE_COMPONENT_PICTURE` tree plus a BIN_DATA item plus a BinData
stream. `synthesize_pictures` (write.rs:590) uses the records of the genuine work_report as a template
and patches only the size and BinItem id.

### The flow

1. Extract the pixel size of each image in `bin_streams` (`image_pixel_size`: PNG IHDR, JPEG SOFn,
   GIF LSD, BMP BITMAPINFOHEADER).
2. Assign a `storage_id` (from the existing max+1, reusing it for shared images), add
   `BinDataItem{attr:1 (embedded), storage_id, extension}`, and rename the stream to
   `BinData/BIN{s:04X}.{ext}`.
3. **Synthesize the 40B gso object common properties** from the placement:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | attr: inline (as a character) = `0x042a6001`, floating = `0x040a6000` (measured on the university document) |
| 4 | 4 | vertical offset (vert_offset, preserved even with treat_as_char) |
| 8 | 4 | horizontal offset |
| 12 | 4 | width |
| 16 | 4 | height |
| 20 | 4 | z_order |
| 24 | 8 | outer margins (left/top, right/bottom) |
| 32 | 4 | instance_id (unique, from 0x30000000) |
| 36 | 4 | keep with next |
| 40 | 2 | **desc_len = 0** (the object description BSTR; required for CommonControl ≥ 5.0.0.5, so even an empty description needs the u16 length 0) |

4. The **196B SHAPE_COMPONENT (0x4C)** template (`SHAPE_COMPONENT_TEMPLATE`): chid `"$pic"` (reversed
   `"cip$"`), with width patched at 20 and 28 and height at 24 and 32 (the matrix is the identity, so
   the initial and final values are the same).
5. The **91B SHAPE_COMPONENT_PICTURE (0x55)** (`build_picture_extras`), the genuine 5.1.x layout:

| Offset | Size | Contents |
|---|---|---|
| 0 | 12 | borders |
| 12 | 32 | the four display rectangle corners (0,0)(w,0)(w,h)(0,h) |
| 44 | 16 | cropping = (0, 0, **natural width, natural height**) |
| 60 | 8 | inner margins |
| 68 | 3 | brightness, contrast, effect |
| 71 | 2 | **the BinItem id** |
| 73 | 1 | border transparency |
| 74 | 4 | instance_id (derived as `^0x00100000` from the gso one) |
| 78 | 4 | picture_effect flags = 0 |
| 82 | 8 | picture_effect natural width and height |
| 90 | 1 | reserved |

**Cropping (clip) must be the original natural size (pixels × 7200/96, at 96 DPI), not the display
size.** Using the display size crops to a small top-left fraction of the original (for example
8196/150000 ≈ 5%), so the picture is nearly invisible in Hancom. Filling `extras` through
`build_picture_extras` also prevents `strip_unwritable_pictures` (§3) from dropping it.

After synthesis, `bin_ref = BinRef::Id(storage_id)`. `emit_picture` (write.rs:1941) writes
`common_data` (or a minimal 40B treated-as-character when absent) and emits `extras` as children.

An empty LIST_HEADER for a header or footer (head/foot) is filled by `fill_head_foot_list_header` with
the section PageDef dimensions (textWidth = the body width, textHeight = the header or footer margin)
(the 34B `HEADER_LIST_HEADER_TEMPLATE`, patching only paraCount with the real paragraph count).
Without it, strip drops the header entirely.

---

## 8. Lossless gso raw_children plus safe degradation of hwpx-origin shapes

### hwp5-origin gso: lossless raw_children

A GenericControl of hwp5 origin preserves the original child subtree wholesale in `raw_children` (an
OpaqueRecord tree, in `parse_generic`). The writer (`emit_control`, write.rs:1720) emits
`raw_children` nested as-is and **returns immediately** when present, never flattening through
`paragraph_lists` or `extras` (which are extraction-only). This is what makes the whole fixture set,
including tables, pictures, shapes and bookmarks, round-trip byte-identically.

Only when `raw_children` is absent does it assemble a CTRL_HEADER from `paragraph_lists` (LIST_HEADER
plus paragraphs) and `extras`. The ctrl_id is **stored reversed** (`reversed`, `b"secd"` → `"dces"`).
For `cold` (column definition), an empty data is replaced with the 12B `DEFAULT_COLD_DATA`.

### hwpx-origin structured shapes: safe degradation

Shapes created by the hwpx reader (`ctrl_id != "gso "` with `gso_shapes`) have no hwp5
SHAPE_COMPONENT. Re-synthesizing from a genuine template was judged corrupt in Hancom (using a 252B
line template for a rectangle, 13B off from the genuine 239B). With no way to self-validate, the
approach switched to **safe degradation** (`degrade_hwpx_gso`, write.rs:467):

- **A text box (with text)**: its paragraphs are hoisted into the body after the host paragraph
  (preserving the text), and the shape wrapper is omitted.
- **Purely decorative (no text)**: left untouched so `strip_unwritable_pictures` drops it
  (guaranteeing validity).

When hoisting, the ExtCtrl character of the removed control is deleted with `chars.retain` and the
remaining `ctrl_index` values are remapped (the same logic as strip).
`글상자_hwpx출신_안전저하_텍스트보존` in `synth.rs` prevents a regression.

---

## 9. Auxiliary streams

These always exist in a Hancom-saved file, and their absence risks a corruption verdict
(write.rs:207-228).

- **`/DocOptions/_LinkDoc`**: `[0u8; 524]`.
- **`/Scripts/JScriptVersion`**: the 13B raw sample
  `63 64 80 00 00 F7 DF 88 A9 08 00 00 00`.
- **`/Scripts/DefaultJScript`**: the 16B raw sample
  `63 60 40 05 FF 81 00 00 6E BB 6E D1 14 00 00 00`.
- **`/\u{5}HwpSummaryInformation`** (`hwp_summary_information`, write.rs:977): the OLE property set.
  FMTID `9FA2B660-1061-11D4-B4C6-006097C09D8C` with 14 properties (PID 0x02 title, 0x03 subject,
  0x04 author, 0x05 keywords, 0x09 application = "hwp-cli", three FILETIMEs, two I4 values, and
  **PID 0 = Dictionary**). Writing PID 0 as VT_NULL makes readers such as pyhwp try to read a
  one-entry dictionary and refuse at EOF, so it must be written as a one-item dictionary (id=0, a 1B
  empty name) in 13B. It mirrors the `summary.rs` parser.
- **`/PrvText`**: the first 1000 characters of `plain_text()` in UTF-16LE.

The `hwp_string` helper is `u16 length (in UTF-16 units) + UTF-16LE bytes`.

---

## 10. The top-level assembly order for DocInfo and BodyText

### emit_doc_info (write.rs:1109) root order

1. **The safety net**: when `tab_defs` is empty, inject three defaults (`[0..]`, `[1..]`, `[2..]`, 8B
   each); when `numberings` is empty, inject the 226B `DEFAULT_NUMBERING_DATA`. **Every PARA_SHAPE
   references tab_def_id=0 and numbering_id=0, so an empty table becomes a dangling reference and
   Hancom refuses the file as corrupt** (demonstrated by halla). `synth.rs` asserts they are
   non-empty.
2. `DOCUMENT_PROPERTIES` (0x10): section_count `max(1)`, each of the six start numbers `max(1)` (page
   number 0 is abnormal), and the caret as three u32.
3. `ID_MAPPINGS` (0x11): the count array from §4 plus the child tables (bin_data → fonts, seven slots
   → border_fills → char_shapes → tab_defs → numberings → bullets → para_shapes → styles →
   id_extras).
4. `COMPATIBLE_DOCUMENT` (§5, when synthesizing and not already present).
5. `header.extras` (the COMPATIBLE and similar records of an hwp5 original).

### emit_section (write.rs:1483)

Section paragraphs go through `emit_paragraph`, then `section.extras` is appended. After each section
is serialized, `assign_instance_ids` runs (when synthesizing).

### emit_section_def and the required secd children (write.rs:1760)

The ctrl_id `"dces"` (secd reversed) plus data (or the 43B `DEFAULT_SECD_DATA` when absent). Children:

- `PAGE_DEF` (0x49) 40B: width, height, six margins, gutter (i32 each) and attr (u32).
- **When `def.extras` is empty (synthesis)**: `FOOTNOTE_SHAPE` (0x4A) 28B × 2 (the footnote and
  endnote samples) plus `PAGE_BORDER_FILL` (0x4B) 14B × 3 (BOTH/EVEN/ODD).

**All three PAGE_BORDER_FILL records have first u32 = 1** (properties), four u16 gaps of 1417
(0x0589), and border_fill_id u16 = 1. The BOTH and EVEN values in the hello_world sample
(0x0978f9c1 and so on) are uninitialized garbage and are not adopted. `synth.rs` asserts 0x4A × 2 and
0x4B × 3 under secd with first u32 = 1. With only PAGE_DEF and no footnote shape or page border,
Hancom refuses the file as corrupt.

### emit_table (write.rs:1816)

The CTRL_HEADER data is `" lbt"` (tbl reversed) plus the object common properties (preserving
`common_data`, synthesizing from hwpx placement, or computing cell sizes for markdown). Children: the
`TABLE` (0x4D) record (attr u32, rows/cols/cell_spacing u16, inner_margins 4 × u16,
**row_cell_counts rows × u16**, border_fill u16, tail), then per cell a `LIST_HEADER` (0x48,
`emit_cell_header`, para_count = the paragraph count, `≥1`) plus the paragraphs.

**Even an empty cell needs nparas ≥ 1** (with no paragraph, Hancom judges it corrupt). `from_markdown`
guarantees this by `flush_paragraph_inner(force=true)` on cell close and padding missing cells with
`Paragraph::default()`. The `row_cell_counts` length equals the row count and its sum equals the cell
count (asserted by the row-addition table test in `synth.rs`).

---

## 11. The re-serialization and invariant checklist (pinned by tests)

In `crates/hwp5/tests/`:

- **identity.rs** `레코드_스트림_바이트_동일_재직렬화`: for the whole fixture set (hello_world,
  bookmark, color_fill, outline, work_report, annual_report), `/DocInfo` plus the body sections go
  through a strict scan, a tree and `serialize_forest` and come out **byte-identical to the original**
  (the first proof of losslessness at the record layer).
- **roundtrip.rs** `전체_fixture_바이트_동일_왕복`: re-saving through the IR with
  `preserve_linesegs:true` gives byte-identical decompressed streams plus `is_compressed()`.
  `전체_fixture_의미_왕복`: text, char_shape count, section count and lineseg count are preserved.
- **synth.rs** `합성_문서_한글_규격_충족`: TAB_DEF and NUMBERING non-empty, shade_color ≠ 0, the 0x1F
  and 0x20 children of COMPATIBLE (0x1E), two footnotes and three page borders under secd,
  EncryptVersion=4, PARA_SHAPE = 58B, CHAR_SHAPE = 74B, PARA_HEADER = 24B.
  `합성_문단_본문_구조_정품_동형`: PARA_TEXT ending in 0x0d, nchars bit31, break_type = 0x03,
  char_shape run = 1 (dedup), PAGE_BORDER_FILL attr = 1. Plus `빈_문단은_para_text_없음`,
  `행_추가_표_합성_규격_충족`, `누름틀/책갈피/하이퍼링크_생성_이진_왕복` and
  `글상자_hwpx출신_안전저하_텍스트보존`.

### The record header codec (record/header.rs)

`u32 LE = tag (bits 0-9) | level (bits 10-19) << 10 | size (bits 20-31) << 20`. **When the size field
is 0xFFF, the following u32 is the real size** (0xFFE is still an inline 4B header; from 0xFFF it is an
extended 8B header). `serialize_forest` recomputes level as the tree depth
(`serialize_into(depth)`).

### The control character classification (hwp_model::char_kind, the single source of truth)

| Code | Kind | WCHAR |
|---|---|---|
| 0, 10, 13, 24-31 | Char (character-like) | 1 |
| 4-9, 19, 20 | Inline | 8 |
| 1-3, 11, 12, 14-18, 21-23 | Extended (referencing controls) | 8 |
| 32+ | ordinary characters | len_utf16 |

This classification is the basis for the parser, the writer, text extraction and position arithmetic
alike, and the invariant `wchar_len() == nchars` exposes a classification error immediately.

---

## 12. The key traps when reimplementing

1. CFB must be V3. V4 is immediately corrupt.
2. Hardcode EncryptVersion=4, even for unencrypted documents.
3. `synthesize` is not `preserve_linesegs` (they are orthogonal). On the identity path, do not even
   clone the original (preserving tail, instance_id 0 and chars_flags is the premise of byte
   identity).
4. Version gates branch on **the declared version** for PARA_SHAPE (58/54), PARA_HEADER (24/22) and
   ID_MAPPINGS (18/16/15). Never pad everything to the newest layout.
5. An empty paragraph is nchars=1 with PARA_TEXT omitted (a PARA_TEXT holding only 0x0d is corrupt).
6. nchars bit31 belongs only to the last paragraph of a list.
7. Dedup char_shape runs, and keep shade_color ≠ 0.
8. Picture cropping uses the natural size, not the display size.
9. Prevent dangling references with the TAB_DEF, NUMBERING, PAGE_BORDER_FILL and footnote shape
   safety nets.
10. hwp5-origin gso is lossless through raw_children; hwpx shapes are safely degraded (re-synthesis
    brings the corruption back).

Every one of these values is ground truth confirmed by measuring genuine files and passing the Hancom
gate, not by reading the specification.

---

## 13. Source-preserving native HWP rewrites

Native HWP editing does not rebuild the input container. `rewrite_document_with_report` receives an
immutable source path, the IR read from that exact snapshot, and the edited IR. It re-reads the source
and rejects a snapshot mismatch before deriving a mutation plan.

The plan owns only these targets:

- metadata changes replace `\u0005HwpSummaryInformation`;
- header or BinData relationship changes replace `DocInfo`;
- section changes replace only the corresponding `BodyText/SectionN` stream and its previews;
- changed or removed binaries touch only their `BinData/*` streams.

A no-op is `fs::copy`, so the complete CFB file is byte-identical. For an edit, the source CFB is
copied first and selected streams are patched in place. `FileHeader`, `MemoExtended`, `Scripts`,
`DocOptions`, XMLTemplate, DocHistory, unknown streams, unknown storages, untouched binaries and CFB
directory entries unrelated to patched streams remain source-owned.

BodyText materialization also stays surgical. Paragraphs are matched by unique non-zero
`instance_id`, then exact equality, with a same-length positional fallback. An unchanged paragraph is
the original record subtree. In a changed paragraph, unchanged typed controls are transplanted from
the source tree; a table-cell edit recursively patches only the changed cell paragraph. This matters
because records such as `CTRL_DATA`, `PAGE_DEF` and `PAGE_BORDER_FILL` can be level-sensitive even
when the semantic IR represents them separately. `ParaHeaderInfo::hwp5_child_order` additionally
retains the source order between opaque paragraph children and `CTRL_HEADER` records.

Changed or inserted paragraphs get fresh `PARA_LINE_SEG` values from the renderer. After synthesis,
all semantically unchanged native paragraphs, including table-cell and generic-control paragraphs,
are restored from the immutable snapshot. This avoids both stale layout caches and global layout
churn.

The CLI `convert` no-op, `edit`, and IR `fill` paths take a private immutable input snapshot before
writing. The published output is still atomic and passes the typed preservation gate. The regression
suite covers exact no-op copy, metadata-only targeting, opaque stream preservation, unchanged typed
control subtree preservation, image relationship materialization, and same-format CLI byte identity.
