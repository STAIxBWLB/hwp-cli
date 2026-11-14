//! #517: `run` segments publish `source_chars`, and the numbers are exactly what a run-range
//! `set_format` accepts.
//!
//! The published unit is the SOURCE PARAGRAPH's UTF-16 code units, not the emitted markdown's
//! Unicode scalars, so every case here is one where the two coordinate spaces disagree:
//!
//! - a surrogate pair is one markdown scalar and two source units,
//! - an inline control (a field anchor, a bookmark) is eight source units and no markdown at all,
//! - a block control splits the run into pieces whose spans skip the control's own characters.
//!
//! Each case goes through the real binary: `cat --with-segments --segments v2` publishes the
//! envelope, and `edit --ops` applies a `set_format` whose `address.chars` is the published pair
//! verbatim, with the segment's own id as the address. The test then reads the edited document
//! back and asserts that the addressed characters changed and every other character did not -
//! an off-by-two from counting scalars, or an off-by-eight from dropping an inline control,
//! fails on the neighbour assertion rather than passing on "the edit applied".

use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::control::{Cell, GenericControl, Table};
use hwp_model::header::{CharShape, ParaShape, Style};
use hwp_model::ids::{BorderFillId, CharShapeId, ParaShapeId};
use hwp_model::opaque::OpaqueRecord;
use hwp_model::units::HwpUnit;
use hwp_model::{Control, Document, HwpChar, Paragraph, Section, ctrl_char};

#[path = "common/temp_path.rs"]
mod temp_path;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

// ---------------------------------------------------------------------------
// Documents
//
// Each one is built as IR and written with the hwp5 writer, because the markdown importer can
// express none of them: it has no field, no bookmark, and no way to anchor a table inside a
// paragraph's own text.
// ---------------------------------------------------------------------------

/// Shapes 0 and 1 differ only in size, so the markdown carries no emphasis markers: a run's
/// markdown slice is exactly its text, which keeps the assertions about `source_chars` readable.
fn shape(base_size: i32) -> CharShape {
    CharShape {
        base_size,
        ..Default::default()
    }
}

fn document_of(paragraphs: Vec<Paragraph>) -> Document {
    let mut doc = Document {
        sections: vec![Section {
            paragraphs,
            ..Default::default()
        }],
        ..Default::default()
    };
    doc.header.char_shapes = vec![shape(1000), shape(1200)];
    doc.header.para_shapes = vec![ParaShape::default()];
    doc.header.styles = vec![Style {
        name: "바탕글".into(),
        char_shape: CharShapeId(0),
        para_shape: ParaShapeId(0),
        ..Default::default()
    }];
    doc.header.fonts[0] = vec![hwp_model::header::FaceName {
        name: "함초롬바탕".into(),
        ..Default::default()
    }];
    doc
}

fn paragraph(text: &str, runs: &[(u32, u16)]) -> Paragraph {
    Paragraph {
        chars: text.chars().map(HwpChar::Text).collect(),
        char_shape_runs: runs
            .iter()
            .map(|&(at, shape)| (at, CharShapeId(shape)))
            .collect(),
        ..Default::default()
    }
}

/// The 12-byte ExtCtrl payload the format uses for a control anchor: the control id, reversed,
/// in the first four bytes.
fn ext_payload(ctrl_id: [u8; 4]) -> Vec<u8> {
    let mut payload = vec![0u8; 12];
    let mut reversed = ctrl_id;
    reversed.reverse();
    payload[..4].copy_from_slice(&reversed);
    payload
}

fn attach(para: &mut Paragraph, code: u16, ctrl_id: [u8; 4], control: Control) {
    let index = para.controls.len() as u32;
    para.controls.push(control);
    para.chars.push(HwpChar::ExtCtrl {
        code,
        ctrl_id,
        payload: ext_payload(ctrl_id),
        ctrl_index: Some(index),
    });
}

fn table_of(text: &str) -> Control {
    Control::Table(Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1],
        border_fill: BorderFillId::default(),
        table_tail: Vec::new(),
        cells: vec![Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(1000),
            height: HwpUnit(1000),
            margins: [0; 4],
            border_fill: BorderFillId::default(),
            header_tail: Vec::new(),
            paragraphs: vec![paragraph(text, &[(0, 0)])],
        }],
        caption: None,
        extras: Vec::new(),
    })
}

fn generic(ctrl_id: [u8; 4], raw: Vec<OpaqueRecord>) -> Control {
    Control::Generic(GenericControl {
        ctrl_id,
        data: vec![0u8; 11],
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: raw,
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    })
}

