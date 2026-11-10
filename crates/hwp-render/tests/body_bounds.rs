//! The synthesized line cache and the renderer must agree on the body box (#423). Every number
//! here is page geometry or the paragraph shape's own line metrics, so no assertion depends on a
//! host font, on shaped glyph widths, or on a page count taken from a drawing pass.

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
/// anchors instead of the bare cursor. Enough rows to exceed the body box make it continue onto
/// the next page.
fn floating_table(rows: u16, row_height: i32) -> Table {
    Table {
        common_data: Vec::new(),
        placement: Some(GsoPlacement {
            treat_as_char: false,
            vert_rel_to: 2,
            horz_rel_to: 3,
            ..Default::default()
        }),
        attr: 1,
        rows,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1; usize::from(rows)],
        border_fill: BorderFillId(1),
        table_tail: Vec::new(),
        cells: (0..rows)
            .map(|row| Cell {
                list_attr: 0,
                col: 0,
                row,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(20000),
                height: HwpUnit(row_height),
                margins: [0; 4],
                border_fill: BorderFillId(1),
                header_tail: Vec::new(),
                paragraphs: vec![Paragraph::default()],
            })
            .collect(),
        caption: None,
        extras: Vec::new(),
    }
}

/// One section's page: paper size and the six margins the body box is derived from.
fn page_of(doc: &Document, section: usize) -> PageDef {
    doc.sections[section]
        .section_def()
        .and_then(|definition| definition.page)
        .expect("from_markdown writes a section page")
}

