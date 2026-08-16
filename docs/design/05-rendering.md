[한국어](05-rendering.ko.md) · [English](05-rendering.md)

# The hwp-render rendering engine, specified for a rebuild

Crate root: `crates/hwp-render`. Files: `src/lib.rs`, `layout.rs`, `lineseg.rs`, `shape.rs`,
`shape_draw.rs`, `gso.rs`, `tab.rs`, `fonts.rs`, `display.rs`, `png.rs`, `svg.rs`, `pdf.rs`. The
input is the `hwp_model::Document` IR; the outputs are PNG (tiny-skia pixmaps), SVG strings and a
single multi-page PDF.

---

## 0. Coordinate and unit conventions (invariants shared by every layer)

- **HWPUNIT**: HWP's internal integer unit, where `1 pt = 100 HWPUNIT`. Converting to pt is always
  `hu / 100.0`, matching `HwpUnit::to_pt()`.
- **PARA_SHAPE margins** (margin_left/right, indent, spacing_top/bottom) are the one exception,
  stored in the IR as **2 × HWPUNIT**. Converting to pt uses `/200.0` (`para_geometry`). The reason:
  the hwpx reader stores OWPML `left=1500` as hwp5 `ml=3000`.
- **COLORREF**: `0x00BBGGRR`, extracted as `r=c&0xFF, g=(c>>8)&0xFF, b=(c>>16)&0xFF`. By convention
  `0xFFFFFFFF` means "none / inherit", and rasterization falls back to black.
- **Page coordinates** (DisplayList): origin top left, y downward, units pt (f32). Only the PDF
  backend flips on emission with `y' = page_h - y` (PDF is bottom left with y up).
- **Font glyphs**: y-up, flipped when placed for raster and PDF.

The global pipeline:

```
Document(IR) ─layout::layout_document─▶ display::DisplayList ─┬─ png::render_png (tiny-skia)
                                                             ├─ svg::render_svg (a string)
                                                             └─ pdf::render_pdf (Type0/CID)
```

`build_display_list` in `lib.rs` creates a `FontStore::new()`, loads `--font-dir`, calls
`layout_document`, and merges the font resolution report into the warnings.
`RenderOptions{ dpi: f32 (96 by default), font_dirs }`.

---

## 1. The layout pipeline (`layout.rs`, `layout_document`)

### 1.1 Deriving page geometry

Per section, `section.section_def().page` (or `default_page()` = A4 59528 × 84186 HWPUNIT). Landscape
(`page_def.attr & 1`) swaps width and height.

```
w, h            = paper_w_hu/100, paper_h_hu/100
body_left       = margin_left / 100
body_top        = (margin_top + margin_header) / 100
body_width      = (paper_w - margin_left - margin_right) / 100
body_bottom     = h - (margin_bottom + margin_footer) / 100
```

State variables: `prev_v_pos=-1` (to detect a page reset), `content_bottom=body_top` (the flow
cursor), `paras_on_page=0`, `page_notes: Vec<&Note>` and `list_state: ListState`.

### 1.2 Page break rules (three triggers)

1. **Body overflow**: `content_bottom > body_bottom && paras_on_page>0` renders footnotes, adds the
   page furniture (`PageNumberState::finish`: header, footer and page number), pushes the page and
   resets the state.
2. **Explicit page break**: `para.header.break_type & 0x04 != 0 && paras_on_page>0`.
3. **A cached v_pos reset**: when drawing stored linesegs,
   `seg.v_pos < prev_v_pos && !page.items.is_empty()`. Genuine multi-page documents reset v_pos to 0
   on each page, so a decrease is treated as a page boundary.

### 1.3 Paragraph handling branches

Per paragraph, prepare `footnote::para_marks`/`para_notes` (collecting footnote markers and notes),
`tab::tab_stops`, `para_geometry`, `hyperlink_ranges` and `list_state.marker_for_render`, then:

- **When `line_segs` is empty (fallback)**: an empty paragraph advances `content_bottom += 16.0`.
  Otherwise the whole paragraph is shaped with `shape_range_notes` and broken greedily against
  `body_width` by `place_wrapped`. `baseline_y = content_bottom + spacing_top + max_size*1.2`, and
  `content_bottom = last_y + max_size*0.4 + spacing_bottom`. Center and right alignment are corrected
  only when the content fits on one line.
- **When `line_segs` exists (respecting the cache)**: each seg shapes the range
  `[text_start, next.text_start)` with `shape_range_notes`. The key coordinates:

  ```
  stored_baseline = body_top + (seg.v_pos + seg.baseline_gap) / 100
  baseline_y      = max(stored_baseline, content_bottom + baseline_gap_pt)   // corrected by the flow cursor
  x               = body_left + seg.col_start/100 + align_shift
  line_advance    = max(seg.line_height + seg.line_spacing, seg.line_height) / 100
  content_bottom  = last_y + max(line_height_pt - baseline_gap_pt, 0)
  ```

  `wrap_width` is `seg_width_pt` only when the paragraph has exactly one seg (treating that as an
  incomplete cache and re-wrapping); with several it is `f32::INFINITY` (trusting the cache). The
  list marker is drawn on the first seg (i == 0).