/// 😀 is U+1F600: two UTF-16 code units in the source and one markdown scalar. The middle run
/// therefore sits at [8, 10) in the source paragraph, and a scalar-derived span would name
/// [6, 8) instead.
fn surrogate_pair_document() -> Document {
    document_of(vec![paragraph("😀😀 ab cd ef", &[(0, 0), (8, 1), (10, 0)])])
}

/// One run whose emission a table interrupts. The table's own ExtCtrl anchor is 8 WCHAR wide and
/// belongs to no piece, so the two pieces are [0, 1) and [9, 10).
fn block_interrupted_document() -> Document {
    let mut p = paragraph("앞", &[(0, 0)]);
    attach(&mut p, ctrl_char::OBJECT, *b"tbl ", table_of("칸"));
    p.chars.extend("뒤".chars().map(HwpChar::Text));
    document_of(vec![p])
}

/// A run boundary that sits after an inline field and a bookmark: the field's anchor and
/// terminator are 8 WCHAR each and the bookmark's anchor another 8, so the second run starts at
/// 27. Counting scalars would place it at 3.
fn inline_control_document() -> Document {
    let mut p = Paragraph {
        chars: Vec::new(),
        char_shape_runs: vec![(0, CharShapeId(0)), (27, CharShapeId(1))],
        ..Default::default()
    };
    p.chars.extend("앞 ".chars().map(HwpChar::Text));
    let field = generic(*b"%clk", Vec::new());
    attach(&mut p, ctrl_char::FIELD_START, *b"%clk", field);
    p.chars.extend("값".chars().map(HwpChar::Text));
    p.chars.push(HwpChar::InlineCtrl {
        code: ctrl_char::FIELD_END,
        payload: Vec::new(),
    });
    let bookmark_name = "표식";
    let ctrl_data = hwp_convert::bookmark::make_bokm_ctrl_data(bookmark_name);
    let bookmark = generic(
        *b"bokm",
        vec![OpaqueRecord {
            tag: 0x0010 + 71,
            data: ctrl_data,
            children: Vec::new(),
        }],
    );
    attach(&mut p, ctrl_char::BOOKMARK, *b"bokm", bookmark);
    p.chars.extend("뒤!".chars().map(HwpChar::Text));
    document_of(vec![p])
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn write_doc(dir: &Path, name: &str, doc: &Document) -> PathBuf {
    let path = dir.join(name);
    hwp5::write_document(doc, &path, &hwp5::WriteOptions::default()).expect("write the document");
    path
}

/// The v2 envelope of `file` in `format`, through the real binary.
fn envelope(file: &Path, format: &str) -> serde_json::Value {
    let out = hwp()
        .arg("cat")
        .arg(file)
        .args(["--format", format, "--with-segments", "--segments", "v2"])
        .output()
        .expect("run hwp cat");
    assert!(
        out.status.success(),
        "hwp cat --segments v2 --format {format} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the v2 envelope parses")
}

fn indices(seg: &serde_json::Value) -> Vec<u64> {
    seg["path"]["indices"]
        .as_array()
        .expect("a published path carries indices")
        .iter()
        .map(|v| v.as_u64().expect("an index is a non-negative integer"))
        .collect()
}

/// The run segments sitting at exactly `want`, in published order.
fn runs_at<'a>(env: &'a serde_json::Value, want: &[u64]) -> Vec<&'a serde_json::Value> {
    env["segments"]
        .as_array()
        .expect("segments array")
        .iter()
        .filter(|s| s["kind"] == "run" && indices(s) == want)
        .collect()
}

/// The published source span, which every case here reads as the two numbers a caller writes into
/// `address.chars`.
fn source_chars(seg: &serde_json::Value) -> (u64, u64) {
    let chars = &seg["source_chars"];
    assert!(
        chars.is_object(),
        "a run that emitted text must publish source_chars: {seg:#?}"
    );
    (
        chars["start"].as_u64().expect("start"),
        chars["end"].as_u64().expect("end"),
    )
}

/// The markdown slice a segment's `char_range` names, so a case can prove it picked the run it
/// meant to (scalars into the emitted markdown, emphasis markers included).
fn sliced(env: &serde_json::Value, seg: &serde_json::Value) -> String {
    let md = env["markdown"]
        .as_str()
        .expect("the envelope carries markdown");
    let start = seg["char_range"]["start"].as_u64().expect("start") as usize;
    let end = seg["char_range"]["end"].as_u64().expect("end") as usize;
    md.chars().skip(start).take(end - start).collect()
}

/// An addressed `set_format` that hands the segment's own id and its published numbers straight
/// to `chars`, with no conversion of any kind.
fn set_format_op(seg: &serde_json::Value, prop: &str) -> serde_json::Value {
    let (start, end) = source_chars(seg);
    let mut op = serde_json::json!({
        "op": "set_format",
        "address": { "id": seg["id"], "chars": [start, end] },
    });
    op[prop] = serde_json::json!("on");
    op
}

