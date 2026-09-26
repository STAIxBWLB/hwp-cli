//! Per-(page, segment) geometry rows: where each source segment landed on the page.
//!
//! # What a row is
//!
//! One row per (segment, page) pair. A segment that flows across a page break yields one row
//! per page it touches, each with its own box and its own character range (D-09); that falls
//! out of recording item spans per page rather than per segment, so there is no cross-reference
//! to compute. A segment whose layout produced no display item at all — a bookmark is invisible
//! by design — yields a row with **no box** and a character range (D-08a). A zero-extent
//! rectangle is never fabricated for it: "invisible" and "a dot at the origin" are different
//! claims.
//!
//! A segment id is **not unique** in this set, and not only because of page splits: the header
//! rows of a table that splits are replayed on the following page, so the same `cell` id is
//! reported again with a different box, and two fragments of one cell that land in different
//! columns of a multi-column section stay separate rows rather than being unioned across the
//! gutter. Do not key a map by id alone, and do not assume one row per (id, page) either.
//!
//! # Units and coordinate spaces
//!
//! Boxes are points with the page origin at top-left and y downward — the [`crate::display`]
//! contract, read straight off the `DisplayList` before any backend applies its own transform.
//! PNG, SVG and PDF therefore share one set of numbers and the PDF backend's internal y-flip
//! never reaches a published coordinate (D-07).
//!
//! Character ranges are **UTF-16 code units into the source paragraph**. That is a different
//! space from the segment envelope's Unicode scalar offsets into the markdown output, and the
//! two must never be mixed: the **segment id is the only join key** between a geometry row and
//! an envelope segment. See [`CharRange`].
//!
//! # The join is total, in both directions
//!
//! Every row's id names an envelope segment and every envelope `para`, `table`, `cell` and
//! `bookmark` segment has a row, wherever it lies. The two crates derive ids
//! independently, so this holds because both sides follow the same rules, not because they
//! share code, and the cross-artifact join test in `crates/hwp-cli/tests/render_layout.rs`
//! checks it on every document the host has.
//!
//! A paragraph that shapes no text but is still drawn used to break it (#285): one holding
//! only a drawing object (`gso `) got a row with a real box and no envelope segment at all
//! (`outline.hwp`, one page holding only a drawing, had a row against an envelope of zero
//! segments). The envelope now publishes a point `para` segment for it, and for a paragraph of
//! whitespace, which this module records as shaped text.
//!
//! # What has a row
//!
//! **Kinds.** Only `para`, `table`, `cell` and `bookmark` produce rows. The envelope's other
//! three kinds do not: a `run` has no separate geometry here, an `image`'s display item folds
//! into the row of the paragraph that carries it, and a `field` does likewise. A consumer
//! hit-testing an image or a field resolves it through its enclosing paragraph's row.
//!
//! **Places: everywhere the envelope has a paragraph, at any depth.** A body paragraph opens
//! its span in the body loop ([`SegmentRecorder::begin_paragraph`]); a table and its cells open
//! theirs wherever the table is laid out, and a cell's paragraphs open theirs in
//! `layout_box_para_iter` ([`SegmentRecorder::begin_cell_paragraph`]), whose objects - a
//! nested table, a bookmark, a drawing object - then nest under that span (#283).
//!
//! The text of a drawing object - a text box, an HWPX shape or container - opens its spans in
//! `layout_box_para_iter` too ([`SegmentRecorder::begin_drawing_paragraph`]). The envelope
//! numbers it `[para, control, n]` across ALL of the object's lists, so a caller that lays it
//! out in pieces - a linked text box split into columns, a container's lists each in its own
//! box - passes the offset of each piece's first paragraph (#350). Text the renderer never lays
//! out - an object it cannot place, a kind no layout arm draws - still gets its rows, empty,
//! through [`SegmentRecorder::unlaid`]. Each path is built the way the envelope builds it, one
//! `child(base_path(), index)` at a time.
//!
//! Everything else `layout_box_para_iter` lays out passes it no recorder, and each call site
//! says why: captions and notes, which have no envelope segment; headers and footers, which
//! the default envelope excludes; and the measurement pass, which draws on a scratch page.
//! Such content resolves through the enclosing paragraph's row, whose span contains it.
//!
//! **Segments that produce nothing at all.** A paragraph row with *nothing measured* - no box
//! and no range - would join to nothing: `hwp-convert` emits no envelope segment for the
//! completely empty paragraph, nor for the unanchored `bokm` control (no anchor character, no
//! segment). ONE RULE, NOT TWO COINCIDENCES: it is applied once in
//! [`SegmentRecorder::finish`] rather than case by case.
//!
//! "Nothing measured" is judged **only on the evidence the kind can carry**, which is why
//! `table` and `cell` are exempt. Their [`SegmentRow::chars`] is `None` BY DESIGN - they span
//! several source paragraphs, so no single paragraph's offsets describe them - so for them the
//! rule would silently collapse to "has a box", which is a different rule. A borderless table
//! whose cells are empty produces no display item, and dropping it would delete an ordinary
//! HWP construct from the published geometry; both kinds also always have an envelope segment,
//! so their ids never join to nothing. Adding a kind whose `chars` is structurally `None`
//! means adding it to that exemption, not writing a new special case.
//!
//! A paragraph the envelope ALWAYS has a segment for ([`always_has_para_segment`]) - one with
//! any text, or one holding an object (a table, a picture, an equation, a drawing) - is exempt
//! for the same reason, once.
//! Two ordinary things make such a paragraph draw nothing: its object drew nothing (the
//! paragraph anchoring that borderless empty table), or every line of a cell paragraph was
//! clipped at the cell's edge, which `layout_box_para_iter` does to a line that a shift would
//! push out of the cell and reports as `table_cell_content_overflow`. Either way the paragraph
//! keeps exactly ONE row, and only when it has no measured row anywhere: `box` null (nothing
//! was drawn, so no rectangle is invented - D-08a) and `chars` null (it drew none of its
//! characters, so it claims none, and the ranges of a paragraph's rows still partition what was
//! drawn). A partially clipped paragraph publishes the rows of the lines it drew, as before.
//!
//! Pinned by `a_bookmark_with_no_anchor_character_produces_no_row`,
//! `an_empty_paragraph_produces_no_row`,
//! `a_borderless_table_with_empty_cells_keeps_every_row`,
//! `a_fully_clipped_cell_paragraph_keeps_one_empty_row` and
//! `a_partially_clipped_cell_paragraph_publishes_the_lines_it_drew`, and backstopped end to end
//! by the cross-artifact join-key test, which is what found the empty-paragraph half.
//!
//! **Page furniture.** Page borders, column dividers, headers, footers, page numbers and note
//! blocks carry no segment and produce no row. Borders and dividers are prepended after spans
//! are resolved, so they cannot fall inside one. The rest are emitted at page finalization,
//! which can happen while a paragraph span is still open, so the recorder is told where content
//! ended ([`SegmentRecorder::content_end`]) and a span never reaches past that boundary. The
//! furniture-exclusion tests in this module pin both halves.
//!
//! # Row count bound
//!
//! Recording is opt-in ([`layout_document_with_segments`](crate::layout::layout_document_with_segments)),
//! so an ordinary render pays nothing. When it is on, the span count is capped at
//! [`MAX_SEGMENT_ROWS`] and [`SegmentMap::truncated`] says so, because an untrusted document
//! drives the segment count (T-05-04-01). The upstream `LayoutBudget` still caps the item count
//! a single span can union.

use crate::display::{Item, PageList};
use crate::item_bounds;
use crate::segment_id::{self, SegmentPath};
use hwp_model::Control;
use hwp_model::control::{Cell, GenericControl, Table};
use hwp_model::paragraph::{HwpChar, Paragraph};

/// Upper bound on recorded spans, and so on rows, for one document.
pub const MAX_SEGMENT_ROWS: usize = 100_000;

/// `kind` values, matching the segment envelope's own kind strings.
pub mod kind {
    pub const PARA: &str = "para";
    pub const TABLE: &str = "table";
    pub const CELL: &str = "cell";
    pub const BOOKMARK: &str = "bookmark";
}

/// A box in points, page origin at top-left, y downward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxPt {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// A half-open character range in **UTF-16 code units into the source paragraph**.
///
/// This is `ShapedRun::start_wchar`'s space, which line wrapping maintains through
/// `shape::slice_with_sources`. It is **not** the segment envelope's space: `hwp cat
/// --segments` publishes Unicode scalar offsets into the *markdown output*. The two disagree on
/// any document with a non-BMP character, a table, or an image reference, and nothing converts
/// between them — the **segment id is the only join key** between the two artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharRange {
    pub start: u32,
    pub end: u32,
}

/// One segment's contribution to one page.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRow {
    /// Zero-based index into `DisplayList::pages`.
    pub page: usize,
    /// One of [`kind`]'s constants.
    pub kind: &'static str,
    /// The segment id, derived by [`crate::segment_id`] — the join key to the envelope.
    pub id: String,
    /// The union of this segment's own display items on this page, in points. `None` for a
    /// segment that produced no display item (D-08a).
    pub bbox: Option<BoxPt>,
    /// The source characters this row covers, or `None`.
    ///
    /// Two different things produce a `None`. Always on `table` and `cell` rows: they span
    /// several source paragraphs, so no single paragraph's offsets describe them. Also on a
    /// `para` row whose paragraph shapes no text of its own because its content is entirely an
    /// anchored object - the paragraph carrying a table, or one holding only a drawing, text
    /// box included: a text box's glyphs belong to the rows of its own paragraphs. That second
    /// case is not rare: on `fixtures/samples/report-tables.hwpx`, 11 of the 219 published
    /// `para` ids carry `None` here - the ten table-anchoring paragraphs, six of them inside
    /// cells and one inside a text box, and the paragraph holding that text box. A consumer
    /// must check this on every kind, `para` included.
    pub chars: Option<CharRange>,
    /// How many display items this row's span covers on this page, nested child segments
    /// included. Diagnostic: it is a count, never an index — the indices themselves are
    /// resolved away before page furniture is prepended (see [`SegmentRecorder`]).
    pub item_count: usize,
}

/// The row set for one document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentMap {
    pub rows: Vec<SegmentRow>,
    /// The document exceeded [`MAX_SEGMENT_ROWS`] spans and recording stopped early.
    pub truncated: bool,
}

impl SegmentMap {
    /// The rows of one page, in recording order.
    pub fn page(&self, page: usize) -> impl Iterator<Item = &SegmentRow> {
        self.rows.iter().filter(move |row| row.page == page)
    }
}

/// One span still being filled.
struct OpenSpan {
    uid: usize,
    kind: &'static str,
    id: String,
    path: SegmentPath,
    start_page: usize,
    start_item: usize,
    /// Where source text begins on `start_page`: items before it are a list marker, which is
    /// drawn but is no character of the paragraph. `start_item` unless told otherwise.
    chars_from: usize,
    parent: Option<usize>,
    /// The span cap was already reached when this span opened, so closing it records nothing.
    /// It still occupies the stack, because `end_segment` pops by position.
    dropped: bool,
}

/// One closed span: an item-index range, possibly crossing pages.
struct Span {
    kind: &'static str,
    id: String,
    start_page: usize,
    start_item: usize,
    /// See [`OpenSpan::chars_from`].
    chars_from: usize,
    end_page: usize,
    end_item: usize,
    chars: Option<CharRange>,
    /// The `uid` of the enclosing span, if any. A parent closes *after* its children, so this
    /// cannot be a span index at recording time; it is resolved through a uid map instead.
    uid: usize,
    parent: Option<usize>,
}

/// Records which source segment produced which display items, as index spans around the
/// layout loops.
///
/// # The prepend hazard
///
/// `prepend_page_borders` and `prepend_col_dividers` splice items onto the **front** of
/// `page.items` after a section's pages are pushed, which shifts every item index recorded
/// against that page. So no index recorded here ever leaves this module: [`Self::resolve`] is
/// called at the end of each section, immediately **before** the prepends run, and turns the
/// spans into boxes and character ranges. A row carries a count, never an index. Do not
/// reintroduce an index-carrying row — the failure is silent and produces plausible-looking
/// boxes that are all wrong.
///
/// Provenance is recorded this way, and not as a field on `display::Item`, because `Item` has
/// 79 construction sites in `layout.rs`, plus an exhaustive match in every backend and in
/// `translate_item`. None of them change.
pub(crate) struct SegmentRecorder {
    enabled: bool,
    /// Ordinal of the page currently being filled: the index it will occupy in
    /// `DisplayList::pages`. Bumped by `push_page_checked`, the one place pages are pushed.
    page: usize,
    /// The path of the paragraph currently being laid out.
    para_path: SegmentPath,
    open: Vec<OpenSpan>,
    spans: Vec<Span>,
    /// Monotonic span identity, so a child can name a parent that has not closed yet.
    next_uid: usize,
    /// Per page, the item count at which *content* ended and page furniture began. A span that
    /// is still open when a page is finalized would otherwise run to `page.items.len()` and
    /// swallow the note block, the note separator and the page number - inflating its box to
    /// the page bottom and polluting its character range with `start_wchar` values from a
    /// different source paragraph.
    content_end: std::collections::HashMap<usize, usize>,
    /// Ids of paragraphs the envelope always has a segment for (see [`always_has_para_segment`]),
    /// which `finish` keeps one row for even when nothing was measured.
    segmented: std::collections::HashSet<String>,
    map: SegmentMap,
}

