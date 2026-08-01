[한국어](08-external-research.md) · [English](08-external-research.en.md)

# 08. External evidence: the OWPML standard, open source and pagination behavior

> Collected and verified on 2026-07-04 through multi-source web research plus adversarial
> verification (98 agents). The goal was to **support or refute** what we observed in Hancom Office:
> "a paragraph crowded with shapes drops text boxes and overflows the page".

## Research question

When a hwp5 → hwpx converter faces an original that anchors dozens of shapes on one page (for
example 35) to a single paragraph (`hp:p`), Hancom rendering (a) fails to draw some text boxes
(`hp:rect` + `drawText`) and (b) overflows the page, inserting a blank one. The original hwp5 is
fine; **only our converted hwpx** shows this.

## Verified findings (confidence and source)

| # | Finding | Confidence | Evidence |
|---|---|---|---|
| 1 | **Anchoring dozens of shapes to one paragraph is a normal structure.** The de facto reference converter `hwp2hwpx` also maps paragraphs **1:1**, putting all 35 shapes into one `hp:p` and **never redistributing them across paragraphs**. | High | neolord0/hwplib and hwp2hwpx (`ForSectionXMLFileList.section()` maps paragraphs 1:1; `ForGso.convert()` appends every GSO to the same run) |
| 2 | In both hwp5 and HWPX, shapes (GSO) are **objects anchored under a paragraph**. Placing many of them in one paragraph does not violate the schema. | High | hwplib (Paragraph.controlList), Hancom tech blog (hp:p → hp:run → hp:t) |
| 3 | (Q1) **No documented limit exists on the number of drawing objects per paragraph, run or page**, and no rule requires distributing them. | Medium (argument from silence) | hwp2hwpx README, hancom-io/hwpx-owpml-model |
| 4 | (Q2) **Pagination is not stored in the file; it is computed at render time, paragraph by paragraph.** "While drawing a paragraph, a page is added at the moment the area runs out." | High | hwplib README (no page API), maintainer issue #31 |
| 5 | (Q3) **No documented report of a "text boxes dropped in a shape-heavy paragraph" bug** anywhere: changelogs, issues, or the official model repository. | Medium (argument from silence) | hwp2hwpx changelog (to 2026-06-25) and its 7 issues, the Hancom model repository |
| 6 | HWPX output must follow the national standard **KS X 6101 = OWPML**. Detailed object, z-order and rendering rules live in the external OWPML **detailed specification (PDF)**, not in open source. | High | Hancom official, standard.go.kr, hancom-io model repository |
| 7 | (Q5) hwp5 is a CFB binary (records must be converted into an XML tree) while HWPX is already XML (OPC/ZIP). At least one renderer (rhwp) parses both into **a single unified IR**. | Medium | pyhwp documentation, rhwp onboarding (HWP/HWPX → Parser → IR → Paginator → Layout) |

## ★ Decisive implications: reconstructing the cause

- **The structure is normal.** Even the reference converter does not distribute shapes across
  paragraphs, so that is not the canonical fix. The redesign we had planned, spreading shapes over
  several paragraphs, likely **diverges from the standard and is not the root fix**.
- **Pagination happens at render time, paragraph by paragraph.** A blank page is not a stored
  property but a difference in Hancom's layout computation. The **line height and paragraph
  properties of the anchoring paragraph** drive the page calculation.
- **The leading cause (open question):** even with a structurally 1:1 mapping, the **property
  fidelity** of each object and paragraph (`vertRelTo`, `horzRelTo`, `treatAsChar`, z-order,
  `textWrap`, `offset`, `vpos`, the line height of empty paragraphs and so on) may differ between the
  original hwp5 and our hwpx, changing Hancom's layout. **A property-level diff has not been done.**

## Limitations (important)

- The answers to Q1 and Q3 are **arguments from silence**: they do not prove that no limit exists.
  An undocumented practical limit inside Hancom's closed-source renderer remains possible.
- Four hypotheses that tried to explain the symptom directly (1. missing space reservation and vpos
  correction for TopAndBottom objects, 2. excessive LINE_SEG height for treatAsChar objects,
  3. missing child elements in python-hwpx, 4. shapeObject/connectLine representation) were **all
  refuted in three-vote adversarial verification (0-3 / 1-2)**. This material alone cannot attribute
  a mechanism.
