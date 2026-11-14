[한국어](markdown-export.ko.md) · [English](markdown-export.md)

# Markdown export

How `ahwp` turns HWP/HWPX into GFM and what `--with-segments` adds, for anyone extracting text for search, RAG or an agent that needs to quote by source paragraph.

## Export a document to Markdown

Converts hwp/hwpx to GFM. `ahwp cat --format markdown` writes to stdout only and does not extract images.

```sh
ahwp convert report.hwp -o report.md                    # images extracted to report.media/
ahwp convert report.hwp -o report.md --media-dir figs   # extracted to figs/ and linked as figs/...
ahwp convert report.hwp -o full.md --with-header-footer --with-hidden
```

| HWP element | markdown |
|---|---|
| `개요 N` (outline) style paragraph | `#` × N heading |
| Bold / italic / strikethrough / underline / super- and subscript | `**bold**` / `*italic*` / `~~strike~~` / `<u>` / `<sup>`, `<sub>` |
| Hyperlink (%hlk) | `[text](URL)` |
| Image | `![image](<media>/imageN.png)` (extension detected from magic bytes) |
| Table without merged cells | GFM pipe table |
| Table with merged cells, nested tables or block equations | HTML `<table>` (colspan/rowspan) |
| Bullet / numbered paragraphs | `- ` / `N. ` lists (number format synthesized from the numbering definition) |
| Foot- and endnotes | `[^N]` / `[^eN]` GFM footnotes |
| Equation (eqed) | `$script$` / `$$script$$` (the HWP equation script verbatim, not LaTeX) |
| Headers/footers and hidden comments | Excluded by default; `--with-header-footer` / `--with-hidden` include them |

**Table serialization guarantee**: every `<tr>...</tr>` of an HTML table and every row of a GFM pipe
table is serialized on a single line (a nested table inside a cell is inlined on that same line).
This is pinned by tests so consumers can quote and parse row by row.

Limitations: headings are recognized only from the `개요 N` style, floating object position and
z-order are not represented, and the reverse direction (md → hwp) restores only basic constructs such
as tables and bold.

### `--with-segments` (provenance coordinates)

`ahwp cat <file> --format markdown --with-segments` emits, alongside the markdown, a single-line JSON
envelope stating which source paragraph each output character range came from.

```json
{"markdown": "...", "segments": [
  {"kind": "para", "section": 0, "para": 12, "start": 345, "end": 512}
]}
```

- **Offsets are Unicode scalars (characters)**, the same as Python `str` indexing, so
  `markdown[start:end]` slices directly.
- **Coordinates are IR indices**: `section`/`para` are indices into `sections[]`/`paragraphs[]` from
  `--format json`, so they stay stable across re-decoding.
- **Sorted and non-overlapping**: segments ascend by `start` and never overlap. Output belonging to no
  paragraph remains as a gap.
- Lines produced by a table inherit the index of the paragraph containing it, and foot/endnote
  definitions belong to the referencing paragraph.
- The `markdown` field is byte-identical to the output produced without `--with-segments`.
