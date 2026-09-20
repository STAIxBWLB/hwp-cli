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
//! # What has no row
//!
//! **Kinds.** Only `para`, `table`, `cell` and `bookmark` produce rows. The envelope's other
//! three kinds do not: a `run` has no separate geometry here, an `image`'s display item folds
//! into the row of the paragraph that carries it, and a `field` does likewise. 05-06 publishes
//! from this set, so a consumer hit-testing an image or a field must resolve it through its
//! enclosing paragraph's row and cannot expect a row of its own.
//!
//! **Nesting - the larger hole, and it is structural.** A paragraph inside a table cell
//! produces no row, and a nested table and its cells produce none either. Only top-level
//! paragraphs, and the outermost table with its own cells, are recorded. [`crate::layout`] has
//! exactly ONE `begin_paragraph` call site, on the body paragraph loop; cell content is laid
//! out by `layout_box_para_iter`, which opens no span, and `begin_table` is reachable only
//! from the body control loop. So this is not an oversight in the recorder - nothing in the
//! nested path ever calls it.
//!
//! To scale, on `fixtures/samples/report-tables.hwpx`: the envelope carries 222 paragraphs
//! against this set's 40, 126 cells against 100, and 10 tables against 3. In a table-heavy
//! document that is MOST paragraphs, not an edge case. A consumer resolves any of them
//! through the enclosing top-level `cell` or `table` row, and
//! `schemas/render-layout-v1.schema.json` says so in its published `kind` description - the
//! subset is documented rather than silent. Closing it is its own plan: the path is already
//! available as `child(base_path(), para_index)` and `layout_box_para_iter` never pushes a
//! page, so the page ordinal stays correct, but it means threading
//! `Option<&mut SegmentRecorder>` through call sites that mostly pass `None`, plus a sibling
//! of `begin_paragraph` that does not clobber `para_path` for `bookmark()`.
//!
//! **Segments that produce nothing at all.** A row with an id and *nothing measured* joins to
//! nothing: `hwp-convert` emits no envelope segment for whatever produced it, so the id has no
//! counterpart in the envelope. ONE RULE, NOT TWO COINCIDENCES: it covers both the unanchored
//! `bokm` control (no anchor character, no segment) and the completely empty paragraph (no
//! characters, no segment), and it is applied once in [`SegmentRecorder::finish`] rather than
//! case by case.
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
//! Pinned by `a_bookmark_with_no_anchor_character_produces_no_row`,
//! `an_empty_paragraph_produces_no_row` and
//! `a_borderless_table_with_empty_cells_keeps_every_row`, and backstopped end to end by
//! 05-06's cross-artifact join-key test, which is what found the empty-paragraph half.
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
    /// The source characters this row covers. `None` on `table` and `cell` rows: they span
    /// several source paragraphs, so no single paragraph's offsets describe them.
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

    /// A no-op recorder. Every ordinary render and every nested or scratch-page layout pass
    /// uses one: a nested paragraph's indices do not name a path in the document's own tree,
    /// so recording there would attribute geometry to a segment that does not exist.
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            page: 0,
            para_path: SegmentPath::default(),
            open: Vec::new(),
            spans: Vec::new(),
            next_uid: 0,
            content_end: std::collections::HashMap::new(),
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
        let path = self.para_path.clone();
        let id = segment_id::paragraph_id(&path, para);
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
        let path = child(&self.para_path, control_index);
        let uid = self.next_uid;
        self.next_uid += 1;
        self.spans.push(Span {
            kind: kind::BOOKMARK,
            id: segment_id::control_id(&path, control),
            start_page: self.page,
            start_item: 0,
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
        for open in &mut self.open {
            if open.start_page == page && open.start_item > at {
                open.start_item += count;
            }
        }
        for span in &mut self.spans {
            if span.start_page == page && span.start_item >= at {
                span.start_item += count;
            }
            if span.end_page == page && span.end_item > at {
                span.end_item += count;
            }
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
                let own = own_indices(&spans, &children[index], page_index, lo, hi);
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
        self.map.rows.retain(|row| {
            matches!(row.kind, kind::TABLE | kind::CELL)
                || row.bbox.is_some()
                || row.chars.is_some()
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
            let mut doc = hwp_convert::from_markdown("| 가 | 나 |\n|---|---|\n| 1 | 2 |\n");
            let mut touched = 0;
            for para in &mut doc.sections[0].paragraphs {
                for control in &mut para.controls {
                    if let Control::Table(table) = control {
                        for cell in table.cells.iter_mut().take(cells_to_empty) {
                            for cell_para in &mut cell.paragraphs {
                                cell_para.chars.clear();
                                cell_para.line_segs.clear();
                            }
                            // A border fill id with no entry behind it: no background and no
                            // stroked border, so the cell contributes no display item at all.
                            cell.border_fill = hwp_model::BorderFillId(u16::MAX);
                            touched += 1;
                        }
                    }
                }
            }
            assert_eq!(
                touched, cells_to_empty,
                "the fixture must reach that many cells"
            );
            let (_, map) = lay_out(&doc);
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