After the paragraph, `layout_para_objects` places tables, images, text boxes, shapes and equations.

### 1.4 Alignment (`align_line` / `justify_line`)

`align = (para_shape.attr1 >> 2) & 0x7` (0 justify, 1 left, 2 right, 3 center, 4 distribute,
5 divide).

- Right: `shift = max(seg_width - natural, 0)`.
- Center: `shift = max((seg_width - natural)/2, 0)`.
- Justify (0, except the last line): `justify_line` distributes the surplus
  `slack = clamp(seg_width - natural, 0, natural)` across glyph advances. **When spaces exist it goes
  only to the space glyphs; otherwise it is spread evenly over the gaps before the last visible
  glyph.** Trailing spaces get nothing (so the text reaches the right edge). Rustybuzz cluster
  sources map glyphs back to Unicode sequences, including ligatures and combining text.
- Distribute (4): the full surplus, without the ordinary-justify safety cap, is spread evenly over
  every gap **including after the last glyph**, and the last line is stretched too. A one-glyph line
  can therefore fill the segment through its trailing gap.
- Divide (5): the full surplus is spread evenly over the gaps **excluding after the last glyph**, and
  the last line is stretched too. (The 4/5 last-line semantics await Hancom confirmation.)

### 1.5 Table layout (`layout_table`)

1. **Grid geometry**: column widths come from cells with `col_span==1` and row heights from cells with
   `row_span==1`.
2. `derive_col_widths` derives unknown columns from merged cells (smallest merge first), falls back to
   an average, then scales by `table_true_width` (the maximum of the per-row cell width sums). When
   the real table width exceeds the available width it shrinks proportionally to fit.
3. **Measurement pass**: each cell is drawn into a scratch `PageList` with `layout_box_paragraphs` to
   measure its real content height, giving `row_h[r] = max(row_h[r], content_h + mt + mb)`. For
   `row_span>1`, any shortfall against the spanned sum is added to the last spanned row.
4. Cumulative offsets `col_x = prefix_sums(col_w, x)` and `row_prefix = prefix_sums(row_h, 0)` (per
   fragment a base y is added, so the same prefix serves every page fragment).
5. Per cell: **a background Rect**, then **the content** (margins plus the vertical alignment `voff`
   from `(list_attr>>5)&0x3`: 0 top, 1 center at avail*0.5, 2 bottom at avail), then **four border
   Lines** (`width_mm()*72/25.4`), then **the diagonals** (`diagonal_dirs(attr)`: slash bits 2-4,
   backslash bits 5-7).
6. **Page splitting** (body flow only, via `TableSplitCtx`; nested tables in cells/text boxes never
   split): when the table would cross `body_bottom`, `Table::page_break_policy()` (`attr` bits 0-1)
   decides. NONE pushes the whole table to the next page (the 04 §6.1 invariant), falling back to
   row splitting only when the table is taller than a full page; TABLE/CELL split at row boundaries,
   and CELL additionally continues a single over-height row at an existing cached line boundary
   (soft `v_pos` packing, row-level blank continuation fragments, span-aware packing, page-bottom
   sliver containment — the contract is 21-pdf-parity §6), reporting an unsafe cache or a row span
   intersecting a fragmented row as `table_cell_fragmentation_incomplete`; a band that still
   exceeds a fresh page overflows and is reported as `TableRowTooTallClipped`; a `treat_as_char`
   table never splits ("one character", GE-8). Each
   split runs the standard page-close sequence (notes → furniture → `push_page_checked` → flow-state
   reset), so the next paragraph's Hancom boundary flag is absorbed instead of double-breaking. When
   `repeat_header()` is set, the leading rows whose cells are all `Cell::is_header()` (`list_attr`
   bit18) are redrawn at `body_top` of every continuation page. Candidate boundaries crossed by a
   `row_span` are excluded, so a merged cell moves as one indivisible row band instead of being
   truncated. An indivisible band taller than the usable page is reported as
   `TableRowTooTallClipped`.

`cell_margins`: the cell's own, then the table's `inner_margins`, then the default
`DEFAULT_CELL_MARGINS=[510,510,141,141]` HWPUNIT. The return value is /100 pt.

### 1.6 Block objects (`layout_para_objects`)

Walks `para.controls`, placing objects vertically from `object_y` (the anchor top):

- **Table**: laid out by `layout_table` (may split across pages — §1.5 step 6). The anchor of a
  page-spanning paragraph is re-anchored to the new page's flow position: `para_top` is cleared at
  every cached-lineseg page break, so the object no longer lands at the stale first-page y on the
  final page.
- **Picture**: `doc.resolve_bin(bin_ref)` → `Item::Image`.
- **Text box (gso with paragraph_lists)**: positioned by `gso::parse_gso_box`. With `treat_as_char()`
  it uses the flow position (inline); otherwise `(horz_offset, vert_offset)` is page-absolute
  (floating). The frame is drawn first, behind the text, by `shape_draw::draw_gso_shapes`. Inner
  paragraphs are split into columns by `split_columns` (a v_pos decrease is a column break), and
  `continuation_columns` (a floating gso further right with the same width, height and vertical
  offset) locates linked text boxes so text flows into them.
