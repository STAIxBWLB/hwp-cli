//! Typed break-before, idempotent indentation and evidence-backed keep-together flags.
//!
//! Break-before writes the paragraph-header `break_type` page bit (0x04) of a BODY paragraph,
//! the one placement genuine Hangul-saved files show in both formats (HWP5 `break_type`, HWPX
//! `pageBreak`). Keep-together writes the independent paragraph-shape bits from #540 G4.
//! Tests assert structure, never font-dependent page counts.
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
/// The section's first paragraph (it owns the section and column controls) is given this text
/// after its controls, the layout a Hangul-saved form has.
const LEAD_FIRST: &str = "  Form title";
/// Plain leading blanks, then a bold label, then a plain tail.
const BOLD_LEAD: &str = "  Bold label tail";
const IDEOGRAPHIC: &str = "\u{3000}가. 항목";

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
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
        let bold_lead = json!({"type":"paragraph","runs":[
            {"type":"text","text":"  "},
            {"type":"text","text":"Bold label","bold":true},
            {"type":"text","text":" tail"}]});
        let spec = json!({"version":"1.0","sections":[{"blocks":[
            text(FIRST),
            bold_lead,
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
        // Give the section's first paragraph visible text after its leading controls.
        let mut doc = read(&base);
        let first = &mut doc.sections[0].paragraphs[0];
        assert_eq!(
            first.plain_text(),
            "",
            "compose keeps the section controls alone"
        );
        // After the section and column controls, before the paragraph-break character HWP5
        // stores at the end of the paragraph.
        let lead = first
            .chars
            .iter()
            .take_while(|c| matches!(c, hwp_model::HwpChar::ExtCtrl { .. }))
            .count();
        assert_eq!(lead, 2, "{extension}: secd and cold lead the paragraph");
        for (offset, c) in LEAD_FIRST.chars().enumerate() {
            first
                .chars
                .insert(lead + offset, hwp_model::HwpChar::Text(c));
        }
        first.line_segs.clear();
        if extension == "hwp" {
            hwp5::write_document(&doc, &base, &hwp5::WriteOptions::default()).unwrap();
        } else {
            hwpx::write_document(&doc, &base).unwrap();
        }
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

/// Inspect the persisted state even on refusal: the pre-fix CLI publishes no output, so
/// the original remains the observed document. The red assertion is the requested bit,
/// not an early unwrap of the refusal or a missing output file.
#[test]
fn keep_together_sets_and_clears_only_the_supplied_bit_in_both_formats() {
    use std::io::Read;
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("keep-bits", extension);
        for (field, bit, attribute) in [
            ("keep_with_next", 17, "keepWithNext"),
            ("keep_lines", 18, "keepLines"),
            ("break_before_shape", 19, "pageBreakBefore"),
        ] {
            // Include widow/orphan and the other two keep bits in the preservation premise.
            for on in [true, false] {
                let mut seeded = read(&fixture.base);
                let pi = index(&seeded, TARGET);
                let si = seeded.sections[0].paragraphs[pi].para_shape.0 as usize;
                seeded.header.para_shapes[si].attr1 |= 0xf << 16;
                if on {
                    seeded.header.para_shapes[si].attr1 &= !(1 << bit);
                }
                let input = fixture.path(&format!("seed-{field}-{on}"));
                if extension == "hwp" {
                    hwp5::write_document(&seeded, &input, &hwp5::WriteOptions::default()).unwrap();
                } else {
                    hwpx::write_document(&seeded, &input).unwrap();
                }
                let before = read(&input);
                let p = paragraph(&before, TARGET);
                let prior = before.header.para_shapes[p.para_shape.0 as usize].attr1;
                let header_break = p.header.break_type;
                let (output, run) = fixture.edit(
                    &input,
                    &format!("keep-{field}-{on}"),
                    json!([{"op":"keep_together","address":at(&before, TARGET),field:on}]),
                );
                let after = read(if output.exists() { &output } else { &input });
                let p = paragraph(&after, TARGET);
                let actual = after.header.para_shapes[p.para_shape.0 as usize].attr1;
                let expected = if on {
                    prior | (1 << bit)
                } else {
                    prior & !(1 << bit)
                };
                assert_eq!(
                    actual,
                    expected,
                    "{extension} {field}={on}: exact attr1 including all untouched bits; CLI: {}",
                    String::from_utf8_lossy(&run.stderr)
                );
                success(run, "keep_together");
                assert_eq!(
                    p.header.break_type, header_break,
                    "shape flag is not header break_type"
                );
                for original in &before.sections[0].paragraphs {
                    let other = paragraph(&after, &original.plain_text());
                    if original.plain_text() != TARGET {
                        assert_eq!(
                            after.header.para_shapes[other.para_shape.0 as usize].attr1,
                            before.header.para_shapes[original.para_shape.0 as usize].attr1,
                            "shared source shape must not mutate other paragraphs"
                        );
                    }
                }
                if extension == "hwpx" {
                    let mut zip =
                        zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
                    let mut xml = String::new();
                    zip.by_name("Contents/header.xml")
                        .unwrap()
                        .read_to_string(&mut xml)
                        .unwrap();
                    let mut reader = quick_xml::Reader::from_str(&xml);
                    let mut selected = false;
                    let mut checked = false;
                    loop {
                        use quick_xml::events::Event;
                        match reader.read_event().unwrap() {
                            Event::Start(e) if e.local_name().as_ref() == "paraPr" => {
                                selected = e.attributes().flatten().any(|a| {
                                    a.key.as_ref() == "id"
                                        && a.value.as_ref() == p.para_shape.0.to_string()
                                });
                            }
                            Event::Empty(e)
                                if selected && e.local_name().as_ref() == "breakSetting" =>
                            {
                                let attrs: std::collections::BTreeMap<String, String> = e
                                    .attributes()
                                    .map(|a| {
                                        let a = a.unwrap();
                                        (a.key.as_ref().to_string(), a.value.into_owned())
                                    })
                                    .collect();
                                assert_eq!(attrs[attribute], if on { "1" } else { "0" });
                                for (name, shift) in [
                                    ("widowOrphan", 16),
                                    ("keepWithNext", 17),
                                    ("keepLines", 18),
                                    ("pageBreakBefore", 19),
                                ] {
                                    assert_eq!(
                                        attrs[name],
                                        if expected & (1 << shift) != 0 {
                                            "1"
                                        } else {
                                            "0"
                                        }
                                    );
                                }
                                checked = true;
                            }
                            Event::Eof => break,
                            _ => {}
                        }
                    }
                    assert!(checked, "serialized target breakSetting exists");
                }
                let (repeat, run) = fixture.edit(
                    &output,
                    &format!("repeat-{field}-{on}"),
                    json!([{"op":"keep_together","address":at(&after, TARGET),field:on}]),
                );
                success(run, "keep_together repeated");
                assert_eq!(
                    std::fs::read(output).unwrap(),
                    std::fs::read(repeat).unwrap()
                );
            }
        }
    }
}

#[test]
fn keep_together_supports_first_body_and_cell_paragraph_addresses() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("keep-scope", extension);
        let listed = hwp()
            .arg("cat")
            .arg(&fixture.base)
            .args(["--format", "json", "--with-segments", "--segments", "v2"])
            .output()
            .unwrap();
        success(
            Output {
                status: listed.status,
                stdout: Vec::new(),
                stderr: listed.stderr.clone(),
            },
            "catalogue",
        );
        let envelope: Value = serde_json::from_slice(&listed.stdout).unwrap();
        let cell = envelope["segments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "para" && s["path"]["indices"].as_array().unwrap().len() == 4)
            .unwrap();
        let before = read(&fixture.base);
        let (output, run) = fixture.edit(&fixture.base, "scope",
            json!([
                {"op":"keep_together","address":{"at":{"section":0,"paragraph":0}},"keep_with_next":true},
                {"op":"keep_together","address":{"id":cell["id"]},"keep_lines":true}
            ]));
        success(run, "first body and cell keep flags");
        let after = read(&output);
        let first = &after.sections[0].paragraphs[0];
        assert!(after.header.para_shapes[first.para_shape.0 as usize].keep_with_next());
        assert_eq!(
            first.header.break_type,
            before.sections[0].paragraphs[0].header.break_type
        );
        let cell_para = after.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(&t.cells[0].paragraphs[0]),
                _ => None,
            })
            .unwrap();
        assert!(after.header.para_shapes[cell_para.para_shape.0 as usize].keep_lines());
        assert_eq!(cell_para.plain_text(), CELL);
        assert_eq!(
            paragraph(&after, TARGET).para_shape,
            paragraph(&before, TARGET).para_shape
        );
    }
}

// ---- set_indent_level: run boundaries and leading controls (T12-F1, T12-F2) -----------------

/// The character shape in effect at WCHAR offset `wpos` of `para`.
fn shape_at(para: &Paragraph, wpos: u32) -> hwp_model::CharShapeId {
    para.char_shape_runs
        .iter()
        .rfind(|(start, _)| *start <= wpos)
        .map(|(_, id)| *id)
        .expect("a run covers the offset")
}

#[test]
fn indent_level_keeps_the_shape_of_the_first_surviving_character() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("indent-runs", extension);
        let before = read(&fixture.base);
        let para = paragraph(&before, BOLD_LEAD);
        let blank_shape = shape_at(para, 0);
        let bold_shape = shape_at(para, 2);
        let tail_shape = shape_at(para, 2 + "Bold label".len() as u32);
        assert_ne!(
            blank_shape, bold_shape,
            "{extension}: the blanks are plain, the label bold"
        );
        let (output, run) = fixture.edit(
            &fixture.base,
            "bold",
            json!([indent_op(at(&before, BOLD_LEAD), 1, 5.0, 0.0)]),
        );
        success(run, "indent a paragraph whose blanks end at a run boundary");
        let after = read(&output);
        let para = paragraph(&after, "Bold label tail");
        assert_eq!(
            shape_at(para, 0),
            bold_shape,
            "{extension}: the first surviving character keeps its bold shape"
        );
        assert_eq!(
            shape_at(para, "Bold label".len() as u32),
            tail_shape,
            "{extension}: the tail keeps its shape"
        );
        assert!(
            para.char_shape_runs
                .iter()
                .all(|(start, _)| *start < para.wchar_len()),
            "{extension}: no run starts past the text"
        );
    }
}

