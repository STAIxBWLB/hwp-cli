//! Cache synthesis and temporary table-cell geometry must share stable flow contracts.

#[path = "../../hwp-cli/tests/common/fixture_skip.rs"]
mod fixture_skip;

use hwp_model::{
    BorderFill, BorderFillId, Cell, Control, Document, GsoPlacement, HwpUnit, Paragraph, Table,
};
use hwp_render::display::{DisplayList, Item};
use hwp_render::{FontStore, RenderIssueAccumulator};

fn paragraph(doc: &Document) -> Paragraph {
    let mut para = doc.sections[0].paragraphs[0].clone();
    para.controls.clear();
    para.line_segs.clear();
    para
}

fn add_fill(doc: &mut Document, color: u32) -> BorderFillId {
    doc.header.border_fills.push(BorderFill {
        bg_color: Some(color),
        ..Default::default()
    });
    BorderFillId(doc.header.border_fills.len() as u16)
}

fn table(paragraphs: Vec<Paragraph>, fill: BorderFillId, height: i32) -> Table {
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
        inner_margins: [100; 4],
        row_cell_counts: vec![1],
        border_fill: fill,
        table_tail: Vec::new(),
        cells: vec![Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(20000),
            height: HwpUnit(height),
            margins: [100; 4],
            border_fill: fill,
            header_tail: Vec::new(),
            paragraphs,
        }],
        caption: None,
        extras: Vec::new(),
    }
}

fn synthesize(doc: &mut Document) {
    // No host font is needed: the assertions concern caches and row geometry,
    // and every source paragraph still has one line when no font resolves.
    let mut store = FontStore::new_isolated();
    let mut issues = RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(doc, &mut store, &mut issues);
}

fn layout(doc: &Document) -> DisplayList {
    let mut store = FontStore::new_isolated();
    let mut issues = RenderIssueAccumulator::new();
    hwp_render::layout::layout_document(doc, &mut store, &mut issues)
}

fn rectangles(list: &DisplayList, color: u32) -> Vec<(f32, f32, f32, f32)> {
    list.pages
        .iter()
        .flat_map(|page| &page.items)
        .filter_map(|item| match item {
            Item::Rect { x, y, w, h, fill } if *fill == color => Some((*x, *y, *w, *h)),
            _ => None,
        })
        .collect()
}

#[test]
fn floating_table_in_another_section_does_not_change_inline_caches() {
    let mut source = hwp_convert::from_markdown("Anchor\n\nFollowing paragraph\n");
    let fill = add_fill(&mut source, 0x0012_3456);
    let cell_para = paragraph(&source);
    let mut inline = table(vec![cell_para], fill, 1000);
    inline.placement.as_mut().unwrap().treat_as_char = true;
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(inline));
    let mut extended = source.clone();
    let mut appended = source.sections[0].clone();
    for control in &mut appended.paragraphs[0].controls {
        if let Control::Table(table) = control {
            table.placement.as_mut().unwrap().treat_as_char = false;
        }
    }
    extended.sections.push(appended);

    synthesize(&mut source);
    synthesize(&mut extended);
    for (original, with_other_section) in source.sections[0]
        .paragraphs
        .iter()
        .zip(&extended.sections[0].paragraphs)
    {
        assert_eq!(
            original.line_segs, with_other_section.line_segs,
            "a floating table in another section must not change inline-table advance"
        );
    }
}

