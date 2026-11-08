//! Synthetic CLI contracts for create_style/apply_style (#427 T3).
//! Uses only existing public model/reader APIs, so a pre-feature build reaches
//! the intended CLI success assertion rather than failing to compile.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::{CharShape, Control, Document, Paragraph, Style};
use serde_json::{Value, json};

const INHERITED: &str = "Body inherits.";
const DIRECT_PARA: &str = "Direct paragraph inherits run.";
const MIXED: &str = "Body BOLD tail.";
const HEADING_LIKE: &str = "Body heading-like direct.";
const NOTE: &str = "Note untouched.";
const HEADING: &str = "Heading untouched.";
const NESTED: &str = "Nested body.";
const NESTED_NOTE: &str = "Nested note.";
const CREATED: &str = "Body thirteen";

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn success(output: Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read(path: &Path) -> Document {
    match path.extension().unwrap().to_str().unwrap() {
        "hwp" => hwp5::read_document(path).unwrap().document,
        "hwpx" => hwpx::read_document(path).unwrap().document,
        _ => panic!("fixture must use a native extension"),
    }
}

struct Fixture {
    directory: PathBuf,
    base: PathBuf,
    extension: &'static str,
}

impl Fixture {
    fn new(test: &str, extension: &'static str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "hwp-named-style-{}-{test}-{extension}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let paragraph = |style: &str, text: &str| json!({"type":"paragraph","style":style,"runs":[{"type":"text","text":text}]});
        let spec = json!({
            "version":"1.0",
            "metadata":{"title":"Synthetic named style operations","author":"hwp-cli"},
            "styles":{
                "body":{"font_family":"Fixture Serif","font_size_pt":10,"align":"justify","line_height_percent":160},
                "note":{"font_family":"Fixture Serif","font_size_pt":10,"align":"justify","line_height_percent":160},
                "heading":{"font_family":"Fixture Sans","font_size_pt":14,"bold":true,"align":"left","line_height_percent":160}
            },
            "sections":[{"blocks":[
                paragraph("heading",HEADING),
                paragraph("body",INHERITED),
                paragraph("body",DIRECT_PARA),
                {"type":"paragraph","style":"body","runs":[
                    {"type":"text","text":"Body "},
                    {"type":"text","text":"BOLD","bold":true},
                    {"type":"text","text":" tail."}
                ]},
                paragraph("note",NOTE),
                {"type":"paragraph","style":"body","runs":[
                    {"type":"text","text":HEADING_LIKE,"font_family":"Fixture Sans","font_size_pt":14,"bold":true}
                ]},
                {"type":"table","columns":[{"width_mm":40},{"width_mm":40}],"rows":[{"cells":[
                    {"blocks":[paragraph("body",NESTED)]},
                    {"blocks":[paragraph("note",NESTED_NOTE)]}
                ]}]}
            ]}]
        });
        let spec_path = directory.join("spec.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let hwpx = directory.join("composed.hwpx");
        success(
            hwp()
                .arg("compose")
                .arg(&spec_path)
                .arg("-o")
                .arg(&hwpx)
                .output()
                .unwrap(),
            "existing compose fixture setup",
        );
        let composed = if extension == "hwp" {
            let native = directory.join("composed.hwp");
            success(
                hwp()
                    .arg("convert")
                    .arg(&hwpx)
                    .arg("-o")
                    .arg(&native)
                    .output()
                    .unwrap(),
                "existing native conversion fixture setup",
            );
            native
        } else {
            hwpx
        };
        let base = directory.join(format!("base.{extension}"));
        success(
            hwp()
                .arg("edit")
                .arg(&composed)
                .arg("-o")
                .arg(&base)
                .arg("--set-align")
                .arg(format!("{DIRECT_PARA}=center"))
                .output()
                .unwrap(),
            "existing direct paragraph formatting fixture setup",
        );
        Self {
            directory,
            base,
            extension,
        }
    }

    fn path(&self, stem: &str) -> PathBuf {
        self.directory.join(format!("{stem}.{}", self.extension))
    }

    fn edit(&self, source: &Path, stem: &str, operations: Value) -> (PathBuf, Output) {
        let ops = self.directory.join(format!("{stem}-ops.json"));
        std::fs::write(&ops, serde_json::to_vec(&operations).unwrap()).unwrap();
        let output = self.path(stem);
        let run = hwp()
            .arg("edit")
            .arg(source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .arg("--report")
            .arg(self.directory.join(format!("{stem}-report.json")))
            .output()
            .unwrap();
        (output, run)
    }

    fn created(&self) -> PathBuf {
        let (output, run) = self.edit(
            &self.base,
            "created",
            json!([create(CREATED, json!("body"))]),
        );
        success(run, "create_style via typed operations");
        output
    }
}

fn create(name: &str, based_on: Value) -> Value {
    json!({"op":"create_style","name":name,"based_on":based_on,"kind":"paragraph","size":"13pt","align":"right"})
}

fn paragraph<'a>(doc: &'a Document, text: &str) -> &'a Paragraph {
    doc.sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .find(|para| para.plain_text() == text)
        .unwrap_or_else(|| panic!("fixture paragraph missing: {text}"))
}

fn cell_paragraph<'a>(doc: &'a Document, text: &str) -> &'a Paragraph {
    doc.sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|para| &para.controls)
        .filter_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .flat_map(|table| &table.cells)
        .flat_map(|cell| &cell.paragraphs)
        .find(|para| para.plain_text() == text)
        .unwrap_or_else(|| panic!("fixture cell paragraph missing: {text}"))
}