- **Pure shapes (gso with no paragraph_lists, `has_shape`)**: `draw_gso_shapes`.
- **hwpx structured shapes (with `gso_shapes`)**: when anchored, cloned and adjusted to the flow
  position, then `draw_ir_shapes`.
- **Equations (with `equation`)**: a grey dashed box plus `prettify_equation` text (Greek and operator
  tokens mapped to Unicode).

### 1.7 Headers, footers and footnotes (`Furniture`, `render_page_notes`)

The first `b"head"` and `b"foot"` gso found in a section repeats on every page. Footnotes are numbered
across the whole section by `footnote::collect_notes`, then the notes anchored on a page are stacked
in a scratch list from y=0 to measure the total height; the block is raised so its bottom touches
`body_bottom`, and a separator (`body_width*0.34` wide) plus `translate_item` merges it in.

### 1.8 Multi-column layout (`cold` / `ColumnDef`)

**IR:** a `cold` CTRL_HEADER becomes `ColumnDef{count, kind, direction, same_width, gap, widths,
divider}`. hwp5 parses it from the payload (measured `08 10 dc 08 ...` = attr `0x1008` with bits 2-9
the column count and bit12 equal width, plus gap 2268); hwpx reads `<hp:colPr>` attributes. It agrees
bit for bit with hwplib's COLDEF.

**The key (measured ground truth):** the line_seg Hancom saves has **`col_start` (horzpos) = 0**
(relative to the column) and **`seg_width` (horzsize) = the column width**. The column's x position is
not stored, so it is **computed from a band index**. A v_pos reset (line layout returning to the top)
**means both a column break and a page break**: each reset increments the band index `col_band`, and
when `col_band % count == 0` it is a page break, otherwise a column break on the same page (x moves to
the next column and the cursor returns to the top). The line x is
`body_left + (col_band % count)·(col_width+gap) + col_start`, where
`col_width = (body_width - gap·(count-1))/count`. v1 covers normal sequential flow with equal widths;
balanced (height-balanced) columns, separators and synthesized (markdown) columns come later.
Verified against the ground truth `multicol.hwp/.hwpx` (two columns) giving three pages with the
columns side by side (test `다단_2단_렌더`).

### 1.9 Equation typesetting (`equation.rs`, a mini-TeX)

Hancom equations store **a text script** (EQN compatible) rather than glyphs (hwp5 EQEDIT record 0x58:
attr(4) + len(2) + WCHAR[len]; the script of hwpx `<hp:equation>`). `equation.rs` tokenizes the
script, builds a math tree, typesets it with a box model and emits glyph runs plus fraction and
radical lines (`Item::Line`) in **baseline-relative coordinates**.

