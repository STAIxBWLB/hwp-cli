//! The synthesized line cache and the renderer must agree on the body box and on the per-line
//! fit block (#423). Every number here is page geometry or the paragraph shape's own line
//! metrics, so no assertion depends on a host font, on shaped glyph widths, or on a page count
//! taken from a drawing pass.

use hwp_model::{
    BorderFillId, Cell, Control, Document, GsoPlacement, HwpChar, HwpUnit, PageDef, Paragraph,
    Table,
};
use hwp_render::{FontStore, RenderIssueAccumulator};

/// Paper height of the test pages in HWPUNIT (200 pt). Small enough that a two-dozen-line
/// paragraph paginates several times, so every reset and every bound is exercised.
const PAPER_H: i32 = 20000;

/// A header or footer band of 40 pt. It moves the body bottom without moving the paper, which is
/// exactly the difference between the two body-box definitions before #423.
const BAND: i32 = 4000;

/// Text of `lines` characters separated by hard line breaks (CharCtrl 10). Each break commits a
/// line regardless of width, so the line count comes from the paragraph, not from shaping.
fn breaks(lines: usize) -> Vec<HwpChar> {
    let mut chars = Vec::with_capacity(lines * 2);
    for index in 0..lines {
        if index > 0 {
            chars.push(HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK));
        }
        chars.push(HwpChar::Text('x'));
    }
    chars
}

/// Install `chars` on one paragraph, keeping its char shape so the paragraph's own metrics (base
/// size, line spacing) drive the layout, and drop any cache so synthesis has to write one.
fn set_text(para: &mut Paragraph, chars: Vec<HwpChar>) {
    let shape = para
        .char_shape_runs
        .first()
        .map_or(hwp_model::CharShapeId(0), |&(_, id)| id);
    para.chars = chars;
    para.char_shape_runs = vec![(0, shape)];
    para.line_segs.clear();
}

/// A body-flow table (`treat_as_char == false`), the shape that makes a section read prepass
/// anchors instead of the bare cursor.
fn floating_table(height: i32) -> Table {
    Table {
        common_data: Vec::new(),
        placement: Some(GsoPlacement {
            treat_as_char: false,
            vert_rel_to: 2,
            horz_rel_to: 3,
            ..Default::default()
        }),
        attr: 1,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1],
        border_fill: BorderFillId(1),
        table_tail: Vec::new(),
        cells: vec![Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(20000),
            height: HwpUnit(height),
            margins: [0; 4],
            border_fill: BorderFillId(1),
            header_tail: Vec::new(),
            paragraphs: vec![Paragraph::default()],
        }],
        caption: None,
        extras: Vec::new(),
    }
}

/// The section's page: paper size and the six margins the body box is derived from.
fn page_of(doc: &Document) -> PageDef {
    doc.sections[0]
        .section_def()
        .and_then(|definition| definition.page)
        .expect("from_markdown writes a section page")
}

/// State the whole page box explicitly, so the body height is the paper less exactly the bands a
/// test names and nothing else.
fn set_page(doc: &mut Document, paper_h: i32, header: i32, footer: i32) {
    for control in &mut doc.sections[0].paragraphs[0].controls {
        let Control::SectionDef(definition) = control else {
            continue;
        };
        let page = definition
            .page
            .as_mut()
            .expect("from_markdown defines a page");
        page.height = HwpUnit(paper_h);
        page.margin_top = HwpUnit(0);
        page.margin_bottom = HwpUnit(0);
        page.margin_header = HwpUnit(header);
        page.margin_footer = HwpUnit(footer);
        page.margin_left = HwpUnit(0);
        page.margin_right = HwpUnit(0);
    }
}

/// Body height as the renderer derives it: the paper less both margins and both bands.
fn body_height(doc: &Document) -> i32 {
    let page = page_of(doc);
    page.height.0
        - page.margin_top.0
        - page.margin_bottom.0
        - page.margin_header.0
        - page.margin_footer.0
}

/// The block the fallback planner reserves for one line: a baseline 1.2 x size below the line top
/// plus a 0.4 x size descent, both fixed in `layout.rs` and mirrored for the cache by
/// `lineseg::line_fit_block`.
fn line_block(line_height: i32) -> i32 {
    line_height * 12 / 10 + line_height * 4 / 10
}

fn synthesize(doc: &mut Document) {
    let mut store = FontStore::new_isolated();
    let mut issues = RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(doc, &mut store, &mut issues);
}

/// How many pages the cache opens: every v_pos restart after a line that had advanced.
fn pages_in(segs: &[hwp_model::LineSeg]) -> usize {
    let mut pages = 1;
    let mut previous = 0;
    for seg in segs {
        if seg.v_pos < previous {
            pages += 1;
        }
        previous = seg.v_pos;
    }
    pages
}