fn named<'a>(doc: &'a Document, name: &str) -> (usize, &'a Style) {
    doc.header
        .styles
        .iter()
        .enumerate()
        .find(|(_, style)| style.name == name)
        .unwrap_or_else(|| panic!("named style missing: {name}"))
}

fn shape_at<'a>(doc: &'a Document, para: &Paragraph, wchar: u32) -> &'a CharShape {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(position, _)| *position <= wchar)
        .expect("valid fixture has a covering character shape")
        .1;
    &doc.header.char_shapes[id.0 as usize]
}

fn at(doc: &Document, text: &str) -> Value {
    for (section, contents) in doc.sections.iter().enumerate() {
        if let Some(paragraph) = contents
            .paragraphs
            .iter()
            .position(|para| para.plain_text() == text)
        {
            return json!({"at":{"section":section,"paragraph":paragraph}});
        }
    }
    panic!("top-level fixture paragraph missing: {text}");
}

fn apply(address: Value, style: Value, clear_direct: Option<bool>) -> Value {
    let mut op = json!({"op":"apply_style","address":address,"style":style});
    if let Some(clear) = clear_direct {
        op["clear_direct"] = json!(clear);
    }
    op
}

fn resolved(doc: &Document, para: &Paragraph) -> Value {
    json!({
        "text":para.plain_text(),"style":doc.header.styles[para.style.0 as usize],
        "para":doc.header.para_shapes[para.para_shape.0 as usize],
        "runs":para.char_shape_runs.iter().map(|(position,id)| {
            json!([position,doc.header.char_shapes[id.0 as usize]])
        }).collect::<Vec<_>>()
    })
}

fn assert_non_targets(before: &Document, after: &Document) {
    for text in [HEADING, NOTE] {
        assert_eq!(
            resolved(before, paragraph(before, text)),
            resolved(after, paragraph(after, text)),
            "unrelated named-style paragraph changed: {text}"
        );
    }
    assert_eq!(
        resolved(before, cell_paragraph(before, NESTED_NOTE)),
        resolved(after, cell_paragraph(after, NESTED_NOTE)),
        "unrelated cell paragraph changed"
    );
}