#[test]
fn indent_level_strips_blanks_that_follow_the_sections_leading_controls() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("indent-controls", extension);
        let before = read(&fixture.base);
        let first = &before.sections[0].paragraphs[0];
        assert_eq!(
            first.plain_text(),
            LEAD_FIRST,
            "{extension}: the fixture's first paragraph"
        );
        let controls = |p: &Paragraph| {
            p.chars
                .iter()
                .filter(|c| !matches!(c, hwp_model::HwpChar::Text(_)))
                .count()
        };
        assert!(
            controls(first) >= 2,
            "{extension}: section and column controls lead"
        );
        let (output, run) = fixture.edit(
            &fixture.base,
            "controls",
            json!([indent_op(
                json!({"at":{"section":0,"paragraph":0}}),
                1,
                5.0,
                0.0
            )]),
        );
        success(run, "indent the section's first paragraph");
        let after = read(&output);
        let first_after = &after.sections[0].paragraphs[0];
        assert_eq!(
            first_after.plain_text(),
            "Form title",
            "{extension}: blanks go, words stay"
        );
        assert_eq!(
            controls(first_after),
            controls(first),
            "{extension}: the leading controls stay in place"
        );
        assert!(
            first_after
                .controls
                .iter()
                .any(|c| matches!(c, Control::SectionDef(_))),
            "{extension}: the section setup is preserved"
        );
        assert_eq!(
            first_after.wchar_len(),
            first.wchar_len() - 2,
            "{extension}: exactly the two blanks left the paragraph"
        );
        let ps = shape(&after, first_after);
        assert!(
            ps.margin_left > 0,
            "{extension}: the indentation is now on the shape"
        );
    }
}