**Grammar (Hancom equation spec rev1.2):** `over`/`atop` (fractions), `sqrt` (radical), `^`/`_`
(scripts), `{ }` (groups), `#` (line break, a vertical stack of rows), `&` (column alignment, a space
in v1), `~` and `` ` `` (spaces), plus mappings for Greek letters, operators and function words. The
AST is `Row/Stack/Sym/Frac/Script/Sqrt/Space`. **A stack (multiple rows)** is laid out vertically
using each row's real height (a fraction row is taller). **Sizing is a two-pass process**: typeset
trially at a 12pt baseline, measure the real height, scale to fit `eq.height` (preventing multi-row
equations from being oversized) and align to the top. Verified against the ground truth
`equation.hwp/.hwpx` (multi-row ΑΒΓ, ∑, fractions, radicals and scripts) matching Hancom structurally
(test `수식_정답지_렌더`). Still to come: italic variables versus roman function names, matrices and
large-operator limits.

### 1.10 Page numbers (`PageNumberState`, `page_number.rs`)

Document page numbering starts from `DocHeader.properties.start_numbers[0]` (1 when 0), and every page
completion path passes through `PageNumberState::finish` before incrementing by one. A paragraph's
page number controls are applied after the page break decision, so a control on the first paragraph of
a new page cannot pollute the previous page.

- `pgnp`: the low 8 bits of `props` are the number format and bits 8 to 11 the position. Positions 1
  to 6 are top and bottom, left, center and right; 7 to 10 are outside and inside, mirroring by odd
  and even page. Position 0 means no display. User, prefix, suffix and side characters are preserved,
  so `sideChar='-'` draws `- 1 -`.
- `atno`: only controls of kind PAGE (0) are substituted by `shape_range_page` with the current
  logical page number. Because the same header or footer paragraph is reshaped on every page, the
  number displayed is each page's real number rather than the stored `number` value. Automatic numbers
  other than PAGE remain the responsibility of other number renderers.
- `nwno`: when the kind is PAGE, the logical number of the page containing that paragraph restarts
  immediately.
- `pghd`: on a page where bit5 is set, both positional page numbers and PAGE `atno` are hidden. The
  hidden state resets after the next page is completed.

Hangul inserts these controls wherever the caret was, so a genuine document routinely carries them
inside a nested paragraph list — a text box or a shape — rather than in the body flow. Such a list is
anchored to its paragraph and therefore lands on that paragraph's page, so nested lists are scanned
to a bounded depth (8) alongside the paragraph's own controls. Table cells are deliberately excluded:
a table can split across pages, so a control in a later row belongs to a later page and applying it
at anchor time would restart `nwno` or clear `pghd` on the wrong one. Honoring a cell control needs
the scan to follow fragment layout, which no observed document requires yet.

A positioned number is drawn in the header or footer band, the same band `Furniture::render` uses:
the header band starts at `margin_top`, the footer band ends at `page_height - margin_bottom`. Inside
its band the number hugs the outer edge — the footer's text bottom sits on the bottom-margin line,
measured against the Hancom oracle to within 0.3pt, and the header mirrors that rule (no oracle covers
the header yet). Ascent and descent are approximated from the run size rather than read from the face.

The number formats rendered are those the IR can express: DIGIT, CIRCLED_DIGIT, ROMAN upper and lower,
LATIN upper and lower, HANGUL_SYLLABLE and HANGUL_JAMO. Any other format falls back to decimal so the
number does not vanish, warning once. That is render behavior; GE-4, where HWPX conversion fixes
`pgnp formatType` to DIGIT, remains a separate gap.

---

## 2. lineseg synthesis (`lineseg.rs`, `synthesize_linesegs`)

Documents synthesized from markdown or hwpx have no PARA_LINE_SEG cache, which Hancom judges as
"corruption". Here we shape with the same fonts as genuine files (HCR Batang), reproduce the line
breaking and generate linesegs.

### 2.1 Constants and premises

- `TAB_INTERVAL_PT = 40.0` (must match layout.rs).
- `TABLE_BLOCK_PADDING = 566` HWPUNIT (2.0mm). Measured on genuine files:
  `table advance − Σ row heights = 566`, a constant.
- Fallback page body height 75686, fallback body_width 42520.

### 2.2 Section traversal and page resets

A `doc.clone()` snapshot (snap) provides immutable references while doc is mutated. Per section:

```
body_width = pg.width - margin_left - margin_right
content_h  = max(pg.height - margin_top - margin_bottom, 1)
v_pos      = 0   // accumulated relative to the page
```

Per paragraph, `spacing_top` is added to v_pos (except the first paragraph), `fill_nested` fills the
line layout inside cells and text boxes first, and then the total table height is computed. **When a
table does not fit in the remaining space** (`v_pos + table_total > content_h`), `v_pos=0` starts the
next page. The paragraph anchoring a table sits at its entry v_pos, and a paragraph containing a table
overwrites the cursor with `v_pos = anchor_v + table_total` (preventing overlap). Finally
`spacing_bottom` is added.

### 2.3 Line layout for one paragraph (`compute_linesegs`)

Line metrics are derived from the `base_size` of the first character shape (1000 by default):

```
base          = char_shapes[first_run].base_size (>0)
ls_type       = para_shape.line_spacing_type()   // 0 percent, 1 fixed, 2 margin-only, 3 minimum
ls_val        = para_shape.line_spacing (version-aware; falls back to line_spacing_old)
line_advance  = { fixed:                  max(ls_val, 0) / 2        // exact; zero/overlap allowed
                  margin-only:            base + max(ls_val, 0) / 2
                  minimum:                max(max(ls_val, 0)/2, base) // natural height ~ base for now
                  percent (ls_val>0):     base * ls_val / 100
                  unspecified:            base * 160 / 100 }       // the genuine default of 160%
line_spacing  = max(line_advance - base, 0)
baseline_gap  = base * 85 / 100
seg_width     = max(body_width, 1)
```

Shaping (`shape_range`) measures glyph x_advance in pt and breaks lines greedily against
`seg_width/100` less the indent the renderer will apply inside the line box
(`layout::line_indents`): a positive indent narrows the first line, a hanging indent every line
after it. Without that subtraction a synthesized line overruns the paragraph's right edge once it
is drawn. The list-marker advance needs shaping and is not accounted for, so the first line of a
paragraph whose marker is wider than its hanging indent can still be that much too long.
Breaks on either side of NB_SPACE are suppressed. Line starts use the
UTF-16 length of each shaping-cluster source, rather than the glyph index, so surrogate pairs,
ligatures and combining text retain correct WCHAR offsets. Each line is emitted by `place`:

```
if v_pos>0 && v_pos + base > content_h { v_pos = 0 }   // page reset (cells use content_h=MAX so it never fires)
segs.push(LineSeg{ text_start, v_pos, line_height:base, text_height:base,
                   baseline_gap, line_spacing, col_start:0, seg_width, flags:0x0006_0000 })
