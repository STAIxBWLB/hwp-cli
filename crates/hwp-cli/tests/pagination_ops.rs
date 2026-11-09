//! #420: typed break-before, an idempotent indentation recipe and the keep-together refusal.
//!
//! Break-before writes the paragraph-header `break_type` page bit (0x04) of a BODY paragraph,
//! the one placement genuine Hangul-saved files show in both formats (HWP5 `break_type`, HWPX
//! `pageBreak`). Keep-together has no genuine saved evidence for the paragraph-shape bits and
//! must refuse before writing. Tests assert structure, never font-dependent page counts.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::{Control, Document, Paragraph};
use serde_json::{Value, json};

const FIRST: &str = "First line.";
const TARGET: &str = "Start a new page here.";
const LAST: &str = "Last line.";
const CELL: &str = "Cell text.";
/// Leading ASCII blanks, then words separated by double spaces and a trailing blank.
const SPACED: &str = "  Item  with  inner  spaces ";
/// A leading ideographic space (U+3000) before a Korean label.
const IDEOGRAPHIC: &str = "\u{3000}가. 항목";

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}
fn success(output: Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn text(t: &str) -> Value {
    json!({"type":"paragraph","runs":[{"type":"text","text":t}]})
}
fn read(path: &Path) -> Document {
    if path.extension().unwrap() == "hwp" {
        hwp5::read_document(path).unwrap().document
    } else {
        hwpx::read_document(path).unwrap().document
    }
}
fn index(doc: &Document, t: &str) -> usize {
    doc.sections[0]
        .paragraphs
        .iter()
        .position(|p| p.plain_text() == t)
        .unwrap_or_else(|| panic!("paragraph {t:?} not found"))
}
fn at(doc: &Document, t: &str) -> Value {
    json!({"at":{"section":0,"paragraph":index(doc,t)}})
}
fn paragraph<'a>(doc: &'a Document, t: &str) -> &'a Paragraph {
    &doc.sections[0].paragraphs[index(doc, t)]
}