/// State a section's whole page box explicitly, so the body height is the paper less exactly the
/// bands a test names and nothing else.
fn set_page(doc: &mut Document, section: usize, paper_h: i32, header: i32, footer: i32) {
    for control in &mut doc.sections[section].paragraphs[0].controls {
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
fn body_height(doc: &Document, section: usize) -> i32 {
    let page = page_of(doc, section);
    page.height.0
        - page.margin_top.0
        - page.margin_bottom.0
        - page.margin_header.0
        - page.margin_footer.0
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
/// the cache has to reset at the same bottom the renderer uses, not at the paper height (#423):
/// the bands are not text space.
#[test]
fn synthesized_lines_stay_inside_the_body_box_with_header_and_footer_bands() {
    for (label, header, footer) in [
        ("both bands", BAND, BAND),
        ("header only", BAND, 0),
        ("footer only", 0, BAND),
    ] {
        let mut doc = hwp_convert::from_markdown("Anchor\n");
        set_page(&mut doc, 0, PAPER_H, header, footer);
        let chars = breaks(24);
        set_text(&mut doc.sections[0].paragraphs[0], chars);
        let body = body_height(&doc, 0);
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
    set_page(&mut doc, 0, PAPER_H, 0, 0);
    let chars = breaks(24);
    set_text(&mut doc.sections[0].paragraphs[0], chars);
    let body = body_height(&doc, 0);
    assert_eq!(
        body,
        page_of(&doc, 0).height.0,
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

/// A body whose second paragraph is 24 hard-broken lines after a floating table. The table makes
/// the section read prepass anchors, so that paragraph's cache starts where the planner measured
/// its first line rather than at the bare cursor. `table_rows` rows taller than the body box make
/// the table continue onto the next page before the paragraph starts.
fn floating_table_then_multiline(
    paper_h: i32,
    header: i32,
    footer: i32,
    table_rows: u16,
) -> Document {
    let mut doc = hwp_convert::from_markdown("Anchor\n\nText\n");
    set_page(&mut doc, 0, paper_h, header, footer);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(floating_table(table_rows, 1000)));
    let mut text = doc.sections[0].paragraphs[1].clone();
    set_text(&mut text, breaks(24));
    doc.sections[0].paragraphs[1] = text;
    doc.sections[0].paragraphs.truncate(2);
    doc
}

/// Every segment of the multiline paragraph after a floating table must carry the next slice of
/// the paragraph's text, and every page boundary must be a reset the body box justifies.
/// Checking the paragraph's first anchor alone cannot see this (#423).
fn assert_multiline_segments_after_floating_table(label: &str, mut doc: Document) {
    let body = body_height(&doc, 0);
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
    let mut previous = 0;
    for (index, seg) in segs.iter().enumerate() {
        if seg.v_pos < previous {
            boundaries += 1;
            assert_eq!(
                seg.v_pos, 0,
                "{label}: segment {index} opens a page but does not start at the body top"
            );
        }
        assert!(
            seg.v_pos + seg.line_height <= body,
            "{label}: segment {index} ends at {} beyond the body box {body}",
            seg.v_pos + seg.line_height
        );
        previous = seg.v_pos;
    }
    assert!(
        boundaries > 0,
        "{label}: the paragraph after the floating table must paginate"
    );
}

/// The anchored multiline paragraph with header and footer bands: no segment may reach into the
/// bands, so the cache paginates it inside the drawn body box (#423 in the anchored path).
#[test]
fn paginated_multiline_segments_after_a_floating_table_stay_inside_the_body_box() {
    assert_multiline_segments_after_floating_table(
        "bands",
        floating_table_then_multiline(PAPER_H, BAND, BAND, 1),
    );
}

/// The same paragraph on a page with no bands and no margins, so the body box is the whole paper:
/// the zero-band control for the anchored path, which the shared body box leaves unchanged.
#[test]
fn paginated_multiline_segments_after_a_floating_table_zero_band_control() {
    assert_multiline_segments_after_floating_table(
        "zero bands",
        floating_table_then_multiline(PAPER_H, 0, 0, 1),
    );
}

/// A floating table that has to continue onto the next page: the multiline paragraph then starts
/// after the continuation, and its cache must still stay page-relative inside the body box, with
/// one segment per hard break and no page opened twice (#423).
#[test]
fn paginated_multiline_segments_after_a_table_continuation_stay_inside_the_body_box() {
    let doc = floating_table_then_multiline(PAPER_H, BAND, BAND, 15);
    let body = body_height(&doc, 0);
    // Fifteen rows plus the table's own block padding are taller than one body box, so this
    // document can only place the paragraph after a continuation page.
    assert!(
        15 * 1000 + 566 > body,
        "fixture: the table is taller than the body box {body}, so it must continue"
    );
    assert_multiline_segments_after_floating_table("continuation", doc);
}

/// Every section paginates in its own body box: a band-free section followed by a banded one must
/// not share the first section's bottom (#423).
#[test]
fn each_section_paginates_in_its_own_body_box() {
    let mut doc = hwp_convert::from_markdown("Anchor\n");
    set_page(&mut doc, 0, PAPER_H, 0, 0);
    set_text(&mut doc.sections[0].paragraphs[0], breaks(24));
    let mut banded = doc.sections[0].clone();
    set_text(&mut banded.paragraphs[0], breaks(24));
    doc.sections.push(banded);
    set_page(&mut doc, 1, PAPER_H, BAND, BAND);
    let bodies = [body_height(&doc, 0), body_height(&doc, 1)];
    assert!(
        bodies[0] > bodies[1],
        "fixture: the banded section's body is the shorter one, {bodies:?}"
    );

    synthesize(&mut doc);
    for (section, body) in bodies.iter().copied().enumerate() {
        let segs = doc.sections[section].paragraphs[0].line_segs.clone();
        assert_eq!(segs.len(), 24, "section {section}: one line per hard break");
        for (index, seg) in segs.iter().enumerate() {
            assert!(
                seg.v_pos + seg.line_height <= body,
                "section {section}: line {index} ends at {} beyond its body box {body}",
                seg.v_pos + seg.line_height
            );
        }
        assert!(
            pages_in(&segs) > 1,
            "section {section}: the paragraph must paginate"
        );
    }
}
