[한국어](hancom-verification-checklist.ko.md) · [English](hancom-verification-checklist.md)

# Hancom verification checklist (the write paths of rounds 13 to 23)

A table for opening the files that `scripts/hancom-regression.sh` generates under a private
directory **directly in Hancom Office** and judging whether they are accepted.
These files already pass our own verification (re-reading without warnings plus a visual render check)
with our reader and renderer, so **the only thing left unverified is whether Hancom accepts these
synthesized or converted bytes without corruption or alteration**.

> **Round 25 re-verification**: in the first Hancom test, A2 and A4 (gso re-synthesis round-trip) were
> corrupt and the B hyperlinks did not take effect. A2 and A4 were changed to **safe degradation** for
> text boxes (preserving the text in the body, omitting the shape wrapper), removing the corruption,
> and hyperlinks were given a **blue underlined character shape** on the display text. Those two are
> what this round re-checks.

## Regenerating the artifacts

Run `scripts/hancom-regression.sh /private/directory` to regenerate every artifact this checklist
still verifies, from the release binary, in one command. Per-series manual commands are no longer
the procedure. The destination must be outside this repository: the script refuses the repository
root and any path under it, including a symlink or relative alias of it, and the run writes nothing
into the working tree.

Every artifact is gated on self-reread (`hwp cat` with no warning) and structural validation
(`hwp validate --json` reporting valid with no warnings) before it is published, and the run writes
a `hancom-regression-index-v1` JSON index last. An index file in the destination therefore means
the whole set passed. Each index row carries the file name, its checklist series item, the exact
command that produced it, its SHA-256 and the two gate columns, so an artifact can always be mapped
back to the table entry that judges it.

An artifact whose input is local only (the series A approval document, the series J bordered
source, the series M and N private sources, and the distribution-document read gated on
`HWP_CORPUS_DIR`) is reported as a skip naming the missing input and gets no index row, because a
missing private input is not a regression. A case that fails gets no index row either, and fails
the run, so a gap is never silent.

Series E, F and G are not regenerated. That bisection is closed (see the E section below) and its
diagnostic outputs were removed from the folder to prevent false readings.

The run creates no Hancom observation and no receipt. It writes, beside each artifact, an
`<artifact>.policy.json` naming the one receipt that will bind to that artifact, and an empty
`receipts/` directory for those receipts to land in, so that once a real observation exists
`hwp certify <artifact> --policy <artifact>.policy.json --report <directory>` is a single command
with no hand editing. Never prefill a pass receipt.

## Common verdict (every file)

- **Pass**: the file opens with no warning. (Failure signals: the popups "파일이 손상되었습니다",
  "보안 수준을 낮춰 열겠습니까?" and "문서가 변조되었습니다".)
- **Pass**: body text, tables, text boxes and shapes are visible and the layout resembles the original.
- A blank screen or only black bars on open is a failure (the historical defect pattern).

## A. The full pipeline on real documents (every feature on genuinely complex content)

| File | What it tests | Pass looks like | Failure (likely cause) |
|---|---|---|---|
| `A1_work_report_변환.hwpx` | hwp → our hwpx (tables, text boxes, hyperlinks, headers/footers, images) | Tables, the text box ("나눔글꼴...") and the bottom logo appear as in the original | Missing text boxes or shapes, broken tables |
| `A2_work_report_왕복.hwp` | hwp → hwpx → our hwp (round 25 gso safe degradation) | **Opens with no corruption.** The text box text ("나눔글꼴...") and the hyperlink survive in the body (the box itself is omitted) | A corruption popup |
| `A3_annual_report_변환.hwpx` | hwp → our hwpx (142 shapes and text boxes) | The cover and inner text boxes plus decorative shapes appear | Shapes missing in bulk |
| `A4_annual_report_왕복.hwp` | hwp → hwpx → our hwp (round 25 gso safe degradation) | **Opens with no corruption.** The cover text box text ("Annual Report 2012" and so on) survives in the body (shapes and boxes omitted) | A corruption popup |
| `A5_품의_변환.hwpx` | hwp → our hwpx (%fmu formulas, tables, page numbers) | Table formula values and page numbers are correct | Missing fields or page numbers |
| `A6_품의_왕복.hwp` | The approval document round-tripped to hwp | Table formulas survive (the sum recalculates on F5) | Corruption means a field or table synthesis defect |

