//! The segment id rule has **two implementations by design**, and this file is the only thing
//! that makes them one rule.
//!
//! `hwp-convert` owns one (`crates/hwp-convert/src/segment_id.rs`) and `hwp-render` owns the
//! other (`crates/hwp-render/src/segment_id.rs`). Neither crate may depend on the other
//! (CLAUDE.md invariant 1), and the owner chose duplication over relocating the rule to a
//! shared crate. **Nothing at compile time makes the two agree.**
//!
//! `hwp cat --segments v2` publishes ids from the first and the render side's geometry rows
//! publish ids from the second, and the published contract says those ids are the join key
//! between the two artifacts — an editor persists them across sessions. This test is the only
//! thing that makes that claim true. Every other test in the phase exercises one crate alone
//! and passes whether or not the two agree.
//!
//! It lives in `crates/hwp-cli/tests/` because `hwp-cli` is the only crate that already depends
//! on both. Putting it in either producer would need exactly the crate edge the invariant
//! forbids. Do not move it, and do not reduce the seven cases to a loop over a list: seven
//! named tests fail loudly and individually, while a loop over six entries looks the same as a
//! loop over seven.
//!
//! Each case builds **one** IR value and hands that same value to both implementations. Two
//! separately built documents that look alike can differ in exactly the bytes a divergence
//! would hash, so comparing across them would prove nothing.

use hwp_model::control::{BinRef, Cell, GenericControl, Picture, Table};
use hwp_model::ids::CharShapeId;
use hwp_model::paragraph::{HwpChar, Paragraph, ctrl_char};
use hwp_model::units::HwpUnit;

/// The same positional path on both sides. The path is not IR — each crate has its own
/// `SegmentPath` type with public fields and no constructor — so it is the one thing built
/// twice, from the same literals.
fn paths(
    section: usize,
    indices: &[usize],
) -> (
    hwp_convert::SegmentPath,
    hwp_render::segment_id::SegmentPath,
) {
    (
        hwp_convert::SegmentPath {
            section,
            indices: indices.to_vec(),
        },
        hwp_render::segment_id::SegmentPath {
            section,
            indices: indices.to_vec(),
        },
    )
}

/// A paragraph built so that **every** normalization in the rule is load-bearing on it. An
/// input a normalization does not touch makes the case that depends on it vacuous: it would
/// pass against an implementation that omitted the normalization entirely.
///
/// - `(0, CharShapeId(9))` before `(0, CharShapeId(1))` is two runs at one position, which the
///   HWPX reader collapses by overwriting and the HWP5 reader does not. Drop the
///   same-position arm and this paragraph's ids part company.
/// - `(1, CharShapeId(1))` repeats the previous shape id, which the HWP5 reader stores and the
///   HWPX reader suppresses.
/// - the trailing `CharCtrl(13)` is HWP5's `PARA_TEXT` terminator, which HWPX does not store.
///   It matters to the **last run's** slice as well as to the paragraph's character list, so
///   without it a `run_chars` that forgot to canonicalize would look identical on both sides.
fn paragraph_with_a_redundant_run() -> Paragraph {
    let mut para = Paragraph {
        chars: "가나다라".chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![
            (0, CharShapeId(9)), // same position as the next entry: the last one wins
            (0, CharShapeId(1)),
            (1, CharShapeId(1)), // redundant: the canonicalization must drop this
            (2, CharShapeId(2)),
        ],
        ..Default::default()
    };
    para.chars.push(HwpChar::CharCtrl(ctrl_char::PARA_BREAK));
    para
}

/// A paragraph carrying the trailing `CharCtrl(13)` paragraph-break terminator HWP5's
/// `PARA_TEXT` stores and HWPX's `<hp:t>` does not, so the character canonicalization is
/// actually exercised.
fn paragraph_with_a_trailing_break() -> Paragraph {
    let mut para = Paragraph {
        chars: "문단 내용".chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![(0, CharShapeId(3))],
        ..Default::default()
    };
    para.chars.push(HwpChar::CharCtrl(ctrl_char::PARA_BREAK));
    para
}

fn cell() -> Cell {
    Cell {
        list_attr: 0,
        col: 1,
        row: 2,
        col_span: 1,
        row_span: 1,
        width: HwpUnit(4_000),
        height: HwpUnit(1_200),
        margins: [0; 4],
        border_fill: Default::default(),
        header_tail: Vec::new(),
        paragraphs: vec![paragraph_with_a_redundant_run()],
    }
}

fn table() -> Table {
    Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: 3,
        cols: 2,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![2, 2, 2],
        border_fill: Default::default(),
        table_tail: Vec::new(),
        cells: vec![cell()],
        caption: None,
        extras: Vec::new(),
    }
}

fn picture() -> Picture {
    Picture {
        common_data: Vec::new(),
        width: HwpUnit(7_200),
        height: HwpUnit(5_400),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: Some("그림".into()),
        crop: None,
        flip: 0,
        rotation: None,
        brightness: 0,
        contrast: 0,
        effect_flags: 0,
        effects_raw: Vec::new(),
        caption: None,
        bin_ref: BinRef::ItemRef("image1.png".into()),
        extras: Vec::new(),
    }
}