struct Fixture {
    dir: PathBuf,
    base: PathBuf,
    extension: &'static str,
}
impl Fixture {
    /// Body: an indented line (leading spaces), a plain line, the break target, a table with one
    /// cell paragraph, a last line.
    fn new(label: &str, extension: &'static str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hwp-pagination-{}-{label}-{extension}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = json!({"version":"1.0","sections":[{"blocks":[
            text(FIRST),
            text("    Four-space indented item."),
            text(SPACED),
            text(IDEOGRAPHIC),
            text(TARGET),
            {"type":"table","columns":[{"width_mm":60}],"rows":[{"cells":[{"blocks":[text(CELL)]}]}]},
            text(LAST)
        ]}]});
        let spec_path = dir.join("spec.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let hwpx = dir.join("base.hwpx");
        success(
            hwp()
                .arg("compose")
                .arg(&spec_path)
                .arg("-o")
                .arg(&hwpx)
                .output()
                .unwrap(),
            "compose fixture",
        );
        let base = if extension == "hwp" {
            let native = dir.join("base.hwp");
            success(
                hwp()
                    .arg("convert")
                    .arg(&hwpx)
                    .arg("-o")
                    .arg(&native)
                    .output()
                    .unwrap(),
                "convert fixture",
            );
            native
        } else {
            hwpx
        };
        Self {
            dir,
            base,
            extension,
        }
    }
    fn path(&self, stem: &str) -> PathBuf {
        self.dir.join(format!("{stem}.{}", self.extension))
    }
    fn edit(&self, source: &Path, stem: &str, ops: Value) -> (PathBuf, Output) {
        let ops_path = self.dir.join(format!("{stem}-ops.json"));
        std::fs::write(&ops_path, serde_json::to_vec(&ops).unwrap()).unwrap();
        let output = self.path(stem);
        let run = hwp()
            .arg("edit")
            .arg(source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops_path)
            .arg("--report")
            .arg(self.dir.join(format!("{stem}-report.json")))
            .output()
            .unwrap();
        (output, run)
    }
    /// A request that must fail, leave the source untouched and publish nothing.
    fn refuse(&self, source: &Path, stem: &str, ops: Value) -> String {
        let before = std::fs::read(source).unwrap();
        let (output, run) = self.edit(source, stem, ops);
        assert!(!run.status.success(), "{stem} must be refused");
        assert!(!output.exists(), "{stem}: a refusal must publish nothing");
        assert_eq!(
            std::fs::read(source).unwrap(),
            before,
            "{stem}: source changed"
        );
        let message = String::from_utf8_lossy(&run.stderr).into_owned();
        assert!(
            !message.is_empty(),
            "{stem}: the refusal must explain itself"
        );
        message
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn break_op(address: Value, value: &str) -> Value {
    json!({"op":"set_break_before","address":address,"value":value})
}
fn indent_op(address: Value, level: u8, step: f64, hanging: f64) -> Value {
    json!({"op":"set_indent_level","address":address,"level":level,"step":format!("{step}mm"),"hanging":format!("{hanging}mm")})
}

// ---- set_break_before ----------------------------------------------------------------------

#[test]
fn break_before_sets_the_page_bit_on_exactly_one_body_paragraph_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("break-on", extension);
        let before = read(&fixture.base);
        assert!(
            before.sections[0]
                .paragraphs
                .iter()
                .all(|p| p.header.break_type & 0x04 == 0),
            "{extension}: the fixture starts without page breaks"
        );
        let (output, run) = fixture.edit(
            &fixture.base,
            "on",
            json!([break_op(at(&before, TARGET), "on")]),
        );
        success(run, "set_break_before on");
        let after = read(&output);
        let flagged: Vec<String> = after.sections[0]
            .paragraphs
            .iter()
            .filter(|p| p.header.break_type & 0x04 != 0)
            .map(Paragraph::plain_text)
            .collect();
        assert_eq!(
            flagged,
            [TARGET],
            "{extension}: only the addressed paragraph"
        );
        // Words, order and every other paragraph are untouched.
        assert_eq!(
            before.sections[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            after.sections[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            "{extension}: text and order"
        );
        let report: Value =
            serde_json::from_slice(&std::fs::read(fixture.dir.join("on-report.json")).unwrap())
                .unwrap();
        assert_eq!(report["ops"][0]["op"], "set_break_before");
        assert_eq!(report["ops"][0]["status"], "applied");
        assert_eq!(
            report["ops"][0]["pieces_touched"], 1,
            "{extension}: a real change touches one piece"
        );
    }
}

#[test]
fn break_before_off_clears_only_the_page_bit_and_a_repeat_is_a_noop() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("break-off", extension);
        let before = read(&fixture.base);
        let (on, run) = fixture.edit(
            &fixture.base,
            "on",
            json!([break_op(at(&before, TARGET), "on")]),
        );
        success(run, "on");
        // Applying "on" again changes nothing: same bytes, no pieces touched.
        let (again, run) = fixture.edit(
            &on,
            "on-again",
            json!([break_op(at(&before, TARGET), "on")]),
        );
        success(run, "on again");
        let report: Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.join("on-again-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            report["ops"][0]["status"], "applied",
            "{extension}: an already-in-effect request still succeeds"
        );
        assert_eq!(
            report["ops"][0]["pieces_touched"], 0,
            "{extension}: already in effect"
        );
        assert!(
            read(&again).sections[0]
                .paragraphs
                .iter()
                .filter(|p| p.header.break_type & 0x04 != 0)
                .count()
                == 1
        );
        let (off, run) = fixture.edit(&on, "off", json!([break_op(at(&before, TARGET), "off")]));
        success(run, "off");
        assert!(
            read(&off).sections[0]
                .paragraphs
                .iter()
                .all(|p| p.header.break_type & 0x04 == 0),
            "{extension}: cleared"
        );
    }
}

