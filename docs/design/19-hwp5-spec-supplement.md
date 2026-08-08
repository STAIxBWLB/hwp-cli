[한국어](19-hwp5-spec-supplement.ko.md) · [English](19-hwp5-spec-supplement.md)

# HWP 5.0 specification supplement

A normative index collecting the four things the public specification (한글문서파일형식 5.0) does
not provide: an **errata list, a version-layout matrix, conformance rules and consumption
semantics**. The goal is that this document set alone suffices — regardless of implementation
language — to read and write HWP 5.0 files that Hangul (Hancom Office) accepts. Each entry states
the fact in one line and links to the canonical document for the discovery narrative and byte-level
detail. Every fact here was established against genuine Hangul-saved files and the Hancom gate
([00 §4](00-overview.md), the ground-truth methodology). The specification itself is never
reproduced; it is cited by § and table number only ([docs/README](../README.md), copyright policy).

## Division of roles with the other documents

| Document | Subject | Relationship to this one |
|---|---|---|
| [07-hangul-compat-rules](07-hangul-compat-rules.md) | Discovery narrative of the Hancom-confirmed rules (symptom → cause → fix → ground truth) | The evidence base for §3. Rule IDs (A1 … E6) are cited verbatim |
| [03-hwp5-write](03-hwp5-write.md) | Writer implementation procedure (record assembly, version gates, code anchors) | Procedural detail behind §2 and §3 |
| [05-rendering](05-rendering.md) | Render formulas (line layout synthesis, table height, unit conversion) | The canonical home of the formulas indexed in §4 |
| [10-hwp5-structure-map](10-hwp5-structure-map.md) | Exhaustive record catalog (★ marks spec-vs-measured mismatches) | The original location of the §1 errata |
| [08-external-research](08-external-research.md) | External evidence (standards, open source, issue trackers) | Source of the externally corroborated §1 entries |

**Maintenance protocol**: a new fact is first established on real Hancom against ground truth and
registered in 07 (diagnosis) and 10 (structure map); only then is it added as one row here. This
document indexes facts; it is never their first home.

**Notation**: the ★ glyph means different things per document. In 10 it marks a spec-vs-measured
mismatch; in 08 it marks a decisive conclusion. This document uses no ★; the table rows themselves
are the claims.

---

## 1. Errata — where the specification text disagrees with measured bytes

The exhaustive list of places where implementing the specification as written produces wrong
results. The "measured reality" column is normative.

| # | Subject | What the spec says | Measured reality | Verification | Detail |
|---|---|---|---|---|---|
| E-1 | TABLE record "Row Size" array | Row heights | **Per-row cell counts** | Exhaustive comparison against genuine table documents | [10 §4.1](10-hwp5-structure-map.md) |
| E-2 | Drawing-object border line info (table 86) | 11B total, line width INT16 | **13B** total, width **INT32** | 2026-07-19 full spec audit | [10 §4.1](10-hwp5-structure-map.md) |
| E-3 | BULLET record (table 42) | 20B, bullet character at offset 8 | **25B** — a 4B numbering char-shape id occupies offsets 8-12 and the bullet character sits at **offset 12**. Table 42 has a history of self-contradictory totals | Byte comparison of 5 genuine BULLET records, plus the missing-marker symptom reproduced on Hancom | [07 B7](07-hangul-compat-rules.md) |
| E-4 | PAGE_BORDER_FILL (table 135) | Self-contradictory: declares 12B while its own field sum is 14B | **14B** — attr u32 + gap u16×4 + border-fill id u16. BOTH/EVEN/ODD variants are distinguished by record order | Full sweep of 236 genuine files / 714 records (2026-07-19) | [08 evidence per feature](08-external-research.md) |
| E-5 | COLDEF column definition (tables 138-139) | 14B | **16B** in real files (external corroboration; our own exhaustive sweep has not been run — settle against ground truth when implementing) | hwp.js issue #58 | [08 ecosystem](08-external-research.md) |
| E-6 | CHAR_SHAPE attr strikethrough bits 18-20 (§4.2.7 table 35) | Strikethrough presence and style | **Not trustworthy on read** in wild files — track-change deletion templates pollute bit18 (92% false strikethrough in one measured file). On write, bit18 alone renders a strikethrough (Hancom-confirmed) | Exhaustive corpus attr sweep, plus a Hancom test | [07 B8](07-hangul-compat-rules.md) |
| E-7 | PARA_HEADER nchars bit31 | Line-layout-cache validity marker | Actually consumed as the **"last paragraph of its list (section / table cell / text box)" marker** (a dual meaning). Set it wrongly and every later paragraph disappears | bit31 distribution measured on genuine multi-paragraph files, plus a Hancom test (including one revert) | [07 B3·B4](07-hangul-compat-rules.md) |
| E-8 | CTRL_HEADER / ExtCtrl ctrl_id | A four-character code (e.g. `secd`) | Stored **byte-reversed** in the payload (`dces` → `secd`); flip on read, write reversed. The same reversal reappears in FIELD_END payloads as the reversed 3B ctrl_id (§3.4) | Genuine-file byte comparison | [10 §4.1](10-hwp5-structure-map.md) |