#[test]
fn wholly_uncached_cells_keep_prior_nested_cache_convention() {
    let mut source = hwp_convert::from_markdown("Anchor\n");
    let fill_color = 0x0034_5678;
    let fill = add_fill(&mut source, fill_color);
    let mut cell_para = paragraph(&source);
    let shape_id = cell_para.para_shape.0 as usize;
    let shape = &mut source.header.para_shapes[shape_id];
    shape.margin_left = 2000; // 10pt, serialized at twice HWPUNIT.
    shape.margin_right = 4000; // 20pt.
    shape.spacing_top = 1000; // 5pt.
    shape.spacing_bottom = 1400; // 7pt.
    shape.line_spacing_type = 0;
    shape.line_spacing = 160;
    cell_para.line_segs.clear();
    let outer = table(vec![cell_para.clone(), cell_para], fill, 1);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(outer));

    let uncached = rectangles(&layout(&source), fill_color);
    let mut cached = source.clone();
    synthesize(&mut cached);
    let outer = cached.sections[0].paragraphs[0]
        .controls
        .iter()
        .find_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        outer.cells[0].height.0, 1,
        "cache synthesis must preserve the authored cell minimum for native output"
    );
    let first = &outer.cells[0].paragraphs[0].line_segs[0];
    let second = &outer.cells[0].paragraphs[1].line_segs[0];
    assert_eq!(first.col_start, 1000, "left paragraph margin must survive");
    assert_eq!(
        first.seg_width, 19800,
        "prior nested synthesis uses the supplied cell width without paragraph-margin narrowing"
    );
    assert_eq!(
        first.v_pos, 0,
        "prior nested synthesis does not apply body space-before to a cell"
    );
    assert_eq!(
        second.v_pos - first.v_pos,
        1600,
        "prior nested synthesis accumulates line advance without body paragraph gaps"
    );
    assert_eq!(uncached.len(), 1);
    assert!(
        (uncached[0].3 - 28.0).abs() < 0.01,
        "prior nested convention ends the row at the last line bottom: {uncached:?}"
    );
    let synthesized = rectangles(&layout(&cached), fill_color);
    assert_eq!(synthesized.len(), 1);
    assert!(
        (uncached[0].3 - synthesized[0].3).abs() < 0.01,
        "temporary and persisted cell geometry must have the same height"
    );
}

/// An empty paragraph inside a wholly cacheless cell list advances the renderer's flow by the
/// one line synthesis writes for it, not by a fixed height of its own (#422): the row the two
/// passes measure must agree, so following cell content does not move once the cache exists.
#[test]
fn cacheless_empty_cell_paragraph_advances_like_its_synthesized_line() {
    let mut source = hwp_convert::from_markdown("Anchor\n");
    let fill_color = 0x0034_5679;
    let fill = add_fill(&mut source, fill_color);
    let mut text = paragraph(&source);
    text.line_segs.clear();
    let mut empty = text.clone();
    empty.chars.clear();
    let cell = table(vec![text.clone(), empty, text], fill, 1);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(cell));

    let uncached = rectangles(&layout(&source), fill_color);
    let mut cached = source.clone();
    synthesize(&mut cached);
    let cell = cached.sections[0].paragraphs[0]
        .controls
        .iter()
        .find_map(|control| match control {
            Control::Table(table) => Some(&table.cells[0]),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        cell.paragraphs[1].line_segs.len(),
        1,
        "synthesis writes one line for the empty paragraph, so the flow cannot skip it"
    );
    let after_first = cell.paragraphs[0].line_segs[0]
        .v_pos
        .saturating_add(cell.paragraphs[0].line_segs[0].line_height);
    assert!(
        cell.paragraphs[1].line_segs[0].v_pos >= after_first,
        "the empty paragraph's line starts at the previous line's advance, not a fixed height"
    );
    let synthesized = rectangles(&layout(&cached), fill_color);
    assert_eq!(uncached.len(), 1);
    assert_eq!(synthesized.len(), 1);
    assert!(
        uncached[0].3 > 28.0,
        "the empty paragraph adds its own line to the two text lines: {uncached:?}"
    );
    assert!(
        (uncached[0].3 - synthesized[0].3).abs() < 0.01,
        "cacheless and synthesized cell geometry must agree: cacheless {uncached:?}, synthesized {synthesized:?}"
    );
}

