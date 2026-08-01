[한국어](TODO.ko.md) · [English](TODO.md)

# TODO: specification re-documentation and a full review of hwp-cli

> **Background:** the distribution of the errors found during Hancom verification (the C and D series)
> matched the gaps in our specification coverage. On the hwp5 side, **damaged tables** in
> `docs/spec.txt` (a plain text extraction of the PDF) led to bit fields being interpreted incorrectly
> in the code (for example the object common-property text-wrap bits); on the hwpx side **the OWPML
> specification was simply absent**, so we relied only on measurements of genuine files (for example
> the tabItem hp:switch structure). The plan is to produce a trustworthy Markdown copy of the
> specification and review the whole implementation against it.

---

## Phase 1: reconstruct the specification PDF as Markdown (owner: user; review support: Claude)

### 1.1 Target documents

- [x] **한글문서파일형식 5.0 rev1.3** reconstructed by the user on 2026-07-18
      (`docs/spec/한글문서파일형식_5.0_revision1.3.md`, 2,405 lines with 1,454 pipe-table rows,
      covering §1 to §4.4). Touchstone review passed: table 70 object common-property bits match the
      values confirmed in Hancom, and tables 36/37 tab definitions match the implementation. Remaining
      minor defects are in the §1.4 review notes.
- [ ] OWPML / KS X 6101 detailed specification: **needs to be obtained** (official store.hancom.com)
- [ ] Equation specification rev1.2: needs to be obtained
- [ ] Chart specification rev1.2: needs to be obtained
- [ ] Distribution-document specification rev1.2: needs to be obtained (prerequisite for starting GA-2)

### 1.2 Authoring rules (optimized for agent access)

- Location: `docs/spec/`, which is **gitignored** (no Hancom derivatives may be committed; see
  docs/README.md). UTF-8.
- Split by specification chapter with predictable names: `docs/spec/4.2-docinfo.md`,
  `4.3-bodytext.md`, `4.3.9-objects.md`, `4.3.10-controls.md` and so on.
- **Tables must be Markdown pipe tables**: `| bit | name | value | meaning |`. Enumerate values inside
  the cell as `0=paper, 1=page, 2=para`. Do not turn them into prose.
- Put both the § number and the original page number in the heading:
  `## §4.3.9.1 개체 공통 속성 (표 70, p.37)`.
- Numbers use consistent hex notation (`0x1D`). Byte layouts go in a table or a fixed-width code
  block. No images.

### 1.3 Priority (densest error areas first)

- [ ] 1. §4.3.9 objects (drawing-object common properties, pictures, OLE; the D-series error area)
- [ ] 2. §4.3.10 controls (sections, columns, tabs, numbering, fields; the dead-tab and number-format area)
- [ ] 3. All of §4.2 DocInfo records (CHAR_SHAPE, PARA_SHAPE, BORDER_FILL, TAB_DEF, NUMBERING, BULLET, STYLE)
- [ ] 4. §4.1 data types and record headers, §3 storages and streams
- [ ] 5. The rest (§4.4 document history, the companion specifications)

### 1.4 Completion criteria

- [ ] Claude reads the PDF pages visually and cross-checks **every** reconstructed table against the
      original, chapter by chapter. The first sampled review (2026-07-18) passed: tables 70, 36 and 37
      were compared against the implementation and against values confirmed in Hancom, and all matched.

      **Minor defects found (watch for these during the full review):**
      (a) Cell boundaries shifted on some table rows: table 67 "한글| 97 수식" and "OLE| MAKE...",
      table 69 "UINT32 4|..." missing a pipe.
      (b) OCR typos: "HPWUNIT", "개채", "coloum".
      (c) Suspect value: table 70 HorzRelTo "0 : page, 1 : page" (0 is probably a typo for paper;
      needs comparison against the original PDF).
      (d) Suspect cross-reference: table 71 "캡션(표 67 참조)" (table 72 is more likely).

      **Further specification-Markdown defects confirmed during the Phase 2 audit (2026-07-18),
      recommended for the user to fix:**
      - :154 `HWPUINT` → `HWPUNIT` (table 1 data types; a separate instance from the HPWUNIT in (b))
      - :621 table 21 cell merge: values 0/1 need separate rows, `0=original kind unknown`,
        `1=TrueType (TTF)`, `2=HFT`
      - :600/603/606 table 19 cross-references: tables 15/16/17 → **20/21/22**
      - :865/:882 tables 38/39: paragraph head information length `8` → **12** (field sum 4+2+2+4,
        matching both the code and corpus measurements)
      - :945/946 table 43 cross-references: property 2 "(표 40)" → **table 45**, property 3
        "(표 41)" → **table 46**
      - :1690/:1705-1706 tables 106/107 cross-references: "표 80" → the object element table
        (§4.3.9.2.1), "표 27" → **table 32** (picture information)
      - table 42 bullets: the **20B and character@8 notation is wrong**. Genuine files measure 25B,
        with [8..12] = numbering character shape id (0xFFFFFFFF) and the character at 12 (confirmed
        2026-07-19 by Hancom testing plus five genuine records, 07 B7)
      - §4.3.10.1.2 foot/endnote shape: the **separator length is documented as HWPUNIT16 (2B) but
        measures 4B (HWPUNIT)**, for 28B total (all 476 genuine records plus a two-way comparison with
        hwpx noteLine@length; investigation GC-3)
      - §4.3.10.1.3 table 135 page border: the **"total length 12" declaration is wrong** (fields sum
        to 14, and all 714 genuine records are 14B). This was the root cause of the "layout
        counter-evidence (unconfirmed)" history in 07/08, closed by investigation GC-2.
      - Three cases where the **specification PDF itself is wrong** and the Markdown faithfully
        reproduces it (only an editorial "[original typo]" note is recommended): table 144 nwno fields
        sum to 6B versus a declared total of 8; table 145 pghd data type UINT (4B) versus a declared
        length of 2; table 138 "속성 bit0-15(표 138 참조)" self-reference (table 139 is correct). In all
        three our code follows the genuine-file measurements and is correct.