### 1.1 Points where the specification is canonical (not errata)

The following are the opposite direction — the specification is right and the implementation or an
older revision of these documents was wrong. Kept here so they are not mistaken for errata.

- The DocInfo/BodyText grouping of records follows **spec table 13**. MEMO_SHAPE (+76),
  FORBIDDEN_CHAR (+78), TRACK_CHANGE (+80) and TRACK_CHANGE_AUTHOR (+81) carry tag values in the
  body numeric range but are semantically DocInfo records (see the warning in
  [10 §3](10-hwp5-structure-map.md)).
- The spec basis for the control character (0-31) classification is **table 6 in the body text
  section, §3.2.3**. The §4.2.4 citation in old code comments and the §4.3.2 citation in an old
  revision of document 10 were both wrong (corrected 2026-07-18, [10 §5](10-hwp5-structure-map.md)).
- The semantically parsed prefix of a cell LIST_HEADER is **34B**. The "46B" in an old revision
  confused it with table 69 (the 46B object common properties)
  ([10 §4.1](10-hwp5-structure-map.md)).

---

## 2. Version-layout matrix

Hangul checks that the declared version in the FileHeader (DWORD `0xMMnnPPrr`,
[03 §2](03-hwp5-write.md)) is consistent with the record layouts, and rejects the file as corrupt
or tampered when they disagree. The check cuts both ways: a new-version declaration with old
layouts is rejected ([07 A3](07-hangul-compat-rules.md)), and an old-version declaration with
new-style padding is rejected too (unconditionally padding ID_MAPPINGS to 18 counts corrupted a
5.0.2.x document — [07 A10](07-hangul-compat-rules.md)). The specification never gathers this
history in one place; the table below indexes it, with the procedural detail remaining canonical
in 02 (read) and 03 (write).

| Boundary (declared ≥) | Record | Change | Evidence |
|---|---|---|---|
| 5.0.1.0 | TABLE | Adds a zone-properties size u16 to the tail | [03 §4](03-hwp5-write.md) |
| 5.0.2.1 | CHAR_SHAPE | Adds border_fill_id u16 at tail[0..2] | [02](02-hwp5-read.md), [03 §4](03-hwp5-write.md) |
| 5.0.2.1 | ID_MAPPINGS | Count array 15 → 16 (adds memo shape) | [03 §4](03-hwp5-write.md) |
| 5.0.2.5 | PARA_SHAPE | line_spacing i32 at tail offset 12 becomes meaningful | [02](02-hwp5-read.md) |
| 5.0.3.2 | PARA_HEADER | 22B → **24B** (track-change-merged-paragraph u16, table 58) | [03 §4](03-hwp5-write.md), [07 A3](07-hangul-compat-rules.md) |
| 5.0.3.2 | ID_MAPPINGS | Count array 16 → **18** (adds track change and track-change author) | [03 §4](03-hwp5-write.md), [07 A10](07-hangul-compat-rules.md) |
| 5.1.0.1 | PARA_SHAPE | 54B → **58B** (trailing 4B = 0). Omitting it triggers an integrity warning | [03 §4](03-hwp5-write.md), [07 A3](07-hangul-compat-rules.md) |
| 5.1.0.1 | CHAR_SHAPE | Synthesis format is **74B** (includes the border_fill_id u16 + strikeout color u32 tail) | [03 §4](03-hwp5-write.md) |
| 5.1.x | DocInfo root | **COMPATIBLE_DOCUMENT (0x1E) subtree is mandatory** — children LAYOUT_COMPATIBILITY (0x1F, 20B of zeros) + TRACKCHANGE (0x20, 1032B, data[0]=0x38). Missing it means rejection. Older versions (5.0.2.x) are exempt | [03 §5](03-hwp5-write.md), [07 A4](07-hangul-compat-rules.md) |