/// The empty paragraph's line has to reach the measured row even when nothing follows it: a
/// trailing blank must not collapse the cell back to its single text line (#422 review P2).
#[test]
fn trailing_empty_cell_paragraph_contributes_its_line_to_the_row() {
    let mut source = hwp_convert::from_markdown("Anchor\n");
    let fill_color = 0x0034_567A;
    let fill = add_fill(&mut source, fill_color);
    let mut text = paragraph(&source);
    text.line_segs.clear();
    let mut empty = text.clone();
    empty.chars.clear();
    let cell = table(vec![text, empty], fill, 1);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(cell));

    let uncached = rectangles(&layout(&source), fill_color);
    let mut cached = source.clone();
    synthesize(&mut cached);
    let synthesized = rectangles(&layout(&cached), fill_color);
    assert_eq!(uncached.len(), 1);
    assert_eq!(synthesized.len(), 1);
    // One text line alone is 12.0pt here, so anything above it is the blank's own line.
    assert!(
        uncached[0].3 > 12.0,
        "the trailing empty paragraph keeps the row above the one text line: {uncached:?}"
    );
    assert!(
        synthesized[0].3 > 12.0,
        "a cached trailing blank contributes its line too: {synthesized:?}"
    );
    assert!(
        (uncached[0].3 - synthesized[0].3).abs() < 0.01,
        "cacheless and synthesized rows must agree: cacheless {uncached:?}, synthesized {synthesized:?}"
    );
}

/// A cell holding only an empty paragraph still owns that paragraph's one line: neither pass may
/// collapse the row to its margins and authored minimum (#422 review P2).
#[test]
fn empty_only_cell_paragraph_keeps_its_line_in_the_row() {
    let mut source = hwp_convert::from_markdown("Anchor\n");
    let fill_color = 0x0034_567C;
    let fill = add_fill(&mut source, fill_color);
    let mut empty = paragraph(&source);
    empty.chars.clear();
    let cell = table(vec![empty], fill, 1);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(cell));

    let uncached = rectangles(&layout(&source), fill_color);
    let mut cached = source.clone();
    synthesize(&mut cached);
    let synthesized = rectangles(&layout(&cached), fill_color);
    assert_eq!(uncached.len(), 1);
    assert_eq!(synthesized.len(), 1);
    // 2.0pt is the cell's own margins; the line adds the 10.0pt text line on top of it.
    assert!(
        uncached[0].3 > 2.0,
        "the only paragraph's line keeps the row above the bare margins: {uncached:?}"
    );
    assert!(
        synthesized[0].3 > 2.0,
        "a cached lone blank contributes its line too: {synthesized:?}"
    );
    assert!(
        (uncached[0].3 - synthesized[0].3).abs() < 0.01,
        "cacheless and synthesized rows must agree: cacheless {uncached:?}, synthesized {synthesized:?}"
    );
}

fn glyph_texts(list: &DisplayList) -> Vec<String> {
    list.pages
        .iter()
        .flat_map(|page| &page.items)
        .filter_map(|item| match item {
            Item::Glyphs { run, .. } => Some(run.text.clone()),
            _ => None,
        })
        .collect()
}

/// An empty paragraph in a numbered, wholly cache-less cell list must not consume a number: the
/// visible items keep `1.` and `2.` instead of skipping to `3.` (#422 review P2).
#[test]
fn empty_cell_paragraph_does_not_consume_a_list_number() {
    use hwp_model::{ParaShape, ParaShapeId};

    let mut source = hwp_convert::from_markdown("Anchor\n");
    let fill_color = 0x0034_567B;
    let fill = add_fill(&mut source, fill_color);
    let outline = ParaShapeId(source.header.para_shapes.len() as u16);
    source.header.para_shapes.push(ParaShape {
        attr1: (1 << 23) | (1 << 25),
        ..ParaShape::default()
    });
    let mut text = paragraph(&source);
    text.para_shape = outline;
    text.line_segs.clear();
    let mut empty = text.clone();
    empty.chars.clear();
    let cell = table(vec![text.clone(), empty, text], fill, 1);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(cell));

    let texts = glyph_texts(&layout(&source));
    if texts.is_empty() {
        fixture_skip::resource_unavailable(
            "host font for the cell list-marker precondition",
            "install a system font with digit glyphs",
        );
        return;
    }
    assert!(
        texts.iter().any(|text| text == "1."),
        "the first visible item keeps its number: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "2."),
        "the empty paragraph must not consume the second number: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == "3."),
        "no visible item reaches the third number: {texts:?}"
    );
}