// ---- set_indent_level: range tags and native line caches (T12-F3, T12-F4) -------------------

/// HWPTAG_PARA_RANGE_TAG (HWPTAG_BEGIN + 54).
const RANGE_TAG: u16 = 0x10 + 54;

#[test]
fn indent_level_keeps_a_native_line_cache_when_nothing_changes() {
    let fixture = Fixture::new("indent-cache", "hwp");
    // Give the paragraph a native line cache and write it the way a byte-preserving round trip
    // does, so the file under test carries line segments like one Hangul saved.
    let mut seeded = read(&fixture.base);
    let index = index(&seeded, FIRST);
    seeded.sections[0].paragraphs[index].line_segs = vec![hwp_model::LineSeg {
        text_start: 0,
        v_pos: 0,
        line_height: 1000,
        text_height: 1000,
        baseline_gap: 850,
        line_spacing: 600,
        col_start: 0,
        seg_width: 40000,
        flags: 0,
    }];
    let native = fixture.dir.join("native.hwp");
    let options = hwp5::WriteOptions {
        preserve_linesegs: true,
        ..hwp5::WriteOptions::default()
    };
    hwp5::write_document(&seeded, &native, &options).unwrap();
    let before = read(&native);
    let cache = before.sections[0].paragraphs[index].line_segs.clone();
    assert!(
        !cache.is_empty(),
        "the native file carries a line cache for this paragraph"
    );
    // Level 0 with no hanging on a paragraph that has no indentation is already in effect.
    let (output, run) = fixture.edit(
        &native,
        "noop",
        json!([indent_op(at(&before, FIRST), 0, 5.0, 0.0)]),
    );
    success(run, "a request that is already in effect");
    let report: Value =
        serde_json::from_slice(&std::fs::read(fixture.dir.join("noop-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["ops"][0]["pieces_touched"], 0);
    let after = read(&output);
    assert_eq!(
        after.sections[0].paragraphs[index].line_segs, cache,
        "an op that reports no change must not drop the paragraph's native line cache"
    );
    assert_eq!(
        before.header.para_shapes.len(),
        after.header.para_shapes.len(),
        "no shape is appended"
    );
}

#[test]
fn indent_level_refuses_to_strip_blanks_under_a_range_tag_in_a_native_file() {
    let fixture = Fixture::new("indent-range", "hwp");
    let mut doc = read(&fixture.base);
    let marked = index(&doc, "    Four-space indented item.");
    doc.sections[0].paragraphs[marked]
        .extras
        .push(hwp_model::OpaqueRecord {
            tag: RANGE_TAG,
            data: vec![4, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0],
            children: Vec::new(),
        });
    let tagged = fixture.dir.join("tagged.hwp");
    hwp5::write_document(&doc, &tagged, &hwp5::WriteOptions::default()).unwrap();
    let reread = read(&tagged);
    assert!(
        reread.sections[0].paragraphs[marked]
            .extras
            .iter()
            .any(|e| e.tag == RANGE_TAG),
        "the range tag survives the round trip"
    );
    let message = fixture.refuse(
        &tagged,
        "ranged",
        json!([indent_op(
            at(&reread, "    Four-space indented item."),
            1,
            5.0,
            0.0
        )]),
    );
    assert!(
        message.contains("구간 표시") && message.contains("set_para"),
        "the refusal names the cause and the remedy: {message}"
    );
    // A paragraph with the tag but no leading blanks is not refused, and the tag stays.
    let mut doc = read(&tagged);
    let plain = index(&doc, FIRST);
    doc.sections[0].paragraphs[plain]
        .extras
        .push(hwp_model::OpaqueRecord {
            tag: RANGE_TAG,
            data: vec![0, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0],
            children: Vec::new(),
        });
    let tagged_plain = fixture.dir.join("tagged-plain.hwp");
    hwp5::write_document(&doc, &tagged_plain, &hwp5::WriteOptions::default()).unwrap();
    let (output, run) = fixture.edit(
        &tagged_plain,
        "plain-tag",
        json!([indent_op(at(&doc, FIRST), 1, 5.0, 0.0)]),
    );
    success(
        run,
        "nothing is stripped, so the range tag does not block the shape",
    );
    assert!(
        read(&output).sections[0].paragraphs[plain]
            .extras
            .iter()
            .any(|e| e.tag == RANGE_TAG),
        "the tag is carried through"
    );
}

// ---- batch ordering: run addresses after a text-moving op (T12-F5) --------------------------

const INDENTED: &str = "    Four-space indented item.";

/// `set_format` bold on the run-addressed characters `[from, to)` of paragraph `paragraph`.
fn bold_range(paragraph: usize, from: u32, to: u32) -> Value {
    json!({"op":"set_format",
           "address":{"at":{"section":0,"paragraph":paragraph,"run":0},"chars":[from, to]},
           "bold":"on"})
}

#[test]
fn run_address_after_a_stripping_indent_in_one_batch_is_refused_and_writes_nothing() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("batch-order", extension);
        let before = read(&fixture.base);
        let item = index(&before, INDENTED);
        // "Four" sits at [4, 8) while the four blanks are still there. The strip moves it to
        // [0, 4), so the second op would restyle "-spa" if it ran on the stale range.
        let message = fixture.refuse(
            &fixture.base.clone(),
            "stale-range",
            json!([
                indent_op(at(&before, INDENTED), 1, 5.0, 0.0),
                bold_range(item, 4, 8)
            ]),
        );
        assert!(
            message.contains("set_format")
                && message.contains("set_indent_level")
                && message.contains("순서"),
            "{extension}: the refusal names the ops and the ordering remedy: {message}"
        );
    }
}

#[test]
fn run_address_before_the_stripping_indent_in_one_batch_restyles_the_right_word() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("batch-order-ok", extension);
        let before = read(&fixture.base);
        let item = index(&before, INDENTED);
        let (output, run) = fixture.edit(
            &fixture.base,
            "ordered",
            json!([
                bold_range(item, 4, 8),
                indent_op(at(&before, INDENTED), 1, 5.0, 0.0)
            ]),
        );
        success(run, "set_format first, then the strip");
        let after = read(&output);
        let para = paragraph(&after, "Four-space indented item.");
        let last = para.wchar_len() - 1;
        assert_ne!(
            shape_at(para, 0),
            shape_at(para, last),
            "{extension}: the first word carries the bold shape and the tail does not"
        );
        assert_eq!(
            shape_at(para, 3),
            shape_at(para, 0),
            "{extension}: all of \"Four\" is bold"
        );
        assert_eq!(
            shape_at(para, 4),
            shape_at(para, last),
            "{extension}: the character after \"Four\" is not"
        );
    }
}