The read side infers the same boundaries from tail lengths ([02](02-hwp5-read.md)); the write side
branches on the declared version (the version_target derivation in [03 §4](03-hwp5-write.md)). Both
directions share the boundaries above; this table is the one place they are listed together.

---

## 3. Conformance checklist — what makes Hangul accept a file

A substitute for the "conformance" chapter the public specification lacks. Every rule is a MUST:
violate it and Hangul rejects or misdisplays the file. In the evidence column, A/B/C/E are rule IDs
from [07](07-hangul-compat-rules.md). The verdict has three layers: ① the file opens (no
corrupt/tampered popup), ② the content renders correctly, ③ features work (hyperlink clicks and so
on). Note that passing a lenient parser such as pyhwp guarantees none of the three (07, overall
lesson 1).

### 3.1 Container and FileHeader (layer ①)

| Rule | On violation | Evidence |
|---|---|---|
| The CFB container must be V3 (512B sectors) | "Corrupt file", rejected outright | A1, [03 §1](03-hwp5-write.md) |
| FileHeader EncryptVersion = 4, including unencrypted documents | "Possibly tampered" security warning | A2, [03 §2](03-hwp5-write.md) |
| Record streams (/DocInfo, /BodyText, /Scripts) are compressed with raw deflate (no zlib header), consistent with attribute bit0 | Fails to open | [03 §1](03-hwp5-write.md) |
| Ship the auxiliary streams (DocOptions/_LinkDoc, the two Scripts streams, HwpSummaryInformation) | Contributes to the corruption verdict (part of the A1 fix) | A1, [03 §1·§9](03-hwp5-write.md) |

### 3.2 DocInfo (layers ① and ②)

| Rule | On violation | Evidence |
|---|---|---|
| Record lengths must match the declared version per the §2 matrix | Corrupt/tampered rejection | A3·A10 |
| A 5.1.x declaration requires the COMPATIBLE_DOCUMENT subtree | Corrupt rejection (even with security lowered) | A4 |
| The six DOCUMENT_PROPERTIES start numbers (page, footnote, endnote, picture, table, equation) are ≥ 1 | Abnormal-document verdict | A8 |
| The ID_MAPPINGS count array must match the actual child record counts | Corruption verdict | A10, [03 §4](03-hwp5-write.md) |
| CHAR_SHAPE shade_color must not be 0 — "none" is 0xFFFFFFFF (what counts as the "none" marker for a COLORREF differs by context) | An opaque black highlight behind every glyph ("black bars") | B1, [05 §7](05-rendering.md) |
| PARA_SHAPE tab_def_id and numbering_id must reference existing items (no dangling refs) | Corruption verdict | A10 |
| SECTION_DEF carries its mandatory children (FOOTNOTE_SHAPE ×2, PAGE_BORDER_FILL ×3) | Corruption verdict | A10, [03 §10](03-hwp5-write.md) |

### 3.3 BodyText paragraphs (layers ① and ②)