fn para_id(path: &Path, text: &str) -> String {
    let result = hwp()
        .arg("cat")
        .arg(path)
        .args([
            "--format",
            "markdown",
            "--with-segments",
            "--segments",
            "v2",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "existing cat-v2 fixture read: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let envelope: Value = serde_json::from_slice(&result.stdout).unwrap();
    let mut document = read(path);
    let matches: Vec<_> = envelope["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|segment| {
            if segment["kind"] != "para" {
                return false;
            }
            let path = hwp_convert::segment_id::SegmentPath {
                section: segment["path"]["section"].as_u64().unwrap() as usize,
                indices: segment["path"]["indices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|index| index.as_u64().unwrap() as usize)
                    .collect(),
            };
            hwp_convert::address::paragraph_at_mut(&mut document, &path)
                .is_some_and(|paragraph| paragraph.plain_text() == text)
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "unique fixture paragraph must have one cat-v2 ID: {text}"
    );
    matches[0]["id"].as_str().unwrap().to_string()
}

fn assert_public_style_listing(path: &Path, name: &str, users: &[&str]) {
    let output = hwp()
        .arg("styles")
        .arg(path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "hwp styles must read edited output: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listing["contract"], "hwp-styles-v1");
    let row = listing["styles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap();
    let doc = read(path);
    let (id, style) = named(&doc, name);
    assert_eq!(row["id"], id);
    assert_eq!(row["kind"], "paragraph");
    assert_eq!(row["attrs"]["char"]["size_pt"].as_f64(), Some(13.0));
    assert_eq!(row["attrs"]["para"]["alignment"], "right");
    assert_eq!(row["attrs"]["char_shape_id"], style.char_shape.0);
    assert_eq!(row["attrs"]["para_shape_id"], style.para_shape.0);
    let actual: std::collections::BTreeSet<_> = row["used_by"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    let expected: std::collections::BTreeSet<_> =
        users.iter().map(|text| para_id(path, text)).collect();
    assert_eq!(
        actual, expected,
        "hwp styles must publish exactly the edited style users"
    );
}

#[test]
fn create_style_by_name_and_id_preserves_shared_shapes_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("create-shared", extension);
        let before = read(&fixture.base);
        let (body_id, body) = named(&before, "body");
        let (_, note) = named(&before, "note");
        assert_eq!(
            body.char_shape, note.char_shape,
            "fixture must exercise shared character shapes"
        );
        assert_eq!(
            body.para_shape, note.para_shape,
            "fixture must exercise shared paragraph shapes"
        );
        let (output, run) = fixture.edit(
            &fixture.base,
            "two-created",
            json!([
                create(CREATED, json!("body")),
                create("By numeric body", json!(body_id))
            ]),
        );
        success(run, "create_style with name and numeric based_on");
        assert_public_style_listing(&output, CREATED, &[]);
        let after = read(&output);
        assert_eq!(
            &after.header.styles[..before.header.styles.len()],
            before.header.styles.as_slice(),
            "old style records must not be mutated"
        );
        assert_eq!(
            &after.header.char_shapes[..before.header.char_shapes.len()],
            before.header.char_shapes.as_slice(),
            "old shared character shapes must not be mutated"
        );
        assert_eq!(
            &after.header.para_shapes[..before.header.para_shapes.len()],
            before.header.para_shapes.as_slice(),
            "old shared paragraph shapes must not be mutated"
        );
        assert_eq!(after.header.styles.len(), before.header.styles.len() + 2);
        for name in [CREATED, "By numeric body"] {
            let (_, style) = named(&after, name);
            assert_eq!(
                after.header.char_shapes[style.char_shape.0 as usize].base_size,
                1300
            );
            assert_eq!(
                after.header.para_shapes[style.para_shape.0 as usize].alignment(),
                2
            );
            assert!(!after.header.char_shapes[style.char_shape.0 as usize].is_bold());
        }
        for text in [INHERITED, DIRECT_PARA, MIXED, HEADING_LIKE, NOTE, HEADING] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text)),
                "create_style must not restyle existing users"
            );
        }
        assert_non_targets(&before, &after);
    }
}

