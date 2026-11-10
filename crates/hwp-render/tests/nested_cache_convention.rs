//! Pin the prior nested-cache convention without claiming new Hangul geometry evidence.

use hwp_model::{
    BinRef, BorderFillId, Caption, CaptionDirection, CaptionSide, Cell, Control, Document,
    GenericControl, GsoPlacement, HwpUnit, Paragraph, ParagraphList, Picture, ShapeGeom, ShapeKind,
    Table,
};
use hwp_render::{FontStore, RenderIssueAccumulator};

const CAPTION_WIDTH: i32 = 15000;
const CELL_WIDTH: i32 = 20000;
const TEXT_BOX_WIDTH: i32 = 12000;
const ADVANCE: i32 = 1200;

fn document() -> Document {
    let mut doc = hwp_convert::from_markdown("Nested cache host\n");
    for shape in &mut doc.header.char_shapes {
        shape.base_size = 1000;
    }
    for shape in &mut doc.header.para_shapes {
        shape.margin_left = 2000;
        shape.margin_right = 4000;
        shape.spacing_top = 1000;
        shape.spacing_bottom = 1400;
        shape.line_spacing_type = 0;
        shape.line_spacing = 120;
        shape.line_spacing_old = 120;
    }
    doc
}

fn paragraphs(doc: &Document) -> Vec<Paragraph> {
    let mut first = doc.sections[0].paragraphs[0].clone();
    first.controls.clear();
    first.line_segs.clear();
    vec![first.clone(), first]
}

fn caption(paragraphs: Vec<Paragraph>) -> Caption {
    Caption {
        side: CaptionSide::Bottom,
        direction: CaptionDirection::Horizontal,
        gap: 200,
        width: Some(CAPTION_WIDTH as u32),
        last_width: CAPTION_WIDTH as u32,
        paragraphs,
    }
}

fn table(paragraphs: Vec<Paragraph>) -> Table {
    Table {
        common_data: Vec::new(),
        // Keep the test on nested synthesis, without the floating-body measurement pass.
        placement: Some(GsoPlacement {
            treat_as_char: true,
            ..Default::default()
        }),
        attr: 0,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [100; 4],
        row_cell_counts: vec![1],
        border_fill: BorderFillId(0),
        table_tail: Vec::new(),
        cells: vec![Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(CELL_WIDTH),
            height: HwpUnit(1000),
            margins: [100; 4],
            border_fill: BorderFillId(0),
            header_tail: Vec::new(),
            paragraphs,
        }],
        caption: None,
        extras: Vec::new(),
    }
}

fn picture(paragraphs: Vec<Paragraph>) -> Picture {
    Picture {
        common_data: Vec::new(),
        width: HwpUnit(5000),
        height: HwpUnit(2000),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: None,
        crop: None,
        flip: 0,
        rotation: None,
        brightness: 0,
        contrast: 0,
        effect_flags: 0,
        effects_raw: Vec::new(),
        caption: Some(caption(paragraphs)),
        // No picture is rendered or written; only its caption cache is synthesized.
        bin_ref: BinRef::ItemRef("synthetic-unrendered-picture".to_string()),
        extras: Vec::new(),
    }
}

fn text_box(paragraphs: Vec<Paragraph>) -> GenericControl {
    GenericControl {
        ctrl_id: *b"gso ",
        data: Vec::new(),
        paragraph_lists: vec![ParagraphList {
            header_data: Vec::new(),
            paragraphs,
        }],
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: vec![ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w: TEXT_BOX_WIDTH,
            h: 5000,
            points: Vec::new(),
            fill: 0xFFFF_FFFF,
            fill_gradient: None,
            border_color: 0,
            border_width: 10,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: true,
            description: None,
            image_fill_dropped: false,
        }],
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    }
}

fn synthesize(doc: &mut Document) {
    let mut store = FontStore::new_isolated();
    let mut warnings = RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(doc, &mut store, &mut warnings);
}

fn assert_prior_convention(paras: &[Paragraph], width: i32, context: &str) {
    assert_eq!(paras.len(), 2, "{context}: visit both authored paragraphs");
    for (index, para) in paras.iter().enumerate() {
        assert_eq!(
            para.line_segs.len(),
            1,
            "{context}: isolated one-line cache"
        );
        let seg = &para.line_segs[0];
        assert_eq!(
            seg.v_pos,
            index as i32 * ADVANCE,
            "{context}: prior convention starts at zero and advances only by line metrics"
        );
        assert_eq!(
            seg.seg_width, width,
            "{context}: retain the full supplied width rather than subtracting paragraph margins"
        );
        assert_eq!(
            seg.col_start, 1000,
            "{context}: preserve legacy margin_left/2"
        );
        assert_eq!(seg.line_height, 1000, "{context}: preserve base height");
        assert_eq!(
            seg.line_spacing, 200,
            "{context}: preserve declared 120% advance"
        );
    }
}

fn last_control(doc: &Document) -> &Control {
    doc.sections[0].paragraphs[0].controls.last().unwrap()
}