#[test]
fn successive_uncached_nested_tables_keep_distinct_measured_heights() {
    let mut doc = hwp_convert::from_markdown("Anchor\n");
    // Isolate address-based height reuse from paragraph spacing (covered above).
    for shape in &mut doc.header.para_shapes {
        shape.spacing_top = 0;
        shape.spacing_bottom = 0;
    }
    let outer_fill = add_fill(&mut doc, 0x0056_789A);
    let short_color = 0x0067_89AB;
    let tall_color = 0x0078_9ABC;
    let short_fill = add_fill(&mut doc, short_color);
    let tall_fill = add_fill(&mut doc, tall_color);
    let leaf_fill = add_fill(&mut doc, 0x0089_ABCD);
    let mut paras = Vec::new();
    for (fill, height) in [(short_fill, 2000), (tall_fill, 7000)] {
        let mut inner_para = paragraph(&doc);
        inner_para
            .controls
            .push(Control::Table(table(Vec::new(), leaf_fill, height)));
        let inner = table(vec![inner_para], fill, 1);
        let mut outer_para = paragraph(&doc);
        outer_para.controls.push(Control::Table(inner));
        paras.push(outer_para);
    }
    let outer = table(paras, outer_fill, 1);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(outer));

    // Each layout measures and then draws both nested trees. The cell-address
    // memo must refer to those original trees throughout both passes.
    for _ in 0..3 {
        let display = layout(&doc);
        let short = rectangles(&display, short_color);
        let tall = rectangles(&display, tall_color);
        assert_eq!(short.len(), 1);
        assert_eq!(tall.len(), 1);
        assert!(
            (short[0].3 - 22.0).abs() < 0.01,
            "short nested table: {short:?}"
        );
        assert!(
            (tall[0].3 - 72.0).abs() < 0.01,
            "tall nested table: {tall:?}"
        );
        assert!(
            tall[0].1 >= short[0].1 + short[0].3,
            "successive nested tables must not overlap: {short:?}, {tall:?}"
        );
    }
}

#[test]
fn saved_cache_keeps_row_height_and_following_row_origin() {
    let mut doc = hwp_convert::from_markdown("Anchor\n");
    let color = 0x0001_2345;
    let fill = add_fill(&mut doc, color);
    let mut para = paragraph(&doc);
    doc.header.para_shapes[para.para_shape.0 as usize].spacing_bottom = 1400;
    para.line_segs = vec![hwp_model::LineSeg {
        text_start: 0,
        v_pos: 0,
        line_height: 1000,
        text_height: 1000,
        baseline_gap: 850,
        line_spacing: 600,
        col_start: 0,
        seg_width: 19800,
        flags: 0x0006_0000,
    }];
    let mut saved = table(vec![para], fill, 1200);
    saved.rows = 2;
    saved.row_cell_counts = vec![1, 1];
    let mut second = saved.cells[0].clone();
    second.row = 1;
    saved.cells.push(second);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(saved));
    let rows = rectangles(&layout(&doc), color);
    assert_eq!(rows.len(), 2);
    assert!(
        (rows[0].3 - 12.0).abs() < 0.01,
        "saved first row stays 12pt: {rows:?}"
    );
    assert!(
        (rows[1].3 - 12.0).abs() < 0.01,
        "saved second row stays 12pt: {rows:?}"
    );
    assert!(
        (rows[1].1 - rows[0].1 - 12.0).abs() < 0.01,
        "after-spacing must not move the following saved-cache row: {rows:?}"
    );
}