impl SegmentRecorder {
    /// A recorder that records. Used by
    /// [`layout_document_with_segments`](crate::layout::layout_document_with_segments).
    pub(crate) fn new() -> Self {
        Self {
            enabled: true,
            ..Self::disabled()
        }
    }

    /// A no-op recorder. Every ordinary render uses one, and so does every layout pass whose
    /// paragraphs this module does not record - a scratch or measurement page, page furniture,
    /// a caption (see `layout_box_para_iter`'s callers) - because recording there would
    /// attribute geometry to a segment that has no row by contract.
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            page: 0,
            para_path: SegmentPath::default(),
            open: Vec::new(),
            spans: Vec::new(),
            next_uid: 0,
            content_end: std::collections::HashMap::new(),
            segmented: std::collections::HashSet::new(),
            map: SegmentMap::default(),
        }
    }

    /// Content on the page being filled has ended and page furniture is about to be emitted:
    /// note blocks, the note separator, the page number. Call this immediately **before** that
    /// furniture, at every page-finalization site. The first call for a page wins, so a
    /// finalization sequence that emits furniture in several steps still records the boundary
    /// before the first of them.
    pub(crate) fn content_end(&mut self, page: &PageList) {
        if self.enabled {
            self.content_end
                .entry(self.page)
                .or_insert(page.items.len());
        }
    }

    /// A page was pushed; the page being filled is the next one.
    pub(crate) fn page_pushed(&mut self) {
        if self.enabled {
            self.page += 1;
        }
    }

    /// `resolve` drains `spans` at the end of every section, so counting spans alone would
    /// reset the cap per section while `rows` — which can exceed the span count, one per page a
    /// span touches — grew without a bound.
    fn full(&self) -> bool {
        self.map.rows.len() + self.spans.len() + self.open.len() >= MAX_SEGMENT_ROWS
    }

    fn begin(&mut self, kind: &'static str, id: String, path: SegmentPath, page: &PageList) {
        if !self.enabled {
            return;
        }
        let dropped = self.full();
        self.map.truncated |= dropped;
        let parent = self.open.last().map(|open| open.uid);
        let uid = self.next_uid;
        self.next_uid += 1;
        self.open.push(OpenSpan {
            uid,
            kind,
            id,
            path,
            start_page: self.page,
            start_item: page.items.len(),
            chars_from: page.items.len(),
            parent,
            dropped,
        });
    }

    /// Opens a body paragraph's span and makes its path the base for nested segments.
    pub(crate) fn begin_paragraph(
        &mut self,
        section: usize,
        para_index: usize,
        para: &Paragraph,
        page: &PageList,
    ) {
        if !self.enabled {
            return;
        }
        self.para_path = SegmentPath {
            section,
            indices: vec![para_index],
        };
        self.begin_para(self.para_path.clone(), para, page);
    }

    /// Opens the span of a paragraph inside the open cell: `para_index` indexes
    /// `Cell::paragraphs`, so the path is the envelope's own `child(cell_path, para_index)`.
    ///
    /// Unlike [`Self::begin_paragraph`] it leaves `para_path` alone: that is the body
    /// paragraph's path, and [`Self::bookmark`] resolves against the innermost open paragraph
    /// span instead, so a bookmark in a cell keeps its cell-relative path.
    pub(crate) fn begin_cell_paragraph(
        &mut self,
        para_index: usize,
        para: &Paragraph,
        page: &PageList,
    ) {
        if !self.enabled {
            return;
        }
        self.begin_para(child(self.base_path(), para_index), para, page);
    }

    /// Opens the span of a paragraph of a drawing object's text: the `seq`-th paragraph of the
    /// object at `control_index` of the innermost open paragraph, counted across ALL of the
    /// object's lists, so the path is the envelope's own `[..., para, control, seq]`. A caller
    /// that lays the text out in pieces - per column, per list - passes each piece's offset.
    pub(crate) fn begin_drawing_paragraph(
        &mut self,
        control_index: usize,
        seq: usize,
        para: &Paragraph,
        page: &PageList,
    ) {
        if !self.enabled {
            return;
        }
        let path = child(&child(self.base_path(), control_index), seq);
        self.begin_para(path, para, page);
    }

    /// Records the text of a drawing object that layout skips entirely - a `gso ` whose
    /// geometry header is too short to place, or an object kind no arm draws - so every
    /// envelope id under it still has a row. Nothing is drawn, so every row is unmeasured and
    /// `finish` keeps it exactly when the envelope has a segment for it. Headers, footers,
    /// notes and hidden comments are skipped, as the default envelope skips them.
    pub(crate) fn unlaid(
        &mut self,
        control_index: usize,
        control: &GenericControl,
        page: &PageList,
    ) {
        if !self.enabled
            || matches!(
                &control.ctrl_id,
                b"head" | b"foot" | b"fn  " | b"en  " | b"tcmt"
            )
        {
            return;
        }
        let paragraphs = control
            .paragraph_lists
            .iter()
            .flat_map(|list| &list.paragraphs);
        for (seq, para) in paragraphs.enumerate() {
            self.begin_drawing_paragraph(control_index, seq, para, page);
            self.unlaid_objects(para, page);
            self.end_segment(page);
        }
    }

    /// The objects of a paragraph [`Self::unlaid`] records: its bookmarks, its tables with
    /// their cells and cell paragraphs, and the text of drawing objects nested in it.
    fn unlaid_objects(&mut self, para: &Paragraph, page: &PageList) {
        for (control_index, control) in para.controls.iter().enumerate() {
            match control {
                Control::Generic(g) if g.ctrl_id == *b"bokm" => {
                    self.bookmark(control_index, g, para)
                }
                Control::Generic(g) => self.unlaid(control_index, g, page),
                Control::Table(table) => {
                    self.begin_table(control_index, table, page);
                    for (cell_index, cell) in table.cells.iter().enumerate() {
                        self.begin_cell(cell_index, cell, page);
                        for (para_index, cell_para) in cell.paragraphs.iter().enumerate() {
                            self.begin_cell_paragraph(para_index, cell_para, page);
                            self.unlaid_objects(cell_para, page);
                            self.end_segment(page);
                        }
                        self.end_segment(page);
                    }
                    self.end_segment(page);
                }
                _ => {}
            }
        }
    }

    fn begin_para(&mut self, path: SegmentPath, para: &Paragraph, page: &PageList) {
        let id = segment_id::paragraph_id(&path, para);
        if always_has_para_segment(para) {
            self.segmented.insert(id.clone());
        }
        self.begin(kind::PARA, id, path, page);
    }

    /// Opens a table's span. `control_index` indexes the paragraph's `controls`, which is what
    /// the envelope's path component for a control segment is.
    pub(crate) fn begin_table(&mut self, control_index: usize, table: &Table, page: &PageList) {
        if !self.enabled {
            return;
        }
        let path = child(self.base_path(), control_index);
        let id = segment_id::table_id(&path, table);
        self.begin(kind::TABLE, id, path, page);
    }

    /// Opens a cell's span. `cell_index` indexes `Table::cells`.
    pub(crate) fn begin_cell(&mut self, cell_index: usize, cell: &Cell, page: &PageList) {
        if !self.enabled {
            return;
        }
        let path = child(self.base_path(), cell_index);
        let id = segment_id::cell_id(&path, cell);
        self.begin(kind::CELL, id, path, page);
    }

    /// Records a bookmark: a segment that is invisible by design and so has no box.
    pub(crate) fn bookmark(
        &mut self,
        control_index: usize,
        control: &GenericControl,
        para: &Paragraph,
    ) {
        if !self.enabled {
            return;
        }
        if self.full() {
            self.map.truncated = true;
            return;
        }
        // The envelope opens a bookmark segment only from a `BOOKMARK` control character that
        // references this control (`crates/hwp-convert/src/markdown.rs`). An unanchored `bokm`
        // produces no envelope segment, so a row for it would carry an id that joins to
        // nothing.
        let Some(at) = bookmark_anchor_offset(para, control_index) else {
            return;
        };
        // `para` is the innermost open paragraph: a cell's paragraph when the bookmark is in a
        // cell, the body paragraph otherwise.
        let para_path = self
            .open
            .iter()
            .rev()
            .find(|open| open.kind == kind::PARA)
            .map_or(&self.para_path, |open| &open.path);
        let path = child(para_path, control_index);
        let uid = self.next_uid;
        self.next_uid += 1;
        self.spans.push(Span {
            kind: kind::BOOKMARK,
            id: segment_id::control_id(&path, control),
            start_page: self.page,
            start_item: 0,
            chars_from: 0,
            end_page: self.page,
            end_item: 0,
            chars: Some(CharRange { start: at, end: at }),
            uid,
            parent: None,
        });
    }

    /// `count` items were *inserted* at index `at` on the page being filled, rather than
    /// appended: a paragraph background rectangle goes behind its own text.
    ///
    /// **The inserted item belongs to the span that is open**, which is why the boundary
    /// comparisons are not uniform. `draw_para_bg_slice` inserts at the index captured when the
    /// paragraph opened, and that one index is simultaneously the open paragraph's `start_item`
    /// and the *previous* paragraph's exclusive `end_item`. Shifting both on equality would
    /// push the fill out of the paragraph that owns it **and** pull it into the paragraph
    /// before, so a published box would run down into the next paragraph while the fill itself
    /// belonged to no row at all. So:
    ///
    /// - an open span's `start_item` shifts only when it is strictly after `at` (equality means
    ///   the insert is this span's own first item);
    /// - a closed span's `end_item` shifts only when it is strictly after `at` (equality means
    ///   the insert lands after that span ended);
    /// - a closed span's `start_item` shifts on equality, because an insert at its first index
    ///   pushes the whole span right.
    pub(crate) fn items_inserted(&mut self, at: usize, count: usize) {
        if !self.enabled || count == 0 {
            return;
        }
        let page = self.page;
        // `chars_from` is a start-side index and shifts under the same comparison as
        // `start_item`.
        for open in &mut self.open {
            if open.start_page == page && open.start_item > at {
                open.start_item += count;
            }
            if open.start_page == page && open.chars_from > at {
                open.chars_from += count;
            }
        }
        for span in &mut self.spans {
            if span.start_page == page && span.start_item >= at {
                span.start_item += count;
            }
            if span.start_page == page && span.chars_from >= at {
                span.chars_from += count;
            }
            if span.end_page == page && span.end_item > at {
                span.end_item += count;
            }
        }
    }

    /// A list marker was just drawn at the head of the innermost open span: its glyphs are
    /// synthesized, `start_wchar` 0, and name no character of the paragraph, so the span's
    /// source range starts after them. Its box still includes the marker.
    pub(crate) fn marker_drawn(&mut self, page: &PageList) {
        if let Some(open) = self.open.last_mut().filter(|_| self.enabled) {
            open.chars_from = page.items.len();
        }
    }

    /// Closes the innermost open span.
    pub(crate) fn end_segment(&mut self, page: &PageList) {
        if !self.enabled {
            return;
        }
        let Some(open) = self.open.pop() else {
            return;
        };
        if open.dropped {
            return;
        }
        self.spans.push(Span {
            kind: open.kind,
            id: open.id,
            start_page: open.start_page,
            start_item: open.start_item,
            chars_from: open.chars_from,
            end_page: self.page,
            end_item: page.items.len(),
            chars: None,
            uid: open.uid,
            parent: open.parent,
        });
    }

    fn base_path(&self) -> &SegmentPath {
        self.open.last().map_or(&self.para_path, |open| &open.path)
    }

    /// Turns every span recorded so far into rows. Must run **before** page furniture is
    /// prepended to any page a span touches; see the type's own doc comment.
    pub(crate) fn resolve(&mut self, pages: &[PageList]) {
        if !self.enabled {
            return;
        }
        if !self.open.is_empty() {
            // Only reachable on an abort: the layout pass returned early on an exhausted
            // budget with spans still open. Their extent is unknown, so they are dropped and
            // the map says the row set is incomplete rather than silently missing rows.
            self.open.clear();
            self.map.truncated = true;
        }
        let spans = std::mem::take(&mut self.spans);
        // Direct children, so a paragraph's own character range excludes the glyphs of a table
        // laid out inside it: those belong to cell paragraphs and index a different string.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); spans.len()];
        let by_uid: std::collections::HashMap<usize, usize> = spans
            .iter()
            .enumerate()
            .map(|(index, span)| (span.uid, index))
            .collect();
        for (index, span) in spans.iter().enumerate() {
            if let Some(parent) = span.parent.and_then(|uid| by_uid.get(&uid)) {
                children[*parent].push(index);
            }
        }
        for (index, span) in spans.iter().enumerate() {
            let mut emitted = false;
            for page_index in span.start_page..=span.end_page {
                let Some(page) = pages.get(page_index) else {
                    continue;
                };
                let lo = if page_index == span.start_page {
                    span.start_item
                } else {
                    0
                };
                // Page furniture is emitted after content, so a span never reaches past the
                // recorded content boundary even when it is still open at finalization.
                let content_end = self
                    .content_end
                    .get(&page_index)
                    .copied()
                    .unwrap_or(page.items.len());
                let hi = if page_index == span.end_page {
                    span.end_item
                } else {
                    content_end
                }
                .min(content_end)
                .min(page.items.len());
                if hi <= lo {
                    continue;
                }
                let source_lo = if page_index == span.start_page {
                    lo.max(span.chars_from)
                } else {
                    lo
                };
                let own = own_indices(&spans, &children[index], page_index, source_lo, hi);
                self.map.rows.push(SegmentRow {
                    page: page_index,
                    kind: span.kind,
                    id: span.id.clone(),
                    bbox: union_bounds(&page.items[lo..hi]),
                    chars: (span.kind == kind::PARA)
                        .then(|| char_range(page, &own))
                        .flatten(),
                    item_count: hi - lo,
                });
                emitted = true;
            }
            if !emitted {
                // No display item anywhere: a point segment (D-08a). No box is invented.
                self.map.rows.push(SegmentRow {
                    page: span.start_page,
                    kind: span.kind,
                    id: span.id.clone(),
                    bbox: None,
                    chars: span.chars,
                    item_count: 0,
                });
            }
        }
    }

    /// The rows recorded so far, coalesced.
    ///
    /// Coalescing happens here rather than at recording time because one segment can be
    /// *drawn* more than once on one page: a cell whose content is split into two fragments
    /// that both fit on the same page emits two spans (observed on
    /// `fixtures/samples/report-tables.hwpx`). Those describe one place on one page, so their
    /// boxes, ranges and counts are merged.
    ///
    /// Only fragments that **touch** are merged. Two fragments of one cell in two different
    /// columns of a multi-column section are separated by the gutter, and unioning them would
    /// publish a box spanning both columns and the space between - a rectangle covering ground
    /// the cell does not occupy. Those stay separate rows, so a (page, id) pair is *usually*
    /// one row but is not guaranteed to be.
    ///
    /// Rows on **different** pages always stay separate - that is D-09 - so a segment id
    /// appears more than once in the set either way. Do not key a map by id alone.
    pub(crate) fn finish(mut self) -> SegmentMap {
        let rows = std::mem::take(&mut self.map.rows);
        let mut at: std::collections::HashMap<(usize, String), usize> =
            std::collections::HashMap::new();
        for row in rows {
            let mergeable = at
                .get(&(row.page, row.id.clone()))
                .copied()
                .filter(|&index| {
                    match (self.map.rows[index].bbox, row.bbox) {
                        (Some(a), Some(b)) => touches(a, b),
                        // A box-less row carries no geometry to contradict, so it merges.
                        _ => true,
                    }
                });
            match mergeable {
                Some(index) => {
                    let existing: &mut SegmentRow = &mut self.map.rows[index];
                    existing.bbox = match (existing.bbox, row.bbox) {
                        (Some(a), Some(b)) => Some(BoxPt {
                            x0: a.x0.min(b.x0),
                            y0: a.y0.min(b.y0),
                            x1: a.x1.max(b.x1),
                            y1: a.y1.max(b.y1),
                        }),
                        (a, b) => a.or(b),
                    };
                    existing.chars = match (existing.chars, row.chars) {
                        (Some(a), Some(b)) => Some(CharRange {
                            start: a.start.min(b.start),
                            end: a.end.max(b.end),
                        }),
                        (a, b) => a.or(b),
                    };
                    existing.item_count += row.item_count;
                }
                None => {
                    // A non-touching fragment replaces the merge target, so a run of adjacent
                    // fragments still collapses pairwise down the page.
                    at.insert((row.page, row.id.clone()), self.map.rows.len());
                    self.map.rows.push(row);
                }
            }
        }
        // "Segments that produce nothing at all" in the module doc: one rule covering both the
        // unanchored bookmark and the empty paragraph, applied here rather than case by case.
        //
        // Judged only on the evidence the kind can carry. `table` and `cell` are exempt because
        // their `chars` is `None` BY DESIGN - they span several source paragraphs, so no single
        // paragraph's offsets describe them - which means the absence of characters is not
        // evidence of an empty row for them, and the test would collapse to `bbox.is_some()`.
        // A borderless table whose cells are empty produces no display item, and dropping it
        // would delete an ordinary HWP construct from the published geometry. Both kinds also
        // always have an envelope segment, so their ids never join to nothing.
        //
        // A paragraph the envelope always has a segment for (any text, or an object) is exempt
        // the same way, once: dropping its only row when it drew nothing - its object drew
        // nothing, or every line of it was clipped at a cell's edge - would leave an envelope id
        // with no geometry. It keeps ONE unmeasured row, and only when it has no measured one,
        // so a cell fragment that drew none of it on a continuation page, or a partially
        // clipped paragraph, publishes no extra empty row.
        let measured = |row: &SegmentRow| {
            matches!(row.kind, kind::TABLE | kind::CELL)
                || row.bbox.is_some()
                || row.chars.is_some()
        };
        let segmented_measured: std::collections::HashSet<String> = self
            .map
            .rows
            .iter()
            .filter(|row| self.segmented.contains(&row.id) && measured(row))
            .map(|row| row.id.clone())
            .collect();
        let mut kept = std::collections::HashSet::new();
        self.map.rows.retain(|row| {
            measured(row)
                || (self.segmented.contains(&row.id)
                    && !segmented_measured.contains(&row.id)
                    && kept.insert(row.id.clone()))
        });
        self.map
    }
}