#[test]
fn run_address_after_a_length_preserving_op_in_one_batch_still_applies() {
    let fixture = Fixture::new("batch-same-length", "hwpx");
    let before = read(&fixture.base);
    let item = index(&before, INDENTED);
    // A repeat of an in-effect indent changes no text, so the range stays valid.
    let (_, run) = fixture.edit(
        &fixture.base,
        "same-length",
        json!([
            {"op":"set_para","address":at(&before, INDENTED),"left_mm":"5mm"},
            bold_range(item, 4, 8)
        ]),
    );
    success(
        run,
        "a paragraph-shape op does not move text, so the run range holds",
    );
}

// ---- set_break_before: line-cache geometry on native sources (T12-F6) -----------------------

/// Writes `fixture.base` as a native HWP whose every body paragraph carries a line cache that
/// sits far down a page, like a Hangul-saved file. Returns the file and the seeded caches.
fn seed_native_caches(fixture: &Fixture) -> (PathBuf, Document) {
    let mut doc = read(&fixture.base);
    for (index, para) in doc.sections[0].paragraphs.iter_mut().enumerate() {
        para.line_segs = vec![hwp_model::LineSeg {
            text_start: 0,
            v_pos: 20_000 + 1_600 * index as i32,
            line_height: 1000,
            text_height: 1000,
            baseline_gap: 850,
            line_spacing: 600,
            col_start: 0,
            seg_width: 40000,
            flags: 0x0006_0000,
        }];
    }
    let native = fixture.dir.join("seeded.hwp");
    let options = hwp5::WriteOptions {
        preserve_linesegs: true,
        ..hwp5::WriteOptions::default()
    };
    hwp5::write_document(&doc, &native, &options).unwrap();
    let seeded = read(&native);
    (native, seeded)
}