## B. Minimal per-feature files (isolating a cause on failure)

| File | What it tests | How to check |
|---|---|---|
| `B1_책갈피.hwp` | hwp5 bokm bookmark creation (round 13) | [입력] → [책갈피] (Ctrl+K, B) lists **검증책갈피** |
| `B2_책갈피.hwpx` | hwpx `<hp:bookmark>` writing (round 14) | The same |
| `B3_하이퍼링크.hwp` | hwp5 %hlk creation (round 15) | In **Hancom**, the text is blue and underlined, and Ctrl+click goes to hancom.com |
| `B4_하이퍼링크.hwpx` | hwpx fieldBegin HYPERLINK (round 15) | The same |
| `B5_복합.hwp` | A bookmark and a hyperlink together | Both the bookmark list and the working link |

## C. Character effects and summary information (implemented 2026-07-15: GE-α, GE-β4, [12](design/12-feature-gaps.md) §0.5)

Checking that the character effects and summary information the hwpx writer newly emits are accepted
in Hancom. In addition to the common verdict (opening with no corruption popup), check each effect
below.

> **Re-verification (2026-07-15), ✅ confirmed passing:** the first test found C6 numbering not
> displayed, C8 dates missing and C9 subject missing; these were fixed by comparison against genuine
> files (emitting the paragraph heading link, the PID 0x14 Korean date string in the summary
> information, and content.hpf metadata in the genuine format), and re-testing **confirmed C6, C8 and
> C9 all display correctly**. The whole C series (character effects and summary information) is
> confirmed in Hancom.