#[test]
fn break_before_refuses_the_placements_no_genuine_file_shows() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("break-refuse", extension);
        let before = read(&fixture.base);
        let source = fixture.base.clone();
        assert!(
            before.sections[0]
                .paragraphs
                .iter()
                .any(|p| p.controls.iter().any(|c| matches!(c, Control::Table(_)))),
            "{extension}: the fixture holds a table"
        );
        // A paragraph inside a table cell.
        let cell_id = hwp()
            .arg("cat")
            .arg(&source)
            .args(["--format", "json", "--with-segments", "--segments", "v2"])
            .output()
            .unwrap();
        let envelope: Value = serde_json::from_slice(&cell_id.stdout).unwrap();
        let cell_para = envelope["segments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "para" && s["path"]["indices"].as_array().unwrap().len() == 4)
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .expect("a cell paragraph is published");
        let message = fixture.refuse(
            &source,
            "cell",
            json!([break_op(json!({"id": cell_para}), "on")]),
        );
        assert!(
            message.contains("표 셀") && message.contains("본문"),
            "{extension}: a cell paragraph is refused with the body remedy: {message}"
        );
        // The paragraph that owns the section setup (the first one).
        let message = fixture.refuse(
            &source,
            "first",
            json!([break_op(json!({"at":{"section":0,"paragraph":0}}), "on")]),
        );
        assert!(
            message.contains("구역") || message.contains("첫"),
            "{extension}: the section's first paragraph is refused: {message}"
        );
    }
}

#[test]
fn break_before_rejects_a_value_other_than_on_or_off() {
    let fixture = Fixture::new("break-value", "hwpx");
    let before = read(&fixture.base);
    let message = fixture.refuse(
        &fixture.base.clone(),
        "bad-value",
        json!([break_op(at(&before, TARGET), "maybe")]),
    );
    assert!(
        message.contains("set_break_before") && message.contains("on 또는 off"),
        "the refusal must name the op and the two accepted values: {message}"
    );
    assert!(
        !message.contains("스키마"),
        "a bad value is a parser refusal with a remedy, not a bare schema failure: {message}"
    );
}

// ---- set_indent_level ----------------------------------------------------------------------

fn shape<'a>(doc: &'a Document, p: &Paragraph) -> &'a hwp_model::header::ParaShape {
    &doc.header.para_shapes[p.para_shape.0 as usize]
}

#[test]
fn indent_level_replaces_leading_spaces_with_shape_indentation_and_keeps_every_word() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("indent", extension);
        let before = read(&fixture.base);
        let leading = "    Four-space indented item.";
        let (output, run) = fixture.edit(
            &fixture.base,
            "indented",
            json!([indent_op(at(&before, leading), 2, 5.0, 3.0)]),
        );
        success(run, "set_indent_level");
        let report: Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.join("indented-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            report["ops"][0]["pieces_touched"], 1,
            "{extension}: a real change touches one piece"
        );
        let after = read(&output);
        let item = paragraph(&after, "Four-space indented item.");
        assert_eq!(
            item.plain_text(),
            "Four-space indented item.",
            "{extension}: leading spaces are gone, words are not"
        );
        let ps = shape(&after, item);
        // The step is quantized to HWPUNIT first and then multiplied by the level, so level 2 is
        // exactly twice level 1. IR units are twice HWPUNIT (7200 per inch).
        let hwpunit = |v: f64| (v / 25.4 * 7200.0).round() as i32;
        assert_eq!(
            ps.margin_left,
            2 * hwpunit(5.0) * 2,
            "{extension}: left margin"
        );
        assert_eq!(ps.indent, -hwpunit(3.0) * 2, "{extension}: hanging indent");
        // Untouched paragraphs keep their shape.
        assert_eq!(
            shape(&after, paragraph(&after, FIRST)),
            shape(&before, paragraph(&before, FIRST)),
            "{extension}: other paragraphs keep their shape"
        );
    }
}