| Rule | On violation | Evidence |
|---|---|---|
| Every paragraph has at least one PARA_CHAR_SHAPE run — PARA_HEADER count == PARA_CHAR_SHAPE count | Corruption verdict (lenient external parsers crash instead) | A7 |
| Every paragraph ends with the paragraph terminator (0x0d) | Cascading invariant violations | [03 §6](03-hwp5-write.md)(a) |
| An empty paragraph keeps nchars=1 and omits the PARA_TEXT record — a PARA_TEXT holding only the terminator is forbidden | "Corrupt + body empty" | A5, [03 §6](03-hwp5-write.md)(b) |
| nchars bit31 is set only on the last paragraph of each list (section, table cell, text box) | The first paragraph is taken as the last and everything after it disappears | B4, E-7 |
| ctrl_mask covers only extended and inline control bits (never character-like codes such as 13 and 10) | "A declared control is missing" corruption verdict | A10, [03 §6](03-hwp5-write.md)(f) |
| Consecutive PARA_CHAR_SHAPE runs with the same id are merged | Corruption verdict | [03 §6](03-hwp5-write.md)(d) |
| PARA_HEADER instance_id must not be 0 (unique non-zero per document) | Abnormal-document verdict | A8 |
| A section's first paragraph sets break_type 0x03 (section/column break) | Broken section structure | [03 §6](03-hwp5-write.md)(e) |
| 5.1.x body paragraphs must carry PARA_LINE_SEG (synthesized documents must generate line layout). But a round-trip that modified content must drop the cache to force recomputation — an inaccurate cache re-triggers "tampered" | Zero-height render ("empty content", black bars) or a tampering warning | B2·B3 |
| Empty table cells still hold one paragraph (LIST_HEADER nparas ≥ 1) | Corruption verdict | A6, C3 |

### 3.4 Fields and hyperlinks (layer ③)

A hyperlink click works only as the AND of four conditions: ① the display text carries the link
character shape (blue + underline), ② the field instance id is non-zero, ③ the %hlk command attr is
0x0000a800, ④ the FIELD_END payload holds the reversed 3-byte ctrl_id (without '%'). Miss any one
and the text merely looks like a link without click-through (E1·E2·E4·E5; details in
[07, tier E](07-hangul-compat-rules.md)).

**Scope note**: HWPX-side conformance (mimetype as the first ZIP entry with Stored compression, no
raw control characters inside hp:t, and so on) is covered by [04](04-hwpx-owpml.md),
[07 A11·A12](07-hangul-compat-rules.md) and [11](11-hwpx-structure-map.md).

---

## 4. Undocumented consumption semantics — how Hangul consumes stored values

An index of places where the specification defines the fields but not how they are consumed. Each
formula lives exactly once, in its canonical document.

| Topic | Gist | Formula and detail |
|---|---|---|
| Line layout (PARA_LINE_SEG) synthesis | The default line advance is 160% of the character size. base, line_advance, baseline_gap (85%) formulas and the standard flags 0x0006_0000 | [05 §2.3](05-rendering.md), B2 |
| Status of a stored lineseg | When present it is trusted as first-class input and never recomputed; when absent it is synthesized | [01](01-architecture-ir.md), [05 §2](05-rendering.md) |
| v_pos is page-relative | It must reset to 0 on every page. Monotonic accumulation across a section is judged corrupt | B6, [05 §2.2](05-rendering.md) |
| Table height | Σ over rows of max(rowH) + **566** HWPUNIT (2.0mm). An empirical constant absent from the spec, fixed by cross-measuring two ground-truth documents | C2, [05 §2.4](05-rendering.md) |
| Per-run shape render limit | Hangul renders only the first ~21 shapes of one run (exact limit unknown). The implementation splits runs conservatively at 12 shapes | D8, [04 §7.2](04-hwpx-owpml.md) |
| Two distinct kinds of tab "recomputation" | Render-time tab stops follow `floor(acc/40)×40 + 40`. Separately, Hangul recomputes the HWPX `hp:tab` width attribute on load, so an approximate stored value is acceptable (the latter is an HWPX rule) | [05 §2.3](05-rendering.md) / A12 |
| Multi-column linesegs | Stored as col_start=0 with seg_width = the column width. The column index is not stored and must be derived from v_pos reset boundaries | [05 §1.8](05-rendering.md) |
| HWPUNIT conversion | Only PARA_SHAPE margins divide by 200; every other HWPUNIT divides by 100 | [05 §7](05-rendering.md) |
