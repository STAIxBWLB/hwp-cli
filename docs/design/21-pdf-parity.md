[한국어](21-pdf-parity.ko.md) · [English](21-pdf-parity.md)

# PDF parity contract (Hancom Office 2024)

> **Status:** active contract. Tracked by
> [issue #79](https://github.com/STAIxBWLB/hwp-cli/issues/79). This document is the durable
> definition the parity harness reads: metric set, thresholds, data policy and non-goals. The
> issue body sequences the work (PR 1–9); this document defines what "par" means.

The goal: `hwp convert --to pdf` and `hwp render --format pdf` emit a PDF that a Korean office
worker accepts as interchangeable with Hancom Office 2024 한글's own **파일 → PDF로 저장하기**
for general documents (공문, 보고서, 표, 양식, 이미지, 도형, 수식) — visually faithful and with
selectable, searchable, copyable text.

## 1. Scope

**Regime A — 한글이 저장한 문서 → PDF.** HWP files cache their own line layout in PARA_LINE_SEG,
and the renderer replays that cache (Regime A). Line positions are Hangul's own; the delta is ink
plus a bounded set of structural gaps. This contract covers Regime A only.

**Regime B — hwp-cli가 생성한 문서 → PDF** (synthesized linesegs, 금칙처리, Latin word keeping,
widow/orphan, 어울림 text wrapping) is a separate issue and out of scope here.

### 1.1 Non-goals

Quoted from [release-readiness](../release-readiness.md) and issue #79:

- The release must not claim that the smoke fixtures cover every real document form, provide
  Hancom pixel parity, or prove cross-platform-identical raster bytes.
- Bookmarks (`/Outlines`) and clickable links (`/Annots`): Hancom's PDF emits neither, so these
  **exceed** parity.
- PDF/A-2b and tagged PDF: excluded; tagged PDF is additionally blocked by the deliberately
  domain-free DisplayList.
- No "Hancom pixel parity" claim in release copy. The README "No Hancom parity claim" wording may
  change to a limited "validated Hancom Office 2024 PDF profile" only after the full
  general-document profile passes every gate in §4.
- Charts, OLE, video, word art, memos, master pages, vertical writing and advanced equation
  constructs are reported as explicit omissions, never silently dropped.

## 2. The oracle

The public gate currently has one intentionally bounded oracle: an owner-authored, anonymized
one-page source document and its PDF produced by Mac Hancom HWP. It was saved with HWP 12.30.0
(build 6446) on macOS 26.6.1 (build 25G76), through the default 파일 → PDF로 저장하기 path.
The resulting PDF records `macOS 버전 26.6.1(빌드 25G76) Quartz PDFContext` as its producer, is
A4, and has exactly one page. This provenance is a fixture-specific baseline; it is not a claim
about Windows Hancom, every Hancom build, or universal cross-platform parity.

The committed source/oracle and the exact OFL font bytes used by the Linux candidate are pinned
below. Font files are fetched by `scripts/fetch-pdf-parity-fonts.sh` and are not committed:

| artifact | SHA-256 |
|---|---|
| `public-safety-rfp-p1.hwp` | `8c4e62fb8166828eaddd2d0d304732acd88484ba8526692545b316985e0c0aba` |
| `public-safety-rfp-p1.hwpx` | `a5b6bb59bc4492f81deeada58cd4b6c0a13579d06afe39e0aaec2687a3eaaf5c` |
| `public-safety-rfp-p1.pdf` | `8fe8a4a4f3f6640248a1efde26421c7134374b4203363acca1a5967b2c0602e7` |
| `Noto Sans CJK KR 2.004 Regular` | `6bcb2a0703aa137e874fc2dffa85f6c21ba9a67fa329e81b8c801663af7e992a` |
| `Noto Sans CJK KR 2.004 Bold` | `26d0c6748500a0444844280b308f5b62c7ae92ac6c6ac88148e502dd211eb52a` |

The manifest records the public profile as `hancom-hwp12-macos-12.30.0-build-6446-quartz` and
the expected Poppler binary as `pdfinfo version 24.02.0`. The Quartz producer string is
provenance only; PDF metadata is not a parity metric for this fixture.

The renderer's document-level output contract retains these six catalog/Info fields. They are
not a claim that the one-page Quartz oracle contains the same metadata:

- `/Lang (ko-KR)`, `/PageLayout /SinglePage`, XMP `/Metadata`,
  `/OutputIntents` (GTS_PDFA1 + sRGB IEC61966-2.1, embedded ICC), `/MarkInfo <</Marked false>>`
- `/Info` limited to Author, Creator, Producer, CreationDate, ModDate, PDFVersion
- Fonts: Type0 + CIDFontType2 + FontFile2 + Identity-H + ToUnicode (the scheme hwp-render already
  implements)
- Absent: `/Outlines`, `/Annots`, `/PageLabels`, `/AcroForm`, `/Encrypt`, `/Shading`, `/Pattern`,
  `/ExtGState`, `/SMask` (Hancom flattens gradients — our banded approximation is not a defect)
- PDF 1.4; not a tagged PDF

**Status (2026-08-13, PR 3):** all six catalog/Info features are emitted. `/Lang (ko-KR)`,
`/PageLayout /SinglePage`, `/MarkInfo <</Marked false>>`, a minimal XMP `/Metadata` packet
(dc/pdf/xmp), `/OutputIntents` (GTS_PDFA1 + embedded ICC), and `/Info` limited to the six keys:
Author only when the document has one, Creator/Producer `hwp-cli <version>`, CreationDate/ModDate
converted from the document's FILETIME metadata (never wall-clock, preserving two-run byte
determinism), and a `PDFVersion` pair. The header version is PDF 1.4. The embedded profile is the
ICC Registry's `sRGB2014` v2 profile, committed as
`crates/hwp-render/assets/sRGB2014.icc.hex`, with its source and redistribution terms in the
adjacent `LICENSE-sRGB2014.txt`. The decoded 3,024-byte profile is pinned by SHA-256
`384b832de3412066743b52a75ee906b6fb9fb8d9e09e936fc2c43223815c6e0a`. These fields implement the
captured structural contract; the public one-page oracle gate is now exercised in CI, while a
broader Hancom corpus remains future work.