---

## Phase 2: full review of hwp-cli against the reconstructed specification

### 2.1 Exhaustive specification-to-implementation audit

- [x] **Full feature-coverage audit (2026-07-19)**: §3, §4.2, §4.3 and §4.4 audited in four parallel
      areas (a three-way comparison of code, specification and gap catalog, with every new candidate
      re-verified against the code). Result: 17 new gaps registered (see the §0.4 registration history
      in document 12: GB-13~15, GD-4, GE-9~13, GE-α9, GE-β7~β8, GF-4~5, GG-21~24), 6 existing entries
      strengthened, 3 corrections to documents 10/11 (cell 34B, border 13B, a missing (c) table), and
      3 verdicts deferred (document 12 §14.3, pending genuine-file measurement). The four
      content-loss items (GF-4 22 field kinds, GF-5 hidden comments, GE-β7/β8 storages) are the first
      to fix.
- [ ] Parser (`hwp5/doc_info.rs`, `body_text.rs`): compare record layouts and bit interpretation
      against the new specification tables, section by section (**bit-level value verification**,
      separate from the coverage audit; outstanding)
- [ ] Writer (`hwp5/write.rs` synthesis constants and bits, `hwpx/write/*`): check hardcoded constants
      against the specification
- [ ] IR (`hwp-model`): compare field meanings and units against the specification
- [ ] Hunt down all **D2-class latent defects** (caused by misread bits and damaged tables) with a
      per-section audit fan-out

### 2.2 Design-document accuracy audit

- [ ] Document 10 (hwp5 structure map): compare and correct the payload summaries and status labels in
      tables A to D against the new specification
- [ ] Document 11 (hwpx structure map): extend the element and attribute catalog once the OWPML
      specification is available
- [ ] Document 07 (compatibility rules): separate rules explained by the specification from rules known
      only from Hancom testing

### 2.3 Re-evaluate the gap catalog (document 12)

- [ ] Re-estimate items whose difficulty changes now that the specification is available (for example
      GB-class payload interpretation may drop from M to S if a table exists)
- [ ] Promote distribution documents (GA-2) to a start candidate as soon as the specification is available

### 2.4 Fix and verify

- [ ] Schedule fixes for audit findings by priority
- [ ] Extend the Hancom verification set (A to G) with isolation files per fixed area
- [ ] Finally: an audit report (findings and corrections), a fully green test run, and re-verification
      in Hancom

---

## In progress (2026-07-19)

- [x] **GK-1 and GK-2 implemented and merged with #9 (2026-07-19)**: four primitives
      (merge/split/add-col/delete-col) plus CLI and invariant gates, based on an exhaustive measurement
      of 1,816 merged tables in genuine files (five rules, unanimous). Inherits #9's column insertion
      (total width preserved), recursive locator and replace fast path, unified with merged-table
      support. All of scripts/check.sh passes; verification set 30/0. **K1 to K3 confirmed in Hancom**
      (merging, vertical alignment and column manipulation all correct).

---

## Feature backlog

- [ ] **Upgrade hwp/hwpx → HTML conversion** (registered 2026-07-19). `convert --to html` already
      exists (`hwp-convert/src/html.rs`) but has lower fidelity than markdown. Resolve what remains on
      the HTML path of GH-3 (footnote markers), GH-4 (merged-cell colspan/rowspan) and GH-5 (in-cell
      blocks), which are already solved for markdown, and improve CSS mapping of character and
      paragraph shapes (font, size, color, alignment, line spacing) plus columns and headers/footers.
      The goal is "at least on par with markdown, plus faithful styling". Footnotes as `<sup>` with an
      anchor and merged tables emitting colspan/rowspan are the first scope.

---

## Relationship to work in progress

- **D3 dead tabs**: the F/G bisection (2026-07-18) found F1, F2 and G1 all dead, which **convicts the
  base file** (tabs are innocent). The cause is being isolated by diffing C1 (opens) against F1 (dead),
  in the D3 lane of the Phase 2 audit workflow.
- **Phase 2 started (2026-07-18)**: six per-chapter specification audits plus one D3 diff, feeding an
  adversarial verification (xhigh) workflow per finding.
- The low-cost batch (GC-4/5/8/9, GE-β5, GM-7) and the D-series fixes are **to be committed after
  confirmation in Hancom**.