v_pos += line_advance
```

A tab advances to `tab::next_tab(tabs, acc, 40)` — the next explicit tab stop, or
`floor(acc/40)*40 + 40` when none remains. Even an empty paragraph has one line. `flags=0x0006_0000` is
the standard flag value of a genuine body line.

### 2.4 Table height (`table_height`, `fill_nested`, `para_line_block`)

`fill_nested` first fills the line layout of paragraphs inside cells and text boxes (cell width is
`cell.width - margins[0] - margins[1]`, v_pos resets per cell, and `content_h=i32::MAX` disables page
splitting; a text box uses width `gso_shapes[0].w - 566`).

The table height formula (derived from genuine measurements):

```
line block (paragraph) = last line.v_pos + last line.line_height       // para_line_block
rowH                   = margins[2] (top) + Σ paragraph line blocks + margins[3] (bottom)
table height           = Σ_rows max(rowH of cells with row_span==1) + 566
```

Cells with `row_span!=1` are excluded from the row height computation, and a fully merged row falls
back to `141+1000+141`.

---

## 3. Text shaping (`shape.rs`)

### 3.1 Splitting into pieces then shaping (`shape_range_notes`)

1. **Collecting pieces**: walking `para.chars`, text is gathered into a `Piece` at each boundary of
   (character shape id, language slot). `shape_id_at(para,pos)` searches `char_shape_runs` backwards
   for the last id with `start<=pos`. `lang_slot_of(c)`: Korean 0, Latin 0x0000-0x024F 1, CJK Han 2,
   kana 3, everything else 5. A tab (`ctrl_char::TAB`) becomes `InlineItem::Tab`, a footnote anchor
   becomes `note_mark_run` (a superscript number), and PAGE `atno` is substituted by
   `shape_range_page` with a run for the current logical page number.
2. **Shaping each piece** (`shape_piece`): `face_id = cs.face_ids[lang]` and
   `store.resolve(doc,lang,face_id)` give the primary font. When the requested font is in the heavy
   family (`is_heavy_name`: 견고딕, 헤드라인, Bold and so on), `bold` is forced (faux bold).
   **Per-character coverage fallback**: the primary font is used when it has the glyph (not .notdef),
   otherwise `store.font_covering(c)`. Each font boundary starts a new `ShapedRun`, with `start_wchar`
   advancing by `len_utf16()` (1 for BMP, 2 otherwise).

### 3.2 Shaping with one font (`shape_with_font`)

```
base      = cs.base_size (>0 else 1000)
rel       = cs.rel_sizes[lang] (100 by default)
full_size = (base/100) * (rel/100)                       // pt
size_pt   = sup||sub ? full_size*0.65 : full_size
scale     = size_pt / upem
y_raise   = full_size * cs.char_offset(lang)/100
            + (sup ? full_size*0.34 : 0) + (sub ? -full_size*0.16 : 0)
spacing_pt= hwpunit_round(size_hu * cs.spacings[lang] / 100) / 100   // letter spacing, HWPUNIT half-up
x_scale   = cs.ratios[lang] / 100                        // width scaling
```

After rustybuzz `shape(&face, &[], buffer)`, per glyph:

```
x_advance = gpos.x_advance * scale * x_scale + spacing_pt
x_offset  = gpos.x_offset  * scale * x_scale
y_offset  = gpos.y_offset  * scale + y_raise
```

### 3.3 Character shape bits (`hwp_model::CharShape`, `attr: u32`)

| Effect | Bits | Method |
|---|---|---|
| italic | bit0 | `is_italic` |
| bold | bit1 | `is_bold` |
| underline | bits 2-3 (1 below, 3 above) | `underline_kind` / `has_underline` (==1) |
| outline | bits 8-10 (≠0) | `has_outline` |
| shadow | bits 11-12 (≠0) | `has_shadow` |
| emboss | bit13 | `is_emboss` |
| engrave | bit14 | `is_engrave` |
| superscript | bit15 | `is_superscript` |
| subscript | bit16 | `is_subscript` |
| **strikethrough** | **no bit used**, a separate `strike: bool` | `has_strike` |

The strikethrough bits (18 to 20) are DIFFSPEC and untrusted. The HWP5 reader always sets false, and
only HWPX sets it from a visible `<hp:strikeout>`. `shade_color` is the background highlight
(0xFFFFFFFF = none), and `underline_color` and `shadow_color` are also COLORREF. A `ShapedRun` carries
color, bold, italic, underline, strike, underline_color, shade_color, shadow (Option), outline, emboss
and engrave.

### 3.4 Hyperlinks, lists and footnote markers

`hyperlink_ranges` gives the WCHAR range between a `%hlk` FIELD_START (ExtCtrl code 3) and FIELD_END
(InlineCtrl code 4). `apply_link_style` applies an underline plus `LINK_BLUE=0x00CC0000` to the runs
in that range. `shape_plain` is for synthesized text (equations, markers): it shapes a single run as a
whole (no fallback splitting, with `shade_color=0xFFFFFFFF` to avoid the black-box trap).

List markers come from `ListState::marker_for_render` (`hwp-model/src/list.rs`): numbering
(head_type 2) and bullet (3) paragraphs as before, plus outline (head_type 1) paragraphs, which get
the default fixed per-level markers (`1.` / `가.` / `1)` / `가)` / `(1)` / `(가)` / `①`) from a
dedicated counter family. Empty paragraphs do not consume that counter, and each text box has its
own counter scope. The outline `numbering_id` remains a raw, unnormalized reference: custom
outline definitions, restart behavior, and the sequence beyond the known 14 Hangul markers are
still GG-12 oracle work. Text converters (markdown and friends) keep calling `marker()`, which
leaves outlines as heading structure only.

---

## 4. Shape drawing (`shape_draw.rs`, `gso.rs`)

### 4.1 Two paths: hwp5 direct (raw) versus ShapeGeom (IR)

- **The hwp5 direct path** (`draw_gso_shapes` → `walk` → `draw_component` → `geometry`) parses the gso
  control's `raw_children` (OpaqueRecord) **at render time**. It is consumer-only and touches neither
  the IR nor the writer. Coordinate transform: `local point → render matrix (T·S·R) → + origin → /100
  = pt`.
- **The ShapeGeom (IR) path** (`draw_ir_shapes` → `ir_shape_path`) handles hwpx structured shapes.
  Coordinates are already page-absolute HWPUNIT, so no matrix is needed: `(x+px)/100`.

`has_shape(recs)` decides recursively whether a SHAPE_COMPONENT (0x4C) child contains geometry
(SC_LINE 0x4E, SC_RECTANGLE 0x4F, SC_ELLIPSE 0x50, SC_ARC 0x51, SC_POLYGON 0x52, SC_CURVE 0x53),
passing through SC_CONTAINER 0x56. `MAX_DEPTH=16`.

### 4.2 The SHAPE_COMPONENT byte layout (`parse_style`)

```
base = (d[0..4]==d[4..8]) ? 8 : 4          // top level has CHID twice, a group member once
cnt  = u16 @ base+42                        // the number of scale/rotation pairs
t    = mat @ base+44 (48 bytes, translation)
pair = base+44+48+(cnt-1)*96               // the last scale/rotation pair
m    = t.mul( mat@pair.mul(mat@pair+48) )  // T·S·R
bo   = base+92+cnt*96                       // the border offset
  color = u32@bo, width = i32@bo+4, lattr = u32@bo+8   // lattr&0x3F ≠ 0 means stroke