#[test]
fn apply_style_keeps_direct_char_and_para_formatting_independent_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("apply-inheritance", extension);
        let source = fixture.created();
        let before = read(&source);
        let (created_id, created) = named(&before, CREATED);
        let old_body = named(&before, "body").1;
        let direct = paragraph(&before, DIRECT_PARA);
        assert_ne!(
            direct.para_shape, old_body.para_shape,
            "fixture must have direct paragraph formatting"
        );
        assert_eq!(
            shape_at(&before, direct, 0),
            &before.header.char_shapes[old_body.char_shape.0 as usize],
            "direct PS must retain inherited CS"
        );
        assert!(shape_at(&before, paragraph(&before, MIXED), 5).is_bold());
        let (output, run) = fixture.edit(
            &source,
            "applied",
            json!([
                apply(at(&before, INHERITED), json!(CREATED), None),
                apply(at(&before, DIRECT_PARA), json!(created_id), Some(false)),
                apply(at(&before, MIXED), json!(CREATED), None),
                apply(at(&before, HEADING_LIKE), json!(created_id), None)
            ]),
        );
        success(run, "apply_style preserving direct formatting");
        assert_public_style_listing(
            &output,
            CREATED,
            &[INHERITED, DIRECT_PARA, MIXED, HEADING_LIKE],
        );
        let after = read(&output);
        for text in [INHERITED, DIRECT_PARA, MIXED, HEADING_LIKE] {
            assert_eq!(paragraph(&after, text).style.0 as usize, created_id);
        }
        for text in [INHERITED, MIXED, HEADING_LIKE] {
            assert_eq!(
                paragraph(&after, text).para_shape,
                created.para_shape,
                "inherited PS must follow target style"
            );
        }
        assert_eq!(
            resolved(&before, direct)["para"],
            resolved(&after, paragraph(&after, DIRECT_PARA))["para"],
            "direct PS must survive independently of inherited CS"
        );
        for text in [INHERITED, DIRECT_PARA] {
            assert_eq!(
                shape_at(&after, paragraph(&after, text), 0),
                &after.header.char_shapes[created.char_shape.0 as usize]
            );
        }
        for position in [0, 10] {
            assert_eq!(
                shape_at(&after, paragraph(&after, MIXED), position),
                &after.header.char_shapes[created.char_shape.0 as usize]
            );
        }
        assert_eq!(
            shape_at(&before, paragraph(&before, MIXED), 5),
            shape_at(&after, paragraph(&after, MIXED), 5),
            "direct bold run must survive unchanged"
        );
        assert_eq!(
            shape_at(&before, paragraph(&before, HEADING_LIKE), 0),
            shape_at(&after, paragraph(&after, HEADING_LIKE), 0),
            "direct CS coinciding with another style must survive"
        );
        assert_non_targets(&before, &after);
        assert_eq!(
            resolved(&before, cell_paragraph(&before, NESTED)),
            resolved(&after, cell_paragraph(&after, NESTED))
        );
    }
}

#[test]
fn apply_style_clear_direct_resets_all_runs_and_para_shapes_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("clear-direct", extension);
        let source = fixture.created();
        let before = read(&source);
        let (id, _) = named(&before, CREATED);
        let (output, run) = fixture.edit(
            &source,
            "cleared",
            json!([
                apply(at(&before, DIRECT_PARA), json!(CREATED), Some(true)),
                apply(at(&before, MIXED), json!(id), Some(true)),
                apply(at(&before, HEADING_LIKE), json!(CREATED), Some(true))
            ]),
        );
        success(run, "apply_style clear_direct");
        assert_public_style_listing(&output, CREATED, &[DIRECT_PARA, MIXED, HEADING_LIKE]);
        let after = read(&output);
        let (_, style) = named(&after, CREATED);
        for text in [DIRECT_PARA, MIXED, HEADING_LIKE] {
            let para = paragraph(&after, text);
            assert_eq!(para.style.0 as usize, id);
            assert_eq!(para.para_shape, style.para_shape);
            assert!(!para.char_shape_runs.is_empty());
            for (_, shape) in &para.char_shape_runs {
                assert_eq!(
                    *shape, style.char_shape,
                    "clear_direct must reset every character run"
                );
            }
        }
        assert_non_targets(&before, &after);
        assert_eq!(
            resolved(&before, paragraph(&before, INHERITED)),
            resolved(&after, paragraph(&after, INHERITED))
        );
    }
}