/// The item indices in `lo..hi` on `page_index` that belong to the span itself rather than to
/// one of its direct children.
fn own_indices(
    spans: &[Span],
    children: &[usize],
    page_index: usize,
    lo: usize,
    hi: usize,
) -> Vec<usize> {
    let mut own: Vec<usize> = (lo..hi).collect();
    for &child_index in children {
        let child = &spans[child_index];
        if page_index < child.start_page || page_index > child.end_page {
            continue;
        }
        let child_lo = if page_index == child.start_page {
            child.start_item
        } else {
            0
        };
        let child_hi = if page_index == child.end_page {
            child.end_item
        } else {
            usize::MAX
        };
        own.retain(|index| *index < child_lo || *index >= child_hi);
    }
    own
}

/// How far apart two fragment boxes may sit and still be one place. `item_bounds` inflates a
/// stroked path by half its width, so consecutive fragments of one cell overlap slightly rather
/// than meeting exactly; a gutter between two columns is an order of magnitude wider.
const FRAGMENT_TOUCH_PT: f32 = 0.5;

/// Whether two fragment boxes overlap or meet on both axes.
fn touches(a: BoxPt, b: BoxPt) -> bool {
    a.x0 <= b.x1 + FRAGMENT_TOUCH_PT
        && b.x0 <= a.x1 + FRAGMENT_TOUCH_PT
        && a.y0 <= b.y1 + FRAGMENT_TOUCH_PT
        && b.y0 <= a.y1 + FRAGMENT_TOUCH_PT
}

/// The union of every item's box, or `None` when no item has one.
fn union_bounds(items: &[Item]) -> Option<BoxPt> {
    items.iter().filter_map(item_bounds).fold(None, |acc, b| {
        Some(match acc {
            None => BoxPt {
                x0: b.0,
                y0: b.1,
                x1: b.2,
                y1: b.3,
            },
            Some(a) => BoxPt {
                x0: a.x0.min(b.0),
                y0: a.y0.min(b.1),
                x1: a.x1.max(b.2),
                y1: a.y1.max(b.3),
            },
        })
    })
}

/// The UTF-16 span of the source paragraph the row's own glyph runs cover.
fn char_range(page: &PageList, own: &[usize]) -> Option<CharRange> {
    let mut range: Option<CharRange> = None;
    for &index in own {
        let Some(Item::Glyphs { run, .. }) = page.items.get(index) else {
            continue;
        };
        let start = run.start_wchar;
        let end = start + run.text.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        range = Some(match range {
            None => CharRange { start, end },
            Some(current) => CharRange {
                start: current.start.min(start),
                end: current.end.max(end),
            },
        });
    }
    range
}

/// The child path one index below `path` — the same construction the envelope uses.
fn child(path: &SegmentPath, index: usize) -> SegmentPath {
    let mut indices = path.indices.clone();
    indices.push(index);
    SegmentPath {
        section: path.section,
        indices,
    }
}

/// Whether the envelope always has a `para` segment for `para`, wherever this module records
/// it: any text (a range, or a point when it is only whitespace), or an object the envelope
/// addresses without text - a table, a picture, an equation (each always emits markup) or a
/// drawing object, HWP5 `gso ` or an HWPX shape or container (a point, #285).
///
/// `hwp-convert`'s `segment::always_has_para_segment` is a deliberate second copy of this rule
/// (the two crates may not depend on each other); `crates/hwp-cli/tests/segment_id_parity.rs`
/// pins them together.
pub fn always_has_para_segment(para: &Paragraph) -> bool {
    para.chars.iter().any(|ch| matches!(ch, HwpChar::Text(_)))
        || para.controls.iter().any(|control| match control {
            Control::Table(_) | Control::Picture(_) => true,
            Control::Generic(g) => {
                g.ctrl_id == *b"gso "
                    || g.container_box.is_some()
                    || !g.gso_shapes.is_empty()
                    || g.equation.is_some()
            }
            _ => false,
        })
}