fo   = bo+13                                // fill (table 28)
  ft = u32@fo:  ft&1 solid (u32@fo+4)  |  ft&4 gradient  |  ft&2 image
```

`Mat` is a 3×2 [a,b,c,d,e,f] with `x' = a·x+b·y+c, y' = d·x+e·y+f`. `rd_mat` reads six f64 (48 bytes),
and `mul` is standard affine composition.

### 4.3 Geometry records (`geometry`, local HWPUNIT)

- **SC_LINE**: start = p(0), end = p(8). None when equal.
- **SC_RECTANGLE**: a byte of curvature % plus four (x,y) at p(1), p(9), p(17), p(25). With
  curvature > 0 and radius > 1 it uses `rounded_quad_path`.
- **SC_POLYGON**: u16 n at 0, points at 4+i*8. A closed path.
- **SC_ELLIPSE**: u32 attr plus center at p(4), the ax1 endpoint at p(12) and the ax2 endpoint at
  p(20) → `ellipse_path(cx,cy, ax1-c, ax2-c)`.
- **SC_ARC**: a byte arctype plus center at p(1), start at p(9) and end at p(17) → `arc_path`.
- **SC_CURVE**: u16 n at 0, points at 2+i*8. Approximated as a polyline.

### 4.4 Ellipses: four KAPPA Beziers (`ellipse_path`)

`KAPPA = 0.5522847498 = 4/3·tan(45°/2)`. Given a center C and two conjugate axis vectors a1 and a2,
the anchors are C±a1 and C±a2. Each 90° arc becomes a cubic:

```
MoveTo P0 = C+a1
Cubic( C + a1 + k·a2,  C + a2 + k·a1,  P1=C+a2 )
Cubic( C + a2 − k·a1,  C − a1 + k·a2,  P2=C−a1 )
Cubic( C − a1 − k·a2,  C − a2 − k·a1,  P3=C−a2 )
Cubic( C − a2 + k·a1,  C + a1 − k·a2,  P0 )  Close
```

**Conjugate axes are affine-invariant**: because the control points are defined only as linear
combinations of the axis vectors, applying any affine transform to the original circle (including
non-perpendicular conjugate axes) transforms the Bezier exactly. Rotated and sheared ellipses are
therefore accurate.

### 4.5 Arcs (`arc_path`)

Given a center C and start and end points: `r=|start−C|`, `t0=atan2(s.y,s.x)`, and
`sweep = atan2(e)−t0` normalized to the short way in `[−π,π]`. Then `segs = ceil(|sweep|/(π/2))`,
`dphi=sweep/segs` and `alpha = 4/3·tan(dphi/4)`. Each segment's control points follow the tangent
`T'(θ)=r(−sinθ, cosθ)` as `P ± alpha·T'`:

```
c1 = (C + r·(cosθ, sinθ)) + alpha·r·(−sinθ,  cosθ)
c2 = (C + r·(cosθ₁,sinθ₁)) − alpha·r·(−sinθ₁, cosθ₁)
```

### 4.6 ir_shape_path (ShapeGeom)

- **Arc with points ≥ 3**: `points[0]=center, [1]=ax1, [2]=ax2` (conjugate axes), giving a quarter
  elliptical arc as **a single cubic**. With no points it falls back to an ellipse.
- **Ellipse (and the Arc fallback)**: center = (x0+w/2, y0+h/2) with axes (w/2,0) and (0,h/2) →
  `ellipse_path`.
- **Rect**: `radius = (round_ratio/100)·min(w,h)/2`; above 0.1 it uses `rounded_quad_path`, otherwise
  a plain rectangle.
- **Line**: a polyline when points ≥ 2, otherwise (x0,y0) → (x0+w,y0+h).
- **Polygon and Curve**: a closed polygon.

`rounded_quad_path` computes entry and exit points per corner with a radius capped at half the
adjacent sides, drawing each 90° arc as a KAPPA cubic. Arrowheads (`arrowheads`, `arrow_triangle`) are
isosceles triangles along the endpoint direction (`size=max(width*4,5)`). Dashes
(`dash_pattern(style,width)`): 1 dash, 2 dot, 3 dash-dot, 4 dash-dot-dot, 5 long dash (proportional to
thickness).

### 4.7 Gradient and image fills

`parse_gradient` (table 28): type(i16) angle(i16) ... num(i16); when num > 2 an INT32[num] position
array follows (normalized), then COLORREF[num]. `radial = (gtype==1)`. `parse_image_fill` searches
backwards from the 4-byte-aligned tail for a valid BinData id and calls `resolve_bin`.

### 4.8 gso common properties (`gso.rs`, `parse_gso_box`)

Twenty bytes of the CTRL_HEADER payload after the ctrl_id: `attr(4) vert_offset(4) horz_offset(4)
width(4) height(4)` (all i32 LE in HWPUNIT), **the same layout** as hwp5 `parse_picture_gso`.
`attr` bit0 is treat_as_char (inline), bits 3-4 are vert_rel_to (0 PAPER, 1 PAGE, 2 PARA) and bits 8-9
are horz_rel_to (0 PAPER, 1 PAGE, 2 COLUMN, 3 PARA).

---

## 5. Fonts (`fonts.rs`, `FontStore`)

### 5.1 Loading and HWP_FONT_DIR

`FontStore::new()` calls `load_system_fonts()` on a `fontdb::Database`, and `load_dir` adds more
directories. When `--font-dir` is not given, the CLI (`commands/convert.rs`, `render.rs`) loads the
`HWP_FONT_DIR` environment variable (or the project `fonts/`) by default, that is the bundled **HCR
Batang and HCR Dotum**. The golden tests read `HWP_FONT_DIR` too.

### 5.2 The resolution chain (`resolve`)

`(lang_slot, face_id)` gives the name and substitute name from `doc.header.fonts[lang][face_id]`. The
candidate order:

1. the requested name, then 2. the substitute (alt) name, then 3. **family fallback** (`classify`
   guesses gothic versus serif): gothic uses `GOTHIC_FALLBACKS` (HCR Dotum, Apple SD Gothic,
   NanumGothic and so on), serif uses `SERIF_FALLBACKS` (HCR Batang, AppleMyungjo, NanumMyeongjo and
   so on), and unknown uses `FALLBACKS` (HCR Batang first), then 4. the system SansSerif as a last
   resort, then 5. failure.

`classify` uses Korean keywords (돋움/고딕/굴림 versus 바탕/명조/궁서) plus Latin ones (gothic/dotum
versus batang/myeongjo/serif). **No silent substitution**: every outcome goes into the `report`
(`글꼴 일치` or `글꼴 대체: A → B`). Results are cached in
`resolved: HashMap<requested name, Option<Arc<LoadedFont>>>`.

`font_covering(c)` is the coverage fallback that prevents tofu (□) for a specific character
(HCR → Noto CJK → Nanum and so on), cached per character (with a `\u{1}cover:` key).
`LoadedFont{ data: Arc<Vec<u8>>, index: u32, family }` loads bytes from a `fontdb::Source`
(File/Binary/SharedFile) with a `loaded` cache per id.

---

## 6. Backends

### 6.1 The DisplayList (`display.rs`), the layout-to-backend contract

```
PageList{ width_pt, height_pt, items: Vec<Item> }
Item = Glyphs{x,y,run}                       // the baseline origin
     | Rect{x,y,w,h,fill:COLORREF}
     | Line{x1,y1,x2,y2,color,width}
     | Image{x,y,w,h,data:Arc<Vec<u8>>}       // the encoded original
     | Path{commands:Vec<PathCmd>, fill:Option<Fill>, stroke:Option<Stroke>}