- Sources skew toward open-source READMEs and the standards registry. We could not read the Hancom
  OWPML **detailed specification PDF**, where object anchoring, space reservation and z-order rules
  would actually be described. rhwp is a third-party reimplementation and does not represent Hancom's
  real behavior.

## Follow-up directions (by priority)

1. **Property-level diff**: compare shape and paragraph properties of the original hwp5 against our
   hwpx output, object by object (vertRelTo, horzRelTo, treatAsChar, z-order, textWrap, offset, vpos,
   plus char_shape and line height of empty paragraphs). This is the leading direction.
2. Obtain and read the Hancom OWPML **detailed specification PDF** (hancom.com/etc/hwpDownload.do)
   for object anchoring and space reservation rules.
3. Keep all 35 shapes but **match the properties to genuine files**, then test in Hancom to see
   whether the drop and overflow disappear.
4. Check whether the blank page comes from the **line height of the anchoring paragraph** rather than
   from the shapes, by matching empty-paragraph line height to genuine files.

## Sources

- neolord0/hwplib, neolord0/hwp2hwpx (the de facto reference converter)
- hancom-io/hwpx-owpml-model (the official OWPML model)
- Hancom tech blog (tech.hancom.com/hwpxformat, python-hwpx-parsing)
- KS X 6101 (national standard, standard.go.kr): the OWPML document structure
- pyhwp documentation, rhwp (a third-party unified renderer)

---

# Renderer completion research (2026-07-05, deep research with 102 agents)

> To pursue the top priority of "a complete renderer", we researched the exact implementation of the
> unimplemented or approximated features (equations, charts, page borders, vertical writing, columns,
> justify/letter spacing, OLE) against the HWP 5.0 specification, OWPML and open source. 25 claims
> went through adversarial verification: 23 confirmed, 2 refuted.

## ★ The decisive meta-conclusion

**No open-source HWP "renderer" exists.** hwplib, hwpxlib, the Rust `hwp` crate and hancom-io's
owpml-model are all **parsers and object models** with no layout, draw or paint package. Every
layout, typesetting and drawing algorithm must therefore be ours; only parser evidence is reusable.

## Evidence per feature (confidence and source)

| Feature | Confirmed facts | Evidence |
|---|---|---|
| **Equations** | Both hwp5 (HWPTAG_EQEDIT) and hwpx (CEquationType → CScript) store **a text script**, not glyphs. Reserved characters: `~` space, `` ` `` quarter space, `{}` group, `" "` word, `#` line break, `&` column alignment. Keywords OVER (fraction bar), ATOP (no bar), SQRT, SUP/`^`, SUB/`_`, the integral family INT, OINT, DINT, TINT, ODINT, OTINT, plus SUM, PROD, UNION, INTER, and matrix{`&` columns, `#` rows} with P/B/D-MATRIX. Function words (sin, log, lim and so on) are roman; a space inside a name makes it italic. | Hancom equation spec rev1.2, equation help, hwplib ControlEquation |
| **Charts** | hwp5 stores a ChartObj binary tree (VtChart root, StoredtypeID deduplication). hwpx CChartType carries only `chartIDRef` pointing at an external `chart/chartN.xml` (OOXML DrawingML). Series and axis field layouts are **not obtained (open)**. | Hancom chart spec rev1.2, hancom owpml-model |
| **Page borders** | BorderFill is four sides plus diagonal plus one fill, a u16 bit field (bit0 3D, bit1 shadow, bits 2-4 slash, ..., bit13 center line) and a u32 fill kind (0 none, 1 color, 2 image, 4 gradient). Position is relative to the paper (inside) or the page (outside), with four-direction gaps up to 25mm (a UI constraint). ~~The HWPTAG_PAGE_BORDER_FILL byte layout was refuted (unconfirmed)~~ → **resolved on 2026-07-19 (investigation GC-2)**: the "refutation" was rooted in the specification's own contradiction in table 135 (declared 12B versus fields summing to 14B). All 714 records across 236 genuine files measure **14B** (u32 attribute + four u16 gaps + u16 border id, with BOTH/EVEN/ODD distinguished by record order), so our code was already correct. | Rust hwp border_fill.rs, 5.0 spec table 24, hwplib PageBorderFillProperty, exhaustive genuine-file sweep |
| **Columns** | COLDEF (`cold`, tables 138/139): attr **bits 0-1 kind (0 normal, 1 distributed, 2 parallel), bits 2-9 count (1-255), bits 10-11 direction (0 left, 1 right, 2 facing), bit12 equal width**; gap is HWPUNIT16; unequal widths carry a per-column width array; separators have type, thickness and color. **Bit-for-bit agreement with hwplib.** | 5.0 spec, hwplib ControlColumnDefine |
| **Vertical writing** | The claim that it is "just a direction flag" was **refuted**; no confirmed rendering algorithm exists. **Deferred.** | (open question) |
| **Letter spacing and width scaling** | CharShape width scaling is INT8[7] at 50-200% and letter spacing is effectively -50 to 50%. Glyph advance = base × scale + spacing. (Already applied in our shape.rs.) | Hancom tech, python-hwp-parsing-2 |