#[test]
fn indent_level_applied_twice_is_a_semantic_and_formatting_noop_the_second_time() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("indent-twice", extension);
        let before = read(&fixture.base);
        let op = indent_op(at(&before, "    Four-space indented item."), 1, 4.0, 2.0);
        let (once, run) = fixture.edit(&fixture.base, "once", json!([op.clone()]));
        success(run, "first application");
        let mid = read(&once);
        // The second run addresses the paragraph at the same position (no paragraph moved).
        let again_op = indent_op(
            json!({"at":{"section":0,"paragraph":index(&mid, "Four-space indented item.")}}),
            1,
            4.0,
            2.0,
        );
        let (twice, run) = fixture.edit(&once, "twice", json!([again_op]));
        success(run, "second application");
        let report: Value =
            serde_json::from_slice(&std::fs::read(fixture.dir.join("twice-report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["ops"][0]["status"], "applied",
            "{extension}: the repeat still succeeds"
        );
        assert_eq!(
            report["ops"][0]["pieces_touched"], 0,
            "{extension}: the second run changes nothing"
        );
        let last = read(&twice);
        assert_eq!(
            mid.header.para_shapes.len(),
            last.header.para_shapes.len(),
            "{extension}: no shape is appended by the repeat"
        );
        for (a, b) in mid.sections[0]
            .paragraphs
            .iter()
            .zip(&last.sections[0].paragraphs)
        {
            assert_eq!(a.plain_text(), b.plain_text(), "{extension}: text");
            assert_eq!(a.para_shape, b.para_shape, "{extension}: shape id");
        }
    }
}

#[test]
fn indent_level_never_touches_inner_spaces_labels_or_a_paragraph_without_leading_spaces() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("indent-inner", extension);
        let before = read(&fixture.base);
        // A paragraph with no leading blanks keeps its text exactly.
        let (output, run) = fixture.edit(
            &fixture.base,
            "plain",
            json!([
                indent_op(at(&before, FIRST), 1, 5.0, 0.0),
                indent_op(at(&before, SPACED), 1, 5.0, 0.0),
                indent_op(at(&before, IDEOGRAPHIC), 1, 5.0, 0.0),
            ]),
        );
        success(run, "indent paragraphs");
        let after = read(&output);
        assert_eq!(paragraph(&after, FIRST).plain_text(), FIRST);
        // Only the LEADING blanks go: inner double spaces and the trailing blank stay.
        assert_eq!(
            after.sections[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .filter(|t| t.contains("with"))
                .collect::<Vec<_>>(),
            ["Item  with  inner  spaces "],
            "{extension}: only the leading blanks are removed"
        );
        // An ideographic-space lead goes too, and the Korean label survives untouched.
        assert!(
            after.sections[0]
                .paragraphs
                .iter()
                .any(|p| p.plain_text() == "가. 항목"),
            "{extension}: the label stays, the ideographic space goes"
        );
        // A level above the supported range is refused with the range in the message.
        let message = fixture.refuse(
            &fixture.base.clone(),
            "level-8",
            json!([indent_op(at(&before, FIRST), 8, 5.0, 0.0)]),
        );
        assert!(
            message.contains("set_indent_level") && message.contains("0..=7"),
            "the refusal must name the op and the supported level range: {message}"
        );
        assert!(
            !message.contains("스키마"),
            "an out-of-range level is a parser refusal with a remedy: {message}"
        );
    }
}

// ---- keep_together -------------------------------------------------------------------------

#[test]
fn keep_together_refuses_before_writing_and_names_the_missing_evidence() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("keep", extension);
        let before = read(&fixture.base);
        for field in ["keep_with_next", "keep_lines", "break_before_shape"] {
            let message = fixture.refuse(
                &fixture.base.clone(),
                &format!("keep-{field}"),
                json!([{"op":"keep_together","address":at(&before, TARGET),field:true}]),
            );
            assert!(
                message.contains("keep_together")
                    && message.contains("Hangul")
                    && message.contains("set_break_before"),
                "{extension} {field}: the refusal names the cause and the supported alternative: {message}"
            );
        }
    }
}