PathCmd = MoveTo|LineTo|CubicTo|Close
Fill = Solid(COLORREF) | Gradient(Gradient{radial, angle_deg, stops:[(0..1, COLORREF)]})
Stroke{ color, width, dash:Vec<f32> }
```

`Gradient::color_at(t)` interpolates linearly between stops, and `path_bbox(cmds)` gives the bounding
box used to place a gradient.

### 6.2 PNG (`png.rs`, tiny-skia)

`px_scale = dpi/72`, the pixmap is `ceil(pt·px_scale)` with a white background. Glyphs are extracted
into a tiny-skia Path with `ttf_parser::Face::outline_glyph` plus `OutlinePath` (an OutlineBuilder).
The transform:

```
t = scale(glyph_scale·x_scale, −glyph_scale)   // flip y-up plus width scaling
  ∘ (italic ? skew(−0.2126, 0) : I)            // ITALIC_SKEW
  ∘ translate(pen_x + x_offset + dx, y − y_offset + dy)
  ∘ scale(px_scale, px_scale)
```

`glyph_scale = size_pt/upem`. **Faux bold** is fill plus `stroke(size_pt·0.045/glyph_scale)`
(BOLD_STROKE 4.5%). **Outline** is stroke only (`0.025`). **shade** is a Rect behind the glyph.
**shadow** is a copy offset by 0.06em. **Emboss and engrave** are white highlight offsets (emboss up
and left by −0.05, engrave down and right by +0.05). Images go through
`image::load_from_memory` into premultiplied RGBA, with a magenta placeholder when decoding fails.
Gradients use `gradient_shader` (Linear/Radial, relative to the bbox, with a `px_scale` transform).

### 6.3 SVG (`svg.rs`)

To remove any dependence on viewer fonts, glyphs become outline `<path>` elements, cached by
`(font_ptr, glyph_id) → d`. The transform is `matrix(a 0 skew_c dd e f)` (`a=s·x_scale`, `dd=−s`,
`s=size_pt/upem`). Bold is fill plus stroke (0.045·upem) and outline is stroke only (0.025·upem).
Images use `sniff_mime` plus our own `base64` data URI. Gradients use `<linearGradient>` and
`<radialGradient>` with userSpaceOnUse. `hex_color` converts COLORREF to `#rrggbb` (swapping BGR).

