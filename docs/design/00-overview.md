[한국어](00-overview.ko.md) · [English](00-overview.md)

# hwp-cli design blueprint

> **Purpose:** record everything this project has learned (byte formats, rendering, conversion,
> Hancom compatibility rules, methodology) so that a system for handling Korean HWP/HWPX documents
> could be **rebuilt from scratch**. This document set (`docs/design/`) is the **design baseline**
> holding the "why" and "how" behind the code.

The project implements **the HWP 5.0 binary format and HWPX (OWPML) directly** rather than depending
on an existing HWP library: parsing, serialization, conversion and rendering alike. Documents and
code comments are written in Korean by default, with English pairs for user-facing documents.

---

## 1. What the system does

```
                     ┌─────────────────────────────────────────┐
   .hwp (binary) ───▶│                                         │──▶ .hwpx
   .hwpx (OWPML) ───▶│   read → IR → write / render            │──▶ .hwp
   .md (markdown) ──▶│                                         │──▶ PNG / SVG / PDF
                     └─────────────────────────────────────────┘
                                        │
                       editing (edit / field / bookmark / format / structure)
```

- **Read**: HWP5 (CFB + records) and HWPX (ZIP + OWPML XML) into the shared IR
- **Write**: IR into HWP5 and HWPX, with the goal of opening correctly in Hancom Office
- **Convert**: hwp5 ↔ hwpx ↔ markdown / JSON / HTML
- **Render**: IR → PNG/SVG/PDF, as pixel-accurate as achievable
- **Edit**: formatting, fields, bookmarks, shapes and structural editing primitives, plus a JSON round-trip

---

## 2. Architecture at a glance

### 2.1 Crates (hub and spoke)

```
                hwp-model  (base IR; depends only on serde, never on another internal crate)
               /    |    |    \        \
          hwp5   hwpx   hwp-convert   hwp-render
               \    |  ____/  |          /
                hwp-cli  (bin: `hwp`, depends on all five)
```

| Crate | Responsibility |
|---|---|
| **hwp-model** | The shared **semantic IR** types for HWP and HWPX. Its stability is the project's stability. |
| **hwp5** | HWP 5.0 binary (CFB + records) ↔ IR reader/writer |
| **hwpx** | HWPX (OWPML, ZIP + XML) ↔ IR reader/writer plus patching |
| **hwp-convert** | IR ↔ markdown / JSON / HTML plus editing primitives |
| **hwp-render** | IR → PNG / SVG / PDF renderer |
| **hwp-cli** | Subcommand dispatch (info, cat, convert, render, new, edit, ...) |

**Invariant:** `hwp-model` never depends on another internal crate, and `hwp5` and `hwpx` never
depend on each other; they meet only through the IR. That symmetry is what turns "N formats × M
outputs" into N+M adapters.

### 2.2 The IR: the L1 semantic layer

`Document → Section → Paragraph → (HwpChar sequence + Control + LineSeg + CharShapeRun)`. Body text
is a sequence of **UTF-16 code units (WCHAR)** in which 0 to 31 are control characters. The single
source of truth for position arithmetic is the `char_kind(code)` classification (character-like
width 1, inline width 8, extended width 8). Lossless round-tripping is preserved through
`OpaqueRecord`. Details in [01-architecture-ir.md](01-architecture-ir.md).

---

## 3. Document index