#[test]
fn apply_style_to_nested_cell_cat_v2_id_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("nested-id", extension);
        let source = fixture.created();
        let before = read(&source);
        let id = para_id(&source, NESTED);
        let (output, run) = fixture.edit(
            &source,
            "nested-applied",
            json!([apply(json!({"id":id}), json!(CREATED), None)]),
        );
        success(run, "apply_style to nested cell paragraph ID");
        assert_public_style_listing(&output, CREATED, &[NESTED]);
        let after = read(&output);
        let (style_id, style) = named(&after, CREATED);
        let para = cell_paragraph(&after, NESTED);
        assert_eq!(para.style.0 as usize, style_id);
        assert_eq!(para.para_shape, style.para_shape);
        assert_eq!(
            shape_at(&after, para, 0),
            &after.header.char_shapes[style.char_shape.0 as usize]
        );
        for text in [INHERITED, DIRECT_PARA, MIXED, HEADING_LIKE] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
        assert_non_targets(&before, &after);
    }
}

#[test]
fn named_style_refusals_preserve_existing_destination_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("refusal", extension);
        let source = fixture.created();
        let before = read(&source);
        let cases = [
            ("duplicate", create(CREATED, json!("body"))),
            (
                "unknown-base",
                create("Unknown base", json!("missing base style")),
            ),
            (
                "unknown-style",
                apply(at(&before, INHERITED), json!("missing target style"), None),
            ),
            (
                "unknown-style-id",
                apply(at(&before, INHERITED), json!(250), None),
            ),
            (
                "invalid-address",
                apply(
                    json!({"at":{"section":0,"paragraph":10000}}),
                    json!(CREATED),
                    None,
                ),
            ),
        ];
        for (name, operation) in cases {
            let destination = fixture.path(name);
            std::fs::copy(&source, &destination).unwrap();
            let expected = std::fs::read(&destination).unwrap();
            let (_, run) = fixture.edit(&source, name, json!([operation]));
            assert!(
                !run.status.success(),
                "invalid named-style request must fail: {name}"
            );
            assert!(
                !run.stderr.is_empty(),
                "refusal must remain diagnosable: {name}"
            );
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                expected,
                "refusal must happen before destination publication: {name}"
            );
        }
    }
}

#[test]
fn apply_style_stale_cat_v2_id_after_replace_preserves_destination_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("stale-id", extension);
        let source = fixture.created();
        let old_id = para_id(&source, INHERITED);
        let changed = fixture.path("changed");
        success(
            hwp()
                .arg("edit")
                .arg(&source)
                .arg("-o")
                .arg(&changed)
                .arg("--replace")
                .arg(format!("{INHERITED}=>Changed body."))
                .output()
                .unwrap(),
            "existing replace must change the checksummed source paragraph",
        );
        assert_ne!(
            old_id,
            para_id(&changed, "Changed body."),
            "fixture must make the old ID stale"
        );
        let destination = fixture.path("stale-refused");
        std::fs::copy(&source, &destination).unwrap();
        let expected = std::fs::read(&destination).unwrap();
        let (_, run) = fixture.edit(
            &changed,
            "stale-refused",
            json!([apply(json!({"id":old_id}), json!(CREATED), None)]),
        );
        assert!(
            !run.status.success(),
            "stale apply_style ID must be refused"
        );
        assert!(
            !run.stderr.is_empty(),
            "stale address refusal must have a diagnostic"
        );
        assert_eq!(
            std::fs::read(destination).unwrap(),
            expected,
            "stale ID must refuse before writing"
        );
    }
}