/// Applies `ops` and returns the edited file.
fn apply(dir: &Path, input: &Path, ops: &[serde_json::Value]) -> PathBuf {
    let ops_path = dir.join("ops.json");
    std::fs::write(&ops_path, serde_json::to_vec(ops).expect("serialize ops")).expect("write ops");
    let out = dir.join("edited.hwp");
    let run = hwp()
        .arg("edit")
        .arg(input)
        .arg("-o")
        .arg(&out)
        .arg("--ops")
        .arg(&ops_path)
        .output()
        .expect("run hwp edit");
    assert!(
        run.status.success(),
        "hwp edit --ops failed for {ops:#?}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    out
}

/// Every text character of `para` with the WCHAR offset it starts at.
fn wchars(para: &Paragraph) -> Vec<(u32, char)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for ch in &para.chars {
        if let HwpChar::Text(c) = ch {
            out.push((pos, *c));
        }
        pos += ch.wchar_width();
    }
    out
}

/// Whether the character starting at WCHAR `pos` carries a shape with `attr` set. Reads the shape
/// the paragraph's own run list names at that position, the way the renderer and the edited
/// document both do.
fn has_attr(doc: &Document, para: &Paragraph, pos: u32, attr: fn(&CharShape) -> bool) -> bool {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(at, _)| *at <= pos)
        .map(|(_, id)| *id)
        .expect("every position of a non-empty paragraph is covered by a run");
    attr(&doc.header.char_shapes[id.0 as usize])
}

/// Asserts that exactly the characters in `want` carry `attr`, and returns how many that was so a
/// caller can prove the assertion was not vacuous.
fn assert_exactly(
    doc: &Document,
    para: &Paragraph,
    want: (u32, u32),
    attr: fn(&CharShape) -> bool,
    what: &str,
) -> usize {
    let mut hits = 0;
    for (pos, ch) in wchars(para) {
        let found = has_attr(doc, para, pos, attr);
        let expected = (want.0..want.1).contains(&pos);
        assert_eq!(
            found, expected,
            "{what} at character {ch:?} (wchar {pos}): expected {expected}, found {found}"
        );
        hits += usize::from(found);
    }
    hits
}

// ---------------------------------------------------------------------------
// The three acceptance cases
// ---------------------------------------------------------------------------

/// A surrogate pair: the published span counts the emoji as two source units, so it names the run
/// and not the two characters before it.
#[test]
fn a_surrogate_pair_run_publishes_utf16_offsets_and_targets_its_own_text() {
    let dir = temp_path::TempPath::new("hwp517", "surrogate");
    std::fs::create_dir_all(&*dir).unwrap();
    let input = write_doc(&dir, "surrogate.hwp", &surrogate_pair_document());

    let env = envelope(&input, "json");
    let runs = runs_at(&env, &[0, 1]);
    assert_eq!(runs.len(), 1, "one contiguous piece: {env:#?}");
    assert_eq!(
        sliced(&env, runs[0]),
        "cd",
        "the run must be the one over \"cd\", or the span assertion below proves nothing"
    );
    assert_eq!(
        source_chars(runs[0]),
        (8, 10),
        "UTF-16 code units of the source paragraph: the two emoji are four units, so \"cd\" is \
         [8, 10) and not the [6, 8) a markdown-scalar count would give"
    );

    let out = apply(&dir, &input, &[set_format_op(runs[0], "italic")]);
    let after = hwp5::read_document(&out).unwrap().document;
    let para = &after.sections[0].paragraphs[0];
    assert_eq!(
        para.plain_text(),
        "😀😀 ab cd ef",
        "formatting must never change the text"
    );
    let hits = assert_exactly(&after, para, (8, 10), CharShape::is_italic, "italic");
    assert_eq!(
        hits, 2,
        "exactly the two characters of the run must be italic"
    );
}