fn control(ctrl_id: [u8; 4], text: &str) -> GenericControl {
    GenericControl {
        ctrl_id,
        data: Vec::new(),
        paragraph_lists: vec![hwp_model::ParagraphList {
            header_data: Vec::new(),
            paragraphs: vec![Paragraph {
                chars: text.chars().map(HwpChar::Text).collect(),
                char_shape_runs: vec![(0, CharShapeId(0))],
                ..Default::default()
            }],
        }],
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

#[test]
fn para_kind_agrees_across_both_implementations() {
    let para = paragraph_with_a_trailing_break();
    let (convert, render) = paths(0, &[4]);
    assert_eq!(
        hwp_convert::paragraph_id(&convert, &para),
        hwp_render::segment_id::paragraph_id(&render, &para),
    );
}

#[test]
fn run_kind_agrees_across_both_implementations() {
    let para = paragraph_with_a_redundant_run();
    let (convert, render) = paths(0, &[4]);
    // Every run of the *canonical* list, the last one included. If either side numbered runs
    // off the raw list, the redundant entry above would shift the index; if either side
    // hashed the raw character list, the trailing paragraph-break terminator would lengthen
    // the **last** run's slice on one side only.
    for run_index in 0..2 {
        assert_eq!(
            hwp_convert::run_id(&convert, &para, run_index),
            hwp_render::segment_id::run_id(&render, &para, run_index),
            "run {run_index}",
        );
    }
}

#[test]
fn table_kind_agrees_across_both_implementations() {
    let table = table();
    let (convert, render) = paths(1, &[0, 2]);
    assert_eq!(
        hwp_convert::table_id(&convert, &table),
        hwp_render::segment_id::table_id(&render, &table),
    );
}

#[test]
fn cell_kind_agrees_across_both_implementations() {
    let cell = cell();
    let (convert, render) = paths(1, &[0, 2, 5]);
    assert_eq!(
        hwp_convert::cell_id(&convert, &cell),
        hwp_render::segment_id::cell_id(&render, &cell),
    );
}

#[test]
fn image_kind_agrees_across_both_implementations() {
    let picture = picture();
    let payload: &[u8] = b"\x89PNG\r\n\x1a\n-some-bytes";
    let (convert, render) = paths(0, &[3, 1]);
    assert_eq!(
        hwp_convert::picture_id(&convert, &picture, Some(payload)),
        hwp_render::segment_id::picture_id(&render, &picture, Some(payload)),
    );
    // The third argument really reaches the hash on both sides: an unresolved payload is not
    // the same segment as a resolved one, so the two disagree when only one gets the bytes.
    assert_ne!(
        hwp_convert::picture_id(&convert, &picture, None),
        hwp_render::segment_id::picture_id(&render, &picture, Some(payload)),
    );
    assert_ne!(
        hwp_convert::picture_id(&convert, &picture, Some(payload)),
        hwp_render::segment_id::picture_id(&render, &picture, None),
    );
    assert_eq!(
        hwp_convert::picture_id(&convert, &picture, None),
        hwp_render::segment_id::picture_id(&render, &picture, None),
    );
}

#[test]
fn field_kind_agrees_across_both_implementations() {
    // `control_id` serves both field and bookmark, which is why six id functions cover seven
    // kinds. This case and the next one call it with different `GenericControl` values, so a
    // drift affecting only one ctrl-id family still fails.
    let field = control(*b"%clk", "필드 내용");
    let (convert, render) = paths(0, &[2, 0]);
    assert_eq!(
        hwp_convert::control_id(&convert, &field),
        hwp_render::segment_id::control_id(&render, &field),
    );
}

#[test]
fn bookmark_kind_agrees_across_both_implementations() {
    let bookmark = control(*b"bokm", "책갈피");
    let (convert, render) = paths(0, &[2, 1]);
    assert_eq!(
        hwp_convert::control_id(&convert, &bookmark),
        hwp_render::segment_id::control_id(&render, &bookmark),
    );
    // Not a restatement of the field case: the two must differ from each other, or a single
    // `control_id` case would have covered both and the seventh test would be decoration.
    let field = control(*b"%clk", "책갈피");
    assert_ne!(
        hwp_render::segment_id::control_id(&render, &bookmark),
        hwp_render::segment_id::control_id(&render, &field),
    );
}

/// The two copies must also agree about the *canonical run list itself*, which is the input
/// `run_id` numbers off. `canonical_char_shape_runs` is the seventh public function on both
/// sides and the only one that does not return an id.
#[test]
fn the_canonical_run_list_agrees_across_both_implementations() {
    let para = paragraph_with_a_redundant_run();
    assert_eq!(
        hwp_convert::canonical_char_shape_runs(&para),
        hwp_render::segment_id::canonical_char_shape_runs(&para),
    );
    let runs = hwp_render::segment_id::canonical_char_shape_runs(&para);
    assert_eq!(
        runs,
        vec![(0, CharShapeId(1)), (2, CharShapeId(2))],
        "the redundant run must be collapsed and the same-position run overwritten, or the \
         run case proves nothing: {runs:?}",
    );
    // The paragraph really does carry the HWP5 terminator the character canonicalization drops.
    assert!(matches!(
        para.chars.last(),
        Some(HwpChar::CharCtrl(code)) if *code == ctrl_char::PARA_BREAK
    ));
}