| # | Document | Contents |
|---|---|---|
| 01 | [architecture-ir](01-architecture-ir.md) | Crate structure, IR type hierarchy, data flow, OpaqueRecord |
| 02 | [hwp5-read](02-hwp5-read.md) | CFB, record header bit layout, DocInfo and BodyText parsing, compression and encoding |
| 03 | [hwp5-write](03-hwp5-write.md) | **Hancom-compatible synthesis** (CFB V3, EncryptVersion, COMPATIBLE_DOCUMENT, version gating) |
| 04 | [hwpx-owpml](04-hwpx-owpml.md) | HWPX ZIP (OPC), hp:/hc: elements, shape geometry, floating and inline placement |
| 05 | [rendering](05-rendering.md) | Layout, lineseg synthesis, shape drawing, font shaping, PNG/SVG/PDF |
| 06 | [convert-cli-methodology](06-convert-cli-methodology.md) | Conversion pipeline, CLI, **the ground-truth methodology and diagnostics** |
| 07 | [hangul-compat-rules](07-hangul-compat-rules.md) | ★ **The full catalog of Hancom compatibility rules confirmed by testing** |
| 08 | [external-research](08-external-research.md) | External evidence on the OWPML standard, open source and pagination behavior |
| 09 | [diagnostic-corpus](09-diagnostic-corpus.md) | The feature-isolating diagnostic corpus and self-verification harness |
| 10 | [hwp5-structure-map](10-hwp5-structure-map.md) | **Exhaustive HWP5 map**: CFB stream tree, record catalog (every tag plus implementation status), control characters and ctrl IDs |
| 11 | [hwpx-structure-map](11-hwpx-structure-map.md) | **Exhaustive HWPX map**: OPC tree, namespaces, element catalog, read/write symmetry audit |
| 12 | [feature-gaps](12-feature-gaps.md) | Feature gap catalog plus a difficulty and dependency roadmap (inherits 07 §F; 10 §8 and 11 §5 are the underlying data) |
| 18 | [html-fragment-contract](18-html-fragment-contract.md) | **HTML fragment contract** — XHTML subset for Maru part-based authoring/assembly, table/image/footnote round-trip rules |
| 19 | [hwp5-spec-supplement](19-hwp5-spec-supplement.md) | **HWP 5.0 spec supplement index** — errata, version-layout matrix, conformance checklist, consumption semantics (07·03·05·10·08 are the underlying data) |
| 20 | [remote-mcp](20-remote-mcp.md) | **Remote MCP transport design** - future Streamable HTTP, OAuth resource-server, tenant isolation, artifact transfer, limits and security gates for web clients |
| 21 | [pdf-parity](21-pdf-parity.md) | **PDF parity contract (Hancom Office 2024)** — oracle, five-metric set, thresholds, font gate, data policy and non-goals the parity harness reads (issue #79) |

---

## 4. Core design principles

1. **Direct implementation.** No existing HWP crate. Only infrastructure crates (cfb, zip,
   quick-xml, tiny-skia and so on). Keep dependencies minimal.
2. **Symmetry through the IR.** Every format meets every other only through the IR; hwp5 and hwpx
   never depend on each other directly.
3. **Lossless round-trip first.** A same-format round-trip (hwp5 → hwp5) is gated on byte identity.
   Only synthesis (where the source is not hwp5) reconstructs.
4. **★ The ground-truth methodology.** No guessing. The **bytes of genuine files** saved by Hancom
   Office are the answer key; we isolate the single difference against our output and adopt it. The
   corpus (`~/Documents/hwp_samples`) must never be committed to the repository.
5. **The Hancom gate.** Whether a file actually opens and renders correctly in Hancom Office is the
   final verdict. See [06](06-convert-cli-methodology.md) and [07](07-hangul-compat-rules.md).

---

## 5. Current status (as of 2026-07)

**Working, confirmed in Hancom Office:** HWP5/HWPX reading, writing, conversion and rendering;
single and multi-paragraph documents, long paragraphs, tables (simple, long cells, single row,
many columns, empty cells), mixed body and table content, multi-page and composite reports;
hyperlinks and bookmarks; formatting and structural editing; and the donut, center circle, numerals
and arcs of the annual-report design document.

**Open or under investigation:** the **text-box drop plus page overflow** on pages 5 and 6 of the
annual report. The cause is most likely object property fidelity (vertRelTo, treatAsChar, z-order,
textWrap, offset) rather than structure (shape-to-paragraph placement); see the external research in
[08](08-external-research.md). Beyond that: U2 (justified alignment), U4 (letter spacing) and
text-box render precision. The full catalog of unimplemented features and loss points, with the
roadmap, is in [12-feature-gaps.md](12-feature-gaps.md).

**★ The most valuable asset:** [07-hangul-compat-rules](07-hangul-compat-rules.md), roughly thirty
Hancom-specific rules that static analysis cannot find and that were confirmed only by comparing
against genuine files and testing in Hancom Office. If this system were rebuilt, that catalog would
be the single biggest time saver.