## 3. The five-metric set (priority order)

Both sides are vector text, so pixel diff metrics are dominated by font substitution and
antialiasing — engine artifacts, not fidelity. The decisive metrics need no pixels:

| # | Metric | Tool | Catches |
|---|---|---|---|
| 1 | Embedded font list | `pdffonts` | Font substitution (§5) |
| 2 | Page count delta | `pdfinfo` | Table splitting, overflow, furniture regressions |
| 3 | Per-page extracted text equality | `pdftotext -layout`, normalized | Missing content, wrong pagination, broken ToUnicode |
| 4 | Ink bbox delta (`dx`, `dy`) | Rasterize once at 150 DPI | Systematic margin/baseline offset |
| 5 | `bad_pixel_pct` / `MAE` | `hwp diff` | Tiebreaker only |

Rasterization for metrics 4–5 uses the same pinned Poppler (`pdftoppm -png -r 150`) for both the
Hancom PDF and our PDF.

**Implementation status 2026-08-14 (PR 4):** the batch runner exists —
`scripts/pdf-parity.sh run` scores every manifest case into the committable numeric scoreboard
under `fixtures/pdf-parity/public/scoreboard/` (schema-validated, names + SHA-256 + numbers
only), `selftest` verifies the harness without an oracle, and `hwp diff --format json` /
`--ours-png` is the per-page raster metric source. Scoring fails closed unless the manifest,
Poppler version, font files, and source/oracle digests match their pins; validated outputs are
published as one rollback-protected set. The public corpus now contains one owner-authored,
anonymized one-page oracle, and the fixed `ubuntu-24.04` CI job fetches the pinned OFL fonts,
builds `hwp`, and scores both HWP and HWPX inputs against it. This is a one-case regression gate,
not a universal parity certification.

**Implementation status 2026-08-13 (PR 5):** the border-fidelity batch landed (GG-5, GG-6,
GG-17, GG-21, GG-24). Cell, paragraph, page and diagonal borders honor `BorderLine.line_type`
via `hwp-render/src/border.rs` — the dash family through `Stroke.dash`, the double family as
offset parallel strokes — and `Item::Line` was deleted in favor of `Item::Path`. HWPX column
dividers (`hp:colLine`) round-trip and render; the hwp5 coldef divider parse is deferred
(unconfirmed byte offsets). hwp5 raw-path shapes apply dash patterns and arrowheads, previously
hwpx-only. Tab advance is unified on `tab::next_tab` across `items_width`, `place_wrapped` and
`compute_linesegs`. Double-line weight splits are approximations to confirm in the Hancom
verification round.

