[한국어](capabilities.ko.md) · [English](capabilities.md)

# What ahwp does and its limits

The full feature list, implementation status, known limitations and roadmap, for a person deciding whether `ahwp` fits a job or an agent checking whether a capability exists before trying it. Unimplemented features are catalogued with evidence in [12-feature-gaps](../design/12-feature-gaps.md).

## See what ahwp can do

- **Reading and text extraction** from hwp/hwpx to plain / markdown / HTML / JSON (full IR) / CSV.
  Tables, images, headers/footers and unparsed records are preserved during parsing, and Hancom
  distribution documents (배포용문서) are decrypted and read like any other file.
- **Password-protected input** `ahwp cat`, `convert` and `render` accept `--password` or the safer
  `--password-stdin` for the evidenced HWP5 and HWPX password profiles. The matching MCP tools use
  a per-call password field with no session cache. Wrong or absent passwords return one stable,
  redacted refusal.
- **Format conversion** hwp ↔ hwpx, hwp/hwpx ↔ markdown, hwp/hwpx ↔ HTML and hwp/hwpx ↔ JSON (IR),
  all through a shared document model (IR), plus one-way export to DOCX, ODT, CSV and plain text.
- **Rendering** hwp/hwpx → PNG / SVG / PDF. Stored line layout (PARA_LINE_SEG) is used when present
  and synthesized otherwise. PDF output is a single multi-page document with subset-embedded fonts,
  so text is selectable, searchable and copyable (ToUnicode CMap).
- **Writing, including the hwp binary** Both hwpx package writing and HWP 5.0 binary (CFB) writing
  are implemented. Unmodified hwp-origin documents round-trip byte-identically at the decompressed
  stream level.
- **Structured authoring** Deterministic HWP/HWPX generation from DocumentSpec v1/v2 and
  TemplateSpec/Data v1 (`compose`, `template`). Only a typed AST is allowed, with no string
  interpolation and no expression evaluation.
- **Certification and corpus gate** `certify` certifies package validation, repeated import and
  bounded native rendering, then publishes a report atomically. `corpus` generates a frozen corpus
  twice and requires the document bytes, semantics and render hashes to match.
- **AI editing** A read → edit → rewrite loop over the JSON IR. Text replacement, table cell values
  and field filling are applied while images, formatting and unparsed records are preserved.
- **Official-document authoring (공문서)** Six canonical profiles (`official`, `report`, `plan`,
  `notice`, `minutes`, `press`), eight embedded document templates (`ahwp new --template`), native
  두문/결문 frames, the statutory eight-level item marks, preset table styling
  (`ahwp edit --style-tables`) and a notation and structure linter (`ahwp lint`, ten rules).
- **Document-level workflows** Merge several documents into one (`ahwp merge`, one Section per
  input), split one into per-section or per-page-range fragments, or cut out one typed range such as
  a single form with the binaries it uses (`ahwp split`), and report the paragraph and structural
  differences between two documents (`ahwp compare`).
- **MCP server** A dependency-free (serde_json only) stdio MCP server exposing 23 tools to
  desktop clients, including Amazon Quick Desktop.

## Check the implementation status

| Area | Status |
|---|---|
| hwp/hwpx reading; text, markdown and JSON extraction | Implemented |
| hwpx writing, HWP 5.0 binary writing | Implemented |
| PNG/SVG/PDF rendering (tables, images, text boxes, shapes, foot/endnotes, page numbers, text effects) | Implemented |
| Structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) | Implemented, including merged tables |
| DocumentSpec v1/v2 and TemplateSpec v1 composition | Implemented |
| Certification (`certify`) and structured corpus gate (`corpus`) | Implemented |
| Document-level workflows (`merge`, `split`, `compare`) | Implemented; spot-checked in Hancom on 2026-08-29 (see [12-feature-gaps](../design/12-feature-gaps.md) GM-3/GM-4/GM-8) |
| Official-document authoring (profiles, templates, frames, lint, table styling) | Implemented |
| MCP server (23 tools) | Implemented |
| Distribution documents (배포용문서) | Read |
| HTML conversion | Structural parity with markdown; CSS mapping of character and paragraph shapes is still coarse |
| Equations | Approximated as a box plus the script |
| Charts, OLE | Not supported |
| Password-protected HWP5/HWPX | Read, convert and render with a supplied password |
| Certificate-, signature- or DRM-protected documents | Reading is refused with a specific reason |

### Limitations

- **Scope of lossless round-trip** Only unmodified hwp-origin documents are guaranteed byte-identical.
  Documents that were edited, or that came from hwpx/markdown, are rewritten through the writer's
  synthesis path as **semantically equivalent** (text and structure preserved). hwpx writing is always
  semantically equivalent (template-based regeneration). A fully lossless round-trip including images
  in JSON requires the `--embed-bin` path.
- **Cross-format semantic conversion** Tables, images, sections, headers/footers, text boxes, fields
  and foot/endnotes are parsed and rendered semantically, but shapes, equations, charts and OLE render
  without hwp↔hwpx record synthesis (they are preserved round-trip within the same format).
- **Fields** Existing field values can be filled and a new click-here field can be created after an
  anchor, but arbitrary field kinds cannot be created.
- **Protection scope** Password support is limited to the genuine-corpus-evidenced HWP5
  EncryptVersion 4 and HWPX ODF AES-256 profiles. Certificate encryption, signatures and DRM remain
  explicit refusals. Passwords are invocation-local and are never written to reports or receipts.
- **Render hashes** The hashes recorded by the corpus are observations for that OS and architecture,
  not a claim of cross-platform pixel equivalence.
- **No Hancom parity claim** The final verdict is always whether the file opens correctly in Hancom Office.

## See what is planned

The numbered catalog of unimplemented features, each with its code and specification evidence and a
difficulty estimate, is [docs/design/12-feature-gaps.md](../design/12-feature-gaps.md). That file
is the detailed source; the list below is only the shape of the work.

1. **Property fidelity and render precision** The two open Hangul compatibility items (a text-box
   frame lost on round-trip, and vertical overflow in synthesized multi-page documents; GG-1 and
   GG-2), the remaining Regime-A PDF parity distance on text, raster and region comparisons (issue
   #110), and Hancom confirmation of justified alignment (GG-3).
2. **Specification coverage** The HWP 5.0 rev1.3 body has been reconstructed as reviewable Markdown
   (§1 to §4.4) and audited against the implementation; the errata that audit produced are
   catalogued in [19 §1](../design/19-hwp5-spec-supplement.md). Still outstanding: the OWPML /
   KS X 6101, equation and chart specifications have not been obtained, and the bit-level
   parser/writer value comparison is only partly done.
3. **HTML fidelity** Footnote markers, merged-cell colspan/rowspan and in-cell blocks already match
   the markdown path. What remains is CSS mapping of character and paragraph shapes, columns and
   headers/footers.

Windows is not a roadmap item: ubuntu, macOS and Windows are all required CI gates today.
