[한국어](07-hangul-compat-rules.ko.md) · [English](07-hangul-compat-rules.md)

## Overview: why this document is the project's most valuable asset

hwp-cli passes 100% of its own reader and renderer checks, and pyhwp's, on files that Hancom Office
nonetheless **rejects or misdisplays as corrupt, blank, or covered in black bars**. Each of the rules
below was caught one at a time by testing in Hancom Office and by comparing bytes against ground
truth (`.hwp`/`.hwpx` files the user created directly in Hancom). These rules are **implicit
invariants** absent or unstated in the public specification (HWP 5.0 §), so they were in principle
undiscoverable by static analysis, reading the specification, or validating with an external parser.
The thesis running through all of it:

> **"Our renderer and pyhwp are lenient; only Hancom is strict (that is, Hancom-specific)."**
> Hancom rejects files that lenient tools accept. "Re-reads without warnings and looks right when
> rendered" is therefore only a necessary condition; **the real answer key is what Hancom Office does**.

Each rule below is tabulated as `[symptom | cause | fix | ground truth and evidence | file:function
(commit)]`, with a lesson on **why it could only be caught by Hancom testing and ground truth rather
than static analysis**.

> **Index maintenance**: the rule IDs here (A1 … E6) are cited by the spec-supplement index
> [19-hwp5-spec-supplement](19-hwp5-spec-supplement.md). When adding or renumbering a rule, update
> the corresponding row there.

---

## Diagnostic methodology (the meta-asset learned before the rules)

| Technique | What it is | Why it was necessary |
|---|---|---|
| **Genuine byte comparison** | Diff our synthesized bytes record by record against ground truth saved by the user in Hancom (가나다, 다문단, 첫째문단, 테스트, 테스트2, 도형정답지2 .hwp/.hwpx) | Hancom's accept/reject decision is a black box, so "what bytes differ from a known-good sample" is the only observable signal |
| **Four-axis parallel multi-agent diagnosis** | spec / web / sample comparison / corruption tracing, plus adversarial verification (28 agents) | Five causes of corruption were entangled at once (75fb581, 1f0139b), so no single hypothesis could isolate it |
| **Bisection** | Only tables with empty cells corrupted, isolating empty cell = empty paragraph (4b57b8a) | Narrows the corrupting element to an atomic unit inside a composite document |
| **Injection diagnosis** | Inject only our element into a ground-truth document to separate "element vs context" (b472070) | Confirmed our ellipse element renders fine, decisively isolating the problem as z-order context rather than the element |
| **Placement diagnosis** | Vary shape count and position, observing the render/no-render boundary (1438a1e) | Found the non-specified limit of roughly 21 shapes per run by twenty questions |
| **Adversarial hypothesis rejection** | Reject plausible hypotheses such as "PARA_SHAPE must be 58B" with counter-samples | work_report passes at 46B too, establishing that **version consistency**, not length, is the point |

---

## Typed preservation gate

Native writers expose `hwp-preservation-report-v1`, a content-free ledger whose event code, resource
class and disposition are closed enums. Counts are aggregate only. Document text, package names,
container paths, payload fragments and hashes are never part of the public report.

- Writer omissions and unrepresentable payloads are typed loss events. The old `Vec<String>` APIs
  remain compatibility wrappers, but publication policy never parses a `DROP:` prefix.
- Source-free HWP/HWPX authoring rejects every typed loss event before atomic publication.
- Same-format conversion and editing additionally inventory HWP streams/storages or HWPX entries.
  Any unexpected removal or change to an opaque non-target item blocks publication even without
  `--strict`.
- Cross-format `--strict` rejects semantic asset, control, relationship or metadata loss, and
  package/container-level assets the IR cannot carry (HWPX extra entries → HWP; HWP
  XMLTemplate/DocHistory slots → HWPX). `--loss-report <PATH>` writes the ledger as JSON even on
  success. Non-strict conversion may publish only while returning the explicit typed ledger.

This gate prevents known silent loss; it does not certify Hancom compatibility. The current full HWP
rewriter still requires the source-preserving repair and independent Hancom-open checks tracked by
issue #90. Package-surgical HWPX editing remains a later #90 step.

---

## A. The file "corrupt/tampered" gate (rules that block opening at all)

The most fatal layer. When Hancom shows `"파일이 손상되었습니다"` (the file is corrupt) or
`"문서가 변조되었을 가능성 — 보안 수준을 낮춤"` (the document may have been tampered with), nothing is
displayed at all. Hancom checks the consistency between the declared 5.1.x version and the record layout.