**Implementation status 2026-08-13 (PR 6):** the character-decoration batch landed (GG-8,
GG-9, GG-10, GG-11, GG-22). Emphasis dots (attr bits 21 to 24, all 13 kinds) render per glyph
and hwpx `symMark` round-trips; underline shapes and above-character underlines apply the
0-based decor table (dash family, double/weighted offsets, cubic wave) via
`border.rs::decor_strokes`; strikethrough shapes (bits 26 to 29) share that table and round-trip
in hwpx; character shadows use the real `shadow_gap` percentage in all three backends; and
character-level borders/backgrounds (`CharShape.border_fill_id`) render per run with the
background emitted before the glyphs. Decoration metrics (y offsets, wave constants, emphasis
mark sizes, char-box extents, the (0,0) shadow-gap fallback) are placeholders for the Hancom
verification round.

**Implementation status 2026-08-14 (PR 7):** the advance-affecting batch landed (GG-20, GG-3,
GG-4, GG-18) together with the single re-baseline. Inline control characters now carry width
(HYPHEN shapes `-`, NB_SPACE keeps a no-break source while using the ordinary space advance,
and FW_SPACE gets a fixed 1em advance); justification distinguishes justify/distribute/divide
(trailing-gap and last-line rules per mode, Hancom confirmation pending); letter spacing is computed in the
HWPUNIT integer domain with half-up rounding; and synthesized line spacing honors all four
modes (ratio / fixed / margin-only / minimum) via the version-aware `line_spacing_type`.
`MAX_BAD_PIXEL_PCT` tightened 0.60 → 0.30 as the re-baseline promise — the first committed
scoreboard (owner action from PR 4) validates or adjusts it. GG-20/GG-3/GG-4 move pixels on
genuine Hancom-saved files; GG-18 is synthesis-only.

**Implementation status 2026-08-14 (PR 8):** the images-and-fills batch landed (GG-15, GG-7,
GG-23). `Item::Image` gained its contract change — crop, flip, rotation, brightness, contrast —
parsed from the hwp5 picture record and the hwpx `hp:pic` attributes and honored by all three
backends (png Transform + pre-crop, pdf matrix + clip, svg transform + clipPath; the pdf JPEG
fast path and the svg zero-copy embed are kept when no pixel effect applies). Picture effects
(spec tables 108-116) are parsed and reported as the typed `picture_effects_unsupported`
warning, not rendered. Cell, paragraph and character backgrounds honor hatch and gradient
fills (`Fill::Hatch` is new: png segments, svg `<pattern>`, pdf flattened lines). Converted
ellipses render as arcs/pies/chords via the axis-vector `ellipse_arc_path`. Approximations
flagged for the Hancom round: brightness/contrast curve, hatch spacing/weight, arc kind and
sweep mapping, rotation sign conventions; hwp5 flip bits are unlocated.

**Implementation status 2026-08-14 (PR 9):** the final roadmap batch landed (GB-13, GG-14,
GG-16). Captions are now parsed end to end: a `Caption` IR (side, direction, gap, width,
last_width, paragraphs) on Table/Picture/GenericControl; hwp5 discriminates caption
LIST_HEADERs per pyhwp's `TableCaption`/`GShapeObjectCaption` model (a LIST_HEADER before
the TABLE record is the caption; a direct gso LIST_HEADER child likewise) and re-synthesizes
them; hwpx `<hp:caption>` round-trips with its side/fullSz/width/gap/lastWidth attributes
(direction from `subList@textDirection`); table, picture, generic-shape, and unsupported-GSO
captions retain reading order and render on the attribute side with their gap. Endnotes leave
the anchor page: layout splits page notes by `NoteKind`, keeps the reservation footnote-only,
and paginates accumulated endnotes through a closing block without colliding with final-page
footnotes. Odd/even furniture: all head/foot controls retain their apply value (data bits 0-1:
BOTH/EVEN/ODD), and the renderer selects exact printed parity followed only by a BOTH
fallback. Missing apply data maps to BOTH, preserving ordinary single-header sections without
leaking an odd/even-only entry onto the opposite page. Approximations for the Hancom round: caption listflags
upper bits, the spec table 71/72 length inconsistency (table 72 followed), and FIRST-page
furniture (no source-format representation).