## Open questions, to be resolved against ground truth during implementation

- ~~The exact byte structure of HWPTAG_PAGE_BORDER_FILL (refuted), to be reverse-engineered from
  samples~~: resolved on 2026-07-19, see above.
- Glyph rotation and line progression rules for vertical writing: no confirmed algorithm.
- Exact rules for justified alignment (CJK versus Latin space distribution, and the last line): not
  obtained, currently heuristic.
- Field layouts below chart series and axes, and the OOXML mapping of hwpx chartN.xml: only the tree
  and reference are known.

## Sources (renderer)

- Official Hancom specification PDFs: 한글문서파일형식 5.0 rev1.3, equations rev1.2, charts rev1.2
- Hancom help: equation (script, explanation, font), page_border, vertical
- neolord0/hwplib and hwpxlib, hancom-io/hwpx-owpml-model, the docs.rs `hwp` crate, hahnlee/hwp.js
- Hancom tech blog (tech.hancom.com): hwpxformat, python-hwp-parsing (width scaling and letter spacing)

---

# Ecosystem feature comparison (2026-07-08, deep research with 102 agents)

> Finding features that hwp-cli lacks but real users need, evidenced by other implementations and by
> demand. 91 claims extracted, 25 adversarially verified: **21 confirmed (all 3-0), 4 refuted**. This
> section is the evidence behind [12-feature-gaps](12-feature-gaps.en.md) §10 GJ and the §14 roadmap
> re-evaluation (GA-2 and GB-1 ★).

## (a) Already supported elsewhere, so precedent exists

| Finding | Implication (item in document 12) | Evidence |
|---|---|---|
| **Reading distribution documents**: pyhwp 0.1b7 (2014) onward, H2Orestart v0.7.11 (2026-04) onward. Hancom's official 「배포용문서 rev1.2」 specification publishes the entire decryption path (DISTRIBUTE_DOC_DATA 256B, random array, SHA1-derived key, AES-128 ECB), so **no reverse engineering is needed** | GA-2 difficulty re-rated L → **M** | pypi pyhwp changelog, H2Orestart #42, the Hancom specification PDF read directly |
| **HWP 3.x parsing**: rhwp (Rust, through rendering, validated against a 763-page oracle), kordoc v2.7.1 (text extraction), LibreOffice hwpfilter (V30SIGNATURE). The official 「3.0/HWPML rev1.2」 Part I documents the entire structure | GJ-3 can start | rhwp README and PR #506, kordoc CHANGELOG, LibreOffice docs |
| **HWPML (.hml) input**: kordoc commercialized a dedicated parser; Part II of the same specification is the XML element reference | GJ-2 can start | kordoc src/hwpml/parser.ts |
| **Vertical writing and equation matrices**: rhwp, in the same language (Rust), implements both (text-box vertical-rl/lr, MATRIX/PMATRIX/BMATRIX/DMATRIX with 114 equation tests) | Reference implementations exist for GC-1 and GD-1 | rhwp src/paint/font.rs, renderer/equation/parser.rs |
| **Native HWPX chart generation**: kordoc v3.16 shows charts are not OLE but `Chart/chartN.xml` (an OOXML chartSpace) plus a manifest entry and `hp:chart chartIDRef`, so 20 chart types can be generated by XML manipulation alone | GB-1 hwpx path re-rated L → **M** | kordoc CHANGELOG, src/hwpx/chart-gen.ts |
| **Form objects, word art and OLE object-model manipulation**: hwplib (Java) supports reading and writing (not rendering) | Object-model precedent for GB-2, GB-4 and GB-5 | neolord0/hwplib changelog |
| **Automatic seal stamping, document comparison and format-preserving patching**: kordoc seal (float a seal PNG over an "(인)" anchor), compare_documents, patch_document | Precedent for GM-7 and GM-8 | kordoc README and its MCP tool table |