| # | Symptom | Cause | Fix | Ground truth | File:function (commit) |
|---|---|---|---|---|---|
| A1 | Immediate "corrupt file" rejection | The cfb crate defaults to CFB **V4** (4096B sectors); Hancom accepts only **V3** (512B). Even byte-identical records are rejected on container version | Force `create_with_version(Version::V3)` and include sample-isomorphic auxiliary streams (DocOptions, _LinkDoc, Scripts, HwpSummaryInformation) | pyhwp passed because olefile reads V4 too: a **blind spot** | `hwp5/src/write.rs:167` (1dcf49d) |
| A2 | "Security warning (possible tampering)" | FileHeader **EncryptVersion=0**. All six genuine samples are unencrypted yet carry `encver=4` (the Hancom 7.0+ save marker); 0 is rejected | `encrypt_version: 4` | All samples measure 4 | `hwp5/src/write.rs:144` (75fb581) |
| A3 | Only 5.1.x synthesis is "tampered" (original round-trips are fine) | The document **declares 5.1.0.1** while records use the old layout: `PARA_SHAPE 54→58B`, `PARA_HEADER 22→24B` (the 5.0.3.2+ merged change-tracking UINT16). Version/layout mismatch reads as tampering | Version gate: synthesis (5.1.x) uses 58/24B, older round-trips (5.0.2.x) stay at 22B | Compared against the good sample hello_world (5.1.0.1) | `hwp5/src/write.rs:emit_paragraph` (1f0139b) |
| A4 | 5.1.x synthesis "corrupt" even at lowered security | **Missing COMPATIBLE_DOCUMENT subtree**, which 5.1.x requires in DocInfo. The first workflow looked only at the older work_report (5.0.2.4, exempt) and wrongly concluded it was optional | When the source is not hwp5, add `COMPATIBLE_DOCUMENT(0x1E,4B=0) > LAYOUT_COMPATIBILITY(0x1F,20B=0) + TRACKCHANGE(0x20,1032B)` right after ID_MAPPINGS | Genuine 가나다 (5.1.1.0) and hello_world both have it; bytes replicated from measurement | `hwp5/src/write.rs:1260-1272` (5844ec8) |
| A5 | Only tables with empty cells are "corrupt with empty body" | An empty paragraph becomes `chars=[0x0d]` and is emitted as `PARA_TEXT=[0x0d]`. Hancom treats **a PARA_TEXT whose only content is the paragraph terminator** as corruption | Emit PARA_TEXT only when `char_count>1`. Empty paragraphs keep `nchars=1` but **omit** PARA_TEXT (implicit terminator) | Bisection isolated empty cell = corruption. Every empty paragraph in genuine work_report and the university document: nchars=1 with no PARA_TEXT | `hwp5/src/write.rs:1605-1618` (4b57b8a) |
| A6 | Markdown conversions with empty GFM table cells are "corrupt" | An empty GFM `\| \|` cell got no PARA_HEADER, giving `LIST_HEADER nparas=0`, which Hancom treats as corruption | On cell close, `flush_paragraph_inner(force=true)` guarantees one paragraph even in empty cells, and short rows are padded | Every genuine file: even empty cells hold one paragraph | `hwp-convert/from_markdown.rs:265` (f64165f) |
| A7 | Corruption plus a pyhwp crash | Empty paragraphs had **zero PARA_CHAR_SHAPE runs**, violating Hancom's invariant `count(PARA_HEADER) == count(PARA_CHAR_SHAPE)` | Pad empty paragraphs with one `(0, current shape)` run | The invariant holds across all genuine files | `hwp-convert/from_markdown.rs:515` (f64165f) |
| A8 | Synthesis judged abnormal | DOCUMENT_PROPERTIES **start numbers (page, footnote, endnote, picture, table, equation) = 0** and PARA_HEADER **instance_id=0** | Apply `max(1)`, and assign unique ids from `0x10000001` on the synthesis path (source ≠ hwp5). Original hwp5 round-trips preserve the original values, including 0 | Compared our byte-identical round-trip against the failing sample_m6. All sample start numbers are 1 and all instance_ids are unique and non-zero | `hwp5/src/write.rs:emit_doc_info` (9efd9ce) |
| A9 | Round-tripped hwp "corrupt" for documents with shapes | Nested gso `SHAPE_COMPONENT` lost its LIST_HEADER/paragraphs and was hoisted to a sibling, destroying the record tree | Preserve and re-emit the original child subtree losslessly through `GenericControl.raw_children` (later replaced by safe degradation, see E6) | The dominant cause of round-trip corruption | `hwp-model/src/control.rs`, `hwp5/src/write.rs` (75fb581) |
| A10 | Residual corruption triggers | PARA_HEADER `ctrl_mask` included CharCtrl (character-like controls such as the paragraph terminator), setting bit13 wrongly; ID_MAPPINGS always padded to 18 (older versions use 16); secd missing FOOTNOTE_SHAPE×2 and PAGE_BORDER_FILL×3; dangling TAB_DEF/NUMBERING references | Exclude CharCtrl from ctrl_mask, use per-version ID_MAPPINGS (5.0.2.x=16, 5.0.3.2+=18), synthesize the required secd children, and synthesize three TAB defaults plus one NUMBERING | Measured bytes of genuine hello_world | `hwp5/src/write.rs` (75fb581, 1f0139b) |
| A11 | Opening an **hwpx** hangs Hancom completely (infinite rows), without even a dialog | **Cause confirmed 2026-07-18 (Phase 2 audit C15)**: a body tab modeled in the IR as `Text('\t')` was emitted as a **raw 0x09 byte** inside `<hp:t>`; a raw control character inside hp:t hangs Hancom. Tab definitions (tabPr) are entirely innocent (E/F/G bisection: base F1 without any tab surgery also hangs, while real-document round-trip E1 is fine, and only the base contains a tab character). The first hypothesis (naked tabItem) was disproved, but the `hp:switch>case(HWPUNIT, pos=X)/default(pos=2X)` structure established then remains a measured fact | Invariant that a tab is always `InlineCtrl(9)` → `<hp:tab/>`: block it at the from_markdown entry, defend on write (sanitize control characters), and normalize on read (backward compatibility for polluted files) | Three rounds of Hancom bisection (D3→E→F/G), a full-part diff of F1 against C1, and triangulation against genuine files | `hwp-convert/from_markdown.rs`, `hwpx/write/section.rs`, `templates.rs` (esc), `read/section.rs` |
| A12 | An **hwpx** body tab is **ignored with zero width** (text runs together; the file opens fine) | `<hp:tab/>` was emitted as a sibling **outside** `<hp:t>`, with no attributes. Hancom recognizes only tabs as **mixed content inside `<hp:t>`**: `<hp:t>개요<hp:tab width=".." leader=".." type=".."/>1</hp:t>` | Emit nested inside t and derive attributes: **type = hwp5 tab kind + 1** (LEFT→1, RIGHT→2 measured; CENTER→3, DECIMAL→4 extrapolated), **leader**: NONE→0, DASH→3 measured (a different order from the table 25 codes, so SOLID→1 and DOT→2 are self-consistent approximations and unconfirmed codes fall back to DASH), **width** = the layout value at save time, which Hancom recomputes on open (proven by width being inversely proportional to text length in genuine files), so an approximation (4000) is acceptable | Hancom testing (D3 zero-width run-together) then reverse-engineering all 91 inline tabs in a genuine `.hwpx` (51 with type2/leader3 = RIGHT/DASH, 40 with type1/leader0 = default) | `hwpx/src/write/section.rs` (tab_xml), `read/section.rs` (in-t tab arm) |