#[test]
fn first_line_pushed_after_floating_table_uses_resolved_page_anchor() {
    let mut source = hwp_convert::from_markdown("Anchor\n\nFollowing\n");
    for control in &mut source.sections[0].paragraphs[0].controls {
        if let Control::SectionDef(definition) = control {
            let page = definition.page.as_mut().unwrap();
            page.margin_header = HwpUnit(0);
            page.margin_footer = HwpUnit(0);
        }
    }
    let page = source.sections[0].section_def().unwrap().page.unwrap();
    let content_height = page.height.0 - page.margin_top.0 - page.margin_bottom.0;
    let fill = add_fill(&mut source, 0x0001_3456);
    // The row leaves 12pt: a 10pt cache line fits, but the fallback line's
    // baseline plus descent requires 16pt and must move to the next page.
    let floating = table(vec![Paragraph::default()], fill, content_height - 1200);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(floating));
    let address = hwp_render::segment_id::SegmentPath {
        section: 0,
        indices: vec![1],
    };
    let id = hwp_render::segment_id::paragraph_id(&address, &source.sections[0].paragraphs[1]);
    let mut store = FontStore::new();
    let mut issues = RenderIssueAccumulator::new();
    let following = &source.sections[0].paragraphs[1];
    let shaped = hwp_render::shape::shape_range(
        &mut store,
        &source,
        following,
        (0, following.wchar_len()),
        &mut issues,
    );
    if !shaped.iter().any(
        |item| matches!(item, hwp_render::shape::InlineItem::Run(run) if !run.glyphs.is_empty()),
    ) {
        fixture_skip::resource_unavailable(
            "host ASCII font for first-line pagination precondition",
            "install a system font with ASCII glyphs",
        );
        return;
    }
    let (_, source_map) =
        hwp_render::layout::layout_document_with_segments(&source, &mut store, &mut issues);
    let source_pages: Vec<_> = source_map
        .rows
        .iter()
        .filter(|row| row.id == id)
        .map(|row| row.page)
        .collect();
    assert_eq!(
        source_pages,
        vec![1],
        "fixture's first line must move to a new page"
    );

    let mut cached = source.clone();
    hwp_render::lineseg::synthesize_linesegs(&mut cached, &mut store, &mut issues);
    let anchor = &cached.sections[0].paragraphs[1].line_segs[0];
    assert_eq!(
        anchor.v_pos, 0,
        "synthesized anchor must use the resolved first-line top, not the near-bottom entry cursor"
    );
    assert_ne!(
        anchor.flags & 1,
        0,
        "resolved first-line page break must be explicit in the cache"
    );
    let (_, cached_map) =
        hwp_render::layout::layout_document_with_segments(&cached, &mut store, &mut issues);
    let cached_pages: Vec<_> = cached_map
        .rows
        .iter()
        .filter(|row| row.id == id)
        .map(|row| row.page)
        .collect();
    assert_eq!(
        cached_pages, source_pages,
        "conversion must retain the resolved page anchor"
    );
}

#[test]
fn explicit_break_keeps_prior_space_before_in_both_section_paths() {
    // Keep the prior positive space-before on each path. Their pre-existing
    // raw-vs-half-unit discrepancy remains tracked by #415.
    for floating in [false, true] {
        let mut source = hwp_convert::from_markdown("Anchor\n\nHeading\n");
        let mut heading_shape = source.header.para_shapes
            [source.sections[0].paragraphs[1].para_shape.0 as usize]
            .clone();
        heading_shape.spacing_top = 2400;
        source.header.para_shapes.push(heading_shape);
        source.sections[0].paragraphs[1].para_shape =
            hwp_model::ParaShapeId((source.header.para_shapes.len() - 1) as u16);
        source.sections[0].paragraphs[1].header.break_type |= 0x04;
        let fill = add_fill(&mut source, 0x0002_3456);
        let mut object = table(Vec::new(), fill, 1200);
        object.placement.as_mut().unwrap().treat_as_char = !floating;
        source.sections[0].paragraphs[0]
            .controls
            .push(Control::Table(object));
        let mut zero_before = source.clone();
        zero_before
            .header
            .para_shapes
            .last_mut()
            .unwrap()
            .spacing_top = 0;
        synthesize(&mut zero_before);
        assert_eq!(
            zero_before.sections[0].paragraphs[1].line_segs[0].v_pos, 0,
            "zero space-before control starts at the reset cursor (floating={floating})"
        );
        synthesize(&mut source);
        assert_eq!(
            source.sections[0].paragraphs[1].line_segs[0].v_pos,
            if floating { 1200 } else { 2400 },
            "explicit break resets the cursor but keeps the path's prior space-before (floating={floating})"
        );
    }
}

