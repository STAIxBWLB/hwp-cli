# Changelog

Release notes are written in English only. This file is the source of truth, and
`scripts/release_notes.sh <version>` extracts the section for a version verbatim as the GitHub
Release body.

The workspace `Cargo.toml` `[workspace.package] version` is the single source for version numbers.

---

## [Unreleased]

**Added**

- Password-protected HWP5 and HWPX input now enters the normal read, convert and render paths when
  the user supplies a password. The CLI supports command-local `--password` and
  `--password-stdin` on `cat`, `convert` and `render`; MCP supports a per-call password on
  `hwp_read`, `hwp_convert` and `hwp_render` without caching it in the session.

- The supported profiles are evidence-bound: HWP5 EncryptVersion 4 CFB streams and the observed
  HWPX ODF AES-256/PBKDF2/checksum profile. Wrong and absent passwords share
  `HWP_PASSWORD_REQUIRED_OR_INVALID`, and certificate encryption, signatures and DRM keep their
  existing typed refusals. Credentials, decrypted bytes and parser details do not enter logs,
  reports or receipts.

- A private-corpus contract, profile-evidence schema and seven-case content-free receipt contract
  gate the feature. The final release candidate passed genuine ASCII HWP5/HWPX baselines, a
  distinct non-ASCII password success, wrong/absent cases for both formats, clean-worktree
  inventories and direct Hancom Office comparison.

**Changed**

- Documentation now matches v0.10.0. The READMEs predated v0.9.0 and v0.10.0 and had become wrong
  rather than merely incomplete: sixteen MCP tools instead of seventeen, `hwp grep` and `hwp lint`
  missing from the command reference, three `cat` formats listed where the MCP table one screen
  below listed five, distribution documents described as refused although they have been read since
  v0.8.7, and a roadmap three of whose six items had already shipped. The official-document
  authoring layer and the DOCX and ODT export paths were documented nowhere at all. All of that is
  corrected, and a new "Official-document authoring" section covers the six profiles, the eight
  templates, the 두문/결문 frame flags, the slots and fill workflow and the ten lint rules.

- `TODO.md` and `TODO.ko.md` are removed. They froze on 2026-07-19, targeted a gitignored directory
  that is absent from any checkout, and described a plan the project no longer follows. The half of
  their content that documents the specification rather than a local transcription became errata
  E-8 to E-10 in `docs/design/19-hwp5-spec-supplement.md`. The public roadmap is now the README
  plus `docs/design/12-feature-gaps.md`.

- Four gap-catalog statuses that committed code contradicted are corrected: GA-2 (reading
  distribution documents, 2026-08-20), GN-3 (`hwp lint`, 2026-08-23), GN-8 (slots spanning runs,
  2026-08-26), and the summary row that still listed GH-3 to GH-5 as pending on the HTML path. The
  design overview's document index, language note and status section were likewise brought back in
  line with the catalog.

**Fixed**

- Password-unlocked HWPX entries remain available for every read during one package invocation,
  including the second `version.xml` access. Rewriting a decrypted document also replaces the
  source encryption manifest with the ordinary plaintext manifest, so HWPX-to-HWPX conversion no
  longer publishes plaintext entries under stale encryption metadata or compares authenticated
  opaque plaintext against source ciphertext.

- Password-unlocked HWP5-to-HWP conversion uses the plaintext synthesis path instead of asking the
  source-preserving writer to reopen the encrypted container without a credential. HWPX profile
  validation also caps aggregate PBKDF2 work at eight million iterations before any entry is
  decrypted, preventing a many-entry package from multiplying the per-entry limit into a CPU DoS.
  Strict HWP5 conversion still reports and refuses opaque source streams that plaintext synthesis
  cannot preserve; non-strict conversion records the loss and publishes the authenticated content.

- Compressed `BinData` in password-protected HWP5 now follows the ordinary reader's bounded
  try-DEFLATE path before entering the IR. Images therefore remain usable in cross-format output,
  and native HWP synthesis does not double-compress an already-compressed payload.

- An encrypted HWP5 with no supplied password now returns the same stable credential refusal even
  when its EncryptVersion is unsupported. The typed unsupported-profile detail is exposed only
  after a credential is explicitly supplied.

- Owner-corpus validation now checks the resolved credential bytes against the declared ASCII or
  non-ASCII charset before profile discovery. A mislabeled secret can no longer satisfy the
  distinct non-ASCII evidence role, and mismatch errors expose neither the value nor its reference.

- `hwp cat --preview` now authenticates protected HWP5/HWPX before exposing preview text, including
  encrypted HWPX preview entries. `--password-stdin` reads through a 64 KiB cap so an oversized or
  unterminated credential stream cannot grow process memory without bound.

- `hwp fill` can now fill a `{{slot}}` that inline formatting split across text runs, so it fills
  everything `hwp slots` reports. The two commands read a document differently — `slots` walks the
  IR, where a paragraph's characters are already joined, while `fill` rewrites the raw section XML
  — and a name like `{{이*름*}}`, which compiles to `{{이` / `름` / `}}` in three runs, was listed
  by one and refused by the other. Such a placeholder is now coalesced into its first run before
  replacement, so the value inherits that run's character shape.

  A slot still cannot cross a line break or a paragraph boundary: those genuinely end it, and
  joining across them would invent placeholders that are not there. Names nobody asked to fill are
  left byte-for-byte alone.

## [0.10.0]

**Added**

- `--doc-foot 수신자=…` (MCP `doc_foot`) adds the multi-recipient list to the 결문. It is the one
  결문 row emitted only when supplied, so a document that never names it keeps the exact bytes it
  had before the key existed.

**Fixed**

- `hwp new --template gongmun-basic` generated a document byte-identical to `gian-external`: when
  the 두문/결문 fields moved into the frame builder, the recipient line lost the marker that was
  the templates' only difference. `gongmun-basic` is now modelled as what it is — the
  multi-recipient 공문서, whose 두문 reads the fixed `수신자 참조` and whose 결문 carries the
  `{{수신자}}` list (§5 두문 2). Its `{{수신}}` slot is replaced by `{{수신자}}` accordingly. A new
  test fails if any two templates ever generate the same bytes again.

**Changed**