## 4. Gates

### 4.1 Normative Hancom-oracle gate — current one-case public profile

This is the normative gate for the committed one-page public profile. CI runs it on fixed
`ubuntu-24.04` after installing the distribution `poppler-utils` package, verifying
`pdfinfo version 24.02.0` against the manifest, fetching the exact OFL fonts, building `hwp`,
and comparing both public HWP/HWPX inputs with the committed oracle. It is not a universal or
Windows parity claim, and it is separate from the structured-corpus gate in §4.2.

- Page count: exact match
- MediaBox delta ≤ 0.5 pt
- `dx`, `dy` ≤ 2 px each at 150 DPI; `ink_ratio` in 0.97–1.03
- `bad_pixel_pct` ≤ 5 %, MAE ≤ 5
- Feature ROI ink precision/recall ≥ 0.95 each. An ink pixel matches when the other image has
  ink within the manifest-pinned 10 px square radius at 150 DPI (4.8 pt); this absorbs vector
  hinting and cumulative glyph-advance rounding only. Page-level `dx`/`dy`, raster error, text
  order, and ink-ratio gates remain independent, so the ROI tolerance cannot authorize a shifted,
  missing, or extra feature.
- Font substitution, unsupported omission, fatal render issue: **0**
- `pdffonts`: every candidate font embedded/subset with Unicode support. Oracle rows remain
  diagnostic because the Mac producer's page-number font is embedded without a Unicode cmap.
- `pdftotext -layout` normalized output matches the expected visible text and order
- Byte-identical PDF on two runs with identical input and fonts

### 4.2 Current structured-corpus self-consistency gate

The structured-corpus run (`scripts/check-structured-corpus.sh`) currently enforces backend
self-consistency under the pinned Noto Sans KR:

- PDF page count equals the PNG backend page count.
- Every displayed PDF glyph round-trips through its ToUnicode mapping to the complete expected
  logical text; this is a full mapping check, not only a required-text substring check.
- Two PDF renders with identical input and fonts are byte-identical.

It does not currently rasterize PDF and PNG output against each other. Therefore `dx`, `dy`,
`ink_ratio`, `bad_pixel_pct`, and MAE are not structured-corpus thresholds; they are enforced by
the pinned public Hancom-oracle gate in §4.1.

### 4.3 Manifest-declared gate exclusions (v2)

The v2 manifest accepts an optional `gate_exclusions` array: unique names drawn from the eight
v2 gates (`page_count`, `media_box`, `text`, `fonts`, `render_issues`, `raster`, `roi`,
`determinism`), at most eight entries. A local private profile uses it to declare a gate
measured-but-not-blocking — for example `fonts` when the approved font faces are not yet
available on the measurement host. Unknown gate names, duplicates, or a non-list value fail the
run before any case is measured.

Exclusions relax eligibility only, never measurement:

- Every gate is still measured and reported; `passed_gates`/`failed_gates` are unchanged.
- A case is eligible when it has no `blocking_failed_gates` — the failed gates minus the
  excluded set. Scoreboard eligibility and the runner's exit code follow the same rule.
- Each scorecard and the scoreboard echo `excluded_gates` (sorted) and `blocking_failed_gates`
  as required fields, so an exclusion-assisted pass cannot be mistaken for a full pass.
- When `excluded_gates` is empty the schemas keep the legacy strict rule (eligible if and only
  if no gate failed and all eight gates passed), so the public CI profile — which declares no
  exclusions — is validated exactly as before.