## (b) Clear demand that open source serves poorly: open territory

| Finding | Implication | Evidence |
|---|---|---|
| **HWP → DOCX**: demand high enough that Microsoft ships an official converter (HwpConverter plus the batch BATCHHWPCONV.exe). An OSS implementation of this output is essentially absent (pyhwp emits only ODT/HTML/txt; kordoc takes DOCX only as input) | GJ-1 has the highest demand | microsoft.com id=36772 and id=49153 |
| **Standalone document merge and split**: a full chapter of the pyhwpx cookbook (merging 33 documents, splitting 100 pages), so it is common in practice, yet the current solution is Windows plus Hancom COM only and fragile with clipboard errors | GM-3, GM-4 | wikidocs 8956 (the pyhwpx cookbook) |
| **HWPX distribution documents**: even H2Orestart (#42, open) does not support them, and no implementation can read them. Whether the official HWP5 distribution specification covers the HWPX variant is unverified | GJ-8 (L) | H2Orestart #42 |

## Competitive context and other confirmed points

- **pyhwp is HWP5 only, converts to three formats (ODT/HTML/txt), is officially marked
  "experimental", and has been stalled at 0.1b15 since 2020.** hwp-cli's conversion suite is the
  broadest in this ecosystem. Its hwpx support request (#135) has been open since 2013, showing how
  deep the demand for HWPX runs.
- **H2Orestart plus headless LibreOffice** is the de facto CLI HWP → PDF path in production
  (Dangerzone). A single binary with no JRE or LibreOffice dependency is hwp-cli's differentiator.
- **Three open hwp.js issues are useful conformance material**: parsing 5.1.0.1.1 written by Hancom
  2018 (#59), subscripts (#55), and column definitions being 14B in the specification versus 16B in
  real files (#58, a documented case of specification-versus-reality mismatch).

## Limitations

- Accessibility (alt text), digital signatures and PDF/A **have no evidence that survived
  verification** (the evidence did not survive, which is not the same as an absence of demand). Direct
  evidence for table-to-CSV demand also did not survive (kordoc's XLS/XLSX support is input-only).
- rhwp and kordoc are new projects created in late 2026-03. Their features were verified at code
  level, but render fidelity quality is self-reported (oracle and aHash based). kordoc's HWP 3.x
  support is text extraction only.
- Four refuted claims were discarded: hwplib's unsupported list (0-3), the assertion that hwp-rs is
  read-only (0-3), the assertion that hwp.js is stalled (1-2), and the LibreOffice HWP 2.0/2.1
  lineage (0-3).

## Sources (ecosystem)

- pypi.org/project/pyhwp, pyhwp.readthedocs.io (converters), github.com/mete0r/pyhwp#135
- github.com/ebandal/H2Orestart (#42), extensions.libreoffice.org/27504, freedomofpress/dangerzone
- github.com/edwardkim/rhwp, github.com/chrisryugj/kordoc, github.com/neolord0/hwplib
- github.com/hahnlee/hwp.js (#55, #58, #59), microsoft.com HwpConverter (id=36772, id=49153)
- store.hancom.com/etc/hwpDownload.do (배포용문서 rev1.2, 3.0/HWPML rev1.2), wikidocs.net/book/8956
