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

Ground truth is a PDF produced by **최신 패치를 적용한 Windows용 한컴오피스 2024 한글** via
파일 → PDF로 저장하기. Inspection of genuine Producer `Hancom PDF 1.3.0.550` files established
the document-level parity surface — six catalog/Info features:

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
captured structural contract; exact Hancom value equality still requires the local oracle run.

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

**Implementation status 2026-08-13 (PR 4):** the batch runner exists —
`scripts/pdf-parity.sh run` scores every manifest case into the committable numeric scoreboard
under `fixtures/pdf-parity/public/scoreboard/` (schema-validated, names + SHA-256 + numbers
only), `selftest` verifies the harness without an oracle, and `hwp diff --format json` /
`--ours-png` is the per-page raster metric source. Scoring fails closed unless the manifest,
Poppler version, font files, and source/oracle digests match their pins; validated outputs are
published as one rollback-protected set. The 3–5 Hancom baseline exports and the first committed
numbers are the remaining owner action.

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

## 4. Gates

### 4.1 Normative Hancom-oracle gate — future public corpus

This is the normative target for future public Hancom-oracle scoring. It is not the current
structured-corpus gate; the current self-consistency checks are listed separately in §4.2.

- Page count: exact match
- MediaBox delta ≤ 0.5 pt
- `dx`, `dy` ≤ 2 px each at 150 DPI; `ink_ratio` in 0.97–1.03
- `bad_pixel_pct` ≤ 5 %, MAE ≤ 5
- Feature ROI ink precision/recall ≥ 0.95 each
- Font substitution, unsupported omission, fatal render issue: **0**
- `pdffonts`: every font embedded/subset with Unicode support
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
`ink_ratio`, `bad_pixel_pct`, and MAE are not current structured-corpus thresholds. Those are
future/normative raster requirements for the Hancom-oracle gate in §4.1, activated only when the
pinned oracle PDF/PNG fixtures and comparison harness are available.

## 5. The font gate (F1)

Hancom embeds 함초롬바탕/함초롬돋움; we resolve through fontdb and can fall back to a generic
sans-serif. Every substitution changes every advance, every wrap decision and every glyph shape,
so **a parity number measured under substituted fonts is meaningless**.

- `RenderIssueReport::font_coverage()` aggregates FontMatched / FontSubstituted / FontMissing /
  FontSubsetFallback counts; the CLI render report prints the coverage line.
- `FontCoverage::substitution_free()` is the hard gate: no parity figure may be published for a
  case whose render was not substitution-free.
- Missing coverage is a gate failure, as is any font in either PDF that `pdffonts` does not report
  as embedded, subset, and Unicode-capable.

## 6. Pagination truth (F3)

Hancom already tells us where its page breaks are: `LineSeg.flags` bit0 (페이지 첫 줄) and bit1
(단 첫 줄) are parsed into the IR. The renderer reads these bits as the first-class page/column
break signal and falls back to the `v_pos` reset heuristic only for synthesized linesegs (flags
`0x0006_0000`). Page-indexed comparison is meaningless while a table clips across pages, so
pagination correctness (including table splitting, PR 2) is a **precondition** for measurement,
not a consumer of it. **Implementation status 2026-08-13: PR #81** splits tables at legal row
boundaries per `Table.attr` pageBreak policy, preserves row-spanning cells, and supports repeated
header rows. Hancom Office oracle comparison remains pending, so this is not yet a parity
certification.

## 7. Data policy

- **Public corpus** (`fixtures/pdf-parity/public/`): owner-authored / anonymized source documents
  only, as HWP/HWPX pairs covering basic text, paragraphs, lists, tables, columns, headers/footers/page
  numbers, footnotes/endnotes, images, shapes, equations and composite reports. `.gitignore`
  ignores the entire `fixtures/pdf-parity/` tree by default and re-allows only HWP/HWPX files under
  `public/source/`, Markdown/JSON recipe files under `public/recipes/`, `public/manifest.json`,
  and numeric JSON/CSV scoreboards under `public/scoreboard/` (with the root `scoreboard.json` and
  `scoreboard.csv` names also allowed). Public oracle PDFs/PNGs and every private-corpus path stay
  ignored. The manifest pins the exact Hancom build, Windows version, PDF settings, font SHA-256,
  source/oracle SHA-256, Poppler version and 150 DPI.
- **Private corpus** (`HWP_PDF_PARITY_CORPUS_DIR`): real composite documents. Reports contain
  hashes and aggregate metrics only — never originals, PDFs or absolute paths.
- **Hancom-derived artifacts are never committed** — exported oracle PDFs/PNGs and private source
  documents stay local. The checked-in recipe, manifest and numeric scoreboard contain only the
  procedure, pins, hashes and aggregate numbers; they are not oracle artifacts. Baselines are
  produced manually (3–5 cases) via 파일 → PDF로 저장하기 → `pdftoppm -png -r 150` and kept local.
- Third-party real documents, Hancom spec copies and private oracle PDFs are never committed.

## 8. Anti-pattern guards

- No Hancom Office/COM runtime dependency.
- No PDF page flattening to images — vector text and ToUnicode stay.
- No layout computation duplicated in the PDF backend; DisplayList serialization differences only.
- No silent omission of unsupported objects; placeholders are never counted as correct output.
- No universal-parity claims beyond the general-document profile.

## 9. Done means

- The committed scoreboard improves monotonically from the first baseline (PR 4) through PR 9.
- Font substitution is zero on every scored case.
- `MAX_BAD_PIXEL_PCT` is tightened exactly once, in the advance-affecting re-baseline PR (PR 7),
  rather than left at its placeholder.
- `scripts/check.sh`, the PDF unit/integration tests and the public parity gate all pass.