**Status (2026-08-16, epic #90 PR 8b, in flight):** contract and runner implemented; the public
manifest declares no exclusions. Any exclusion used by a private profile must be listed and
justified in the release notes (see [release-readiness](../release-readiness.md)).

### 4.4 Residual private-profile gaps (2026-08-15 run)

A fresh private composite run on merged main (post PR 7a/7b) passes `page_count` (13 == 13),
`media_box`, `render_issues`, and `determinism`, and declares `gate_exclusions: ["fonts"]`. The
blocking residual gates are `text`, `raster`, and `roi`; their measured shape is (content-free
aggregates only):

- `text`: 1/13 pages exactly equal, but per-page character multisets are 99.17% identical
  overall (141 of 17,093 characters differ) and no Hangul syllable differs at all — the deltas
  are a small fixed set of symbol/ASCII characters plus extraction-order differences (sequence
  similarity 0.63–0.99 per page). So the gap is ordering and a handful of symbol mappings, not
  content loss.
- `raster`: bad-pixel ratios 0.145–0.234 (threshold 0.05) and MAE 19–28 (threshold 5) are
  distributed uniformly across all 13 pages with ink ratio ≈ 1.0 on 11 of them, rather than
  missing content. Two pages deviate in ink ratio (1.42 / 0.86), indicating layout-level
  shifts on those pages. (The earlier reading — that the 8 substitutions alter glyph shapes
  everywhere — is disproven in §4.5.)
- `roi`: 3 of 4 ROIs pass; only the page-2 diagram region fails (precision 0.849, recall 0.887
  vs the 0.95 threshold), correlating with the same page's ink-ratio shift — a structural
  layout difference on the diagram page, not a global regression.

### 4.5 Root causes behind the residual gaps (2026-08-16 diagnosis)

A per-page overlay run (candidate vs oracle rasters through `hwp diff`, private-only outputs)
plus a byte-level comparison of the oracle's embedded font subsets replaced the §4.4
hypothesis with measured causes:

- **The substitutions are oracle-equivalent, not a defect.** Every face the oracle embeds
  except `ArialMT` is present in the pinned font directory with an identical `fontRevision`,
  `unitsPerEm`, and hhea metrics. The document's dominant body face is declared with a
  `substFont`, the oracle host did not have that face either, and Hancom followed the same
  declaration — so our substitutions resolve to the same faces the oracle embedded. The
  `fonts` gate's `substitution_free` criterion is therefore unreachable for this case by
  construction, and the criterion (not the render) is what has to change. Installing the
  missing originals would move our render *away* from the oracle.
- **First-line indent was applied outside the text area (fixed).** A genuine `line_seg` stores
  only the paragraph's left margin in `horzpos` — never the first-line indent, not even for a
  hanging indent (verified across paragraphs with and without a left margin). The renderer
  treated the hanging space as living to the left of the line box, so every list paragraph —
  most of the document's body — was drawn one hanging width (14–18pt) left of Hangul's
  position, and the list marker one marker width further left still. The hanging space is
  Hangul's marker slot *inside* the text area: marker at the box edge, first line clearing it,
  following lines indented by the hanging width. After the fix every page's leftmost ink lands
  within 1px of the oracle's (was up to 29px off).
- **Remaining, still open:** line advances diverge along a line (ours runs both narrow and
  wide depending on the character shape, overflowing some table cells), the page-number footer
  is not rendered at all, some vector-image text lands at the page origin, and image bullets
  (`useImage`) fall back to the declared character. These are the next targets; the raster
  metrics are computed without alignment, so each of them inflates `bad_pixel_pct`/`mae`
  across the whole page.

## 5. The font gate (F1)

Hancom embeds 함초롬바탕/함초롬돋움; we resolve through fontdb and can fall back to a generic
sans-serif. Every substitution changes every advance, every wrap decision and every glyph shape,
so **a parity number measured under substituted fonts is meaningless**.

- `RenderIssueReport::font_coverage()` aggregates FontMatched / FontSubstituted / FontMissing /
  FontSubsetFallback counts; the CLI render report also publishes hash-only requested/resolved font
  identities, requested weight state, face index, and resolution completeness.
- `FontCoverage::substitution_free()` is part of the hard gate: no parity figure may be published
  for a case whose render was not substitution-free. Resolution must also be complete, and every
  resolved font-byte hash must belong to the manifest-pinned set. Only a directly requested family,
  including a proven secondary alias on the selected face, may count as matched.
- Missing coverage or identity evidence is a gate failure, as is any font in either PDF that
  `pdffonts` does not report as embedded, subset, and Unicode-capable.

## 6. Pagination truth (F3)

Hancom already tells us where its page breaks are: `LineSeg.flags` bit0 (페이지 첫 줄) and bit1
(단 첫 줄) are parsed into the IR. The renderer reads these bits as the first-class page/column
break signal. An unflagged `v_pos` reset is only a soft boundary and may pack into remaining CELL
fragment capacity; the reset heuristic for synthesized linesegs (flags `0x0006_0000`) stays
subordinate to explicit flags. Page-indexed comparison is meaningless while a table clips across pages, so
pagination correctness (including table splitting, PR 2) is a **precondition** for measurement,
not a consumer of it. **Implementation status 2026-08-13: PR #81** splits tables at legal row
boundaries per `Table.attr` pageBreak policy, preserves row-spanning cells, and supports repeated
header rows. **PR #89 follow-up:** `CELL` policy can also continue a single row at an existing
cached line boundary, including the last line that fits the current-page remainder when the cache
has no explicit page-reset flag. Fragment content is emitted once with continuation borders, and
declared row height beyond the cached content is preserved as page-capacity blank continuation
fragments. A row span that can move as a unit is kept together on a fresh page. An unsafe cache,
a row span that intersects an internally fragmented row, or a span too tall for a fresh page is
surfaced as typed `table_cell_fragmentation_incomplete` instead of being silently clipped. The one-page public profile is now compared in CI; broader Hancom corpus
coverage remains pending, so this is not a universal parity certification.

## 7. Data policy

- **Public corpus** (`fixtures/pdf-parity/public/`): owner-authored / anonymized source documents
  only, as HWP/HWPX pairs covering basic text, paragraphs, lists, tables, columns, headers/footers/page
  numbers, footnotes/endnotes, images, shapes, equations and composite reports. `.gitignore`
  ignores the entire `fixtures/pdf-parity/` tree by default and re-allows only HWP/HWPX files under
  `public/source/`, Markdown/JSON recipe files under `public/recipes/`, `public/manifest.json`,
  numeric JSON/CSV scoreboards under `public/scoreboard/` (plus the root `scoreboard.json` and
  `scoreboard.csv` names), and exactly
  `public/oracle/public-safety-rfp-p1.pdf`. All other public oracle PDFs/PNGs and every
  private-corpus path stay ignored. The manifest pins the Mac HWP provenance, A4/one-page
  profile, source/oracle SHA-256, exact OFL font SHA-256 values, Poppler version and 150 DPI.
- **Private corpus** (`HWP_PDF_PARITY_CORPUS_DIR`): real composite documents. Reports contain
  hashes and aggregate metrics only — never originals, PDFs or absolute paths.
- **Public oracle exception:** the single committed PDF above is an owner-authored, anonymized
  one-page Mac Hancom HWP 12.30.0 build 6446 capture, produced on macOS 26.6.1 build 25G76 with
  Quartz PDFContext and default Save as PDF settings. It is a bounded regression fixture, not a
  universal or Windows claim. Its source/oracle/font hashes and reproduction steps live in
  `fixtures/pdf-parity/public/recipes/public-safety-rfp-p1.md`.
- Private Hancom artifacts, third-party real documents, third-party Hancom exports, Hancom
  specification copies and every other oracle PDF/PNG remain forbidden. The checked-in recipe,
  manifest and numeric scoreboard contain only procedure, provenance, pins, hashes and aggregate
  numbers apart from this one explicitly allowed public oracle.

## 8. Anti-pattern guards

- No Hancom Office/COM runtime dependency.
- No PDF page flattening to images — vector text and ToUnicode stay.
- No layout computation duplicated in the PDF backend; DisplayList serialization differences only.
- No silent omission of unsupported objects; placeholders are never counted as correct output.
- No universal-parity claims beyond the general-document profile.

## 9. Done means

- The committed scoreboard is monotonically non-worsening from PR 4 through PR 9 on the same
  public source/oracle/font/Poppler pins; flat rows are valid when a fixture does not exercise a
  batch, and any regression is invalid.
- `MAX_BAD_PIXEL_PCT` was tightened once at PR 7 from 0.60 to 0.30 and is no longer its original
  placeholder.
- The fixed Ubuntu public gate passes both HWP/HWPX cases against the committed one-page oracle.
- Font substitution is zero on every scored case, and the exact source/oracle/font hashes remain
  unchanged from the manifest and recipe.
- Any committed scoreboard is schema-valid, path-free, and generated only after the pinned gate
  passes; broader corpus coverage remains explicitly scoped work.
- `scripts/check.sh`, the PDF unit/integration tests and the public parity gate all pass in their
  applicable environments.