| File | What it tests | How to check |
|---|---|---|
| `C1_그림자.hwpx` | Emitting charPr shadow (IR bit11 plus color and offset) | Paragraph text has a grey (#808080) shadow, and the character shape dialog (Alt+L) shows shadow enabled |
| `C2_외곽선.hwpx` | charPr outline (bit8, SOLID) | Characters appear outlined |
| `C3_양각음각.hwpx` | emboss (bit13) and engrave (bit14) | The first paragraph embossed, the second engraved |
| `C4_첨자.hwpx` | supscript (bit15) and subscript (bit16) over **a partial range** | Only the 2 in "x2" is superscript and only the 2 in "H2O" is subscript (smaller with a vertical shift) |
| `C5_밑줄모양.hwpx` | Three underline shapes | The three paragraphs are underlined dotted, double and circle-dotted (the paragraph labeled "물결" is actually circle-dotted, substituted because the reader does not support WAVE) |
| `C6_번호형식.hwpx` | A user number format plus **the paragraph-to-numbering link** (GE-α8, fixed in the second round) | The numbers **"제5조." "제6조." "제7조."** actually appear before each article paragraph |
| `C7_글자효과통합.hwpx` | All seven effects above combined (labeled per paragraph) | Each labeled paragraph shows its effect |
| `C8_요약정보.hwp` | Eight hwp5 summary information fields plus the date string (GE-β4, aligned with genuine PID 0x14 in the second round) | File > Document information shows the title "실기 검증 요약정보 문서", author "홍길동", subject, description, last saved by, and **the date "2026년 7월 15일 수요일 오후 6:00:00"** (the creation time in KST) |
| `C9_요약정보.hwpx` | Eight hwpx content.hpf metadata fields (aligned with the genuine format in the second round, including subject, keywords and date) | The same, and **the subject and date must appear too** |

## D. Seal stamping and user tabs (implemented in the third round on 2026-07-15: GM-7, GC-4, [12](design/12-feature-gaps.md) §0.5)

In addition to the common verdict, check the following. **The floating placement of a seal is the only
compatibility-sensitive point in this batch** (the 07 §F area), so D1 and D2 are the crux.

> **Third-round Hancom test (2026-07-16):** ✅ **D1 passed** (the overlap is confirmed, so the hwpx
> seal is confirmed in Hancom). ❌ D2 pushed "(인)" to the right; the cause was confirmed and fixed:
> the text-wrap bits were SQUARE (pushing the body aside). Cross-checking the specification's §4.3.9.1
> bit table against the genuine annual attr gave in-front-of-text (5) plus lifting the body area
> restriction (attr 0x040a6310 → 0x04aa4310). **D2 needs re-testing.** ❌ D3 still hangs, so the
> **E-series bisection results are needed** (below).

## E. The tab hang bisection plus a request for seal ground truth (2026-07-16)

⚠ **Save every other open document before opening each file** (there is a hang risk). Only record
whether each file opens or hangs; checking the content is unnecessary.

**E round results (2026-07-16):** E1 opens, E2 hangs, E3 hangs. Interpretation: a real-document
round-trip (holding both items and references) is fine, so the writer in general is innocent. On the
synthesized side (the D3 base), both definitions-only (E2) and references-only (E3) hang, so rather
than "items or references alone are guilty", **the base itself or the common factor of "adding a
fourth tabPr"** is the likely culprit. The F and G control groups isolate it:

| File | What it isolates | Interpretation |
|---|---|---|
| `F1_베이스만.hwpx` | The D3 base as-is with no tab surgery (the default three tabPr, zero references) | **A hang confirms the base is guilty (tabs are irrelevant)**, to be pinned down by diffing against a C file. Opening means the tab side is guilty |
| `F2_빈4번탭.hwpx` | The base plus an **empty** fourth tabPr (no items, no references) | A hang means the mere existence of a fourth tabPr is the cause |
| `G1_정품autoTab.hwpx` | F2 plus the genuine autoTab pattern (id1 autoTabLeft=1) | F2 hanging while G1 opens means the autoTab flag rule is the cause |

**F/G round results (2026-07-18): F1, F2 and G1 all hang, confirming the base is guilty and tabs are
entirely innocent.**

**★ Cause confirmed and fixed (2026-07-18, Phase 2 audit C15 plus 07 A11):** the hang was caused by
the body tab character being emitted as a **raw 0x09 byte** inside `<hp:t>`. Only the D3-family base
contained a tab character, which is why the C files were fine, and the tabPr definitions were innocent
from the start. The fix established the invariant that a tab is `InlineCtrl(9)` → `<hp:tab/>` (blocked
at the entry, defended on write, normalized on read). The regenerated D3 measures zero raw 0x09 bytes
and two `<hp:tab/>`. **E2, E3, F1, F2 and G1 (diagnostic outputs from the polluted base) were removed
from the folder to prevent false readings**; only E1 (healthy) remains.

**→ The single remaining re-test target is `D3_사용자탭.hwpx`.** Pass: it opens without hanging and
"이름/직책/서명" aligns to the tab positions (left 30mm, center 80mm, with a dash leader).

**Fourth-round fix (2026-07-18):** it opened, but the tabs were ignored at zero width (the text ran
together). Comparison against ground truth confirmed the cause: Hancom recognizes only a mixed-content
tab **inside** `<hp:t>` (with width, leader and type attributes) (07 **A12**). Corrected by emitting
it nested inside t.

**✅ Final Hancom pass (2026-07-18):** tab alignment confirmed, so **the whole D series (D1, D2, D3) is
confirmed in Hancom.** Seal stamping (hwpx and hwp5) and the user tab round-trip are settled by
testing.

**A request for seal ground truth (for D1 and D2, two minutes):** in Hancom, type `결재란: (인)` in a
new document, insert any image, set the object's **text placement to in front of text**, drag it to
overlap "(인)", and save it as both `도장정답지.hwpx` and `도장정답지.hwp` into
`~/Documents/hwp-verification/`. Replicating the placement coordinate system and properties Hancom
actually uses is the only remaining frontal approach.

| File | What it tests | How to check |
|---|---|---|
| `D1_도장.hwpx` | `edit --seal` floating (in front of text) seal placement | A red circle (18mm) **overlaps** the text "결재란: (인)" while the "(인)" text remains visible. Clicking the seal selects it as an object (a picture) |
| `D2_도장.hwp` | The same feature through the hwp5 synthesis path | The same. **Opening with no corruption popup matters especially here** (hwp5 floating picture synthesis) |
| `D3_사용자탭.hwpx` | The user tab definition round-trip (GC-4) | Body tab characters align to **left 30mm and center 80mm** with a dotted leader displayed. (Check that both tab definitions exist in Shape > Paragraph shape > Tab settings) |

## H. The markdown import round-trip (2026-07-19: GI-1, GI-2, [12](design/12-feature-gaps.md) §0.5)

Checking that footnotes and numbered lists synthesized from markdown are accepted in Hancom. **The
compatibility-sensitive point in this batch is the synthesized footnote** (the footnote LIST_HEADER
uses a template substitute with no ground truth, and the BULLET record is synthesized per §4.2.9, not
compared against genuine files).

| File | What it tests | How to check |
|---|---|---|
| `H1_md왕복.hwpx` | md → hwpx: footnotes, strikethrough, ordered lists, nested bullets | A footnote reference number (a small superscript) appears in the body with **the footnote content at the bottom of the page**. Strikethrough displays. Lists display as numbers ("1. 2. 3.") and bullets (nested indentation) |
| `H2_md왕복.hwp` | The same content through the hwp5 synthesis path | The same. **Opening with no corruption popup matters especially here** (synthesized footnotes and NUMBERING/BULLET records). Strikethrough is not recorded as formatting on the hwp5 path (text only), by design |

**First Hancom test (2026-07-19):** ✅ **H1 fully correct, confirming synthesized hwpx footnotes and
lists in Hancom.** ❌ H2 opened with correct footnotes and numbered lists but **only the bullet marker
(*) was missing**.

**Second fix (2026-07-19):** comparing all five BULLET records of a genuine file (제주한라대
사업계획서.hwp) byte by byte confirmed the cause: the real layout is 25B (character at offset 12) but
we synthesized per the typo in specification table 42 (20B, character at 8), so Hancom read a null at
the right position (registered as 07 **B7**). Both the writer and the reader were corrected (the
reader also stopped misreading bullet characters of genuine files).

**Second re-test: ✅ the marker displays, confirming BULLET in Hancom.** ❌ Strikethrough ("지운 글")
had been excluded by design, so a **third fix (2026-07-19)** recorded strikethrough attr bit18
(§4.2.7 table 35) on the hwp5 synthesis path as **write-only**. The reader continues to distrust it
(corpus measurement: bit18 is polluted by the change-tracking deletion template, making 92% of one
file falsely struck through, so it cannot be adopted on read). The worst case is that it is not
displayed, never corruption.
**Third re-test: ✅ strikethrough displays, confirming the whole H series (H1, H2) in Hancom
(2026-07-19).** bit18 alone was proven to make Hancom render strikethrough (07 **B8**). The markdown
import round-trip (footnotes, strikethrough, numbered and bulleted lists) is settled by testing on
both the hwp5 and hwpx paths.

| File | What it tests | How to check |
|---|---|---|
| `I1_md이미지코드.hwpx` | markdown image embedding plus inline code formatting (GI-3, GI-4) | (Optional; low risk because it reuses verified paths.) Confirmed when the picture displays and the code appears in Dotum with a light grey background |

## J. Page border cross-conversion (2026-07-19: GC-2, [12](design/12-feature-gaps.md) §0.5)

| File | What it tests | How to check |
|---|---|---|
| `J1_쪽테두리.hwpx` | Cross-converting a genuine hwp with real borders into our hwpx (⚠ **a blocking test**: emitting real `pageBorderFill` properties as XML from hwp5 raw is a new emission shape) | It opens with no corruption and **a solid black rectangular border 5mm inside the page edge** appears on every page |

**Hancom results (2026-07-19, judged visually across all 34 pages, correcting an earlier sampled
verdict):**

- ✅ **I1 passed** (images plus fixed-width code on a grey background), confirming GI-3 and GI-4.
- ✅ **J1 page borders passed**: all 34 pages have correct borders with no corruption, confirming the
  GC-2 page borders in Hancom.
- ❌ **A separate new defect found in J1 (pages 4 and 6)**: **a long table spanning pages does not
  split and runs through the bottom border** (overlapping the page number, clipping rows). The other 32
  pages are fine. Registered as the new gap **GE-8**.

**The GE-8 fix history (2026-07-19):** the first linesegarray hypothesis was rejected in Hancom. The
second attempt **confirmed the real culprits by direct comparison with ground truth**
(`표분할정답지.hwpx`, the same document saved as hwpx by the user in Hancom): 1. emitting treatAsChar
fixed to 1 (the original page-spanning table uses 0, that is floating; a table treated as a character
cannot split) and 2. recomputing sz height by summing rows (twice the original value). Corrected by
inheriting and emitting the TABLE object common properties in the IR, giving **an exact match on 33/33
tables against the ground truth** (the two problem tables: treatAsChar=0, sz 47339×54801 and
47622×17021).

**→ ✅ J1 finally passed in Hancom (2026-07-19): table splitting confirmed, closing GE-8.**
The whole J series (page borders plus page-spanning tables) is confirmed, completing GC-2, GC-3 and
GE-8.

## K. Cell merging and column manipulation (2026-07-19: GK-1, GK-2, [12](design/12-feature-gaps.md) §11)

Checking that merged tables **created** by the editing primitives are accepted in Hancom (a blocking
test, since creating a merged table is a new combination; the structure was verified to match the
rules measured across 1,816 genuine tables). ⚠ The first verdict is whether a corruption or alteration
popup appears.

| File | What it tests | How to check |
|---|---|---|
| `K1_셀병합.hwpx` | Merging the top two cells of a 2×2 table (hwpx) | It opens with no corruption and the first row displays as **a single merged cell**, with the contents of both cells joined |
| `K2_셀병합.hwp` | The same merge (hwp5 synthesis, where LIST_HEADER consistency is the gate) | The same. **No corruption popup matters especially here** |
| `K3_열조작.hwpx` | Adding a column to a three-column table then deleting another | It opens with no corruption and the three-column table displays correctly (no cell misalignment) |

**✅ First Hancom pass (2026-07-19, judged across three captures):** K1 and K2 display merged cells
correctly (including the joined content, with no hwp5 corruption), and K3's column structure is exact.

**A minor fix (2026-07-19):** K2 showed less space above the cell text than below (stuck to the top).
Comparison against genuine files confirmed the cause: the vertical alignment bits 5-6 of the cell
LIST_HEADER default to **CENTER (0x20)** in genuine files, but only the markdown → hwp synthesis
emitted TOP (0) (hwpx uses the CENTER constant and was unaffected). The shared cell synthesis path was
fixed, and among the existing test cases only K2's output changed (for the better).

**✅ Second re-test passed (2026-07-19): vertical centering confirmed, closing the K series entirely
and confirming GK-1 and GK-2 in Hancom (including the CENTER default rule for cell vertical
alignment).**

## L. Equation emission (2026-07-27: GE-14, GE-15, [12](design/12-feature-gaps.md) §5.1)

Checking that the `<hp:equation>` the hwpx writer newly emits is accepted in Hancom. **Because no
ground truth (a genuine hwpx containing an equation) could be obtained, the equation-specific
attributes (`version`, `baseLine`, `baseUnit`, `font`) are standard estimates**, so for this case the
decisive question is not "does it open" but **"is the equation actually typeset and visible"**.

| File | What it tests | How to check |
|---|---|---|
| `L1_수식.hwpx` | Three inline equations (a fraction plus a radical, a summation, XML special characters) | Three equations appear **side by side like characters** at the end of the "수식 자리:" paragraph: 1. a fraction and radical in the form `a/b + √(x²+y²)`, 2. a summation with limits above and below Σ, and 3. `x < y & y > z` with **the inequality signs and ampersand intact** (their absence indicates an esc-to-entity round-trip defect). Double-clicking an equation should open the equation editor with the original script |

What to report on failure: whether the equation appears as **an empty box or question mark**, whether
**only a placeholder is there with nothing visible**, and whether the equation editor opens. Which of
the three it is points at a different wrong attribute (`font`/`baseUnit` versus `version` versus child
element order). Obtaining a single genuine saved file would let us replace the attributes immediately.

## M. Source-preserving native HWP edits (issue #90)

Use a private, genuinely complex HWP containing tables, images and opaque auxiliary streams. Do not
commit the source or derived outputs. Run each case from the same immutable source snapshot.

| Case | Required result |
|---|---|
| No-op same-format convert | Exact file bytes; opens without a warning |
| Metadata edit | Only summary information changes; opens without a warning |
| Text edit | Unchanged controls and assets remain byte-identical; edited text is visible |
| Paragraph insertion | The inserted paragraph is visible and surrounding layout remains stable |
| Table-cell edit | Only the target cell content changes; table geometry remains intact |
| Image insertion | Existing assets remain byte-identical; one new relationship and payload appear |

For every output, verify `FileHeader`, `MemoExtended`, scripts, document options, unknown entries and
untouched BinData before opening it in Hancom. A parser-only pass is insufficient.

## N. Package-surgical HWPX edits and cross-format loss gates (issue #90)

Use a private, genuinely complex HWPX (36 package entries, 24 BinData items including 7 WMF, 5
`hp:container` groups). Do not commit the source or derived outputs. The harness is
`run-authoring-validation-c.sh` (private, not committed); the oracle is Hancom Office HWP 12.30.0
on macOS.

| Case | Required result |
|---|---|
| Metadata edit (HWPX→HWPX) | ZIP entry set identical; every opaque entry byte-identical; opens without a warning |
| Text edit | The same, with the edited text visible |
| Paragraph insertion | The same, with the inserted paragraph visible and the surrounding layout stable |
| Table-cell edit | The same, with only the target cell content changed |
| Image insertion | The same plus one new BinData item (media 24→25); opens without a warning |
| Non-strict hwpx→hwp convert | The typed loss report lists the removed HWPX package entries; opens without a warning |
| Non-strict hwp→hwpx convert | All 24 media preserved (previously 9); opens without a warning |
| HWP edit (the source-preserving writer) | The container round-trips; opens without a warning |

CLI-side gates to check before opening: ZIP entry-set identity for every surgical edit, byte
identity of every opaque entry (media 24→24, containers 5→5), `hwp validate` clean on all outputs,
strict hwpx→hwp failing closed (`binary_asset_removed`, `control_removed`,
`hwpx_package_entry_removed`, `opaque_control_unrepresentable`) and strict hwp→hwpx failing closed
(`control_removed`, `metadata_value_removed`).

**Hancom results (2026-08-14):** all eight outputs (5 HWPX edits, 2 non-strict conversions, 1 HWP
edit) opened with no corruption or repair dialog.

## Reporting results

Tell us pass or fail per file and, on failure, the popup message and symptom; we then fix only the
failing items using genuine comparison patterns (measured on 가나다 and 다문단). Passing items move
from unverified to confirmed in Hancom. When the C series passes, the items resolved on 2026-07-15 in
[12](design/12-feature-gaps.md) §0.5 become confirmed in Hancom.

## O. Phase 2.2 official profiles and eight-level numbering (unverified)

`scripts/hancom-regression.sh` regenerates this set with the rest of the checklist; it delegates to
`tools/gen_verification_set.sh` in its Phase 2.2 mode and absorbs the twelve labelled documents,
one HWP and one HWPX for each canonical profile, into its own index. The generator only proves
internal reread and structural validation; it creates no Hancom pass receipt and makes no Hancom
acceptance claim.

For every generated HWP and HWPX, perform all seven observations below. Record the actual result
against the matching SHA-256 in the private index. A skipped, unavailable, or failed observation is
not a pass and blocks this phase.

1. **Open without warning:** Open in genuine Hancom Office. Confirm there is no corruption, repair,
   tampering, or security-lowering warning.
2. **Eight visible marks:** Confirm the ordered list marks are `1.`, `가.`, `1)`, `가)`, `(1)`,
   `(가)`, `①`, and `㉮`.
3. **Evidenced continuation:** At levels 2, 6, and 8, inspect sibling items 14 and 15. Confirm the
   post-`하` continuation is `거` (including the native circled form at level 8).
4. **Source order:** Confirm `SENTINEL BEFORE LIST`, the complete list, and `SENTINEL AFTER LIST`
   remain in that order.
5. **Profile layout:** Check the profile row below for body font, size, line spacing, A4 margins
   (top 20, bottom 10, left 20, right 20 mm), and header/footer area.
6. **Page number:** Confirm the stated off state, or that bottom-center page numbers use `- N -`.
7. **Record actual native behavior:** Record format, SHA-256, indentation, and the actual HWP5
   encoding behavior. The expected HWP5 contract is `safe/direct`; do not substitute an inferred
   or literal result.

| Profile | Body font | Body size | Line spacing | Header/footer | Page number |
|---|---|---:|---:|---:|---|
| official | Malgun Gothic | 12 pt | 160% | 0 mm | off |
| report | HCR Batang | 15 pt | 160% | 15 mm | on, `- N -` |
| plan | HCR Batang | 15 pt | 160% | 15 mm | on, `- N -` |
| notice | Malgun Gothic | 15 pt | 160% | 10 mm | on, `- N -` |
| minutes | HCR Batang | 14 pt | 130% | 0 mm | off |
| press | HCR Batang | 14 pt | 160% | 10 mm | on, `- N -` |

After each actual observation, create a private `hancom-verification-receipt-v1` with the observed
result, application, timestamp, verifier, and `artifact_sha256`, then validate it through the
private certification policy that requires `document.hancom_open`. Never prefill a pass receipt.