#[test]
fn repeated_apply_style_is_byte_identical_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("idempotent", extension);
        let source = fixture.created();
        let before = read(&source);
        let operations = json!([apply(at(&before, MIXED), json!(CREATED), None)]);
        let (once, run) = fixture.edit(&source, "once", operations.clone());
        success(run, "first apply_style");
        let (twice, run) = fixture.edit(&once, "twice", operations);
        success(run, "identical repeated apply_style");
        let report: Value = serde_json::from_slice(
            &std::fs::read(fixture.directory.join("twice-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["ops"][0]["status"], "applied");
        assert_eq!(report["ops"][0]["pieces_touched"], 0);
        assert_eq!(report["ops"][0]["changed"], json!([]));
        assert_eq!(
            std::fs::read(once).unwrap(),
            std::fs::read(twice).unwrap(),
            "repeating apply_style must be a byte-stable no-op"
        );
    }
}

#[test]
fn hwp_style_id_overflow_refuses_before_publication_but_hwpx_keeps_u16_ids() {
    let fixture = Fixture::new("native-style-capacity", "hwpx");
    let mut doc = read(&fixture.base);
    let base = doc.header.styles[0].clone();
    while doc.header.styles.len() < 257 {
        let mut style = base.clone();
        style.name = format!("Capacity {}", doc.header.styles.len());
        style.english_name = style.name.clone();
        doc.header.styles.push(style);
    }
    let source = fixture.path("many-styles");
    hwpx::write_document(&doc, &source).unwrap();
    let loaded = read(&source);
    assert_eq!(loaded.header.styles.len(), 257);
    let cases = [
        create("Beyond native capacity", json!(0)),
        apply(at(&loaded, INHERITED), json!(256), None),
    ];
    for (index, operation) in cases.into_iter().enumerate() {
        let operations = fixture.directory.join(format!("capacity-{index}.json"));
        std::fs::write(
            &operations,
            serde_json::to_vec(&json!([operation])).unwrap(),
        )
        .unwrap();
        let output = fixture.directory.join(format!("capacity-{index}.hwp"));
        let sentinel = b"existing destination must survive";
        std::fs::write(&output, sentinel).unwrap();
        let run = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&operations)
            .output()
            .unwrap();
        assert!(!run.status.success(), "HWP StyleId must never wrap");
        assert!(
            String::from_utf8_lossy(&run.stderr).contains("HWPX"),
            "refusal must name a usable output format"
        );
        assert_eq!(std::fs::read(output).unwrap(), sentinel);
    }
    let (output, run) = fixture.edit(
        &source,
        "wide-id",
        json!([apply(at(&loaded, INHERITED), json!(256), None)]),
    );
    success(run, "HWPX keeps its u16 style id");
    assert_eq!(paragraph(&read(&output), INHERITED).style.0, 256);
}

#[test]
fn named_style_schema_refuses_unknown_fields_and_run_addresses_before_writing() {
    let fixture = Fixture::new("schema-boundary", "hwpx");
    let before = read(&fixture.base);
    let mut unknown = create("Unknown property", json!("body"));
    unknown["font_size"] = json!(13);
    let mut nested = create("Nested properties", json!("body"));
    nested["character"] = json!({"bold":"on"});
    let cases = [
        unknown,
        nested,
        json!({"op":"create_style","name":"Missing kind"}),
        json!({"op":"create_style","name":"Wrong kind","kind":"PARA"}),
        apply(
            json!({"at":{"section":0,"paragraph":1,"run":0}}),
            json!("body"),
            None,
        ),
        apply(
            json!({"at":{"section":0,"paragraph":1},"chars":[0,1]}),
            json!("body"),
            None,
        ),
        apply(at(&before, INHERITED), json!(-1), None),
    ];
    for (index, operation) in cases.into_iter().enumerate() {
        let stem = format!("schema-{index}");
        let output = fixture.path(&stem);
        std::fs::write(&output, b"sentinel").unwrap();
        let (_, run) = fixture.edit(&fixture.base, &stem, json!([operation]));
        assert!(
            !run.status.success(),
            "invalid named-style contract must refuse"
        );
        assert_eq!(std::fs::read(output).unwrap(), b"sentinel");
    }
}