#[test]
fn table_cells_keep_prior_nested_cache_geometry() {
    let mut doc = document();
    let control = Control::Table(table(paragraphs(&doc)));
    doc.sections[0].paragraphs[0].controls.push(control);
    synthesize(&mut doc);
    let Control::Table(table) = last_control(&doc) else {
        panic!("table")
    };
    assert_prior_convention(&table.cells[0].paragraphs, CELL_WIDTH - 200, "table cell");
}

#[test]
fn table_captions_keep_prior_nested_cache_geometry() {
    let mut doc = document();
    let mut control = table(paragraphs(&doc));
    control.caption = Some(caption(paragraphs(&doc)));
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(control));
    synthesize(&mut doc);
    let Control::Table(table) = last_control(&doc) else {
        panic!("table")
    };
    assert_prior_convention(
        &table.caption.as_ref().unwrap().paragraphs,
        CAPTION_WIDTH,
        "table caption",
    );
}

#[test]
fn picture_captions_keep_prior_nested_cache_geometry() {
    let mut doc = document();
    let control = Control::Picture(picture(paragraphs(&doc)));
    doc.sections[0].paragraphs[0].controls.push(control);
    synthesize(&mut doc);
    let Control::Picture(picture) = last_control(&doc) else {
        panic!("picture")
    };
    assert_prior_convention(
        &picture.caption.as_ref().unwrap().paragraphs,
        CAPTION_WIDTH,
        "picture caption",
    );
}

#[test]
fn text_box_lists_keep_prior_nested_cache_geometry() {
    let mut doc = document();
    let control = Control::Generic(text_box(paragraphs(&doc)));
    doc.sections[0].paragraphs[0].controls.push(control);
    synthesize(&mut doc);
    let Control::Generic(generic) = last_control(&doc) else {
        panic!("text box")
    };
    assert_prior_convention(
        &generic.paragraph_lists[0].paragraphs,
        TEXT_BOX_WIDTH - 566,
        "text-box list",
    );
}

#[test]
fn generic_captions_keep_prior_nested_cache_geometry() {
    let mut doc = document();
    let mut control = text_box(paragraphs(&doc));
    control.caption = Some(caption(paragraphs(&doc)));
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(control));
    synthesize(&mut doc);
    let Control::Generic(generic) = last_control(&doc) else {
        panic!("generic")
    };
    assert_prior_convention(
        &generic.caption.as_ref().unwrap().paragraphs,
        CAPTION_WIDTH,
        "generic caption",
    );
}

#[test]
fn picture_caption_nested_in_table_caption_keeps_its_own_cursor() {
    let mut doc = document();
    let mut outer_caption = caption(paragraphs(&doc));
    outer_caption.paragraphs[0]
        .controls
        .push(Control::Picture(picture(paragraphs(&doc))));
    let mut control = table(paragraphs(&doc));
    control.caption = Some(outer_caption);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(control));
    synthesize(&mut doc);
    let Control::Table(table) = last_control(&doc) else {
        panic!("table")
    };
    let outer = table.caption.as_ref().unwrap();
    assert_prior_convention(&outer.paragraphs, CAPTION_WIDTH, "outer caption");
    let Control::Picture(picture) = &outer.paragraphs[0].controls[0] else {
        panic!("nested picture")
    };
    assert_prior_convention(
        &picture.caption.as_ref().unwrap().paragraphs,
        CAPTION_WIDTH,
        "nested picture caption",
    );
}

fn assert_native_cell_width(cell_margins: [u16; 4], table_margins: [u16; 4], expected: i32) {
    let mut doc = document();
    let mut object = table(paragraphs(&doc));
    object.inner_margins = table_margins;
    object.cells[0].margins = cell_margins;
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Table(object));
    synthesize(&mut doc);
    let Control::Table(object) = last_control(&doc) else {
        panic!("table")
    };
    assert_eq!(
        object.cells[0].margins, cell_margins,
        "preserve raw cell margin bytes"
    );
    assert_eq!(
        object.inner_margins, table_margins,
        "preserve raw table margin bytes"
    );
    assert_eq!(
        object.cells[0].paragraphs[0].line_segs[0].seg_width, expected,
        "native width matches base raw-margin arithmetic, including sentinels"
    );
}

#[test]
fn native_all_zero_cell_margins_keep_full_width() {
    assert_native_cell_width([0; 4], [100; 4], CELL_WIDTH);
}

#[test]
fn native_cell_margin_sentinel_keeps_base_raw_width_with_nonzero_table_margins() {
    assert_native_cell_width([u16::MAX, u16::MAX, 100, 100], [100, 200, 100, 100], 1);
}

#[test]
fn native_cell_margin_sentinel_keeps_base_raw_width_with_zero_table_margins() {
    assert_native_cell_width([u16::MAX, u16::MAX, 100, 100], [0; 4], 1);
}

#[test]
fn native_cell_and_table_margin_sentinels_keep_base_raw_width() {
    assert_native_cell_width([0, u16::MAX, 100, 100], [u16::MAX; 4], 1);
}