#[test]
fn floating_section_first_paragraph_omits_only_its_space_before() {
    let mut source = hwp_convert::from_markdown("First\n\nFollowing\n");
    let mut first_shape =
        source.header.para_shapes[source.sections[0].paragraphs[0].para_shape.0 as usize].clone();
    first_shape.spacing_top = 2400;
    source.header.para_shapes.push(first_shape);
    source.sections[0].paragraphs[0].para_shape =
        hwp_model::ParaShapeId((source.header.para_shapes.len() - 1) as u16);
    let fill = add_fill(&mut source, 0x0003_4567);
    source.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(table(Vec::new(), fill, 1200)));
    source.sections.push(source.sections[0].clone());
    for mut store in [FontStore::new_isolated(), FontStore::new()] {
        let mut cached = source.clone();
        let mut issues = RenderIssueAccumulator::new();
        hwp_render::lineseg::synthesize_linesegs(&mut cached, &mut store, &mut issues);
        for section in &cached.sections {
            assert_eq!(
                section.paragraphs[0].line_segs[0].v_pos, 0,
                "B5: every section's first body paragraph starts at zero despite space-before"
            );
        }
    }
}

#[test]
fn native_floating_edit_restarts_the_base_cursor_at_an_explicit_break() {
    let mut doc = hwp_convert::from_markdown("First\n\nEdited\n");
    doc.meta.source_format = "hwp5".into();
    for shape in &mut doc.header.char_shapes {
        shape.base_size = 1000;
    }
    for shape in &mut doc.header.para_shapes {
        shape.spacing_top = 0;
        shape.spacing_bottom = 0;
        shape.line_spacing_type = 0;
        shape.line_spacing = 160;
        shape.line_spacing_old = 160;
    }
    for (index, para) in doc.sections[0].paragraphs.iter_mut().enumerate() {
        para.line_segs = vec![hwp_model::LineSeg {
            text_start: 0,
            v_pos: if index == 0 { 0 } else { 1766 },
            line_height: 1000,
            text_height: 1000,
            baseline_gap: 850,
            line_spacing: 600,
            col_start: 0,
            seg_width: 20000,
            flags: 0x0006_0000,
        }];
    }
    let fill = add_fill(&mut doc, 0x0004_5678);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(table(
            vec![Paragraph::default()],
            fill,
            1200,
        )));
    let changed = &mut doc.sections[0].paragraphs[1];
    changed.chars.insert(0, hwp_model::HwpChar::Text('X'));
    changed.header.break_type |= 0x04;
    changed.line_segs.clear();
    synthesize(&mut doc);
    // Genuine Hangul-saved HWP5 files (#420, 07-hangul-compat-rules J1): a body paragraph that
    // carries the break bit starts its first line at v_pos 0 (or its space-before), also after
    // a floating table, and sets no page-start flag. The earlier expectation of the running
    // cursor (1766) had no byte evidence behind it.
    assert_eq!(
        doc.sections[0].paragraphs[1].line_segs[0].v_pos, 0,
        "native edits restart the cursor at an explicit break, like genuine caches"
    );
    assert_eq!(
        doc.sections[0].paragraphs[1].line_segs[0].flags & 1,
        0,
        "the native base path sets no cacheless page-start flag"
    );
}