/// The WCHAR offset of the `BOOKMARK` control character that references `control_index`, or
/// `None` when the paragraph carries no such anchor.
fn bookmark_anchor_offset(para: &Paragraph, control_index: usize) -> Option<u32> {
    let mut offset = 0u32;
    for ch in &para.chars {
        if matches!(
            ch,
            HwpChar::ExtCtrl { code, ctrl_index: Some(index), .. }
                if *code == hwp_model::paragraph::ctrl_char::BOOKMARK
                    && *index as usize == control_index
        ) {
            return Some(offset);
        }
        offset = offset.saturating_add(ch.wchar_width());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::DisplayList;
    use crate::fonts::FontStore;
    use crate::issues::RenderIssueAccumulator;
    use hwp_model::{BorderFill, BorderLine, Control, Document, GenericControl};

    /// Every assertion in this module is derived from the display list the run produced, never
    /// from a coordinate, a glyph or a page count, because CI bundles no fonts.
    fn lay_out(doc: &Document) -> (DisplayList, SegmentMap) {
        let mut store = FontStore::new();
        let mut warnings = RenderIssueAccumulator::new();
        crate::layout::layout_document_with_segments(doc, &mut store, &mut warnings)
    }

    /// A document with a **page border**, which is the only way the prepend hazard surfaces:
    /// `prepend_page_borders` splices items onto the front of every page of the section after
    /// layout. Built in-test rather than loaded from `fixtures/samples/`, whose border and
    /// divider content no plan has established — a fixture-based version would go red on a
    /// missing premise and read as a code defect.
    fn bordered(markdown: &str) -> Document {
        let mut doc = hwp_convert::from_markdown(markdown);
        doc.header.border_fills.push(BorderFill {
            attr: 0,
            sides: [BorderLine {
                line_type: 1,
                width: 2,
                color: 0,
            }; 4],
            diagonal: BorderLine::default(),
            fill_type: 0,
            bg_color: None,
            hatch: None,
            gradient: None,
            tail: Vec::new(),
        });
        let id = doc.header.border_fills.len() as u16;
        // PAGE_BORDER_FILL: attr(u32, bit0 = paper-relative), four u16 gaps, then the id.
        let mut raw = 1u32.to_le_bytes().to_vec();
        for _ in 0..4 {
            raw.extend_from_slice(&1_000u16.to_le_bytes());
        }
        raw.extend_from_slice(&id.to_le_bytes());
        for para in &mut doc.sections[0].paragraphs {
            for control in &mut para.controls {
                if let Control::SectionDef(sd) = control {
                    sd.page_border_fills_raw = vec![raw.clone()];
                }
            }
        }
        doc
    }

    /// The paragraph-background colour the fixture below uses, distinctive enough to pick its
    /// own rectangles back out of the display list.
    const FILL_COLOR: u32 = 0x00EE_EEEE;

    fn bookmark_control() -> GenericControl {
        GenericControl {
            ctrl_id: *b"bokm",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }
    }

    fn contains(outer: BoxPt, inner: BoxPt) -> bool {
        outer.x0 <= inner.x0 + 0.01
            && outer.y0 <= inner.y0 + 0.01
            && outer.x1 + 0.01 >= inner.x1
            && outer.y1 + 0.01 >= inner.y1
    }

    /// Two boxes overlap by more than a shared border. Adjacent cells share an edge and
    /// `item_bounds` inflates a stroked path by half its width, so their boxes touch by a
    /// fraction of a point by construction; only a real overlap is a misattribution.
    fn overlaps(a: BoxPt, b: BoxPt) -> bool {
        const SHARED_EDGE_PT: f32 = 1.0;
        a.x0 < b.x1 - SHARED_EDGE_PT
            && b.x0 < a.x1 - SHARED_EDGE_PT
            && a.y0 < b.y1 - SHARED_EDGE_PT
            && b.y0 < a.y1 - SHARED_EDGE_PT
    }

    fn rows_of(map: &SegmentMap, kind: &str) -> Vec<SegmentRow> {
        map.rows
            .iter()
            .filter(|row| row.kind == kind)
            .cloned()
            .collect()
    }

    /// A document whose paragraphs carry a **filled** `ParaShape.border_fill_id`, so
    /// `draw_para_bg_slice` actually inserts a rectangle into the middle of `page.items`. No
    /// other test in the suite sets `border_fill_id`, which is why the insert-ownership defect
    /// this fixture exists for went unnoticed.
    fn with_paragraph_backgrounds(markdown: &str) -> Document {
        let mut doc = hwp_convert::from_markdown(markdown);
        doc.header.border_fills.push(BorderFill {
            attr: 0,
            sides: [BorderLine::default(); 4],
            diagonal: BorderLine::default(),
            fill_type: 1,
            bg_color: Some(FILL_COLOR),
            hatch: None,
            gradient: None,
            tail: Vec::new(),
        });
        let id = doc.header.border_fills.len() as u16;
        for shape in &mut doc.header.para_shapes {
            shape.border_fill_id = id;
        }
        doc
    }

    /// A paragraph forced across a page break **mid-paragraph** by its own cached line
    /// geometry. `LineSeg::flags` bit 0 marks a page-first line, so the second line opens a new
    /// page; that comes from the model, not from shaping, so the split happens whatever fonts
    /// the host has. This is the only way to reach `layout.rs`'s mid-paragraph break band,
    /// where the third `draw_para_bg_slice` call site and one page-finalization site live.
    fn split_mid_paragraph(background: bool) -> Document {
        let mut doc = if background {
            with_paragraph_backgrounds("문단 하나.\n")
        } else {
            hwp_convert::from_markdown("문단 하나.\n")
        };
        let para = &mut doc.sections[0].paragraphs[0];
        para.chars
            .extend("가나다라마바사아자차카타파하".chars().map(HwpChar::Text));
        let text_len = para.wchar_len();
        let seg = |text_start: u32, flags: u32| hwp_model::paragraph::LineSeg {
            text_start,
            v_pos: 0,
            line_height: 1_600,
            text_height: 1_600,
            baseline_gap: 1_300,
            line_spacing: 0,
            col_start: 0,
            seg_width: 40_000,
            flags,
        };
        // Both lines are flagged page-first; the second one is therefore a hard page break
        // reached in the middle of the paragraph.
        para.line_segs = vec![seg(0, 0x1), seg(text_len / 2, 0x1)];
        doc
    }

    /// The insert-ownership test. `draw_para_bg_slice` inserts a paragraph's background at the
    /// index captured when the paragraph opened, and that one index is both the open
    /// paragraph's first item and the previous paragraph's exclusive end. Treating the insert
    /// as belonging to both pushed each fill out of its own paragraph and into the one above,
    /// so a published box ran down into the next paragraph and the fill belonged to no row.
    #[test]
    fn a_paragraph_background_belongs_to_the_paragraph_it_fills() {
        let (list, map) = lay_out(&with_paragraph_backgrounds(
            "첫 문단입니다.\n\n둘째 문단입니다.\n\n셋째 문단입니다.\n",
        ));
        // The fixture really does insert: a filled background is one extra item per paragraph.
        let plain = lay_out(&hwp_convert::from_markdown(
            "첫 문단입니다.\n\n둘째 문단입니다.\n\n셋째 문단입니다.\n",
        ));
        assert!(
            list.pages[0].items.len() > plain.0.pages[0].items.len(),
            "the fixture must add background items, or this test proves nothing"
        );

        let paras: Vec<SegmentRow> = map
            .page(0)
            .filter(|row| row.kind == kind::PARA)
            .cloned()
            .collect();
        assert!(paras.len() > 1, "more than one paragraph on the page");
        // Consecutive paragraphs occupy disjoint bands. With the insert credited to both
        // neighbours, each box grew down by the height of the next paragraph's fill.
        for (i, a) in paras.iter().enumerate() {
            for b in &paras[i + 1..] {
                let (Some(x), Some(y)) = (a.bbox, b.bbox) else {
                    continue;
                };
                assert!(
                    !overlaps(x, y),
                    "paragraph boxes must not overlap: {x:?} {y:?}"
                );
            }
        }
        // And the fill is inside some row rather than orphaned: every paragraph row covers
        // strictly more items than the same paragraph covers without a background.
        let plain_covered: usize = plain
            .1
            .page(0)
            .filter(|row| row.kind == kind::PARA)
            .map(|row| row.item_count)
            .sum();
        let covered: usize = paras.iter().map(|row| row.item_count).sum();
        assert_eq!(
            covered - plain_covered,
            paras.len(),
            "each paragraph must own exactly its own background item"
        );

        // The sharp assertion: the n-th paragraph's box contains the n-th fill and nothing of
        // the next paragraph's. `bg_fill_item` emits the paragraph background as an
        // `Item::Rect` in the fixture's fill colour, one per paragraph, in document order.
        let mut fills: Vec<BoxPt> = list.pages[0]
            .items
            .iter()
            .filter(|item| matches!(item, Item::Rect { fill, .. } if *fill == FILL_COLOR))
            .filter_map(item_bounds)
            .map(|b| BoxPt {
                x0: b.0,
                y0: b.1,
                x1: b.2,
                y1: b.3,
            })
            .collect();
        fills.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        assert_eq!(fills.len(), paras.len(), "one background per paragraph");
        let mut boxes: Vec<BoxPt> = paras.iter().filter_map(|row| row.bbox).collect();
        boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        assert_eq!(boxes.len(), fills.len());
        for (i, (bbox, fill)) in boxes.iter().zip(&fills).enumerate() {
            assert!(
                contains(*bbox, *fill),
                "paragraph {i}'s box must contain its own fill: {bbox:?} vs {fill:?}"
            );
            if let Some(next) = fills.get(i + 1) {
                assert!(
                    bbox.y1 <= next.y0 + 0.01,
                    "paragraph {i}'s box must not reach the next paragraph's fill: \
                     {bbox:?} vs {next:?}"
                );
            }
        }
    }

    /// The prepend test. A recorded index that was not corrected for the page border spliced
    /// onto the front of `page.items` produces boxes that all look plausible and are all wrong;
    /// the only way it shows is a paragraph box that has swallowed the border.
    #[test]
    fn paragraph_boxes_exclude_the_page_border_prepended_after_layout() {
        let (list, map) = lay_out(&bordered("첫 문단입니다.\n\n둘째 문단입니다.\n"));
        let page = &list.pages[0];
        // The border was prepended, so item 0 is one of its edges; its union is the border box.
        let border = union_bounds(&page.items[..1]).expect("a prepended page-border item");
        let paras: Vec<SegmentRow> = map
            .page(0)
            .filter(|r| r.kind == kind::PARA)
            .cloned()
            .collect();
        assert!(!paras.is_empty(), "a body paragraph must produce a row");
        for row in &paras {
            let Some(bbox) = row.bbox else { continue };
            assert!(
                contains(border, bbox) && !contains(bbox, border),
                "a paragraph box must sit strictly inside the page border, \
                 not have swallowed it: {bbox:?} vs border {border:?}"
            );
        }
        // Every glyph on the page belongs to some paragraph row.
        let glyphs = page
            .items
            .iter()
            .filter(|item| matches!(item, Item::Glyphs { .. }))
            .filter_map(item_bounds)
            .fold(None, |acc: Option<BoxPt>, b| {
                Some(match acc {
                    None => BoxPt {
                        x0: b.0,
                        y0: b.1,
                        x1: b.2,
                        y1: b.3,
                    },
                    Some(a) => BoxPt {
                        x0: a.x0.min(b.0),
                        y0: a.y0.min(b.1),
                        x1: a.x1.max(b.2),
                        y1: a.y1.max(b.3),
                    },
                })
            });
        if let Some(glyphs) = glyphs {
            let union = paras
                .iter()
                .filter_map(|row| row.bbox)
                .reduce(|a, b| BoxPt {
                    x0: a.x0.min(b.x0),
                    y0: a.y0.min(b.y0),
                    x1: a.x1.max(b.x1),
                    y1: a.y1.max(b.y1),
                })
                .expect("a paragraph row with a box");
            assert!(contains(union, glyphs), "{union:?} must cover {glyphs:?}");
        }
    }

    /// Page furniture carries no segment, so it produces no row. Asserted as a cardinality:
    /// the paragraph rows of a page cover strictly fewer items than the page holds, and the
    /// difference is the border. Paragraph spans are disjoint and every other row nests inside
    /// one, so summing the paragraph rows double-counts nothing.
    #[test]
    fn page_furniture_produces_no_row() {
        let (list, map) = lay_out(&bordered("본문 한 줄.\n"));
        let covered: usize = map
            .page(0)
            .filter(|row| row.kind == kind::PARA)
            .map(|row| row.item_count)
            .sum();
        assert!(
            covered < list.pages[0].items.len(),
            "the page border must lie outside every row: {covered} covered of {}",
            list.pages[0].items.len()
        );
    }

    /// A table gets its own row alongside its cells, so an editor can hit-test either.
    #[test]
    fn a_table_row_contains_every_cell_row_on_the_same_page() {
        let (_, map) = lay_out(&hwp_convert::from_markdown(
            "| 머리 | 글 |\n|---|---|\n| 가 | 나 |\n| 다 | 라 |\n",
        ));
        let tables = rows_of(&map, kind::TABLE);
        let cells = rows_of(&map, kind::CELL);
        assert!(!tables.is_empty(), "a table must produce a table row");
        assert!(!cells.is_empty(), "a table must produce cell rows");
        for table in &tables {
            let Some(outer) = table.bbox else { continue };
            for cell in cells.iter().filter(|c| c.page == table.page) {
                let Some(inner) = cell.bbox else { continue };
                assert!(contains(outer, inner), "{outer:?} must contain {inner:?}");
            }
        }
    }

    /// Two cells of one page describe two places on it.
    #[test]
    fn cell_rows_on_one_page_do_not_overlap() {
        let (_, map) = lay_out(&hwp_convert::from_markdown(
            "| 머리 | 글 |\n|---|---|\n| 가 | 나 |\n| 다 | 라 |\n",
        ));
        let cells = rows_of(&map, kind::CELL);
        for (i, a) in cells.iter().enumerate() {
            for b in &cells[i + 1..] {
                if a.page != b.page || a.id == b.id {
                    continue;
                }
                let (Some(x), Some(y)) = (a.bbox, b.bbox) else {
                    continue;
                };
                assert!(!overlaps(x, y), "cells must not overlap: {x:?} {y:?}");
            }
        }
    }

    /// A table taller than any page splits, and each fragment's rows describe their own page.
    /// The split is forced by declared row heights, which come from the model and not from
    /// shaping, so it happens whatever fonts the host has.
    #[test]
    fn a_split_table_reports_rows_on_every_page_it_touches() {
        let mut md = String::from("| 가 | 나 |\n|---|---|\n");
        for i in 0..200 {
            md.push_str(&format!("| {i} | 값 |\n"));
        }
        let (list, map) = lay_out(&hwp_convert::from_markdown(&md));
        let cells = rows_of(&map, kind::CELL);
        let mut pages: Vec<usize> = cells.iter().map(|row| row.page).collect();
        pages.sort_unstable();
        pages.dedup();
        assert!(
            pages.len() > 1,
            "a table 200 rows tall must reach more than one page"
        );
        for row in &cells {
            let Some(bbox) = row.bbox else { continue };
            let page = &list.pages[row.page];
            assert!(
                !crate::outside_page_bounds(
                    (bbox.x0, bbox.y0, bbox.x1, bbox.y1),
                    page.width_pt,
                    page.height_pt
                ),
                "a cell row's box must lie on its own page: {bbox:?}"
            );
        }
    }

    /// Every row's box lies on its own page, checked with `diagnose_pages`'s own predicate.
    #[test]
    fn every_box_lies_inside_its_page() {
        let (list, map) = lay_out(&bordered(
            "첫 문단.\n\n| 가 | 나 |\n|---|---|\n| 다 | 라 |\n\n마지막 문단.\n",
        ));
        assert!(!map.rows.is_empty());
        for row in &map.rows {
            let Some(bbox) = row.bbox else { continue };
            let page = &list.pages[row.page];
            assert!(
                !crate::outside_page_bounds(
                    (bbox.x0, bbox.y0, bbox.x1, bbox.y1),
                    page.width_pt,
                    page.height_pt
                ),
                "{:?} escaped page {} ({} x {})",
                bbox,
                row.page,
                page.width_pt,
                page.height_pt
            );
        }
    }

    /// One segment's rows partition its characters: no two overlap, and together they are one
    /// contiguous run. Holds however the page break falls, so no font decides it.
    #[test]
    fn a_segments_character_ranges_are_disjoint_and_contiguous() {
        let (_, map) = lay_out(&bordered(
            "첫 문단입니다.\n\n둘째 문단입니다.\n\n셋째 문단입니다.\n",
        ));
        let mut by_id: std::collections::HashMap<&str, Vec<CharRange>> =
            std::collections::HashMap::new();
        for row in map.rows.iter().filter(|row| row.kind == kind::PARA) {
            if let Some(chars) = row.chars {
                by_id.entry(&row.id).or_default().push(chars);
            }
        }
        assert!(!by_id.is_empty(), "paragraph rows must carry a range");
        for (id, mut ranges) in by_id {
            ranges.sort_by_key(|range| (range.start, range.end));
            for pair in ranges.windows(2) {
                assert!(
                    pair[0].end <= pair[1].start,
                    "ranges of {id} overlap: {pair:?}"
                );
                assert_eq!(pair[0].end, pair[1].start, "ranges of {id} have a hole");
            }
        }
    }

    /// A bookmark is invisible by design, so it has no box — and no zero-extent rectangle is
    /// invented for it either (D-08a).
    #[test]
    fn an_invisible_segment_has_a_range_and_no_box() {
        let mut doc = hwp_convert::from_markdown("책갈피 문단.\n");
        let para = &mut doc.sections[0].paragraphs[0];
        let control_index = para.controls.len();
        para.controls.push(Control::Generic(bookmark_control()));
        para.chars.insert(
            1,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::paragraph::ctrl_char::BOOKMARK,
                ctrl_id: *b"bokm",
                payload: Vec::new(),
                ctrl_index: Some(control_index as u32),
            },
        );
        // The marker's WCHAR offset is whatever precedes it — an extended-control character
        // is 8 WCHARs wide, and `from_markdown` already emits `secd` before the text.
        let at: u32 = doc.sections[0].paragraphs[0].chars[..1]
            .iter()
            .map(hwp_model::HwpChar::wchar_width)
            .sum();
        let (_, map) = lay_out(&doc);
        let bookmarks = rows_of(&map, kind::BOOKMARK);
        assert_eq!(bookmarks.len(), 1, "one bookmark, one row");
        assert!(bookmarks[0].bbox.is_none(), "no box is invented");
        assert_eq!(bookmarks[0].item_count, 0);
        assert_eq!(
            bookmarks[0].chars,
            Some(CharRange { start: at, end: at }),
            "the range is the marker's own position"
        );
    }

    /// Fragments of one segment that land on one page and touch are one row, even where the
    /// layout pass drew them separately. Rows of one id on *different* pages stay separate, so
    /// an id still repeats in the set. (Non-touching fragments on one page also stay separate;
    /// this single-column fixture has none, which is why the uniqueness assertion holds here
    /// and is not a general guarantee - see `non_touching_fragments_are_not_merged_into_one_box`.)
    #[test]
    fn one_row_per_segment_and_page() {
        let mut md = String::from("| 가 | 나 |\n|---|---|\n");
        for i in 0..200 {
            md.push_str(&format!("| {i} | 값 |\n"));
        }
        let (_, map) = lay_out(&hwp_convert::from_markdown(&md));
        let mut keys: Vec<(usize, &str)> = map
            .rows
            .iter()
            .map(|row| (row.page, row.id.as_str()))
            .collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(total, keys.len(), "a (page, id) pair must appear once");
        let mut ids: Vec<&str> = map.rows.iter().map(|row| row.id.as_str()).collect();
        ids.sort_unstable();
        let distinct = {
            let mut d = ids.clone();
            d.dedup();
            d.len()
        };
        assert!(
            distinct < ids.len(),
            "a split table must report one id on more than one page"
        );
    }

    /// A paragraph split across pages owns its background slice on **every** page, including
    /// the one drawn in the mid-paragraph break band (the third `draw_para_bg_slice` call site,
    /// which used to drop its insert count). With the insert credited to the open span, that
    /// site's report is a no-op on every input reachable today - no closed span sits after the
    /// insertion point during the line loop - so this test pins the property rather than the
    /// line; `#[must_use]` on `draw_para_bg_slice` is what stops a fourth site dropping it.
    #[test]
    fn the_mid_paragraph_background_slice_is_attributed_too() {
        let (list, map) = lay_out(&split_mid_paragraph(true));
        let plain = lay_out(&split_mid_paragraph(false));
        assert!(
            list.pages[0].items.len() > plain.0.pages[0].items.len(),
            "the fixture must add a background slice on the first page"
        );
        let rows: Vec<SegmentRow> = map
            .rows
            .iter()
            .filter(|row| row.kind == kind::PARA)
            .cloned()
            .collect();
        assert!(
            rows.len() > 1,
            "the paragraph must reach more than one page, or this test proves nothing"
        );
        let covered: usize = rows.iter().map(|row| row.item_count).sum();
        let plain_covered: usize = plain
            .1
            .rows
            .iter()
            .filter(|row| row.kind == kind::PARA)
            .map(|row| row.item_count)
            .sum();
        assert_eq!(
            covered - plain_covered,
            list.pages.iter().map(|p| p.items.len()).sum::<usize>()
                - plain.0.pages.iter().map(|p| p.items.len()).sum::<usize>(),
            "every background slice must belong to the paragraph it fills"
        );
    }

    /// A paragraph split across pages reports one range per page, and the ranges partition its
    /// characters. Before the split fixture existed this test's `windows(2)` loop was empty and
    /// both of its assertions were dead.
    #[test]
    fn a_split_paragraphs_ranges_are_disjoint_and_contiguous() {
        let (_, map) = lay_out(&split_mid_paragraph(false));
        let ranges: Vec<CharRange> = map
            .rows
            .iter()
            .filter(|row| row.kind == kind::PARA)
            .filter_map(|row| row.chars)
            .collect();
        assert!(
            ranges.len() > 1,
            "the split paragraph must report a range per page"
        );
        let mut ranges = ranges;
        ranges.sort_by_key(|range| (range.start, range.end));
        for pair in ranges.windows(2) {
            assert!(pair[0].end <= pair[1].start, "ranges overlap: {pair:?}");
            assert_eq!(pair[0].end, pair[1].start, "ranges have a hole: {pair:?}");
        }
    }

    /// A paragraph split across pages by the **fallback** band - the one a document this tool
    /// generates takes, because it carries no cached `LineSeg`s (#282). The line count comes
    /// from markdown hard breaks (`LINE_BREAK` control characters), which advance the baseline
    /// without asking a font anything, so the split happens whatever fonts the host has.
    fn fallback_split_paragraph() -> Document {
        let md: String = (0..400).map(|i| format!("{i}번째 줄  \n")).collect();
        let doc = hwp_convert::from_markdown(&md);
        assert!(
            doc.sections[0].paragraphs[0].line_segs.is_empty(),
            "the fixture must reach the fallback band"
        );
        doc
    }

    /// The fallback band's split records geometry the same way the cached band's does: one row
    /// per (segment, page), which is the shape `schemas/render-layout-v1.schema.json`
    /// publishes. Before #282 the band never broke a page, so the paragraph produced a single
    /// row whose box ran tens of pages below the page it sat on.
    #[test]
    fn a_fallback_split_paragraph_records_one_row_per_page() {
        let (list, map) = lay_out(&fallback_split_paragraph());
        assert!(
            list.pages.len() > 1,
            "the fixture must paginate, or this test proves nothing — got {} page(s)",
            list.pages.len()
        );
        let rows = rows_of(&map, kind::PARA);
        assert_eq!(
            rows.len(),
            list.pages.len(),
            "one row per page the paragraph touches"
        );
        let ids: std::collections::BTreeSet<&str> =
            rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids.len(), 1, "every row names the same paragraph: {ids:?}");
        let pages: Vec<usize> = rows.iter().map(|row| row.page).collect();
        assert_eq!(
            pages,
            (0..list.pages.len()).collect::<Vec<_>>(),
            "the rows name consecutive pages"
        );
        // Each row's box stays inside the page it names. The published geometry is what an
        // editor hit-tests against, and the defect published boxes 37 pages tall.
        for row in &rows {
            let Some(bbox) = row.bbox else { continue };
            assert!(
                bbox.y1 <= list.pages[row.page].height_pt,
                "row on page {} runs past the page: {bbox:?}",
                row.page
            );
        }
        // The rows partition the paragraph's characters, exactly as the cached path's do.
        let mut ranges: Vec<CharRange> = rows.iter().filter_map(|row| row.chars).collect();
        assert_eq!(ranges.len(), rows.len(), "every row carries a range");
        ranges.sort_by_key(|range| (range.start, range.end));
        for pair in ranges.windows(2) {
            assert!(pair[0].end <= pair[1].start, "ranges overlap: {pair:?}");
        }
    }

    // --- recorder-level tests -------------------------------------------------------------
    //
    // The next few drive `SegmentRecorder` directly. The properties they pin depend on the
    // order of the recorder's own calls, not on what a document happens to lay out, and a
    // document that reaches them (a footnote block emitted mid-paragraph, a hundred thousand
    // segments) is either font-dependent or too large to keep in the suite.

    fn page_with(items: usize) -> PageList {
        PageList {
            width_pt: 595.0,
            height_pt: 842.0,
            items: (0..items)
                .map(|i| Item::Rect {
                    x: i as f32,
                    y: i as f32,
                    w: 1.0,
                    h: 1.0,
                    fill: 0,
                })
                .collect(),
        }
    }

    /// Page furniture is emitted while a paragraph span is still open at a mid-paragraph page
    /// break: the note block, the note separator and the page number all land after the
    /// paragraph's content but before the page is pushed. Without the recorded content
    /// boundary the span ran to `page.items.len()` and swallowed them, inflating its box to the
    /// page bottom and polluting its character range with `start_wchar` values belonging to a
    /// different source paragraph.
    #[test]
    fn furniture_emitted_at_a_page_break_stays_out_of_an_open_span() {
        let para = Paragraph::default();
        let mut rec = SegmentRecorder::new();
        rec.begin_paragraph(0, 0, &para, &page_with(0));
        rec.content_end(&page_with(3)); // three items of content ...
        rec.page_pushed(); // ... then two of furniture, then the push
        rec.end_segment(&page_with(2));
        rec.resolve(&[page_with(5), page_with(2)]);
        let map = rec.finish();
        assert_eq!(map.rows.len(), 2, "one row per page touched");
        assert_eq!(
            map.rows[0].item_count, 3,
            "the first page's row stops where content stopped"
        );
        assert_eq!(map.rows[1].item_count, 2);
    }

    /// A glyph run with no font behind it: `char_range` reads only `text` and `start_wchar`.
    fn glyphs(start_wchar: u32, text: &str) -> Item {
        Item::Glyphs {
            x: 10.0,
            y: 20.0,
            run: crate::shape::ShapedRun {
                font: std::sync::Arc::new(crate::fonts::LoadedFont {
                    data: std::sync::Arc::new(Vec::new()),
                    index: 0,
                    family: String::new(),
                }),
                size_pt: 10.0,
                x_scale: 1.0,
                color: 0,
                bold: false,
                italic: false,
                underline_kind: 0,
                underline_shape: 0,
                strike: false,
                strike_shape: 0,
                emphasis: 0,
                underline_color: 0xFFFF_FFFF,
                shade_color: 0xFFFF_FFFF,
                shadow: None,
                shadow_gap: (0, 0),
                outline: false,
                emboss: false,
                engrave: false,
                border_fill_id: 0,
                glyphs: Vec::new(),
                width_pt: 10.0,
                text: text.into(),
                start_wchar,
            },
        }
    }

    /// A list marker is the paragraph's - inside its box and its item count - but it is
    /// synthesized text at `start_wchar` 0, no character of the paragraph. A split cell
    /// paragraph redraws it at the head of every fragment, so counting it put each fragment's
    /// range back at 0 and the rows of one paragraph overlapped (D-09), on the committed
    /// sample's `.0.47.0.30.1`: `(0,18) (0,36) (0,61)` instead of `(0,18) (18,36) (36,61)`.
    #[test]
    fn a_list_marker_is_in_the_box_but_not_in_the_source_range() {
        let para = Paragraph::default();
        let mut rec = SegmentRecorder::new();
        let mut page = page_with(0);
        rec.begin_paragraph(0, 0, &para, &page);
        page.items.push(glyphs(0, "•"));
        rec.marker_drawn(&page);
        page.items.push(glyphs(18, "가나다"));
        rec.end_segment(&page);
        rec.resolve(&[page]);
        let map = rec.finish();
        assert_eq!(map.rows.len(), 1);
        assert_eq!(map.rows[0].chars, Some(CharRange { start: 18, end: 21 }));
        assert_eq!(
            map.rows[0].item_count, 2,
            "the marker still belongs to the row"
        );
    }

    /// The third comparison in `items_inserted` is the one that must **stay** `>=`, and the
    /// available mistake is changing all three because two needed it.
    ///
    /// A closed span whose `start_item` equals the insertion point genuinely begins after the
    /// inserted item, so it shifts. That case is real: a paragraph whose only content is a
    /// table opens the table's span at the paragraph's own first index, and the background fill
    /// is inserted at exactly that index once the table has been laid out and its span closed.
    /// With `>` there, the table would own the paragraph's background.
    #[test]
    fn an_insert_at_a_closed_spans_first_index_pushes_that_span_right() {
        let para = Paragraph::default();
        let table = hwp_model::control::Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: Default::default(),
            table_tail: Vec::new(),
            cells: Vec::new(),
            caption: None,
            extras: Vec::new(),
        };
        let mut rec = SegmentRecorder::new();
        rec.begin_paragraph(0, 0, &para, &page_with(0));
        rec.begin_table(0, &table, &page_with(0)); // the table starts at the paragraph's index 0
        rec.end_segment(&page_with(2)); // and covers two items
        rec.items_inserted(0, 1); // then the paragraph's background goes in at index 0
        rec.end_segment(&page_with(3));
        rec.resolve(&[page_with(3)]);
        let map = rec.finish();
        let table_row = map
            .rows
            .iter()
            .find(|row| row.kind == kind::TABLE)
            .expect("a table row");
        let para_row = map
            .rows
            .iter()
            .find(|row| row.kind == kind::PARA)
            .expect("a paragraph row");
        assert_eq!(
            table_row.item_count, 2,
            "the table keeps its own two items and does not acquire the fill"
        );
        assert_eq!(
            para_row.item_count, 3,
            "the paragraph owns the fill as well as the table"
        );
    }

    /// The row cap has to bound **rows**. `resolve` drains the span list at the end of every
    /// section, so a cap counting spans alone reset itself per section while the row set - one
    /// row per page a span touches - grew without a bound, and `truncated` stayed false.
    #[test]
    fn the_cap_bounds_rows_and_survives_a_section_boundary() {
        let para = Paragraph::default();
        let mut rec = SegmentRecorder::new();
        let filler = SegmentRow {
            page: 0,
            kind: kind::PARA,
            id: String::new(),
            bbox: None,
            chars: None,
            item_count: 0,
        };
        rec.map.rows = vec![filler; MAX_SEGMENT_ROWS];
        // Spans are empty here, exactly as they are just after a section resolve.
        assert!(rec.spans.is_empty());
        rec.begin_paragraph(0, 0, &para, &page_with(0));
        rec.end_segment(&page_with(1));
        rec.resolve(&[page_with(1)]);
        let map = rec.finish();
        assert!(
            map.rows.iter().all(|row| row.id.is_empty()),
            "the capped paragraph must not have produced a row"
        );
        assert!(map.truncated, "and the map says the set is incomplete");
    }

    /// Two fragments of one cell in different columns of a multi-column section are separated
    /// by the gutter. Unioning them would publish one box covering both columns and the space
    /// between, which is ground the cell does not occupy, so they stay separate rows.
    #[test]
    fn non_touching_fragments_are_not_merged_into_one_box() {
        let row = |x0: f32, x1: f32| SegmentRow {
            page: 0,
            kind: kind::CELL,
            id: "abc.0.1.2".into(),
            bbox: Some(BoxPt {
                x0,
                y0: 10.0,
                x1,
                y1: 20.0,
            }),
            chars: None,
            item_count: 1,
        };
        let mut rec = SegmentRecorder::new();
        // Two vertically adjacent fragments in one column, plus one across the gutter.
        rec.map.rows = vec![row(10.0, 100.0), row(100.0, 150.0), row(300.0, 400.0)];
        let map = rec.finish();
        assert_eq!(map.rows.len(), 2, "touching merges, a gutter does not");
        assert_eq!(map.rows[0].bbox.unwrap().x0, 10.0);
        assert_eq!(map.rows[0].bbox.unwrap().x1, 150.0);
        assert_eq!(map.rows[1].bbox.unwrap().x0, 300.0);
    }

    /// The 2x2 table with `cells_to_empty` of its cells stripped of their text and their
    /// border.
    fn borderless_empty_table(cells_to_empty: usize) -> Document {
        let mut doc = table_markdown();
        let mut touched = 0;
        for cell in first_table(&mut doc).cells.iter_mut().take(cells_to_empty) {
            for cell_para in &mut cell.paragraphs {
                cell_para.chars.clear();
                cell_para.line_segs.clear();
            }
            // A border fill id with no entry behind it: no background and no stroked border,
            // so the cell contributes no display item at all.
            cell.border_fill = hwp_model::BorderFillId(u16::MAX);
            touched += 1;
        }
        assert_eq!(
            touched, cells_to_empty,
            "the fixture must reach that many cells"
        );
        doc
    }

    /// A borderless table whose cells are empty keeps every row, because `table` and `cell`
    /// are exempt from the no-box-no-range drop.
    ///
    /// Their `chars` is `None` by design, so for them that rule would collapse to
    /// `bbox.is_some()` - a different rule from the one it states. A borderless layout table
    /// with empty cells is an ordinary HWP construct and produces no display item, so the
    /// widened version deleted it from the published geometry: one emptied cell published 3
    /// cell rows instead of 4, and emptying all four removed the `table` row as well, leaving
    /// zero rows for a table that is really there.
    ///
    /// It also broke `render-layout-v1`'s own text. That schema says `box: null` means "this
    /// segment produced no display item, by design"; with the widened rule, `box: null` became
    /// reachable only for `bookmark`, so the emitter could no longer produce a state its own
    /// schema documents. This case had no coverage before, which is why the widening went
    /// unnoticed.
    #[test]
    fn a_borderless_table_with_empty_cells_keeps_every_row() {
        // `cells_to_empty` = how many of the 2x2 table's cells lose their text and their
        // border. Both counts are checked: one emptied cell was the reviewer's repro, all four
        // is the case that also took the table row with it.
        fn empty_cells(cells_to_empty: usize) -> (usize, usize) {
            let (_, map) = lay_out(&borderless_empty_table(cells_to_empty));
            (
                rows_of(&map, kind::CELL).len(),
                rows_of(&map, kind::TABLE).len(),
            )
        }

        let full = empty_cells(0);
        assert_eq!(full, (4, 1), "the untouched 2x2 table is the baseline");
        assert_eq!(
            empty_cells(1),
            (4, 1),
            "one borderless empty cell must still publish its own row"
        );
        assert_eq!(
            empty_cells(4),
            (4, 1),
            "a wholly borderless empty table is still a table that is there"
        );
    }

    /// An EMPTY paragraph produces no envelope segment either, so a row for it would carry an
    /// id that joins to nothing - the same rule as the unanchored bookmark below.
    ///
    /// Found by 05-06's cross-artifact join-key test, which compares the two PUBLISHED
    /// artifacts: `hwp cat --segments v2` emits nothing for a paragraph with no characters,
    /// while the recorder was emitting a row for it with no box AND no character range. Such a
    /// row measures nothing at all - it is an id, a kind and two nulls - and it made layout ids
    /// stop being a subset of envelope ids on the committed sample.
    #[test]
    fn an_empty_paragraph_produces_no_row() {
        let mut doc = hwp_convert::from_markdown("첫 문단.\n\n둘째 문단.\n");
        // An empty paragraph between the two, exactly as the committed sample carries.
        let para_shape = doc.sections[0].paragraphs[0].para_shape;
        doc.sections[0].paragraphs.insert(
            1,
            hwp_model::Paragraph {
                para_shape,
                ..Default::default()
            },
        );
        let (_, map) = lay_out(&doc);
        assert!(
            map.rows
                .iter()
                .all(|row| row.bbox.is_some() || row.chars.is_some()),
            "a row with neither a box nor a range joins to nothing: {:?}",
            map.rows
                .iter()
                .filter(|r| r.bbox.is_none() && r.chars.is_none())
                .map(|r| (&r.id, r.kind))
                .collect::<Vec<_>>()
        );
    }

    /// An unanchored `bokm` control produces no envelope segment, so a row for it would carry
    /// an id that joins to nothing.
    #[test]
    fn a_bookmark_with_no_anchor_character_produces_no_row() {
        let mut doc = hwp_convert::from_markdown("책갈피 문단.\n");
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Generic(bookmark_control()));
        let (_, map) = lay_out(&doc);
        assert!(
            rows_of(&map, kind::BOOKMARK).is_empty(),
            "no anchor character, no segment, no row"
        );
    }

    /// The layout pass can return early on an exhausted budget. The pages it returns carry
    /// content whose provenance was recorded, so the rows for them must be published too:
    /// before, the in-progress section's spans were dropped on the floor and the caller got a
    /// `DisplayList` with pages and a `SegmentMap` with nothing for them.
    #[test]
    fn an_aborted_layout_still_publishes_the_rows_it_recorded() {
        let doc = split_mid_paragraph(false);
        let mut store = FontStore::new();
        let mut warnings = RenderIssueAccumulator::new();
        warnings.set_page_limit(1); // the second page cannot be opened
        let (list, map) =
            crate::layout::layout_document_with_segments(&doc, &mut store, &mut warnings);
        assert_eq!(list.pages.len(), 1, "the budget stopped the second page");
        assert!(
            !map.rows.is_empty(),
            "the rows for the page that was returned must be published"
        );
        assert!(
            map.rows.iter().all(|row| row.page < list.pages.len()),
            "and every row must index a page the caller actually received"
        );
    }

    // --- the cross-artifact join, on documents built here ----------------------------------
    //
    // `crates/hwp-cli/tests/render_layout.rs` checks the join on the published artifacts of
    // every fixture a host has; most of those are gitignored, so these build each case the
    // join depends on and run it everywhere.

    /// Path depth of an id: `<checksum>.<section>.<indices...>`.
    fn depth(id: &str) -> usize {
        id.split('.').count() - 2
    }

    /// Both directions of the join on one document: row ids that name no envelope segment, and
    /// envelope `para`/`table`/`cell`/`bookmark` ids that have no row - anywhere, drawing-object
    /// text included.
    fn join_orphans(doc: &Document) -> (Vec<String>, Vec<String>) {
        use hwp_convert::SegmentKind;
        let (_, map) = lay_out(doc);
        let (_, segments) =
            hwp_convert::to_markdown_with_segments_v2(doc, &Default::default()).expect("no IO");
        let envelope: std::collections::HashSet<&str> =
            segments.iter().map(|s| s.id.as_str()).collect();
        let rows: std::collections::HashSet<&str> =
            map.rows.iter().map(|row| row.id.as_str()).collect();
        let mut layout_orphans: Vec<String> = rows
            .iter()
            .filter(|id| !envelope.contains(*id))
            .map(|id| id.to_string())
            .collect();
        layout_orphans.sort();
        let envelope_orphans = segments
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    SegmentKind::Para
                        | SegmentKind::Table
                        | SegmentKind::Cell
                        | SegmentKind::Bookmark
                ) && !rows.contains(s.id.as_str())
            })
            .map(|s| s.id.clone())
            .collect();
        (layout_orphans, envelope_orphans)
    }

    fn assert_joins(doc: &Document, what: &str) {
        let (layout, envelope) = join_orphans(doc);
        assert!(
            layout.is_empty() && envelope.is_empty(),
            "{what}: rows naming no envelope segment {layout:?}; envelope segments with no row \
             {envelope:?}"
        );
    }

    /// Appends an extended-control character bound to `control`, the way a real file anchors
    /// one.
    fn attach(para: &mut Paragraph, code: u16, control: Control) {
        para.chars.push(HwpChar::ExtCtrl {
            code,
            ctrl_id: control.ctrl_id(),
            payload: Vec::new(),
            ctrl_index: Some(para.controls.len() as u32),
        });
        para.controls.push(control);
    }

    fn table_markdown() -> Document {
        hwp_convert::from_markdown("| 가 | 나 |\n|---|---|\n| 1 | 2 |\n")
    }

    fn first_table(doc: &mut Document) -> &mut Table {
        doc.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|para| para.controls.iter_mut())
            .find_map(|control| match control {
                Control::Table(table) => Some(table),
                _ => None,
            })
            .expect("the fixture carries a table")
    }

    /// A drawing object: a rectangle when `drawn`, otherwise a `gso ` with nothing the renderer
    /// can draw.
    fn drawing(drawn: bool) -> Control {
        let mut control = bookmark_control();
        control.ctrl_id = *b"gso ";
        if drawn {
            control.gso_shapes.push(hwp_model::ShapeGeom {
                kind: hwp_model::ShapeKind::Rect,
                x: 1_000,
                y: 1_000,
                w: 2_000,
                h: 2_000,
                points: Vec::new(),
                fill: 0,
                fill_gradient: None,
                border_color: 0,
                border_width: 10,
                round_ratio: 0,
                border_style: 0,
                arrow_start: 0,
                arrow_end: 0,
                anchored: false,
                description: None,
            });
        }
        Control::Generic(control)
    }

    /// #283: a cell's paragraphs, a table nested in one, the nested table's cells and their
    /// paragraphs all have rows, under exactly the paths the envelope gives them.
    #[test]
    fn cell_paragraphs_and_nested_tables_join_the_envelope_both_ways() {
        let mut doc = table_markdown();
        let inner = first_table(&mut doc).clone();
        attach(
            &mut first_table(&mut doc).cells[0].paragraphs[0],
            hwp_model::paragraph::ctrl_char::OBJECT,
            Control::Table(inner),
        );
        let (_, map) = lay_out(&doc);
        for (kind, at) in [
            (kind::PARA, 4),  // a paragraph in a cell
            (kind::TABLE, 5), // the table nested in it
            (kind::CELL, 6),  // a cell of the nested table
            (kind::PARA, 7),  // a paragraph in that cell
        ] {
            assert!(
                map.rows
                    .iter()
                    .any(|row| row.kind == kind && depth(&row.id) == at),
                "no {kind} row at depth {at}: {:?}",
                map.rows.iter().map(|r| &r.id).collect::<Vec<_>>()
            );
        }
        assert_joins(&doc, "a nested table");
    }

    /// A bookmark in a cell is recorded under its cell paragraph's path, not the body
    /// paragraph's: `begin_cell_paragraph` leaves `para_path` alone, and `bookmark` resolves
    /// against the innermost open paragraph.
    #[test]
    fn a_bookmark_inside_a_cell_keeps_its_cell_relative_path() {
        let mut doc = table_markdown();
        attach(
            &mut first_table(&mut doc).cells[1].paragraphs[0],
            hwp_model::paragraph::ctrl_char::BOOKMARK,
            Control::Generic(bookmark_control()),
        );
        let (_, map) = lay_out(&doc);
        let bookmarks = rows_of(&map, kind::BOOKMARK);
        assert_eq!(bookmarks.len(), 1, "one anchored bookmark, one row");
        assert_eq!(
            depth(&bookmarks[0].id),
            5,
            "[para, table, cell, cell paragraph, bookmark]: {}",
            bookmarks[0].id
        );
        assert_joins(&doc, "a bookmark in a cell");
    }

    /// #285: a paragraph that shapes no text but is drawn has a row, and the envelope a point
    /// `para` segment for it - a drawing alone in the body or in a cell, a drawing the renderer
    /// cannot draw, and a paragraph of whitespace.
    #[test]
    fn a_drawn_paragraph_without_text_joins_the_envelope_both_ways() {
        for (what, drawn, in_cell) in [
            ("a drawing alone", true, false),
            ("a drawing alone in a cell", true, true),
            ("a drawing that draws nothing", false, false),
        ] {
            let mut doc = table_markdown();
            let mut para = Paragraph {
                para_shape: doc.sections[0].paragraphs[0].para_shape,
                ..Default::default()
            };
            attach(
                &mut para,
                hwp_model::paragraph::ctrl_char::OBJECT,
                drawing(drawn),
            );
            let id = if in_cell {
                let cell = &mut first_table(&mut doc).cells[0];
                cell.paragraphs.push(para);
                None
            } else {
                doc.sections[0].paragraphs.push(para);
                Some(doc.sections[0].paragraphs.len() - 1)
            };
            let (_, map) = lay_out(&doc);
            if let Some(index) = id {
                let row = map
                    .rows
                    .iter()
                    .find(|row| {
                        depth(&row.id) == 1 && row.id.split('.').nth(2) == Some(&index.to_string())
                    })
                    .unwrap_or_else(|| panic!("{what}: the paragraph must have a row"));
                assert_eq!(row.bbox.is_some(), drawn, "{what}: {row:?}");
                assert_eq!(row.chars, None, "{what}: it shapes no text");
            }
            assert_joins(&doc, what);
        }

        let mut doc = hwp_convert::from_markdown("첫 문단.\n\n둘째 문단.\n");
        let para = &mut doc.sections[0].paragraphs[1];
        para.chars.retain(|ch| !matches!(ch, HwpChar::Text(_)));
        para.chars.extend("   ".chars().map(HwpChar::Text));
        assert_joins(&doc, "a paragraph of whitespace");
    }

    /// The paragraph anchoring a borderless table with empty cells draws nothing, but the
    /// envelope has a segment for it (the table's markup), so it keeps its row.
    #[test]
    fn the_paragraph_anchoring_a_borderless_empty_table_keeps_its_row() {
        let doc = borderless_empty_table(4);
        let (_, map) = lay_out(&doc);
        assert!(
            rows_of(&map, kind::PARA)
                .iter()
                .any(|row| row.bbox.is_none() && row.chars.is_none()),
            "the anchoring paragraph's row measures nothing and must survive anyway"
        );
        assert_joins(&doc, "a borderless empty table");
    }

    /// Cell 0 of the table holding its one-line fallback paragraph (no cached geometry, so it
    /// flows) and then a cached paragraph whose two lines Hancom stored at the top of the cell.
    /// The flow floor pushes the cached paragraph down below the first one by about one line,
    /// and `cell_height` - the stored row height, which a cached cell never grows past its own
    /// cache - decides how much of it stays inside the cell: `layout_box_para_iter` clips, and
    /// does not draw, every shifted line that would cross the cell's bottom edge. Positions come
    /// from the cached `LineSeg`s and the character shape's size, not from shaping, so no font
    /// decides where the edge falls.
    fn shifted_cell(cell_height: i32) -> (Document, String) {
        let mut doc = table_markdown();
        let cell = &mut first_table(&mut doc).cells[0];
        let mut cached = cell.paragraphs[0].clone();
        cached.chars.retain(|ch| !matches!(ch, HwpChar::Text(_)));
        let text_start = cached.wchar_len();
        cached.chars.extend("첫줄둘째".chars().map(HwpChar::Text));
        let seg = |text_start: u32, v_pos: i32| hwp_model::paragraph::LineSeg {
            text_start,
            v_pos,
            line_height: 1_000,
            text_height: 1_000,
            baseline_gap: 850,
            line_spacing: 0,
            col_start: 0,
            seg_width: 4_000,
            flags: 0,
        };
        cached.line_segs = vec![seg(text_start, 0), seg(text_start + 2, 1_000)];
        cell.paragraphs.push(cached);
        cell.height = hwp_model::units::HwpUnit(cell_height);
        let (para_index, control_index) = first_table_at(&doc);
        let cell_path = SegmentPath {
            section: 0,
            indices: vec![para_index, control_index, 0, 1],
        };
        let para = first_table(&mut doc).cells[0].paragraphs[1].clone();
        (doc, segment_id::paragraph_id(&cell_path, &para))
    }

    /// (body paragraph index, control index) of the first table.
    fn first_table_at(doc: &Document) -> (usize, usize) {
        doc.sections[0]
            .paragraphs
            .iter()
            .enumerate()
            .find_map(|(p, para)| {
                para.controls
                    .iter()
                    .position(|control| matches!(control, Control::Table(_)))
                    .map(|c| (p, c))
            })
            .expect("the fixture carries a table")
    }

    fn lay_out_reporting_overflow(doc: &Document) -> (SegmentMap, bool) {
        let mut store = FontStore::new();
        let mut warnings = RenderIssueAccumulator::new();
        let (_, map) = crate::layout::layout_document_with_segments(doc, &mut store, &mut warnings);
        let overflow =
            warnings.finish().issues.iter().any(|issue| {
                issue.code == crate::issues::RenderIssueCode::TableCellContentOverflow
            });
        (map, overflow)
    }

    /// A cell paragraph with text, every line of which was clipped at the cell's edge, drew
    /// nothing - but the envelope has its text segment, so it keeps exactly one row: no box,
    /// because nothing was drawn and none is invented (D-08a), and no range, because it drew
    /// none of its characters. It used to publish no row at all, which the merge-gate review
    /// measured as an envelope id with no geometry in 15 of 50 real documents.
    #[test]
    fn a_fully_clipped_cell_paragraph_keeps_one_empty_row() {
        let (doc, id) = shifted_cell(100);
        let (map, overflow) = lay_out_reporting_overflow(&doc);
        assert!(
            overflow,
            "the fixture must clip, or this test proves nothing"
        );
        let rows: Vec<&SegmentRow> = map.rows.iter().filter(|row| row.id == id).collect();
        assert_eq!(rows.len(), 1, "exactly one row: {rows:?}");
        assert_eq!(rows[0].kind, kind::PARA);
        assert_eq!(
            rows[0].bbox, None,
            "nothing was drawn, and no box is invented"
        );
        assert_eq!(rows[0].chars, None, "it drew none of its characters");
        assert_joins(&doc, "a fully clipped cell paragraph");
    }

    /// A partially clipped cell paragraph publishes the row of the lines it drew, exactly as
    /// before: one row, its box, and a range covering its first line only - no extra empty row
    /// for the clipped line.
    #[test]
    fn a_partially_clipped_cell_paragraph_publishes_the_lines_it_drew() {
        let (doc, id) = shifted_cell(3_300);
        let (map, overflow) = lay_out_reporting_overflow(&doc);
        assert!(
            overflow,
            "the second line must be clipped, or this test proves nothing"
        );
        let rows: Vec<&SegmentRow> = map.rows.iter().filter(|row| row.id == id).collect();
        assert_eq!(rows.len(), 1, "one row, for the line it drew: {rows:?}");
        assert!(rows[0].bbox.is_some(), "the first line was drawn: {rows:?}");
        let (p, c) = first_table_at(&doc);
        let Control::Table(table) = &doc.sections[0].paragraphs[p].controls[c] else {
            unreachable!("first_table_at found it")
        };
        let first_line = table.cells[0].paragraphs[1].line_segs[0].text_start;
        assert_eq!(
            rows[0].chars,
            Some(CharRange {
                start: first_line,
                end: first_line + 2,
            }),
            "the range covers the first line only"
        );
        assert_joins(&doc, "a partially clipped cell paragraph");
    }

    /// An empty paragraph just after a list draws nothing and has no envelope segment: the
    /// blank line emitted at its position closes the list block, and is not its content.
    #[test]
    fn an_empty_paragraph_after_a_list_joins_the_envelope() {
        let mut doc = hwp_convert::from_markdown("- 항목\n\n뒤 문단\n");
        let para_shape = doc.sections[0].paragraphs[1].para_shape;
        doc.sections[0].paragraphs.insert(
            1,
            Paragraph {
                para_shape,
                ..Default::default()
            },
        );
        assert_joins(&doc, "an empty paragraph after a list");
    }

    // --- drawing-object text (#350) ---------------------------------------------------------

    /// A body document whose last paragraph is a plain text template for the ones built below.
    fn prose() -> Document {
        hwp_convert::from_markdown("첫 문단.\n\n둘째 문단.\n")
    }

    /// A paragraph of `text` shaped like `template`, with no controls and no cached lines.
    fn text_like(template: &Paragraph, text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            controls: Vec::new(),
            line_segs: Vec::new(),
            ..template.clone()
        }
    }

    fn lists(paragraphs: Vec<Vec<Paragraph>>) -> Vec<hwp_model::ParagraphList> {
        paragraphs
            .into_iter()
            .map(|paragraphs| hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs,
            })
            .collect()
    }

    /// An HWP5 text box: a floating `gso ` whose 20-byte geometry header places it.
    fn text_box(text: Vec<Vec<Paragraph>>) -> Control {
        let mut control = bookmark_control();
        control.ctrl_id = *b"gso ";
        for field in [0i32, 1_000, 1_000, 20_000, 10_000] {
            control.data.extend_from_slice(&field.to_le_bytes());
        }
        control.paragraph_lists = lists(text);
        Control::Generic(control)
    }

    /// An HWPX shape carrying text: the arm that lays text inside the first shape's box.
    fn shape_with_text(text: Vec<Vec<Paragraph>>) -> Control {
        let Control::Generic(mut control) = drawing(true) else {
            unreachable!("drawing builds a generic control")
        };
        control.ctrl_id = *b"rect";
        control.paragraph_lists = lists(text);
        Control::Generic(control)
    }

    /// `doc` with a paragraph of "앞" holding `control` appended to the body; its index.
    fn host(doc: &mut Document, control: Control) -> usize {
        let template = doc.sections[0].paragraphs.last().expect("a body").clone();
        let mut para = text_like(&template, "앞");
        attach(&mut para, hwp_model::paragraph::ctrl_char::OBJECT, control);
        doc.sections[0].paragraphs.push(para);
        doc.sections[0].paragraphs.len() - 1
    }

    /// The ids of the `para` rows whose path is `[.., control, n]` under `prefix`.
    fn rows_under<'m>(map: &'m SegmentMap, prefix: &str) -> Vec<&'m SegmentRow> {
        map.rows
            .iter()
            .filter(|row| {
                row.kind == kind::PARA
                    && row
                        .id
                        .split('.')
                        .skip(1)
                        .collect::<Vec<_>>()
                        .join(".")
                        .starts_with(prefix)
            })
            .collect()
    }

    /// The text of a text box, of an HWPX shape, and of a text box inside a cell has rows,
    /// under the paths the envelope gives them.
    #[test]
    fn drawing_object_text_joins_the_envelope_both_ways() {
        let template = prose().sections[0].paragraphs[1].clone();
        let text = || {
            vec![vec![
                text_like(&template, "가나"),
                text_like(&template, "다라"),
            ]]
        };

        for (what, control) in [
            ("a text box", text_box(text())),
            ("an HWPX shape with text", shape_with_text(text())),
        ] {
            let mut doc = prose();
            let at = host(&mut doc, control);
            let (_, map) = lay_out(&doc);
            let rows = rows_under(&map, &format!("0.{at}.0."));
            assert_eq!(
                rows.len(),
                2,
                "{what}: one row per paragraph: {:?}",
                map.rows
            );
            assert!(
                rows.iter().all(|row| depth(&row.id) == 3),
                "{what}: {rows:?}"
            );
            assert_joins(&doc, what);
        }

        let mut doc = table_markdown();
        let mut para = text_like(&template, "");
        attach(
            &mut para,
            hwp_model::paragraph::ctrl_char::OBJECT,
            text_box(text()),
        );
        first_table(&mut doc).cells[0].paragraphs.push(para);
        let (_, map) = lay_out(&doc);
        assert!(
            map.rows
                .iter()
                .any(|row| row.kind == kind::PARA && depth(&row.id) == 6),
            "a text box in a cell: [para, table, cell, cell paragraph, control, n]"
        );
        assert_joins(&doc, "a text box in a cell");
    }

    /// Hancom continues a linked text box in a new column where a paragraph's cached `v_pos`
    /// restarts. Each column is laid out on its own, but the paragraphs are numbered across the
    /// whole box, so the second column's first paragraph is `n = 2`, not 0.
    #[test]
    fn a_column_split_text_box_numbers_its_paragraphs_across_columns() {
        let template = prose().sections[0].paragraphs[1].clone();
        let cached = |text: &str, v_pos: i32| {
            let mut para = text_like(&template, text);
            para.line_segs = vec![hwp_model::paragraph::LineSeg {
                text_start: 0,
                v_pos,
                line_height: 1_000,
                text_height: 1_000,
                baseline_gap: 850,
                line_spacing: 0,
                col_start: 0,
                seg_width: 18_000,
                flags: 0,
            }];
            para
        };
        let mut doc = prose();
        let at = host(
            &mut doc,
            text_box(vec![vec![
                cached("가", 0),
                cached("나", 1_000),
                cached("다", 0), // v_pos restarts: a second column
                cached("라", 1_000),
            ]]),
        );
        let (_, map) = lay_out(&doc);
        for n in 0..4 {
            assert_eq!(
                rows_under(&map, &format!("0.{at}.0.{n}")).len(),
                1,
                "paragraph {n} has one row: {:?}",
                map.rows
            );
        }
        assert_joins(&doc, "a column-split text box");
    }

    /// An HWPX container laying each list out in its own box still numbers the paragraphs
    /// across all of its lists.
    #[test]
    fn a_containers_lists_are_numbered_across_all_of_them() {
        let template = prose().sections[0].paragraphs[1].clone();
        let mut control = bookmark_control();
        control.ctrl_id = *b"cont";
        control.paragraph_lists = lists(vec![
            vec![text_like(&template, "가"), text_like(&template, "나")],
            vec![text_like(&template, "다")],
        ]);
        control.container_box = Some(hwp_model::ContainerBox {
            x: 1_000,
            y: 1_000,
            w: 20_000,
            h: 10_000,
            anchored: false,
            skipped_objects: 0,
            text_boxes: vec![Some([0, 0, 10_000, 5_000]), Some([0, 5_000, 10_000, 5_000])],
        });
        let mut doc = prose();
        let at = host(&mut doc, Control::Generic(control));
        let (_, map) = lay_out(&doc);
        assert_eq!(
            rows_under(&map, &format!("0.{at}.0.2")).len(),
            1,
            "the second list's first paragraph is n = 2: {:?}",
            map.rows
        );
        assert_joins(&doc, "a container with two lists");
    }

    /// A table and a bookmark in a text box nest under the text box paragraph that holds them.
    #[test]
    fn a_table_and_a_bookmark_in_a_text_box_have_rows() {
        let template = prose().sections[0].paragraphs[1].clone();
        let mut inner = text_like(&template, "가");
        attach(
            &mut inner,
            hwp_model::paragraph::ctrl_char::OBJECT,
            Control::Table(first_table(&mut table_markdown()).clone()),
        );
        attach(
            &mut inner,
            hwp_model::paragraph::ctrl_char::BOOKMARK,
            Control::Generic(bookmark_control()),
        );
        let mut doc = prose();
        host(&mut doc, text_box(vec![vec![inner]]));
        let (_, map) = lay_out(&doc);
        for (kind, at) in [
            (kind::TABLE, 4),    // [para, control, n, control]
            (kind::CELL, 5),     // ... cell
            (kind::PARA, 6),     // ... cell paragraph
            (kind::BOOKMARK, 4), // [para, control, n, control]
        ] {
            assert!(
                map.rows
                    .iter()
                    .any(|row| row.kind == kind && depth(&row.id) == at),
                "no {kind} row at depth {at}: {:?}",
                map.rows
            );
        }
        assert_joins(&doc, "a table and a bookmark in a text box");
    }

    /// A table in a text box paragraph flushes the envelope's output, so that paragraph is a
    /// point there; its row still joins.
    #[test]
    fn a_block_interrupted_drawing_paragraph_joins_the_envelope() {
        let template = prose().sections[0].paragraphs[1].clone();
        let mut inner = text_like(&template, "전");
        attach(
            &mut inner,
            hwp_model::paragraph::ctrl_char::OBJECT,
            Control::Table(first_table(&mut table_markdown()).clone()),
        );
        inner
            .chars
            .extend("표 뒤에 오는 긴 문장".chars().map(HwpChar::Text));
        let mut doc = prose();
        host(&mut doc, text_box(vec![vec![inner]]));
        assert_joins(&doc, "a block-interrupted text box paragraph");
    }

    /// A text-less HWPX shape is drawn but shapes no text, in the body and in a text box; both
    /// paragraphs have a row and a point segment to join it to.
    #[test]
    fn a_textless_hwpx_shape_joins_in_the_body_and_in_a_text_box() {
        let template = prose().sections[0].paragraphs[1].clone();
        let shape_alone = || {
            let mut para = text_like(&template, "");
            let Control::Generic(mut control) = drawing(true) else {
                unreachable!("drawing builds a generic control")
            };
            control.ctrl_id = *b"rect";
            attach(
                &mut para,
                hwp_model::paragraph::ctrl_char::OBJECT,
                Control::Generic(control),
            );
            para
        };
        let mut doc = prose();
        doc.sections[0].paragraphs.push(shape_alone());
        host(&mut doc, text_box(vec![vec![shape_alone()]]));
        assert_joins(&doc, "a text-less HWPX shape");
    }

    /// Text the renderer never lays out - a text box whose geometry header is too short to
    /// place it, or an object kind no arm draws - still has one unmeasured row per paragraph,
    /// its table included, so the join stays total. The object carries two lists, so the
    /// numbering must run across them: the second list's paragraph is `n = 2`, not 0.
    #[test]
    fn drawing_text_the_renderer_skips_still_has_rows() {
        let template = prose().sections[0].paragraphs[1].clone();
        let text = || {
            let mut with_table = text_like(&template, "나");
            attach(
                &mut with_table,
                hwp_model::paragraph::ctrl_char::OBJECT,
                Control::Table(first_table(&mut table_markdown()).clone()),
            );
            vec![
                vec![text_like(&template, "가"), with_table],
                vec![text_like(&template, "다")],
            ]
        };
        let Control::Generic(mut short) = text_box(text()) else {
            unreachable!("text_box builds a generic control")
        };
        short.data.truncate(10);
        let mut unknown = bookmark_control();
        unknown.ctrl_id = *b"conn";
        unknown.paragraph_lists = lists(text());
        for (what, control) in [
            ("a text box that cannot be placed", short),
            ("an object kind no arm draws", unknown),
        ] {
            let mut doc = prose();
            let at = host(&mut doc, Control::Generic(control));
            let (_, map) = lay_out(&doc);
            let rows = rows_under(&map, &format!("0.{at}.0."));
            assert!(
                rows.iter().any(|row| depth(&row.id) == 3)
                    && rows.iter().all(|row| row.bbox.is_none()),
                "{what}: unmeasured rows under the object: {:?}",
                map.rows
            );
            assert_eq!(
                rows_under(&map, &format!("0.{at}.0.2")).len(),
                1,
                "{what}: the second list's paragraph is numbered on from the first list: {:?}",
                map.rows
            );
            assert_joins(&doc, what);
        }
    }

    /// The paragraph holding a text box claims its own characters only: the text box's glyphs
    /// belong to the text box paragraphs' rows, whose offsets index a different string.
    #[test]
    fn the_paragraph_holding_a_text_box_claims_none_of_its_characters() {
        let template = prose().sections[0].paragraphs[1].clone();
        let mut doc = prose();
        let at = host(
            &mut doc,
            text_box(vec![vec![text_like(&template, "가나다라마바사")]]),
        );
        let (_, map) = lay_out(&doc);
        let inner = rows_under(&map, &format!("0.{at}.0.0"));
        assert!(
            inner.iter().any(|row| row.chars.is_some()),
            "the text box paragraph must draw text, or this test proves nothing: {inner:?}"
        );
        let own = map
            .rows
            .iter()
            .find(|row| {
                row.kind == kind::PARA && depth(&row.id) == 1 && row.id.ends_with(&format!(".{at}"))
            })
            .expect("the host paragraph's row");
        assert_eq!(
            own.chars,
            Some(CharRange { start: 0, end: 1 }),
            "the host shaped \"앞\" and nothing else: {own:?}"
        );
    }

    /// `finish` keeps an unmeasured row for a segmented paragraph only when the paragraph has
    /// no measured row anywhere: a cell paragraph drawn in one fragment and replayed as nothing
    /// in the next - every continuation line clipped - publishes the fragment it drew, alone.
    #[test]
    fn a_paragraph_measured_in_one_fragment_keeps_no_empty_row_from_another() {
        let para = text_like(&prose().sections[0].paragraphs[1], "가나");
        let mut rec = SegmentRecorder::new();
        let mut first = page_with(0);
        rec.begin_paragraph(0, 0, &para, &first);
        first.items.push(glyphs(0, "가나"));
        rec.end_segment(&first);
        rec.page_pushed();
        let second = page_with(0);
        rec.begin_paragraph(0, 0, &para, &second);
        rec.end_segment(&second);
        rec.resolve(&[first, second]);
        let map = rec.finish();
        assert_eq!(
            map.rows.len(),
            1,
            "the drawn fragment alone: {:?}",
            map.rows
        );
        assert_eq!(map.rows[0].page, 0);
        assert!(map.rows[0].chars.is_some());
    }

    /// Recording is opt-in: an ordinary render records nothing at all.
    #[test]
    fn a_plain_layout_records_no_rows() {
        let doc = bordered("본문.\n");
        let mut store = FontStore::new();
        let mut warnings = RenderIssueAccumulator::new();
        let list = crate::layout::layout_document(&doc, &mut store, &mut warnings);
        assert!(!list.pages.is_empty());
        let (_, map) = lay_out(&doc);
        assert!(
            !map.rows.is_empty(),
            "the recording entry point does record"
        );
    }
}