### 6.4 PDF (`pdf.rs`) and CFF

**Font embedding**: a `FontInfo` is collected per unique font, then the used glyphs go through
`GlyphRemapper::remap` plus `orig_to_unicode` (preferring the original text, with `reverse_cmap`
filling in partial runs). `subsetter::subset` produces the subset (embedding the whole font with
CID=GID on failure). By outline kind:

- **glyf (TrueType)**: `CIDFontType2` plus `FontFile2` (Length1).
- **CFF (OTF, a `CFF ` table)**: `CIDFontType0` plus `FontFile3` (Subtype=OpenType), branched on
  `face.tables().cff.is_some()`.

Both are **Type0 (composite) plus Identity-H plus a ToUnicode CMap**. The objects: Type0 → CIDFont (a
W width array, `glyph_hor_advance·1000/upem`) → FontDescriptor (SYMBOLIC, with bbox, ascent, descent
and cap_height × `1000/upem`) → FontFile (FlateDecode) → ToUnicode (`UnicodeCmap`). A subset prefixes
BaseFont with a six-letter tag (`subset_tag`).

**Content**: y is flipped as `h−y`. Glyphs go through `write_glyph_run`: `begin_text`,
`set_font(size_pt)`, `set_horizontal_scaling(x_scale·100)` (Tz width scaling), the render mode (bold =
FillStroke `size·0.045`, outline = Stroke `0.025`, otherwise Fill), and per glyph
`set_text_matrix([1,0,shear,1, pen_x+x_offset, page_h−(y−y_offset)−dy])` followed by
`show(out_gid.to_be_bytes())`. `out_gid` is the remapped GID for a subset and the original otherwise.
The advance uses the same `pen_x += x_advance` as png and svg, so pixels agree.

**Paths and images**: `pdf_emit_path` (CubicTo also flips y). Gradients clip to the path then use
`pdf_gradient_bands` (48 linear bands or concentric radial circles, with `pdf_circle` at KAPPA
0.552285). Dash state is restored to solid after an item with `set_dash_pattern([])`. Images: JPEG
(gray or RGB) passes through as `DctDecode`, and everything else is decoded to RGB (plus an alpha
SMask) with FlateDecode. `jpeg_info` parses (w, h, comps) from the SOF marker.

---

## 7. Core invariants to keep when rebuilding

1. **A single source of shaping advances**: lineseg line breaking (`compute_linesegs`), layout
   placement (`place_wrapped`) and all three backends must accumulate **the same** `glyph.x_advance`
   for pixels to agree. Tab advance is `tab::next_tab` in every accumulator — the next explicit
   tab stop, with the 40 pt default interval only as the fallback.
2. **Respecting the cached v_pos versus the flow cursor**: a stored lineseg's
   `baseline = body_top + (v_pos+baseline_gap)/100` is trusted, but pushed down only by
   `max(stored, content_bottom+gap)` and never pulled up (preventing tall text boxes from drifting).
   Inside a cell (`layout_box_para_iter`), the flow lower bound applies only to flow-placed content.
3. **v_pos is page-relative**: both synthesis and rendering reset v_pos to 0 per page. Accumulating
   monotonically across a section makes Hancom judge the file corrupt.
4. **The table constant 566**: `TABLE_BLOCK_PADDING` must be added exactly once to the total table
   height.
5. **Conjugate axes are affine-invariant**: ellipse and arc control points are defined only as linear
   combinations of the axis vectors, which is the basis of accuracy for rotated and sheared ellipses.
6. **No silent substitution**: font substitution, image decode failure and unsupported controls are
   all made visible through a report, a warning or a magenta placeholder.
7. **COLORREF 0xFFFFFFFF**: for shade it means "none", while raster text color falls back to black;
   interpret it per context.
8. **Only PARA_SHAPE margins divide by 200**; every other HWPUNIT divides by 100.