**Lesson for layer A: why testing beat static analysis**

- pyhwp and olefile are **lenient parsers**: they accept V4 containers, nparas=0, PARA_TEXT=[0x0d]
  and start number 0. A1 (CFB V3) taught painfully that passing external validation does not imply
  passing Hancom.
- Corruption had **five simultaneous causes** (75fb581), so fixing one hypothesis left the dialog
  unchanged. Isolation was impossible without parallel multi-agent work plus bisection.
- The decisive insight was **"version consistency, not length"** (A3): the plausible hypothesis that
  "58B is the universal layout" was rejected by the counter-example of work_report passing at 46B.
  Statically we would have concluded "the sample is 58B, so 58B is right".

---

## B. Render consistency: black bars, empty content, multiple paragraphs, vertical position

The file opens, but text is covered by black bars, everything after the first paragraph disappears, or
the spacing between paragraphs collapses. The 5.1.x line-layout cache (PARA_LINE_SEG) and the meaning
of the top bit of nchars are the crux.

| # | Symptom | Cause | Fix | Ground truth | File:function (commit) |
|---|---|---|---|---|---|
| B1 | **Black bars** (a black bar where each character should be; text invisible) | `char_shape.shade_color` defaults to **0, an opaque black shade**. Hancom draws a black background highlight per character cell, hiding the black text | `shade_color=0xFFFFFFFF` (the "none" marker). `shadow_color=0xC0C0C0`, `shadow_gap=(10,10)`, PARA_SHAPE `attr1=0x180` (line break plus line grid) and `border_fill_id=2` also match genuine files | Every genuine file has shade_color ≠ 0 (가나다 = 0x00C0C0C0, hello_world = 0xFFFFFFFF). The **face_id=0 hypothesis was rejected** (genuine hello also has face_id=0 and is fine, so it is harmless) | `hwp-convert/from_markdown.rs:51-57` (dad441b) |
| B2 | 5.1.x body drawn at **zero height**, so empty content or black bars | Body paragraphs lack **PARA_LINE_SEG**. Genuine 5.1.x files have linesegs on 100% of body paragraphs (work_report at 5.0.2.4 recomputes without them). bit31 SET with zero linesegs is a contradiction | Synthesize per-paragraph line layout with `synthesize_linesegs`. The genuine formula: `line_height = font size`, `baseline_gap = base × 0.85`, `line_spacing = base × 0.6` (160%), `flags = 0x00060000` | PARA_LINE_SEG bytes identical to genuine 가나다. Table cell paragraphs are handled recursively | `hwp5/src/write.rs`, `hwp-render/lineseg.rs:synthesize_linesegs` (a7abdfc) |
| B3 | Handling **bit31** of nchars (the revert saga) | bit31 (0x80000000) declares "the PARA_LINE_SEG cache is consistent with the content" (bit31=1 ⟺ that paragraph has linesegs). Declaring consistency with zero cache entries gives black bars or corruption | SET bit31 only when linesegs are actually emitted, then **abandoned (reverted in e41b440)**: inaccurate linesegs re-triggered "tampering" and contradicted multi-paragraph v_pos. The final approach synthesizes linesegs and sets bit31 only on the last paragraph | work_report 73/73 and 가나다 1/1 have bit31=1. **A rule that was reverted once**, undiscoverable statically | `hwp5/src/write.rs` (e32a2a8 → e41b440 → a7abdfc) |
| B4 | **Everything from the second paragraph onward is invisible** | bit31 of nchars was **SET on every paragraph**. bit31 actually marks **the last paragraph of a list (section or cell)**. Setting it on the first paragraph makes Hancom treat that as the last and ignore the rest | `set_last_para_flag`: only the **last paragraph** of each list (section, table cell, text box) gets `chars_flags \| 0x80`; the rest are cleared | Genuine 다문단.hwp (5.1.1.0): of four paragraphs in the section, only the fourth is SET. Single-paragraph 가나다 worked only **by coincidence**, since one paragraph is also the last | `hwp5/src/write.rs:252 set_last_para_flag` (ae73e3c) |
| B5 | Spacing between paragraphs collapses (the gap above a heading disappears) | Synthesized v_pos did not account for paragraph spacing above and below, so everything appeared compressed | Add "gap between paragraphs = previous bottom spacing + this top spacing" to v_pos (excluding the top spacing of a section's first paragraph, where v_pos=0) | Measured vertical misalignment | `hwp-render/lineseg.rs:50-62` (7686444) |
| B6 | Only **multi-page documents** are "corrupt" | Synthesized v_pos accumulated monotonically across the section without a page reset (up to 354408), exceeding the page body height (75686) | For markdown sources, reset v_pos to 0 when `content_h` is exceeded (next page), and move a whole table to the next page when the remainder is insufficient. For hwpx sources, preserve the original linesegarray instead of overwriting it | Genuine university hwpx: body vertpos resets to 0 on each page, the maximum 59668 is below the body height, and reset attempts keep flags 0x60000 | `hwp-render/lineseg.rs:38-48`, `convert.rs` (78d478b) |
| B7 | Only **hwp5 bullet markers** are missing (the file opens; footnotes and numbering are fine) | The synthesized BULLET record followed specification table 42 at 20B with the character at offset 8. The real layout is **25B**: offsets [8..12] hold a 4B numbering character-shape id (0xFFFFFFFF, isomorphic to NUMBERING) and **the bullet character is at offset 12**. Hancom read a null at 12 and drew nothing (table 42 has a history of self-contradictory total length; it is a typo) | Rewrite `make_bullet_data` to the genuine 25B layout, and **fix the reader from @8 to @12** as well; it had been misreading the bullet character of genuine files as the low word of the character-shape id (0xFFFF) | Hancom testing (H2 missing markers) then byte comparison of all five BULLET records in a genuine business-plan .hwp (identical structure, only the character differing) | `hwp5/src/write.rs:make_bullet_data`, `doc_info.rs` (reader fix) |
| B8 | (Rule established) hwp5 strikethrough is **write-only bit18** | CHAR_SHAPE attr bit18 (§4.2.7 table 35, strikethrough) **must not be trusted on read**: corpus measurement shows a change-tracking deletion template (fixed value 0x3c0400f8) sets bit18, making 92% of one file falsely struck through. The reader keeps strike:false | Record `attr \|= 1<<18` only for synthesis where strikethrough is certain (for example markdown origin). **Confirmed in Hancom (2026-07-19 H2): bit18 alone makes Hancom render strikethrough** (shape bits 26 to 29 are unnecessary). The worst case is that it is not displayed, never corruption | Exhaustive corpus attr measurement (plain samples have bit18=0; the polluted pattern is isolated) plus H2 testing | `hwp5/src/write.rs:emit_char_shape` (write), `doc_info.rs` (read stays distrustful) |

**Lessons for layer B:**

- The true cause of black bars (B1) was **the default value of a single UINT32 field**. Our renderer
  and pyhwp leniently read shade_color=0 as "no shading"; only Hancom draws an opaque black
  highlight. The specification carries no warning that 0 means opaque, so it looks harmless statically.
- The meaning of bit31 (B3/B4) is documented only as "line layout cache consistency", while its
  **second meaning as the last-paragraph-of-a-list marker** emerged only from measuring genuine
  다문단.hwp. With single-paragraph samples the two interpretations are indistinguishable (one
  paragraph is also the last), so we passed by luck: **insufficient sample diversity produces static
  misjudgment**.
- B3 is the only rule that was **adopted and then reverted**. Static reasoning ("genuine files always
  have bit31=1, so always SET it") caused black bars in Hancom and was rolled back. Only Hancom
  decides whether a rule is true.

---

## C. Table layout consistency

| # | Symptom | Cause | Fix | Ground truth | File:function (commit) |
|---|---|---|---|---|---|
| C1 | Body text after a table overlaps it and is "corrupt" | The v_pos of the paragraph anchoring a table advanced only by `line_advance` (1600, one line), so the following body text overlapped the table | For paragraphs containing a table, correct to `v_pos = entry value + Σ table heights` | Genuine 첫째문단.hwp: in body + table (3x7) + body, the table advance is 4412 | `hwp-render/lineseg.rs:88` (0e2d568) |
| C2 | Table height constant | `table height = Σ_rows max(top margin + line block + bottom margin) + **566**` (TABLE_BLOCK_PADDING, 2.0mm), where `line block = last cell lineseg.v_pos + line_height` | Implement the table_height formula | 3x7: 3×(141+1000+141)+566 = **4412 exactly**; work_report 1x2 (two-line cell) = 6048 also matches, cross-validating on two samples | `hwp-render/lineseg.rs:194 table_height` (0e2d568) |
| C3 | Empty table cell corruption | Same axis as A6/A7: empty cell nparas=0 or zero char_shape runs | Guarantee one paragraph and one char_shape run per cell | All 60 empty cells in genuine files have nchars=1 | `hwp-convert/from_markdown.rs` (f64165f) |

**Lesson for layer C:** 566 (2.0mm cell block padding) is an **empirical constant absent from the
specification**, adopted only because it matched **exactly and simultaneously** on two ground-truth
files (첫째문단 3x7 = 4412 and work_report 1x2 = 6048). With a single sample it is indistinguishable
from coincidence; cross-measurement is the only way to fix such a constant. (Limitation: it assumes
base 1000 with 160% line spacing, so a different cell font size may make it inaccurate. The current
writer always uses body 1000, so it is safe.)

---

## D. Drawing objects: the ring diagram on page 6 of annual_report

The longest and hardest investigation. Cover and infographic shapes that failed to render in bulk,
but only in Hancom, were isolated over eight rounds using the user's ground truth (테스트2.hwpx,
도형정답지2.hwpx) plus injection and placement diagnosis.

| # | Symptom | Cause | Fix | Ground truth | File:function (commit) |
|---|---|---|---|---|---|
| D1 | **Blank cover page** (many shapes not rendered) | Elements Hancom requires were missing entirely from `<hp:rect>` and friends: `hc:pt0` to `pt3` (the four outline corners), `hc:fillBrush`, `hp:shadow`, pos flowWithText/allowOverlap, textWrap | Emit the four bbox corners pt0 to pt3 for Rect/Ellipse/Arc, always emit fillBrush, emit shadow NONE, `textWrap=IN_FRONT_OF_TEXT`, and flowWithText=0 / allowOverlap=1 for floating shapes | Byte comparison against ground truth 테스트2.hwpx (the only remaining differences were linesegarray, recomputed, and shapeComment, a comment). **Our renderer and pyhwp are fine with document order plus bbox, so this is Hancom-specific** | `hwpx/write/section.rs:661 write_shape_element` (99d6b87) |
| D2 | Text box text is misplaced | Shape text paragraphs lacked `<hp:linesegarray>` (convert defaults to preserve_linesegs=false) | Force `preserve_linesegs=true` on the write_paragraph call inside write_draw_text (shape text only) | Measured on genuine files: Hancom recomputes when a text-box paragraph has no lineseg | `hwpx/write/section.rs:591 write_draw_text` (0e397de) |
| D3 | **Nearly blank cover page** (143 shapes) | The original gso z-orders were all unique (1 to 143), but parse_gso_header read only up to offset 16 and write_shape_element hardcoded `zOrder="0"`, so every shape had z=0. Hancom draws equal z in undefined order, letting cover shapes hide the content | Read z-order (offset 20) in parse_gso_header and emit the real value | work_report, structurally identical and rendering each shape, differed only in z-order | `hwpx/write/section.rs:parse_gso_header` (241f8d3) |
| D4 | Fifteen ellipse/arc rings not rendered | We put pt0 to pt3 (rectangle corners) on ellipses and arcs, but Hancom defines them by **center and axes**. SC_ARC (0x51) was excluded as "not v1" by gso.rs, dropping all four arcs entirely | Ellipse uses `center/ax1/ax2/start-end`, arc uses `center/ax1/ax2`, and pt0 to pt3 are Rect-only. Added SC_ARC (0x51) parsing (BYTE kind + center + ax1 + ax2, 25B) | Ground truth 도형정답지2.hwpx. ★ Side benefit: annual hwp→hwpx **DROP went from 80+ to 0** | `hwpx/write/section.rs`, `hwp-convert/gso.rs:280` (43948ff) |
| D5 | Ellipse rings still not rendered | The only remaining difference was `curSz`. Genuine ellipses and arcs carry `<hp:curSz width="0" height="0"/>` (the "not pre-sized" marker) while we emitted (w,h) | Ellipse and arc use curSz=(0,0); rectangles and others keep (w,h) | Value comparison against 도형정답지2 (center, axes, start-end and fillBrush already matched) | `hwpx/write/section.rs:704` (73910e8) |
| D6 | Donuts and the center circle not rendered (only arcs visible) | The change in an earlier round to "emit #FFFFFF even for no fill" turned the **large unfilled guide circles into opaque white discs** that covered the donuts behind them | Emit fillBrush **only when there is a fill**; omit it for no fill (0xFFFFFFFF), leaving it transparent | Parsing the fill flag showed the original large ellipse has fill=0x0 (no fill). **Our renderer and pyhwp are lenient; only Hancom paints it opaque, so this is Hancom-specific** | `hwpx/write/section.rs:725` (7efac19) |
| D7 | Four donuts not rendered | In a grouped shape (a donut is a grey outer plus a white hole, two ellipses in one gso) both ellipses had **the same z**, and Hancom draws only one and skips the other on a z collision. Duplicated z = 94/96/98/100 gave exactly four donuts | write_gso assigns unique z to multiple shapes within one gso: `zorder * Z_SCALE(64) + shape index` | **Injection diagnosis**: injecting our ellipse into the ground truth rendered fine (unique z), decisively isolating the problem as context (z collision) rather than the element | `hwpx/write/section.rs:820 write_gso` (b472070) |
| D8 | **No rings render at all** (only arcs), the root cause | Hancom renders only about the **first 21 shapes in a single `<hp:run>`** and discards the rest. write_paragraph packed shapes with equal char_shape into one run (35 on page 6), truncating every ellipse from the 22nd onward (positions 22 to 34). Arcs (12 to 15) and polygons (16 to 19) were inside the limit and rendered | Count shapes per run and force a run split at `SHAPE_RUN_LIMIT(12)` even with the same char_shape | **Confirmed in Hancom**: with annual_run분할.hwpx (12 per run) page 6 shows all four donuts, the center circle and the arcs. Contrast with page 3 (29 shapes, ellipses at early positions 7 to 20), which rendered | `hwpx/write/section.rs:94 SHAPE_RUN_LIMIT / write_paragraph` (1438a1e) |
| D9 | Arcs render as **full ellipse loops** (our renderer) | The arc path in shape_draw.rs treated arcs like ellipses (full bbox ellipse) because the reader discarded arc center/ax1/ax2, leaving points empty | The reader captures center/ax1/ax2 and the renderer draws **a quarter elliptical arc as a cubic Bezier** from three points (affine-invariant, so non-perpendicular shear axes stay accurate) | Rendering page 6 of the converted hwpx now matches direct rendering of the original hwp, arcs included | `hwp-render/shape_draw.rs:192`, `hwpx/read/section.rs` (a5aae3f) |
| D10 | Arcs skew into a **pinwheel** (in Hancom) | Hancom's OWPML arc interprets center/ax1/ax2 as **an ellipse with two perpendicular axes** only. The converter baked the gso matrix (rotation plus non-uniform scale, that is shear) into the three points, making the axes non-perpendicular | In gso.rs `geometry()`, **isotropize** the two arc axes to ±45° around their bisector with the mean length (approximating a perpendicular circular quarter arc). Rotation and position are preserved perfectly; only slight ellipticity is lost | Measured on ground truth 도형정답지2 (Hancom interprets perpendicular axes only). Verified the axis dot product ≈ 0 with equal lengths | `hwp-convert/gso.rs:geometry` (0ebeef2) |

**Lessons for layer D:**

- D8 (the roughly 21 shapes per run limit) is **an internal renderer limit that appears in no
  specification**. Our renderer and pyhwp draw everything in document order, so it is unobservable
  statically. Without the experiment design of diffing "page 3 works, page 6 does not", analyzing
  shape positions per run, and then placement diagnosis, it could not have been found.
- D6, D1, D3, D7 and D10 all follow the **"our renderer and pyhwp are lenient, only Hancom is strict"**
  pattern: no fill as transparent, z=0 as document order, bbox without pt0 to pt3, non-perpendicular
  axes as-is. Our tools accept all of it; only Hancom refuses. This is the concentrated reason why
  passing static analysis means nothing.
- **Injection diagnosis (D7) was decisive**: transplanting only our element into the ground truth
  separated "the element is fine, the context (z collision) is the problem". Pure static analysis has
  no way to distinguish element from context.

---

## E. Hyperlinks and fields (the click-through gate)

The link looks blue and underlined but does not respond to clicks. Fields work only when
FIELD_START/FIELD_END pairing, the instance id and the per-kind attr all line up, four layers deep.

| # | Symptom | Cause | Fix | Ground truth | File:function (commit) |
|---|---|---|---|---|---|
| E1 | A hyperlink is **treated as plain text** (not recognized as a link) | The display text had no hyperlink character shape (blue plus underline) | create_hyperlink obtains and applies a `#0000FF` underlined CharShape (including guarding against shade_color=0) | In genuine work_report, "설치하기" has its own charPr | `hwp-convert/field.rs:548,710` (cea2b66) |
| E2 | hwp5 hyperlinks do not work (hwpx does) | The field **instance id was 0**, and Hancom does not treat an id=0 field as a hyperlink | Deterministic non-zero id per URL from an FNV-1a hash | Genuine %hlk id = 0xd707bf6d (non-zero). Contrast with hwpx B4, which worked because its id was non-zero | `hwp-convert/field.rs:472` (87bd62e) |
| E3 | Per-kind field attr mismatch | The hwpx read path emitted attr=0 for both %hlk and %fmu | Per kind: `%hlk=(0x00008800,0)`, `%fmu=(0,0x08)`, others `(0,0)` | Measured on genuine files | `hwp-convert/field.rs:make_field_command_data` (87bd62e) |
| E4 | Blue and underlined, but **clicking does nothing** | %hlk attr was `0x00008800` (copied from work_report, missing bit 0x2000); without that bit Hancom does not follow the link | attr `0x8800 → 0xa800` (measured on genuine files) | A genuine %hlk authored in Hancom is 0x0000a800 | `hwp-convert/field.rs:477` (241f8d3) |
| E5 | **Still no click-through** (the final cause) | The FIELD_END payload was all zeros. Hancom closes a field by **pairing FIELD_START and FIELD_END through ctrl_id**, and a zero END leaves it unfinished | `field_end_payload(ctrl_id)`: the reversed 3B ctrl_id (excluding %) plus 0. hwpx finds the matching START LIFO | Genuine 테스트.hwp: %hlk END = `6b 6c 68 00` ("klh\0"). attr, id, character shape and command were all identical; only END differed | `hwp-convert/field.rs:420 field_end_payload` (39c728c) |
| E6 | Round-tripped hwp is corrupt (documents with text boxes) | The gso re-synthesis SHAPE_COMPONENT template was 252B against 239B in genuine files, off by 13B. Hancom offers no self-validation, so re-synthesis risks recurring corruption | **Safe degradation**: a text box (which holds text) has its paragraphs **hoisted into the body** (preserving text and fields), and purely decorative shapes are dropped | Compared against ground truth. Corruption gone, text preserved | `hwp5/src/write.rs:467 degrade_hwpx_gso` (cea2b66) |

**Lesson for layer E:** a hyperlink click works only when **four conditions hold simultaneously**
(character shape, non-zero id, attr 0xa800, END payload). Four rounds of Hancom testing peeled them
off one at a time (E1 → E2 → E4 → E5). Only once the ground truth provided a minimal counter-example
where "attr, id, character shape and command are all the same and only END differs" (E5) could the
last condition be isolated. Without genuine ground truth we would have stalled at "everything matches,
so why does it not work?". **A ground-truth file that differs by exactly one variable is a truth table.**

---

## F. Open or under investigation (property fidelity is the leading hypothesis)

| # | Symptom | Current state and hypothesis | Evidence and direction | File:function |
|---|---|---|---|---|
| F1 | **Text box drop** (the text-box frame itself is lost in a round-tripped hwp) | Provisionally resolved by **deliberate safe degradation** (E6): text is hoisted into the body and preserved while the shape wrapper is omitted. A real fix (lossless gso re-synthesis) requires **property fidelity**, that is byte agreement with the genuine 239B SHAPE_COMPONENT across every field (border, fill, attr, zorder, desc). That is the leading cause | The 252B template was off by 13B and caused corruption. Every field of the genuine 239B record needs measuring. Hancom offers no self-validation, so only testing can verify | `hwp5/src/write.rs:degrade_hwpx_gso` |
| F2 | **Page overflow** (synthesized multi-page documents overflow vertically) | Markdown sources are defended by the content_h reset (B6), and hwpx sources preserve the original linesegarray. The remaining risk is font shaping line breaks differing slightly from genuine files so page boundaries drift; **line layout property fidelity** (seg_width, line_height, spacing) is the leading cause | Verified exact reproduction of the genuine university document's max v_pos of 59668; markdown gives 72712 < 75686. Needs wider testing across font sizes and multi-column documents | `hwp-render/lineseg.rs:synthesize_linesegs / compute_linesegs` |

**Direction for layer F:** the leading hypothesis is that both open items **resolve naturally once
property fidelity (full-field agreement with genuine bytes) is high enough**. Every rule solved so far
followed the same principle, that matching one differing field at a time against genuine ground truth
makes Hancom accept the file, so the remainder should be narrowed the same way, by repeated testing
and by obtaining more ground truth. Static analysis cannot supply the criterion for "what fidelity is
sufficient"; only Hancom decides.

---

## Appendix 1. Ground-truth assets

| Ground truth | Version | Truth table for | Rules established |
|---|---|---|---|
| hello_world.hwp | 5.1.0.1 | A minimal well-formed 5.1.x sample | A3 (58/24B), A4 (COMPATIBLE), B1 (shade), B2 (lineseg) |
| 가나다.hwp | 5.1.1.0 | A user-authored single paragraph | A4, B1, the B2 lineseg formula |
| 다문단.hwp | 5.1.1.0 | bit31 distribution across paragraphs | B4 (only the last paragraph gets bit31) |
| 첫째문단입니다.hwp | 5.1.1.0 | body + table + body | C1/C2 (table height 4412 exactly) |
| work_report.hwp | 5.0.2.4 | The older version (exempt rules) | The A3 version gate, C2 cross-validation (6048), E1/E4 |
| 테스트.hwp | - | Plain text plus a Hancom-authored hyperlink | E5 (FIELD_END ctrl_id) |
| 테스트2.hwpx | - | Rectangle plus text | D1 (pt0 to pt3, fillBrush, shadow, pos, textWrap) |
| 도형정답지2.hwpx | - | Ellipse plus arc | D4 (center/axes), D5 (curSz 0,0), D10 (perpendicular axes) |
| 타원진단, annual_run분할 and others | - | Injection and placement diagnosis files | D7 (z collision), D8 (run limit) |

## Appendix 2. File and function index

- `hwp5/src/write.rs`: `create_with_version` (A1), `encrypt_version` (A2), `emit_paragraph`
  (A3 58/24B, A5 PARA_TEXT), `emit_doc_info` (A4 COMPATIBLE, A8 start numbers and ids),
  `set_last_para_flag` (B4), `degrade_hwpx_gso` (E6/F1)
- `hwp-convert/from_markdown.rs`: `default_header` (B1 shade_color), `flush_paragraph_inner`
  (A6/A7 empty cells)
- `hwp-render/lineseg.rs`: `synthesize_linesegs` (B2/B6 page reset), `table_height` (C2 566),
  paragraph spacing (B5)
- `hwpx/write/section.rs`: `write_shape_element` (D1), `write_draw_text` (D2), `parse_gso_header`
  (D3 z), `SHAPE_RUN_LIMIT` / `write_paragraph` (D8), fillBrush (D6), curSz (D5), `write_gso`
  (D7 Z_SCALE)
- `hwp-convert/gso.rs`: SC_ARC parsing (D4), `geometry` (D10 isotropization)
- `hwp-render/shape_draw.rs:192`: cubic Bezier arcs (D9)
- `hwp-convert/field.rs`: `make_field_command_data` (E2/E3/E4), `field_end_payload` (E5),
  hyperlink charPr (E1)

---

## G. Source-preserving native HWP edits

| # | Symptom | Cause | Fix | Ground truth |
|---|---|---|---|---|
| G1 | A text edit corrupts an otherwise genuine complex document | Re-emitting a whole section moves level-sensitive typed-control children such as `CTRL_DATA` around `PAGE_DEF` and `PAGE_BORDER_FILL` | Copy the source CFB and merge unchanged paragraph/control subtrees into only the changed section stream | BodyText bisection plus a complex genuine proposal fragment; all unchanged subtrees are byte-identical |
| G2 | Removing stale line layout or globally regenerating it causes a corruption warning or layout churn | A changed 5.1.x paragraph needs a content-consistent `PARA_LINE_SEG`, while unchanged paragraphs must retain their original caches | Synthesize layout for the edited document, then restore every semantically unchanged native paragraph from the immutable snapshot | Hancom opened no-op, metadata, text, paragraph insertion, table-cell edit and image insertion outputs without warnings |
| G3 | Same-format no-op loses auxiliary streams or changes CFB metadata | A fresh container assembly cannot reproduce unknown streams, storages and directory properties | Treat no-op as an exact file copy; on edits patch only streams named by the mutation plan | Exact-file comparison plus preservation of `MemoExtended`, untouched BinData and unknown entries |

The Hancom acceptance procedure is now part of the native writer contract, not an optional smoke
test. Internal re-read and typed preservation checks must pass first, followed by real Hancom opens of
the six edit classes in [the checklist](../hancom-verification-checklist.md).

---

## Overall lessons: why this project stakes everything on Hancom testing and ground truth

1. **Lenient tools give false passes.** pyhwp and our own renderer can pass 100% while Hancom refuses
   (A1 CFB V3, D6 no fill, D8 the run limit all pass in our tools). "Re-reads without warnings" is
   necessary, never sufficient.
2. **Implicit invariants absent from the specification dominate.** bit31 as the last-paragraph marker
   (B4), roughly 21 shapes per run (D8), the 566 cell padding (C2) and the black highlight of
   shade_color=0 (B1) are written down nowhere. Genuine bytes are the only specification.
3. **Version consistency beats field presence.** The rule is not "58B is the layout" but "a 5.1.x
   declaration needs the 5.1.x layout" (A3). One sample cannot distinguish length from consistency;
   only cross-samples of an older version (work_report) and a newer one (hello) reveal the real rule.
4. **A minimal counter-example is a truth table.** E5 isolated the final variable thanks to ground
   truth where "everything is the same and only END differs". A file the user made in Hancom by
   changing exactly one variable is the experimental control.
5. **Hancom judges a rule, and a wrong rule gets reverted.** B3 (always SET bit31) looked right
   statically but caused black bars in Hancom and was rolled back (e41b440). Genuine Hancom rendering
   is the only final arbiter.