/// A synthesized line may never sit below the drawn body box. With header and/or footer bands
/// the cache has to reset at the same bottom the renderer uses, not at the paper height (#423
/// part 1): the bands are not text space.
#[test]
fn synthesized_lines_stay_inside_the_body_box_with_header_and_footer_bands() {
    for (label, header, footer) in [
        ("both bands", BAND, BAND),
        ("header only", BAND, 0),
        ("footer only", 0, BAND),
    ] {
        let mut doc = hwp_convert::from_markdown("Anchor\n");
        set_page(&mut doc, PAPER_H, header, footer);
        let chars = breaks(24);
        set_text(&mut doc.sections[0].paragraphs[0], chars);
        let body = body_height(&doc);
        synthesize(&mut doc);
        let segs = doc.sections[0].paragraphs[0].line_segs.clone();
        assert_eq!(segs.len(), 24, "{label}: one line per hard break");
        for (index, seg) in segs.iter().enumerate() {
            assert!(
                seg.v_pos + seg.line_height <= body,
                "{label}: line {index} ends at {} but the body box ends at {body} \
                 (v_pos {}, header {header}, footer {footer})",
                seg.v_pos + seg.line_height,
                seg.v_pos
            );
        }
        assert!(
            pages_in(&segs) > 1,
            "{label}: the paragraph must paginate, but the cache reports {} page",
            pages_in(&segs)
        );
    }
}

/// The zero-band control: without header or footer bands the body box is the margin-only box, so
/// this rule changed nothing and the cache keeps paginating the paragraph exactly as it did.
#[test]
fn zero_band_control_keeps_the_margin_only_body_box() {
    let mut doc = hwp_convert::from_markdown("Anchor\n");
    set_page(&mut doc, PAPER_H, 0, 0);
    let chars = breaks(24);
    set_text(&mut doc.sections[0].paragraphs[0], chars);
    let body = body_height(&doc);
    assert_eq!(
        body,
        page_of(&doc).height.0,
        "control fixture: with no margins the body box is the whole paper"
    );
    synthesize(&mut doc);
    let segs = doc.sections[0].paragraphs[0].line_segs.clone();
    assert_eq!(segs.len(), 24, "one line per hard break");
    for (index, seg) in segs.iter().enumerate() {
        assert!(
            seg.v_pos + seg.line_height <= body,
            "control: line {index} ends at {} beyond the body box {body}",
            seg.v_pos + seg.line_height
        );
    }
    assert!(
        pages_in(&segs) > 1,
        "the control paragraph must still paginate, but reports {} page",
        pages_in(&segs)
    );
}

/// The cache reserves the same per-line block the fallback planner does (#423 part 2). A line
/// whose glyph height fits but whose baseline-plus-descent does not is one the planner moves to
/// the next page, so the cache has to move it too; the guard below proves a height-only rule
/// would have kept it.
#[test]
fn synthesized_lines_reserve_the_planners_line_block() {
    let mut doc = hwp_convert::from_markdown("Anchor\n");
    // Body 18900 HWPUNIT sits between the two candidates for the line at 17600: its glyphs end
    // at 18600 (inside the body) while its block ends at 19200 (outside), which is the window
    // where the two rules disagree.
    set_page(&mut doc, 18900, 0, 0);
    let chars = breaks(24);
    set_text(&mut doc.sections[0].paragraphs[0], chars);
    let body = body_height(&doc);
    synthesize(&mut doc);
    let segs = doc.sections[0].paragraphs[0].line_segs.clone();
    assert_eq!(segs.len(), 24, "one line per hard break");

    for (index, seg) in segs.iter().enumerate() {
        if seg.v_pos == 0 {
            continue;
        }
        assert!(
            seg.v_pos + line_block(seg.line_height) <= body,
            "line {index} reserves {} at v_pos {} and ends at {} beyond the body box {body}: the \
             fallback planner breaks the page before this line",
            line_block(seg.line_height),
            seg.v_pos,
            seg.v_pos + line_block(seg.line_height)
        );
    }

    // Non-vacuity: some page must end on a line a height-only rule would have kept, so the
    // assertion above is exercising the difference and not a paragraph that never reaches the
    // bottom of the body.
    let mut guarded = 0;
    for pair in segs.windows(2) {
        let (last, next) = (&pair[0], &pair[1]);
        if next.v_pos < last.v_pos && last.v_pos > 0 {
            assert_eq!(next.v_pos, 0, "a new page starts at the body top");
            let advance = last.line_spacing + last.line_height;
            assert!(
                last.v_pos + advance + last.line_height <= body,
                "page break after v_pos {} does not demonstrate the block rule: the next line's \
                 glyphs would already have crossed the body box {body}",
                last.v_pos
            );
            guarded += 1;
        }
    }
    assert!(
        guarded > 0,
        "the paragraph must break a page at a line the glyph-height rule would have kept"
    );
}