/// A run interrupted by a table: one `source_chars` per piece, and the table's own characters
/// belong to no piece, so a caller restyling both spans leaves the block alone.
#[test]
fn a_block_interrupted_run_publishes_one_span_per_piece() {
    let dir = temp_path::TempPath::new("hwp517", "interrupted");
    std::fs::create_dir_all(&*dir).unwrap();
    let input = write_doc(&dir, "interrupted.hwp", &block_interrupted_document());

    let env = envelope(&input, "json");
    let runs = runs_at(&env, &[0, 0]);
    assert_eq!(
        runs.len(),
        2,
        "the interrupted run must be published once per piece: {env:#?}"
    );
    assert_eq!(
        runs[0]["id"], runs[1]["id"],
        "the pieces are one run, so one id - which is why each needs its own span"
    );
    assert_eq!(sliced(&env, runs[0]), "앞");
    assert_eq!(sliced(&env, runs[1]), "뒤");
    assert_eq!(
        source_chars(runs[0]),
        (0, 1),
        "the first piece is one character"
    );
    assert_eq!(
        source_chars(runs[1]),
        (9, 11),
        "the table anchor is 8 WCHAR of the paragraph and belongs to no piece, so the second \
         piece starts after it - and it ends at 11, the paragraph's own stored length: the \
         reader appends the paragraph break as one more character, the same bound the resolver \
         gives the last run"
    );

    let before_text =
        hwp5::read_document(&input).unwrap().document.sections[0].paragraphs[0].plain_text();
    let out = apply(
        &dir,
        &input,
        &[
            set_format_op(runs[0], "italic"),
            set_format_op(runs[1], "bold"),
        ],
    );
    let after = hwp5::read_document(&out).unwrap().document;
    let para = &after.sections[0].paragraphs[0];
    assert_eq!(
        para.plain_text(),
        before_text,
        "formatting must never change the text"
    );
    assert_eq!(
        assert_exactly(&after, para, (0, 1), CharShape::is_italic, "italic"),
        1,
        "only the first piece's character is italic"
    );
    assert_eq!(
        assert_exactly(&after, para, (9, 11), CharShape::is_bold, "bold"),
        1,
        "only the second piece's character is bold"
    );
}

/// Inline controls - a field and a bookmark - sit inside the paragraph's characters, so they are
/// counted in the span that follows them, exactly as `address.chars` counts them.
#[test]
fn inline_controls_are_counted_in_the_run_span() {
    let dir = temp_path::TempPath::new("hwp517", "inline");
    std::fs::create_dir_all(&*dir).unwrap();
    let input = write_doc(&dir, "inline.hwp", &inline_control_document());

    let env = envelope(&input, "json");
    let runs = runs_at(&env, &[0, 1]);
    assert_eq!(runs.len(), 1, "one contiguous piece: {env:#?}");
    assert_eq!(
        source_chars(runs[0]),
        (27, 30),
        "three ExtCtrl/InlineCtrl anchors of 8 WCHAR each sit before the run, so it starts at 27 \
         - a text-only count would say 3 - and it ends at 30, one past \"뒤!\", because the \
         stored paragraph break is the last character of the paragraph the reader hands back"
    );
    assert_eq!(sliced(&env, runs[0]), "뒤!");

    let out = apply(&dir, &input, &[set_format_op(runs[0], "italic")]);
    let after = hwp5::read_document(&out).unwrap().document;
    let para = &after.sections[0].paragraphs[0];
    let hits = assert_exactly(&after, para, (27, 30), CharShape::is_italic, "italic");
    assert_eq!(hits, 2, "exactly the run's two characters must be italic");
}

/// `char_range` is unchanged by all of this: it still indexes the emitted markdown in Unicode
/// scalars, which on the surrogate-pair document is a different pair of numbers for the same run.
#[test]
fn char_range_stays_a_markdown_scalar_range() {
    let dir = temp_path::TempPath::new("hwp517", "two-spaces");
    std::fs::create_dir_all(&*dir).unwrap();
    let input = write_doc(&dir, "two-spaces.hwp", &surrogate_pair_document());

    let env = envelope(&input, "json");
    let runs = runs_at(&env, &[0, 1]);
    let range = (
        runs[0]["char_range"]["start"].as_u64().unwrap(),
        runs[0]["char_range"]["end"].as_u64().unwrap(),
    );
    assert_eq!(
        range,
        (6, 8),
        "the markdown offsets are scalars: two emoji are two of them"
    );
    assert_eq!(
        source_chars(runs[0]),
        (8, 10),
        "the source offsets are UTF-16 code units - the two spaces are not interchangeable"
    );
}

/// Both `--format markdown` and `--format json` publish the same `source_chars`; the field is a
/// property of the run, not of the carrier.
#[test]
fn both_v2_formats_publish_the_same_source_offsets() {
    let dir = temp_path::TempPath::new("hwp517", "formats");
    std::fs::create_dir_all(&*dir).unwrap();
    let input = write_doc(&dir, "formats.hwp", &block_interrupted_document());

    let collect = |format: &str| -> Vec<(String, (u64, u64))> {
        let env = envelope(&input, format);
        env["segments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["kind"] == "run" && s.get("source_chars").is_some())
            .map(|s| (s["id"].as_str().unwrap().to_string(), source_chars(s)))
            .collect()
    };
    let markdown = collect("markdown");
    let json = collect("json");
    assert!(
        markdown.len() >= 3,
        "expected the paragraph, both pieces and the cell run: {markdown:?}"
    );
    assert_eq!(
        markdown, json,
        "the two formats must publish identical source offsets"
    );
}
