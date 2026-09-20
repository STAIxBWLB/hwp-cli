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
//! reported again with a different box. Do not key a map by id alone.
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
//! Items pushed outside the content loops carry no segment and produce no row, by construction
//! rather than by accident: page borders, column dividers, headers, footers, page numbers and
//! note blocks are all emitted outside the recorded spans. The furniture-exclusion test in this
//! module pins that.
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
            map: SegmentMap::default(),
        }
    }

    /// A page was pushed; the page being filled is the next one.
    pub(crate) fn page_pushed(&mut self) {
        if self.enabled {
            self.page += 1;
        }
    }

    fn full(&self) -> bool {
        self.spans.len() + self.open.len() >= MAX_SEGMENT_ROWS
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
        let path = child(&self.para_path, control_index);
        let at = control_wchar_offset(para, control_index);
        let uid = self.next_uid;
        self.next_uid += 1;
        self.spans.push(Span {
            kind: kind::BOOKMARK,
            id: segment_id::control_id(&path, control),
            start_page: self.page,
            start_item: 0,
            end_page: self.page,
            end_item: 0,
            chars: at.map(|start| CharRange { start, end: start }),
            uid,
            parent: None,
        });
    }

    /// `count` items were *inserted* at index `at` on the page being filled, rather than
    /// appended: a paragraph background rectangle goes behind its own text. Every recorded
    /// index at or after `at` on this page shifts by `count`. Appends need no such call.
    pub(crate) fn items_inserted(&mut self, at: usize, count: usize) {
        if !self.enabled || count == 0 {
            return;
        }
        let page = self.page;
        for open in &mut self.open {
            if open.start_page == page && open.start_item >= at {
                open.start_item += count;
            }
        }
        for span in &mut self.spans {
            if span.start_page == page && span.start_item >= at {
                span.start_item += count;
            }
            if span.end_page == page && span.end_item >= at {
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
        debug_assert!(self.open.is_empty(), "a segment span was left open");
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
                let hi = if page_index == span.end_page {
                    span.end_item
                } else {
                    page.items.len()
                }
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

    /// The rows recorded so far.
    pub(crate) fn finish(self) -> SegmentMap {
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

/// The WCHAR offset of the extended-control character that points at `control_index`.
fn control_wchar_offset(para: &Paragraph, control_index: usize) -> Option<u32> {
    let mut offset = 0u32;
    for ch in &para.chars {
        if matches!(
            ch,
            HwpChar::ExtCtrl { ctrl_index: Some(index), .. } if *index as usize == control_index
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
        para.controls.push(Control::Generic(GenericControl {
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
        }));
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