/// A body whose second paragraph is 24 hard-broken lines after a floating table. The table makes
/// the section read prepass anchors, so that paragraph's cache starts where the planner measured
/// its first line rather than at the bare cursor.
fn floating_table_then_multiline(paper_h: i32, header: i32, footer: i32) -> Document {
    let mut doc = hwp_convert::from_markdown("Anchor\n\nText\n");
    set_page(&mut doc, paper_h, header, footer);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(floating_table(1000)));
    let mut text = doc.sections[0].paragraphs[1].clone();
    set_text(&mut text, breaks(24));
    doc.sections[0].paragraphs[1] = text;
    doc.sections[0].paragraphs.truncate(2);
    doc
}

/// Every segment of the multiline paragraph after a floating table must carry the next slice of
/// the paragraph's text, and every page boundary must be a reset the body box justifies.
/// Checking the paragraph's first anchor alone cannot see this (#423). `block_guard` also demands
/// proof that some break happens at a line a glyph-height-only rule would have kept, which only a
/// run whose two body boxes agree can state.
fn assert_multiline_segments_after_floating_table(
    label: &str,
    mut doc: Document,
    block_guard: bool,
) {
    let body = body_height(&doc);
    let total = doc.sections[0].paragraphs[1].wchar_len();

    synthesize(&mut doc);
    let segs = doc.sections[0].paragraphs[1].line_segs.clone();
    assert_eq!(
        segs.len(),
        24,
        "{label}: one line per hard break, with no extra line invented after the table"
    );

    // Every segment owns the text up to the next segment's start, in order, with none skipped:
    // the paragraph's text is preserved across the page resets.
    assert_eq!(
        segs[0].text_start, 0,
        "{label}: the first line starts at the text"
    );
    for (index, pair) in segs.windows(2).enumerate() {
        assert!(
            pair[1].text_start > pair[0].text_start,
            "{label}: segment {index} carries no text: starts {} and {}",
            pair[0].text_start,
            pair[1].text_start
        );
    }
    assert_eq!(
        segs.last().map(|seg| seg.text_start),
        Some(total - 1),
        "{label}: the last line starts at the paragraph's final character"
    );

    // Page boundaries: a restart is a reset to the body top, and every line stays inside the body
    // box the planner drew the anchors in.
    let mut boundaries = 0;
    let mut guarded = 0;
    let mut previous = 0;
    for (index, seg) in segs.iter().enumerate() {
        if seg.v_pos < previous {
            boundaries += 1;
            assert_eq!(
                seg.v_pos, 0,
                "{label}: segment {index} opens a page but does not start at the body top"
            );
            let last = &segs[index - 1];
            let advance = last.line_spacing + last.line_height;
            if last.v_pos + advance + last.line_height <= body {
                guarded += 1;
            }
        }
        assert!(
            seg.v_pos + seg.line_height <= body,
            "{label}: segment {index} ends at {} beyond the body box {body}",
            seg.v_pos + seg.line_height
        );
        if seg.v_pos > 0 {
            assert!(
                seg.v_pos + line_block(seg.line_height) <= body,
                "{label}: segment {index} reserves a block ending at {} beyond the body box {body}",
                seg.v_pos + line_block(seg.line_height)
            );
        }
        previous = seg.v_pos;
    }
    assert!(
        boundaries > 0,
        "{label}: the paragraph after the floating table must paginate"
    );
    if block_guard {
        assert!(
            guarded > 0,
            "{label}: no page breaks at a line a glyph-height-only rule would have kept, so the \
             block assertion above is vacuous"
        );
    }
}

/// The anchored multiline paragraph with header and footer bands: no segment may reach into the
/// bands, so the cache paginates it inside the drawn body box (#423 part 1 in the anchored path).
#[test]
fn paginated_multiline_segments_after_a_floating_table_stay_inside_the_body_box() {
    assert_multiline_segments_after_floating_table(
        "bands",
        floating_table_then_multiline(PAPER_H, BAND, BAND),
        false,
    );
}

/// The same paragraph without bands, on a page whose body ends inside the window where the two fit
/// rules disagree: with the two body boxes agreeing, this run isolates the per-line block, so a
/// segment may not start where the planner would have broken the page (#423 part 2 in the anchored
/// path). The 19200 body ends between the anchored line at 17900 (glyphs end at 18900, inside) and
/// its block (19500, outside).
#[test]
fn paginated_multiline_segments_after_a_floating_table_reserve_the_planners_line_block() {
    assert_multiline_segments_after_floating_table(
        "zero bands",
        floating_table_then_multiline(19200, 0, 0),
        true,
    );
}