fn first_v_pos(para: &Paragraph) -> i32 {
    para.line_segs.first().expect("a line cache").v_pos
}

#[test]
fn break_before_on_a_native_source_restarts_the_cache_at_the_page_top() {
    let fixture = Fixture::new("break-cache", "hwp");
    let (native, before) = seed_native_caches(&fixture);
    let target = index(&before, TARGET);
    let target_para = &before.sections[0].paragraphs[target];
    assert_eq!(
        shape(&before, target_para).spacing_top,
        0,
        "the target has no space-before, so its first line sits at the page top"
    );
    assert!(
        first_v_pos(target_para) >= 20_000,
        "the seeded cache is far down the old page"
    );
    let (output, run) = fixture.edit(
        &native,
        "cache",
        json!([break_op(at(&before, TARGET), "on")]),
    );
    success(
        run,
        "set_break_before on a native source that carries line caches",
    );
    let after = read(&output);
    let paragraphs = &after.sections[0].paragraphs;
    // Genuine Hangul-saved files: the first line of a paragraph with the break bit has v_pos
    // 0 (its space-before when above 0) and no page-start flag (#420, J1).
    assert_eq!(
        first_v_pos(&paragraphs[target]),
        0,
        "the target's first line is at the page top"
    );
    assert_eq!(
        paragraphs[target].line_segs[0].flags & 1,
        0,
        "genuine files set no page-start bit"
    );
    // Everything after it was measured on the old page; none of that offset may remain.
    for (offset, para) in paragraphs.iter().enumerate().skip(target + 1) {
        let seeded = first_v_pos(&before.sections[0].paragraphs[offset]);
        assert!(
            para.line_segs
                .first()
                .is_none_or(|seg| seg.v_pos < seeded - 10_000),
            "paragraph {offset} still carries its pre-break offset: {:?}",
            para.line_segs.first().map(|seg| seg.v_pos)
        );
    }
    let next = &paragraphs[target + 1];
    assert!(
        !next.line_segs.is_empty(),
        "the following paragraph is re-measured, not dropped"
    );
    assert!(
        first_v_pos(next) > 0 && first_v_pos(next) < 10_000,
        "the following paragraph continues from the new page top: {}",
        first_v_pos(next)
    );
    // Paragraphs before the break keep their genuine caches untouched.
    for (offset, (kept, original)) in paragraphs
        .iter()
        .zip(&before.sections[0].paragraphs)
        .take(target)
        .enumerate()
    {
        assert_eq!(
            kept.line_segs, original.line_segs,
            "paragraph {offset} before the break keeps its cache"
        );
    }
}

#[test]
fn break_before_off_on_a_native_source_also_remeasures_what_follows() {
    let fixture = Fixture::new("break-cache-off", "hwp");
    let (native, before) = seed_native_caches(&fixture);
    let target = index(&before, TARGET);
    let (on, run) = fixture.edit(&native, "on", json!([break_op(at(&before, TARGET), "on")]));
    success(run, "on");
    let broken = read(&on);
    let (off, run) = fixture.edit(&on, "off", json!([break_op(at(&broken, TARGET), "off")]));
    success(run, "off");
    let after = read(&off);
    assert_eq!(
        after.sections[0].paragraphs[target].header.break_type & 0x04,
        0
    );
    // Without the break the target no longer starts a page: it continues the cursor of the
    // paragraph above it instead of sitting at 0.
    assert!(
        first_v_pos(&after.sections[0].paragraphs[target]) > 0,
        "the cleared break lets the paragraph continue the page"
    );
}
