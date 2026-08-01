[한국어](18-html-fragment-contract.md) · [English](18-html-fragment-contract.en.md)

# HTML Fragment Contract (v1)

An HTML fragment interchange contract for the Maru document composer's **part-based
authoring and assembly**. Prose is written in markdown; non-prose blocks (tables, figures,
etc.) are written as HTML fragments under this contract. It is the exchange format for
workflows that write large documents (business plans, final reports) part-by-part and
compose them.

- **Producer**: `hwp-convert/src/html.rs` (`to_html`, `to_html_fragment`)
- **Consumers**: `hwp-convert/src/from_html.rs` (contract parser), the HTML block path in
  `from_markdown.rs`

## 1. Principles

1. **Machine-generated only** — input must come from a producer that knows this contract,
   not arbitrary hand-written HTML.
2. **Well-formed XML (XHTML)** — empty tags are self-closing (`<br/>`, `<img …/>`), attributes
   are double-quoted, text is XML-escaped. Must be parseable with quick-xml.
3. **Structural round-trip, not style round-trip** — `class`/`style` attributes are
   presentational and ignored on import. What round-trips is document structure (table
   spans, cell blocks, images, links, inline marks).
4. **Contract violations are hard errors** — unknown tags, malformed XML, and span overflow
   are never repaired by guessing (same "no guessing" stance as the oracle methodology).

## 2. Supported Elements

### 2.1 Blocks

| Element | Meaning | Notes |
|---|---|---|
| `h1`..`h6` | "개요 N" (outline N) style paragraph | export & import |
| `p` | body paragraph | export & import |
| `ul`/`ol`/`li` | lists (nested `li` = level) | import only (export does not emit yet) |
| `table` | table — see §3 | export & import |
| `figure` + `figcaption` | image + caption paragraph | import only (export emits bare `img`) |
| `section.footnotes` | footnote/endnote definitions (§5) | presentational only |

### 2.2 Inline

| Element | Meaning |
|---|---|
| `strong`/`em`/`u`/`s` | bold / italic / underline / strikethrough |
| `sup`/`sub` | superscript / subscript (see §5 for the footnote-marker carve-out) |
| `a[href]` | hyperlink field |
| `br/` | line break (LINE_BREAK) |
| `img` | picture — see §4 |

## 3. Table Contract

- Structure: `<table>` → `<tr>` → `<th>`/`<td>`. `thead`/`tbody` are optional.
- **First row uses `<th>` by convention** — presentational only. Import does not
  distinguish `th`/`td` (the IR has no header-row concept).
- **Merged cells**: only the origin cell is emitted, carrying `colspan`/`rowspan`. Covered
  slots are not emitted. Import reconstructs `Cell.col_span`/`row_span` via an occupancy
  grid.
- **Span overflow** (a span extending past the grid or overlapping a covered slot) is an
  import error.
- **Blocks inside cells**: cells may contain nested `table` and `img` in addition to
  inline content. Paragraph boundaries are expressed with `<br/>`.

## 4. Image Contract

- Three `src` forms:
  1. `data:<mime>;base64,…` — self-contained embed (what export emits)
  2. Relative path — resolved against the part file's `base_dir`
  3. `*.svg` — routed through the DocumentSpec v2 svg visual policy (hwpx native, hwp5
     follows the existing fallback policy). If reuse is impossible in v1, raise an explicit
     error rather than silently rasterizing.
- `alt` is preserved as the IR Picture's alt text (export fixes it to `"image"` — a
  convention, not a rule).

## 5. Footnotes/Endnotes (presentational only)

- Body marker: `<sup id="fnref-{N}"><a href="#fn-{N}">{N}</a></sup>` (endnotes use `e{N}`).
- Definitions: `<ol>`/`<li id="fn-{N}">` inside `<section class="footnotes">`, each ending
  with a back-link `<a href="#fnref-{N}">↩</a>`.
- Import reads this structure as **plain text** only (recreating footnote semantics is out
  of scope for v1 — to avoid ambiguity with the `sup` inline mark). A `sup` carrying a
  `fnref` id is recognized as a marker and only its text is taken, and the
  `<section class="footnotes">` definitions section is dropped entirely (so the markers and
  the definitions are not duplicated into the body).

## 6. Unsupported · Errors

- Unlisted tags such as `script`, `style`, `iframe`, `form` → import error.
- Malformed XML (mismatched closing tags, unquoted attributes) → surfaced parser error.
- CSS class based char/para shape restoration → out of scope for v1 (ignored).

## 7. Version

- v1 (2026-08): initial contract. Established by the export-side GH-3/GH-4/GH-5 resolution
  and the introduction of from_html.