- `hwp new --template <slug>` now produces a fully framed document on its own. A template carries
  the canonical profile it is written for and its native 두문/결문 frames, whose values default to
  the template's own `{{slot}}` tokens — so the one-command form yields real 두문/결문 **tables**
  instead of loose paragraphs, and every field stays fillable (`hwp fill` substitutes inside table
  cells). `--preset` and the frame flags (`--doc-head`, `--doc-foot`, `--notice-head`,
  `--notice-foot`, `--press-head`) now **override** one template default each instead of being
  refused; `--template` and `--from` stay mutually exclusive. Same behavior over MCP `hwp_new`.

  Every slot name the templates previously documented still resolves. `gian-external` and
  `gongmun-basic` additionally gain `{{협조자}}`, and `gongmun-basic` gains `{{접수번호}}`
  `{{접수일자}}`. The `[관인 — 전자문서시스템이 삽입]` and `[결재란 영역 …]` notes are gone from
  the skeletons: the 결재/협조 placeholder rows the 결문 frame renders (D-04) say the same thing
  structurally.

**Fixed**

- The bundled skill guide said an unmatched `hwp fill --set` slot is ignored. It is an error:
  `hwp fill` fails closed and publishes nothing unless `--allow-partial` is given. The guide now
  matches the shipped behavior.

## [0.9.0]

**Added**