#[test]
fn character_style_requests_refuse_and_existing_character_styles_are_preserved_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("character-kind", extension);
        let destination = fixture.path("character-create-refused");
        std::fs::write(&destination, b"sentinel").unwrap();
        let (_, run) = fixture.edit(
            &fixture.base,
            "character-create-refused",
            json!([
                {"op":"create_style","name":"Character accent","based_on":"body",
                 "kind":"character","bold":"on"}
            ]),
        );
        assert!(
            !run.status.success(),
            "T3 supports paragraph styles only, for {extension}"
        );
        assert!(String::from_utf8_lossy(&run.stderr).contains("#427"));
        assert_eq!(std::fs::read(destination).unwrap(), b"sentinel");

        // A pre-existing character style is source data, not a request to create one.
        let mut doc = read(&fixture.base);
        let mut accent = named(&doc, "body").1.clone();
        accent.name = "Existing character accent".into();
        accent.english_name = accent.name.clone();
        accent.attr = (accent.attr & !7) | 1;
        doc.header.styles.push(accent);
        let source = fixture.path("existing-character-source");
        if extension == "hwp" {
            hwp5::write_document(&doc, &source, &hwp5::WriteOptions::default()).unwrap();
        } else {
            hwpx::write_document(&doc, &source).unwrap();
        }
        let before = read(&source);
        assert_eq!(
            named(&before, "Existing character accent").1.attr & 7,
            1,
            "{extension}: existing source must retain character kind (PR #442 prerequisite)"
        );
        let (output, run) = fixture.edit(
            &source,
            "character-preserved",
            json!([create("Unrelated paragraph style", json!("body"))]),
        );
        success(
            run,
            "creating a paragraph style preserves existing character styles",
        );
        let after = read(&output);
        assert_eq!(
            named(&before, "Existing character accent"),
            named(&after, "Existing character accent")
        );
        let destination = fixture.path("character-apply-refused");
        std::fs::write(&destination, b"sentinel").unwrap();
        let (_, run) = fixture.edit(
            &output,
            "character-apply-refused",
            json!([apply(
                at(&after, INHERITED),
                json!("Existing character accent"),
                None
            )]),
        );
        assert!(
            !run.status.success(),
            "a paragraph address cannot apply a character style"
        );
        assert!(String::from_utf8_lossy(&run.stderr).contains("#427"));
        assert_eq!(std::fs::read(destination).unwrap(), b"sentinel");
    }
}

#[test]
fn unknown_hwpx_kind_refuses_application_and_explicit_creation_verifies() {
    let fixture = Fixture::new("unknown-kind", "hwpx");
    let mut doc = read(&fixture.base);
    let (body_id, _) = named(&doc, "body");
    doc.header.styles[body_id].kind_unknown = true;
    let source = fixture.path("unknown-source");
    hwpx::write_document(&doc, &source).unwrap();
    let before = read(&source);
    assert!(named(&before, "body").1.kind_unknown);
    let address = at(&before, INHERITED);
    let output = fixture.path("unknown-refused");
    std::fs::write(&output, b"sentinel").unwrap();
    let (_, run) = fixture.edit(
        &source,
        "unknown-refused",
        json!([apply(address.clone(), json!("body"), None)]),
    );
    assert!(
        !run.status.success(),
        "an unknown kind cannot be applied as a paragraph style"
    );
    assert!(String::from_utf8_lossy(&run.stderr).contains("hwp styles"));
    assert_eq!(std::fs::read(output).unwrap(), b"sentinel");

    let operations = fixture.directory.join("unknown-ops.json");
    std::fs::write(
        &operations,
        serde_json::to_vec(&json!([
            create(CREATED, json!("body")),
            apply(address, json!(CREATED), None)
        ]))
        .unwrap(),
    )
    .unwrap();
    let output = fixture.path("explicit-created");
    success(
        hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&operations)
            .arg("--verify")
            .output()
            .unwrap(),
        "explicit paragraph creation from unknown HWPX base passes verification",
    );
    let after = read(&output);
    assert!(!named(&after, CREATED).1.kind_unknown);
    assert_eq!(named(&after, CREATED).1.attr & 7, 0);
    assert!(named(&after, "body").1.kind_unknown);
    assert_public_style_listing(&output, CREATED, &[INHERITED]);
}