- `hwp lint <file>` checks Korean official-document notation and structure on markdown, HWP, HWPX
  or stdin ([#132](https://github.com/STAIxBWLB/hwp-cli/pull/132)). Ten rules across three
  families: `notation-date`, `notation-time`, `notation-money`, `notation-punctuation`,
  `notation-attach-colon`, `notation-attach-number`, `notation-end-dot`, `struct-item-mark`,
  `struct-roman-heading`, `ai-style-marks`. `--profile gongmun|report`, `--json` emitting the new
  `hwp-lint-report-v1` contract, and `--strict` to exit 1 on an error-severity finding. Advisory
  by default: it always exits 0 unless `--strict` is given. Exposed as the MCP tool `hwp_lint`,
  taking the tool count from sixteen to seventeen.
- `hwp new` builds official-document frames from repeatable `key=value` flags
  ([#133](https://github.com/STAIxBWLB/hwp-cli/pull/133)): `--doc-head` and `--doc-foot` for the
  기안문 두문/결문, `--notice-head`/`--notice-foot` for 공고문, and `--press-head` for 보도자료.
  Every frame block is emitted as a table. The 결재란 is not rendered — the approval system owns
  it — so 결재 and 협조 are emitted as placeholder rows, 협조 always on its own row.
- `hwp new --template <slug|한국어 별칭>` creates a document from one of the eight embedded
  skeletons (기안문 내부결재·대외시행, 공문서, 보고서, 사업계획서, 회의록, 공고문, 보도자료), and
  `--list-templates` lists them. Templates and frame flags are mutually exclusive: a template
  already carries its own 두문/결문.
- Tables generated under any official preset now carry a shaded, bold, centered header row and
  content-proportional column widths. `hwp edit --style-tables <preset>` applies the same to an
  existing document and is byte-stable when re-applied.
- `hwp_new` and `hwp_edit` MCP input schemas cover the new frame, template and table-styling
  arguments.

**Changed**

- The `gian` deprecation note now names `official`, the canonical preset key, instead of pointing
  at the second alias `gongmun`.

**Removed**

- The `gaejosik` official-document profile and its `개조식` alias
  ([#131](https://github.com/STAIxBWLB/hwp-cli/pull/131)). 개조식 is a writing style — the
  noun-form sentence ending used inside 보고서·계획서 and 내부결재 bodies — not a document class,
  and the profile's five typography fields could not express it. Its emitted document header was
  identical to `notice`; the two differed only by a header/footer margin with no cited source. The
  canonical set is now the six document types: `official`, `report`, `plan`, `notice`, `minutes`
  and `press`. `--preset gaejosik` and the MCP equivalent fail with a message naming the profile
  to use instead.

**Fixed**

- `hwp lint` no longer aborts the process on non-ASCII digits. The regex crate's `\d` is
  Unicode-aware, so a full-width `２０２６. ８. ２０.` matched the date candidate and then panicked
  in `parse::<u32>()`, taking down the CLI and the MCP server on a single line of user markdown.
  All six digit classes are pinned to ASCII, which is also what 편람 §6 requires.
- `hwp lint` no longer accepts a truncated Korean amount reading. `금113,560원(금` suppressed
  `notation-money` because the check was a prefix test; §6 requires the complete `(금…원)`
  parenthetical.
- `hwp edit --style-tables` on an already-styled document publishes an unmodified file instead of
  failing. Zero edits was indistinguishable from "no table matched", so the second of two
  identical runs errored.
- A horizontally merged cell is given the summed width of every column it spans, not just its
  starting column's.
- The HWPX patch writer no longer stamps the wall clock onto entries it rewrites. Two identical
  edits a second apart produced different bytes, which broke byte-stability guarantees and made
  the same input look non-deterministic across runs.

**Verification**

- CI green on ubuntu, macOS and Windows for every merged PR, plus the pinned-toolchain lint,
  PDF-parity runner and structured-corpus gates.
- Twenty artifacts covering frames, both writers, all six presets, the styled/unstyled pair and
  all eight templates were opened in genuine Hancom Office 12.30.0 build 6446 on macOS 26.6.2:
  twenty windows, no repair or damage dialog. Twenty content-free
  `hancom-verification-receipt-v1` receipts were recorded privately and schema-validated. This is
  a bounded structural and application-acceptance claim, not a pixel-parity claim.

**Known issues**

- `hwp cat --format markdown` writes a level-2 item as `- 가. 대상`, a bullet plus the
  engine-assigned mark as literal text, so a markdown round trip hardens the mark into the body
  ([#134](https://github.com/STAIxBWLB/hwp-cli/issues/134)).
- `hwp edit --verify` can fail an edit/re-read semantic-hash comparison on some Hancom-authored
  HWPX documents ([#135](https://github.com/STAIxBWLB/hwp-cli/issues/135)).

## [0.8.8]

**Added**

- A bundled bilingual Korean official-document skill tree with an authoring guide,
  regulation reference, and eight Markdown templates
  ([#126](https://github.com/STAIxBWLB/hwp-cli/pull/126)).
- Evidence-backed eight-level official numbering for HWPX and native HWP5:
  `1.`, `가.`, `1)`, `가)`, `(1)`, `(가)`, `①`, and `㉮`, including the
  verified post-`하` continuation. Seven canonical profiles are available:
  `official`, `report`, `plan`, `notice`, `minutes`, `gaejosik`, and
  `press`.
- Per-side `hwp new` margin overrides and matching `hwp_new` MCP fields. CLI and
  MCP share canonical preset aliases, range/content-area validation, and atomic
  no-publication behavior.

**Changed**

- Official authoring now fails closed before publication when native HWP5 input
  requests an unproven list topology, non-default start, continuation range, or
  depth. HWPX keeps its broader supported list-start semantics.
- Official profile defaults now use A4 top/bottom/left/right margins of
  20/10/20/20 mm, with profile-specific typography, header/footer bands, and
  bottom-center `- N -` page numbering where enabled.

**Fixed**

- Native HWP5 paragraphs now persist the observed zero-based list-level binding
  in `PARA_SHAPE`; without it Hancom displayed every nested level as decimal
  level one despite valid numbering definitions.
- Ordered-list start parsing no longer narrows values above `u32::MAX` or
  overflows while formatting later markers. Nested table and materializable
  control lists now share the same evidence-bound continuation checks.
- The private Hancom verification-set generator is fail-fast, publishes its index
  only after all fourteen documents validate, and rejects repository or
  symlink-resolved repository destinations.

**Verification**

- The seven-profile HWP/HWPX matrix, code review, security audit, and CI passed.
  Fourteen private artifacts were opened in genuine Hancom Office and passed
  hash-bound certification. This is a bounded structural and application-
  acceptance claim, not a pixel-parity claim.

## [0.8.7]

**Added**

- Hancom distribution documents (배포용문서) are now read: `hwp cat`, `hwp convert` and
  `hwp render` accept them, decrypting the ViewText streams and feeding the result through
  the normal read path, with the unwrap announced on stderr
  ([#116](https://github.com/STAIxBWLB/hwp-cli/pull/116)). Verified against 11 genuine
  corpus documents at HWP 5.1.0.1 and 5.1.1.0. The source-preserving edit path
  (`hwp edit`, `hwp fill`) still refuses these documents — their content lives in ViewText
  streams rather than BodyText, so there is no source structure to rewrite against; convert
  to an ordinary format first to edit.
- Protected documents are refused by name instead of failing downstream
  ([#116](https://github.com/STAIxBWLB/hwp-cli/pull/116)): password-encrypted,
  certificate-encrypted, certificate-DRM, DRM-protected and digitally signed HWP5 documents,
  and password-encrypted HWPX packages (which previously surfaced as an XML parse error),
  each with a message naming the condition and suggesting a remedy. The certificate, DRM and
  signature branches are **unverified against a genuine file** — no such document was
  obtainable, so what is established is that the header bits are parsed and branched on, not
  that Hancom sets them in the situations their labels name.

**Changed**

- Writing a formerly-distribution document out to a different format produces an
  unprotected output, because the writer synthesizes a fixed attribute value for every
  output; the tool now warns about this on stderr at read time
  ([#116](https://github.com/STAIxBWLB/hwp-cli/pull/116)). A same-format
  `hwp convert --to hwp` remains a byte-identical copy and keeps the protection bit.

**Fixed**

- Paragraph line-break settings (`breakSetting`) now survive hwp5 → hwpx conversion: the
  hwpx writer derives them from the paragraph shape instead of emitting a fixed literal
  ([#116](https://github.com/STAIxBWLB/hwp-cli/pull/116)).

## [0.8.6]

**Changed**

- **Text metrics now follow Hancom's own rules, so rendered geometry changes for every
  document.** Three shaping rules were wrong at once, all of them invisible on full-width
  Hangul because a 1.0 em advance makes the competing readings identical
  ([#108](https://github.com/STAIxBWLB/hwp-cli/pull/108), rule B9 in
  `docs/design/07-hangul-compat-rules.md`):
  자간 (letter spacing) scales each glyph's own advance — `advance * (100 + pct) / 100` —
  instead of adding a fixed fraction of the font size; a clear `글꼴에 어울리는 빈칸`
  (CHAR_SHAPE bit 25) means a space takes a fixed half em rather than the font's space glyph;
  and `useKerning` off now disables the `kern` feature, which the shaper enabled by default.
  Latin text is what separates the readings: an 11pt 자간 -10% word measures 32.07pt in the
  Hancom oracle, 31.99 under the new rules and 29.52 under the old. On the public one-page
  Hancom oracle the raster distance drops from `bad_pixel_pct` 0.01792 / MAE 2.87 to
  **0.00569 / 0.75**.
- Table rows keep the height the document stores when their cells carry a cached line layout;
  our own measurement no longer grows them, because growing one row moves every row below it
  and the page fragments after it ([#109](https://github.com/STAIxBWLB/hwp-cli/pull/109)).
  Cells without a cached layout — documents this tool authors — keep the measurement pass.
  A cell whose content exceeds its stored row is reported as the new typed warning
  `table_cell_content_overflow`; the content is still drawn.
- A table row split across pages is now closed with its own horizontal edge on each page, the
  way Hangul draws it, at real page crossings only ([#109](https://github.com/STAIxBWLB/hwp-cli/pull/109)).

**Fixed**

- The hanging indent (내어쓰기) is placed **inside** the text area
  ([#106](https://github.com/STAIxBWLB/hwp-cli/pull/106)). A genuine `line_seg` stores only the
  paragraph's left margin in `horzpos`, never the first-line indent, so every list paragraph was
  drawn one hanging width (14–18pt in the measured corpus) left of Hangul's position, with the
  marker a further marker width to the left. Alignment and wrap widths are now measured against
  the width left after the indent, and the lineseg synthesizer breaks lines against the same
  width.
- Page numbers render again where documents carry the control in a nested paragraph list — a
  text box or shape rather than the body flow — and the number is placed in the header/footer
  band instead of over the paper margin
  ([#107](https://github.com/STAIxBWLB/hwp-cli/pull/107)).

**Added**

- Over-height CELL table rows paginate at cached line boundaries
  ([#102](https://github.com/STAIxBWLB/hwp-cli/pull/102)).
- A privacy-safe font identity gate: the render report publishes hash-only requested/resolved
  font identities, weight state, face index and resolution completeness
  ([#103](https://github.com/STAIxBWLB/hwp-cli/pull/103)).
- Certification gained preservation and Hancom-open evidence checks, including the closed
  `hancom-verification-receipt-v1` schema
  ([#104](https://github.com/STAIxBWLB/hwp-cli/pull/104)).
- A PDF-parity manifest may declare `gate_exclusions`: every gate is still measured and echoed,
  and only eligibility is relaxed ([#105](https://github.com/STAIxBWLB/hwp-cli/pull/105)).

**Parity status**

Epic [#90](https://github.com/STAIxBWLB/hwp-cli/issues/90) closed with this release. The private
composite profile passes `page_count`, `media_box`, `render_issues` and `determinism`. Four gates
are declared exclusions in that private manifest and are **not** claimed by this release:
`fonts` (the oracle itself substitutes, following the document's own `substFont`, so
"substitution-free" is unreachable for that case), and `text`, `raster`, `roi` — measured at
1/13 pages byte-equal (the differences are pagination, not characters), `bad_pixel_pct`
0.1418–0.2332, and 3 of 4 ROIs passing. The distance and its known causes are recorded in
`docs/design/21-pdf-parity.md` §4.6 and tracked in
[#110](https://github.com/STAIxBWLB/hwp-cli/issues/110). This release makes no Hancom pixel
parity claim.

## [0.8.5]

**Added**

- Bounded WMF vector image rendering
  ([#90](https://github.com/STAIxBWLB/hwp-cli/issues/90)). WMF picture binaries
  (the Windows Metafile exports Hancom produces for complex figures) previously
  failed raster decode in every backend and rendered as magenta placeholders.
  A new pure-Rust WMF interpreter (`hwp-render/src/wmf.rs`) now expands the
  observed record subset — window/viewport state, DC stacks, pen/brush/font
  objects, polygons and polylines with even-odd or winding fills, DIB blits
  (including 1-bpp mask + color transparency pairs, with dithered pattern
  brushes approximated as density-blended solids), and CP949 `ExtTextOut`
  text resolved through the normal font pipeline — into display-list items at
  layout time, so PDF, PNG, and SVG all render them. Records outside the
  bounded subset are bounded-skips with the typed
  `wmf_unsupported_record_omitted` issue; malformed streams fall back to the
  placeholder with `wmf_parse_invalid_placeholder`. Neither counts as parity
  success. Adds the `encoding_rs` dependency (pure Rust, CP949 decode).

- HWPX container (`hp:container`) rendering
  ([#90](https://github.com/STAIxBWLB/hwp-cli/issues/90)). The hwpx reader now
  parses container children into `gso_shapes` plus a new
  `GenericControl.container_box` (container origin, size, and treat-as-char),
  recursing into nested containers with accumulated offsets, and the renderer
  draws the child shapes at the container origin and lays container text out in
  the container box — so grouped drawing objects render in PDF, PNG, and SVG
  instead of being counted as `unsupported_control_omitted`. The verbatim raw
  XML remains the reserialization source of truth, so same-format rewrites stay
  byte-identical; hwpx→hwp5 conversion keeps the typed
  `OpaqueControlUnrepresentable` failure.

- Deep table cloning
  ([#78](https://github.com/STAIxBWLB/hwp-cli/issues/78)). `--clone-table
  "SOURCE_TABLE=>ANCHOR[=>blank|keep]"` (MCP `clone_table` with
  `source_table`/`anchor`/`text_mode`) deep-copies a table — geometry, merge
  topology, widths, borders, fills, and styles — and inserts the clone after the
  anchor paragraph. `blank` (default) keeps one empty styled paragraph per cell
  and drops all source text and content controls; `keep` also clones nested
  tables and pictures, remapping every paragraph/control/object instance ID
  above the document maxima and reusing binary assets in place. Keep mode aborts
  atomically on opaque controls (fields, equations, text boxes) whose raw
  identity bytes cannot be safely remapped; every failure mode publishes
  nothing.

- Positioned, counted table row/column insertion
  ([#77](https://github.com/STAIxBWLB/hwp-cli/issues/77)). `--add-row` now accepts
  `TABLE[:AT[:COUNT[:TEMPLATE_ROW]]]` and `--add-col` accepts `TABLE[:AT[:COUNT]]`
  (`AT` omitted or `end` appends; a numeric `AT` inserts before that row/column).
  MCP `add_row`/`add_col` gained the matching optional `at`, `count`, and
  `template_row` fields. Insertion validates the logical grid first, extends merges
  crossing the boundary, never creates a cell under a covering span, and projects
  styles for new blank cells from the visible cell at `TEMPLATE_ROW` — so merged
  tables (including rows covered by vertical merges) now work, with text never
  cloned. Every failure mode (bad bounds, `COUNT` 0, u16 overflow, invariant
  violation) publishes nothing.

- Cross-format loss detection and an explicit typed loss report for `hwp convert`
  ([#90](https://github.com/STAIxBWLB/hwp-cli/issues/90)). Cross-format native conversion
  now inventories package/container-level assets the IR cannot carry: HWPX extra package
  entries (DocOptions, original META-INF overrides, scripts) lost on the way to HWP, and
  the hwp5 XMLTemplate/DocHistory pass-through slots lost on the way to HWPX. These are
  emitted as content-free typed events, so `--strict` cross-format conversion now fails
  closed on them, and the new `--loss-report <PATH>` flag publishes the
  `hwp-preservation-report-v1` ledger as JSON (schema-validated, empty-but-valid on a
  lossless run) even when strict mode rejects the output.

- Lossless HWPX package round-trip and package-surgical same-format editing
  ([#90](https://github.com/STAIxBWLB/hwp-cli/issues/90)). The hwpx reader retains
  verbatim XML for run-level controls the IR does not model
  (`GenericControl.hwpx_raw_xml`, e.g. `hp:container`) and the writer re-emits it, so an
  opaque control survives a full rewrite. Package entries the writer does not regenerate
  (original META-INF overrides, DocOptions, `Contents/memoExtended.xml`, extra previews)
  ride the new `Document.hwpx_extra_entries` slot, and unreferenced BinData entries pass
  through instead of being dropped, listed in the regenerated content.hpf manifest. Every
  same-format HWPX→HWPX `hwp edit` operation now goes through
  `hwpx::patch::rewrite_document_staged`: only the dirty content entries (header.xml,
  content.hpf, section*.xml, decided by before/after IR comparison) are reserialized from
  the IR, every other ZIP entry is raw-copied byte-for-byte, and inserted images append as
  new BinData entries with the original OPF manifest ids preserved. Two latent writer
  fidelity bugs fixed en route: the inline `hp:pic` zOrder and the no-border lineShape
  color now round-trip instead of being hardcoded.

- Captions, endnotes and odd/even furniture, the ninth and final step of the PDF parity
  roadmap ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). Table, picture and shape
  captions are parsed end to end (GB-13): a new `Caption` IR (side, direction, gap, width,
  paragraphs) on `Table`/`Picture`/`GenericControl`, hwp5 caption LIST_HEADER
  discrimination per pyhwp's `TableCaption`/`GShapeObjectCaption` model with re-synthesis,
  hwpx `<hp:caption>` round-trip, shape-caption preservation, reading-order text extraction,
  and caption block placement by side and gap in the renderer. Endnotes leave the anchor
  page: footnotes keep their per-page bottom placement while endnotes accumulate
  section-wide and paginate through a closing block without colliding with last-page
  footnotes (GG-14). Odd/even headers and footers are selected by printed page parity from
  each control's preserved apply value (BOTH/EVEN/ODD); absent apply data defaults to BOTH,
  while parity-only entries do not leak onto the opposite page (GG-16).

- Image and fill fidelity, the eighth step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). `Item::Image` gained its contract
  change — crop, flip, rotation, brightness, contrast — parsed from the hwp5 picture record
  and the hwpx `hp:pic` attributes and honored by the png (Transform + pre-crop), pdf (matrix
  + clip) and svg (transform + clipPath) backends; the pdf JPEG fast path and the svg
  zero-copy embed are kept when no pixel effect applies (GG-15). Picture effects (spec tables
  108 to 116) are parsed and surfaced as the typed `picture_effects_unsupported` render
  warning rather than rendered. Cell, paragraph and character backgrounds now honor hatch and
  gradient fills via the new `Fill::Hatch` (png segments, svg `<pattern>`, pdf flattened
  lines) and the existing gradient support (GG-7). Ellipses converted to arcs render as
  arcs/pies/chords through the axis-vector `ellipse_arc_path` (GG-23).

- Advance-affecting fidelity batch plus the single re-baseline, the seventh step of the PDF
  parity roadmap ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). Inline control
  characters now carry width: HYPHEN shapes a real `-`, NB_SPACE takes the space advance
  without adding a wrap opportunity, and FW_SPACE gets a fixed 1em advance (GG-20).
  Justification distinguishes justify/distribute/divide: distribute includes the trailing gap,
  divide excludes it, and both stretch the last line (GG-3; last-line semantics await Hancom
  confirmation). Letter spacing is computed in the HWPUNIT integer domain with half-up
  rounding (GG-4). Synthesized line spacing honors all four modes via the version-aware
  `line_spacing_type`: ratio, fixed (exact, no clamp), margin-only and minimum (GG-18). The
  golden gate `MAX_BAD_PIXEL_PCT` tightens 0.60 to 0.30 as the re-baseline promise; the first
  committed scoreboard validates or adjusts it.

- Character decoration fidelity, the sixth step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). Emphasis dots (CharShape attr bits
  21 to 24) render per glyph in all 13 documented kinds and round-trip through hwpx
  `symMark` (GG-8). Underline shapes — dash family, double/weighted offsets, and cubic wave
  paths — plus the above-character underline kind apply via the new `decor_strokes` table in
  `hwp-render/src/border.rs` (GG-9). Strikethrough shapes (bits 26 to 29) share the same table
  and round-trip in hwpx (GG-10). Character shadows now use the real `CharShape.shadow_gap`
  percentage offset in the png, svg and pdf backends (GG-11). Character-level borders and
  backgrounds (`CharShape.border_fill_id`) render per run, background emitted before the
  glyphs (GG-22). Decoration metrics are initial placeholders pending the Hancom verification
  round.

- Border line-type fidelity, the fifth step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). Cell, paragraph, page and diagonal
  borders now honor `BorderLine.line_type` via the new `hwp-render/src/border.rs` helper: the
  dash family (DASH/DOT/DASH_DOT/DASH_DOT_DOT/LONG_DASH, CIRCLE approximated as DOT) renders
  through `Stroke.dash`, and the double family (DOUBLE_SLIM/SLIM_THICK/THICK_SLIM/
  SLIM_THICK_SLIM) renders as offset parallel strokes — the weight split is an approximation
  pending Hancom verification. Every border emit site migrated from `Item::Line` to
  `Item::Path`, and `Item::Line` was deleted from the display list and all three backends.
  HWPX column divider lines (`hp:colLine`) now round-trip and render between column bands
  (GG-17; the hwp5 coldef divider parse is deferred — byte offsets unconfirmed). hwp5 raw-path
  shapes apply dash patterns and arrowheads, previously wired only to the hwpx path (GG-21).
  Resolves GG-5, GG-6 and the line-type part of GG-24.

- Hancom baseline parity scoreboard, the fourth step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). `scripts/pdf-parity.sh run` scores
  every manifest case against a local Hancom-exported oracle PDF with the five-metric set of
  docs/design/21-pdf-parity.md §3: `pdffonts` embedded/subset/unicode flags, `pdfinfo` page
  count, per-page normalized `pdftotext -layout` equality, and `dx`/`dy`/`ink_ratio` +
  `bad_pixel_pct`/`MAE` from rasterizing both PDFs with the same `pdftoppm -png -r 150`, and
  writes schema-validated numeric scoreboards (names, SHA-256 and numbers only, no local
  paths) under `fixtures/pdf-parity/public/scoreboard/`. Manifest, Poppler, font-file, and
  source/oracle pins are verified before rendering. Cases with a page-count delta, missing
  coverage, any font substitution, or a PDF font contract violation are recorded but marked
  unscored (F1 gate). `scripts/pdf-parity.sh selftest` verifies the harness without an oracle,
  and `hwp diff` gained `--format json`
  (contract `hwp-diff-report-v1`) plus `--ours-png` for raster-vs-raster comparison. The
  Hancom baseline exports and the first committed numbers are owner actions;
  `fixtures/golden/README.md` documents the per-case procedure.

- PDF parity groundwork for Hancom Office 2024 equivalence
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). The renderer now reads
  `LineSeg.flags` bit0/bit1 (page-first / column-first line) as the first-class page/column
  break signal for Hancom-saved documents, keeping the `v_pos` reset heuristic only as the
  fallback for synthesized linesegs. The structured corpus run additionally renders every case
  to PDF under the pinned font and asserts page-count parity with the PNG backend, a full
  GID round-trip through the emitted ToUnicode CMap, and two-run byte determinism; PDF
  artifacts are pinnable in the corpus schemas. `hwp render` now prints a font-coverage line
  (matched/substituted/missing/subset-fallback) and warns that no parity figure may be
  published from a substituted-font render. The durable contract — oracle, five-metric set,
  thresholds, font gate, data policy and non-goals — is
  [docs/design/21-pdf-parity.md](docs/design/21-pdf-parity.md).
- Outline numbering and PDF document metadata, the third step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). Outline paragraphs (head_type 1) now
  render the default fixed per-level markers (`1.` / `가.` / `1)` / `가)` / `(1)` / `(가)` / `①`),
  including inside text boxes and without consuming counters for empty paragraphs. Custom outline
  definitions and sequences beyond the known 14 Hangul markers remain GG-12 oracle work. Emitted
  PDFs now carry the contracted document-level surface: PDF 1.4 header,
  `/Lang (ko-KR)`, `/PageLayout /SinglePage`, `/MarkInfo <</Marked false>>`, XMP `/Metadata`,
  `/OutputIntents` with the official ICC Registry sRGB2014 profile, and an `/Info` dictionary
  limited to Author, Creator, Producer, CreationDate, ModDate and PDFVersion, sourced from
  document metadata only (including pre-1970 FILETIME; two-run byte determinism preserved).
  ToUnicode mappings now preserve complete shaping-cluster source sequences, wrapped-run text,
  combining text and distinct Unicode aliases that share one source-font GID.
- Table page splitting with header-row repeat, the second step of the PDF parity roadmap
  ([#79](https://github.com/STAIxBWLB/hwp-cli/issues/79)). A table taller than the remaining
  body space no longer clips silently at the media box: `pageBreak=NONE` pushes it wholesale to
  the next page, `TABLE`/`CELL` split it at row boundaries (cell-internal splitting is
  approximated as row-boundary splitting), and `repeatHeader` redraws the leading all-header-cell
  rows at the top of every continuation page. Row-spanning cells are kept intact by excluding
  boundaries that cross them. Tables treated as characters never split ("one character"). Splits
  and oversized indivisible row bands are reported as typed render issues
  (`table_split_across_pages` info, `table_row_too_tall_clipped`). Hancom Office oracle comparison
  remains pending, so this does not yet certify output parity.

**Changed**

- The Linux release asset is now cross-built with `cargo-zigbuild` against a glibc 2.17 baseline
  instead of natively on the ubuntu-24.04 runner. The native build required `GLIBC_2.39`, so it
  could not run on serverless runtimes (Vercel's Node runtime and AWS Lambda are Amazon Linux 2023,
  glibc 2.34) and downstream projects had to hand-build and vendor their own binary. The asset name,
  archive layout and `.sha256` scheme are unchanged, so `scripts/install.sh`, `hwp update` and the
  Homebrew formula keep working as before; the release workflow now asserts the floor stays at
  `GLIBC_2.17`. No other platform's build changed.

**Fixed**

- Synthesized line spacing misread the margin-only mode (2) as a ratio and clamped the fixed
  mode (1) to the base height; fixed is now exact `value/2` (overlap by design) and
  margin-only is `base + value/2`.

- Above-character underlines (CharShape underline kind 3) were dropped entirely because
  `has_underline()` only recognized kind 1; the renderer now switches on the kind.

- Tab advance disagreement between line breaking and placement: `items_width` and
  `compute_linesegs` used a floor-only 40 pt rule while `place_wrapped` honored explicit tab
  stops, so width estimates diverged from actual placement whenever a paragraph defined tab
  stops. All three now share `tab::next_tab` (explicit stops first, 40 pt default as the
  fallback).

- Objects (tables, pictures) anchored to a page-spanning paragraph were placed on the final page
  at the stale first-page y coordinate; the anchor is now re-bound to the new page's flow
  position at every cached-lineseg page break.

## [0.8.4]

**Fixed**

- Amazon Quick Desktop Windows guidance now uses a dedicated
  `%USERPROFILE%\AppData\LocalLow\hwp-quick-workspace` MCP root. Quick starts local MCP children at
  Low mandatory integrity, so discovery can succeed under a normal Medium-integrity `C:\TEMP`
  root while the first atomic document write still fails with `Access is denied (os error 5)`.

## [0.8.3]

**Fixed**

- CI no longer re-runs platform-independent gates on every OS, nor the whole test matrix on
  every tag: `ci.yml` runs fmt, clippy, and the structured-corpus gate once in an ubuntu `lint`
  job and only `cargo test --workspace` in the 3-OS `test` matrix, and `release.yml` verifies the
  tagged commit's already-green CI via the check-runs API instead of re-running it. The ubuntu
  test step also drops from ~15 minutes to ~2: render tests now resolve glyf-outline Nanum TTFs
  instead of CFF Noto CJK fonts, and the dev profile builds ttf-parser/rustybuzz/tiny-skia with
  opt-level 2 (release binaries unaffected).

## [0.8.2]

**Added**

- A bilingual, copy-paste Amazon Quick Desktop runbook now covers Windows binary verification,
  connector import, skill and agent setup, an end-to-end create/validate smoke test, daily file
  staging, and symptom-driven recovery for quoting, stale tool IDs, auto-disable, rendering, and
  sandbox path failures.

**Fixed**

- Amazon Quick Desktop on Windows: keep canonical MCP paths in verbatim form for sandbox
  authorization, then use ordinary drive/UNC spelling for filesystem I/O only when every component
  has equivalent Win32 semantics. Paths with trailing dots/spaces, reserved device names, or other
  verbatim-only semantics remain verbatim or fail closed. This removes the verbatim-path failure
  without weakening root containment. Also document separate JSON arguments and recovery from the
  `Access is denied` handshake loop that causes Quick to auto-disable the connector.

## [0.8.1]

**Added**

- Amazon Quick Desktop integration: `hwp skill export --install amazon-quick` installs the
  publish-safe bundled skill into the active Quick profile, with explicit profile ID or absolute
  path override, registry validation, and symlink-safe profile-relative writes.
- Amazon Quick documentation now covers Desktop stdio setup and all 16 tools, agent/skill
  publishing, troubleshooting, and the Quick Web limitation. The future authenticated Streamable
  HTTP, tenant isolation, and artifact model remain design-only in `docs/design/20-remote-mcp.md`
  and issue #52.

**Fixed**

- Release workflow: `update-formula` no longer pushes the Homebrew formula commit directly to
  main — branch protection rejects the Actions token (first seen on the v0.8.0 tag). The job
  now opens a `brew/formula-vX.Y.Z` PR and enables squash auto-merge; when the repository has
  no auto-merge or required checks never run for a token-created PR, the PR stays open for a
  human merge. `scripts/update_formula.sh X.Y.Z` remains the local recovery path.

---

## [0.8.0]

**Added**

- MCP server: `--root <dir>` (repeatable) sandboxes every path-typed tool argument — reads,
  writes, nested `insert_image`/`seal`/`parts` paths, compose/template `base_dir`, per-call
  `font_dir`, and the certify report directory. Roots are canonicalized at startup (a missing or
  unreadable root fails fast); write guards reject `..` components and close the
  symlink-overwrite escape. With no `--root` the server is unrestricted as before and prints a
  one-line stderr warning at startup. (#50)
- MCP protocol negotiation: `initialize` echoes the client's `protocolVersion` when it is one of
  `2025-06-18` / `2025-03-26` / `2024-11-05`, and otherwise replies with the latest supported
  version. (#50)
- Compose/template asset hardening: the MCP `--root` sandbox now also binds spec-internal asset
  references. DocumentSpec v1/v2 image and visual assets fail with
  `asset_snapshot_outside_roots`, and TemplateSpec `reference_hwpx` packages fail with
  `reference_outside_roots`, unless the resolved file sits under at least one root — so a spec
  cannot reach files outside the sandbox even when `base_dir` itself is attacker-influenced.
  The binding is verified against the opened file handle rather than the request pathname,
  closing the rename-swap race between the path check and the open. CLI and corpus callers
  pass no roots and behave exactly as before. (#53)
- MCP `hwp_edit` typed-operation parity: the seven edits that existed only as CLI string flags
  are now structured JSON arguments — `add_table` (`[{anchor, rows: [[...]]}]`), `set_para`
  (`[{pattern, line_spacing_pct | line_spacing_pt, indent_mm, left_mm, right_mm, top_mm,
  bottom_mm}]`), `set_page` (a single object: `width_mm`/`height_mm`/`margin_*_mm`/
  `orientation`), `delete_image` (`[{anchor}]`), `delete_table` (`[{index | anchor}]`, exactly
  one of the two), and `delete_field`/`delete_bookmark` (`[{name}]`). They run through the same
  strict, atomic, re-read-verified edit path as the CLI with identical applied/unapplied
  semantics. (#50)
- Edit `--verify` (and therefore MCP `hwp_edit`) now accepts `add_table` and image deletion:
  the semantic canonicalizer gained the exact HWPX writer projections for freshly synthesized
  tables (page-break/repeat-header attr and the inline default placement) and for bin streams
  no control references anymore (the HWPX writer only embeds referenced streams), instead of
  failing the re-read comparison. (#50)
- MCP read/convert/render parity + new `hwp_grep` (16 tools total) (#50): `hwp_read` gains
  `html`/`csv` formats, `with_header_footer`/`with_hidden`, and `with_segments` (markdown-only;
  segments are filtered to the paginated window but keep absolute offsets). `hwp_convert` gains
  explicit `to`, `font_dir`, `media_dir` and header/footer/hidden options — and PDF conversion
  now actually receives the configured font directories instead of an empty list. `hwp_render`
  gains `format` (png/svg/pdf), a `pages` range spec (mutually exclusive with the legacy `page`),
  and `output_path` to write PNG/SVG/PDF files and return metadata instead of base64 (the escape
  hatch from the 16 MiB response cap; single-page base64 PNG stays the default). The new
  `hwp_grep` tool searches paragraph text and returns `{matches, count, truncated}` — zero
  matches is a normal result.
- First-class agent skill (#51): the canonical `skills/hwp/SKILL.md` (command quick reference,
  MCP server usage, safety rules) is committed and embedded in the binary;
  `hwp skill export [-o DIR]` materializes it, and `--install claude-code|codex` writes it to
  `~/.claude/skills/hwp/` or `~/.codex/skills/hwp/`. The file is English-only by design — it
  is consumed by agents, and one canonical language avoids bilingual double-maintenance.
- Release packaging + AI integration docs (#51): every tag now also publishes
  `hwp-skill-claude-web.zip` (+ `.sha256`) — SKILL.md, a bootstrap script and the bundled
  Linux x86_64 binary, because the claude.ai sandbox network is registry-restricted and cannot
  download the binary at runtime — and `docs/manual/ai-integrations.md` (English canonical,
  Korean pair) documents per-client setup: Claude Code/Desktop, Codex CLI/cloud, Kiro/Kimi,
  claude.ai skill upload, and Amazon Quick Suite (convert to docx/pdf and upload; remote HTTP
  MCP tracked in #52).

**Fixed**

- Markdown/HTML image references are now bound to the MCP `--root` sandbox (#56, the gap left
  over from #53): `hwp_new` markdown input and `hwp_fill` part files fail closed when a
  referenced image resolves outside every root — whether by absolute path or a `../` escape —
  instead of following the reference and embedding the file. The check shares the
  canonicalized startup roots with the tool-argument guards, runs before any read, and its
  error does not leak the resolved path. CLI callers and root-less servers pass no roots and
  keep the previous degrade-to-alt-text behavior exactly.
- Markdown export: footnotes referenced inside an HTML-fallback table emitted their definition
  as a `<div class="hwp-footnote">` block, which the importer contract rejects — the footnote
  body was dropped in the `cat --format markdown` → `new --from` round-trip. Definitions are now
  always emitted as GFM footnote syntax (`[^N]: body`), and the importer reattaches `fnref`
  markers inside HTML fragments to those definition bodies as real footnote/endnote anchors, so
  the round-trip preserves the note (and no longer leaks a dangling `#fn-N` hyperlink). (#47)

---

## [0.7.1]

**Fixed**

- DOCX export: Word opened the result in Compatibility Mode and reported accessibility as
  unavailable. The package carried no `word/settings.xml`, so Word fell back to the 2007 content
  model; the part is now emitted with `compatibilityMode` 15 (Word 2013+). Found by the first
  real-hardware check of the writer on Word 16.111.2 for macOS — the structural gates could not
  see it. That same session confirmed the v0.7.0 output otherwise opens correctly: no repair
  dialog, correct heading sizes, correct colspan/vMerge tables.

---

## [0.7.0]

**Added**

- DOCX export (GJ-1): `convert --to docx` writes OOXML from the IR (`hwp-convert::docx`) —
  paragraphs with Heading styles, run properties (font, size, color, shade, letter-spacing,
  super/subscript), para alignment/spacing/indents, tables with gridSpan/vMerge and nesting,
  embedded images, hyperlinks, numbering lists, footnotes/endnotes, and page setup from
  SectionDef. Equations fall back to script text. DOCX input stays open (L-tier).

**Fixed**

- DOCX export: Word rejected or misrendered documents produced by the first GJ-1 writer.
  `w:pPr` and `w:rPr` children are now emitted in `CT_PPr`/`CT_RPr` schema order, and line
  spacing is merged with before/after into the single `w:spacing` the schema allows (it was
  emitted twice). Hyphens, non-breaking spaces, line breaks and tabs no longer escape their
  run. Every `abstractNum` defines all nine levels so a `numPr` cannot reference an undefined
  `w:ilvl` (HWP's 10th level is dropped — `w:ilvl` above 8 is invalid). A cell that is both
  colspan and rowspan no longer emits one `vMerge` cell per covered column, which made covered
  rows overflow the table grid; `w:tcW` accounts for the span, and a cell ending in a nested
  table gets the trailing `w:p` the schema requires. C0 control characters are dropped instead
  of producing an unparseable part.

**Verification**

- The DOCX unit tests now validate the emitted package structurally — property order and
  cardinality, run containment, per-row grid coverage against the `w:gridCol` count, and
  `numId`/`ilvl` resolution against `numbering.xml` — rather than asserting on substrings.

---

## [0.6.0]

**Added**

- HTML fragment round-trip (contract: docs/design/18). A new `from_html` importer parses a
  well-formed XHTML subset (table colspan/rowspan, cell blocks, data-URI/relative-path images,
  inline marks, lists) into the IR. Contract violations are hard errors.
- HTML export alignment: `convert --to html` now emits merged cells as colspan/rowspan (GH-4),
  preserves nested tables/images inside cells (GH-5), and renders footnotes/endnotes as `<sup>`
  anchors with a trailing definitions section (GH-3). The output is XHTML that `from_html`
  reads back.
- Mixed md+HTML part files: HTML table blocks can be embedded in markdown body text and are
  parsed by `new`/`convert`. Inline `<u>`, `<sup>` and `<sub>` round-trip through the IR.
- Template + part fill: `hwp fill --set name=@part.md` (or a `parts` map in `--data`) replaces
  the `{{name}}` anchor paragraph with the part file's blocks, composing large documents
  part-by-part (limited to hwp-cli-generated documents in the default palette family).
- SVG images in parts: `<img src="*.svg">` is embedded via closed-subset validation and
  deterministic PNG rasterization. The validation/rasterization implementation was extracted
  into `hwp-convert::svg` and is now shared with DocumentSpec v2.
- A `parts` argument on the MCP `hwp_fill` tool, exposing part grafting over MCP.
- ODT export GH-3/4/5: footnotes/endnotes as `<text:note>`, merged cells as
  number-columns/rows-spanned with covered-table-cell, and nested tables/images preserved
  inside cells. All export paths (md/html/odt) are now covered.
- HTML style round-trip (contract v2, docs/design/18 section 8): the html export carries
  char and para shapes as `.cs{n}`/`.ps{n}` CSS rules (fragments stay self-contained with a
  leading `<style>`) and `from_html` restores them. Tags stay authoritative for marks,
  default-palette shapes are reused (dedup), and unknown fonts are appended on restore.
- Edit primitives: `edit --add-table "anchor=>rows-json"` (table insertion), `--set-para`
  (line spacing, indent, margins, paragraph spacing), `--set-page` (paper size, margins,
  orientation), and `--delete-image/--delete-table/--delete-field/--delete-bookmark`
  (object deletion with anchor-char and FIELD_END surgery).
- CLI workflow: convert accepts multiple inputs with `--out-dir` (batch) and `-` for
  stdin input / stdout output of text formats. New `hwp grep <pattern> <file>` (recursive
  paragraph search, exit code 1 on no match).
- Extraction formats: `cat --format csv` / `convert --to csv` (tables to CSV, RFC 4180)
  and `convert --to txt` with `.txt` inference.

---

## [0.5.0]

**Added**

- A structured authoring path: `compose` and `template` deterministically generate HWP/HWPX from
  DocumentSpec v1/v2 and TemplateSpec/Data v1. Only a typed AST is allowed, with no string
  interpolation and no expression evaluation.
- Native-only certification (`certify`): pins font identity, forbids font substitution, macros and
  external references, renders every page, fails on bounds, collision and unresolved fields, and
  publishes the report atomically.
- A structured corpus gate (`corpus`): generates seven self-authored Korean documents as HWPX and HWP
  twice each and passes only when document bytes, semantic statistics, page PNG hashes, render-issue
  hashes and font identities all agree.
- Published JSON Schemas (`schemas/`), examples (`examples/`) and design documents 13 to 17.
- Three new MCP tools (`hwp_compose`, `hwp_template`, `hwp_certify`), for 15 in total.
- Localized CLI help: English by default, Korean under a Korean locale, and overridable with
  `--lang <en|ko>` or `HWP_LANG`. The CLI reference is generated in both languages.

**Fixed**

- Restored the Windows build. File identity and link counts no longer use the nightly-only
  `windows_by_handle` API; they are read from a handle via `GetFileInformationByHandle`. An identity
  that cannot be read never compares equal, so the TOCTOU recheck fails closed.
- Pass an explicit impersonation token when computing inherited DACLs on Windows. With a NULL token
  the call returned `ERROR_NO_TOKEN` and every publish failed.
- Open the staged file for write before fsync, because `FlushFileBuffers` requires write access on
  Windows. With a read-only handle every surgical hwpx publish failed with `ERROR_ACCESS_DENIED`.
- Pin `eol=lf` for the hash-pinned inputs under `examples/` so golden reports still match on a Windows
  checkout.

**Changed**

- Corpus fonts are fetched rather than committed. `scripts/fetch-corpus-fonts.sh` downloads them from
  the manifest's pinned URL and verifies each against its SHA-256, accepting only `https` from the
  pinned host and only relative in-corpus destinations.
- User-facing documentation is now a Korean original plus an English pair (`NAME.md` / `NAME.en.md`).
- Release notes are now bilingual and driven by `CHANGELOG.md`; a release fails without a section for
  that version.

---

## [0.4.1]

- Page numbers are rendered in both HWP and HWPX output (#30).
- Added Korean official-document presets (gian, report) to the markdown import path (#28).
- Fixed □/○ paragraph spacing and list marker (-, ·) visibility in gaejosik documents (#27).
- Renamed the Hancom verification output directory to `hwp-verification` (#29).

---

## [0.4.0]

- Added `hwp update` self-updating and a one-line installation script that does not need Homebrew (#25).

---

## [0.3.0]

- Homebrew installation is supported: the repository is its own tap and the formula is updated
  automatically on release (#24).
- Added hwpx equation emission and `hp:script` entity resolution (#23).
- Added `--font-dir` to `convert` so PDF conversion can select external fonts from the CLI (#22).
- The CLI reference is generated from the clap definitions, with a drift gate (#20).