fn assert_empty_native_runs_apply(old_shape: u16, extension: &str) {
    let fixture = Fixture::new(&format!("empty-runs-{old_shape}-{extension}"), "hwp");
    let mut doc = read(&fixture.base);
    let (body_id, _) = named(&doc, "body");
    doc.header.styles[body_id].char_shape = hwp_model::CharShapeId(old_shape);
    let edited_paragraph = doc
        .sections
        .iter_mut()
        .flat_map(|section| &mut section.paragraphs)
        .find(|paragraph| paragraph.plain_text() == INHERITED)
        .unwrap();
    edited_paragraph.char_shape_runs.clear();
    let source = fixture.path("empty-native-source");
    hwp5::write_document(&doc, &source, &hwp5::WriteOptions::default()).unwrap();
    let before = read(&source);
    assert!(
        paragraph(&before, INHERITED).char_shape_runs.is_empty(),
        "the CLI source must reach apply_style with no run record"
    );
    assert!(!paragraph(&before, INHERITED).plain_text().is_empty());
    let operations = fixture.directory.join("empty-native-ops.json");
    std::fs::write(
        &operations,
        serde_json::to_vec(&json!([
            create(CREATED, json!("body")),
            apply(at(&before, INHERITED), json!(CREATED), None)
        ]))
        .unwrap(),
    )
    .unwrap();
    let output = fixture.directory.join(format!("empty-applied.{extension}"));
    success(
        hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&operations)
            .output()
            .unwrap(),
        "apply_style with absent native run record",
    );

    // HWPX serialization itself fills in shape0. Check the CLI's pre-native IR as well,
    // so the direct-zero branch cannot pass merely because serialization hides empty runs.
    let ir_path = fixture.directory.join("empty-applied.json");
    success(
        hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&ir_path)
            .arg("--ops")
            .arg(&operations)
            .output()
            .unwrap(),
        "inspect the edited IR before native serialization",
    );
    let ir = hwp_convert::from_json(&std::fs::read_to_string(ir_path).unwrap()).unwrap();
    let target = named(&ir, CREATED).1.char_shape;
    assert_ne!(target.0, 0);
    let expected = if old_shape == 0 {
        target
    } else {
        hwp_model::CharShapeId(0)
    };
    assert_eq!(
        paragraph(&ir, INHERITED).char_shape_runs,
        vec![(0, expected)],
        "CLI must materialize effective shape0 before applying the follow rule ({extension}, old={old_shape})"
    );
    let after = read(&output);
    let target = named(&after, CREATED).1.char_shape;
    let expected = if old_shape == 0 {
        target
    } else {
        hwp_model::CharShapeId(0)
    };
    assert_eq!(
        paragraph(&after, INHERITED).char_shape_runs,
        vec![(0, expected)],
        "native output must preserve the inherited/direct decision ({extension}, old={old_shape})"
    );
}

#[test]
fn empty_native_runs_follow_style_zero_in_hwp_output() {
    assert_empty_native_runs_apply(0, "hwp");
}
#[test]
fn empty_native_runs_keep_direct_zero_in_hwp_output() {
    assert_empty_native_runs_apply(1, "hwp");
}
#[test]
fn empty_native_runs_follow_style_zero_in_hwpx_output() {
    assert_empty_native_runs_apply(0, "hwpx");
}
#[test]
fn empty_native_runs_keep_direct_zero_in_hwpx_output() {
    assert_empty_native_runs_apply(1, "hwpx");
}
