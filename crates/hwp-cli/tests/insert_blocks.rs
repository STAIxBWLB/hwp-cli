//! Synthetic text/break insert_blocks CLI contracts (#428 T2).
//! Existing APIs only: baseline code must compile and refuse the new op at runtime.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::{Control, Document, Paragraph, Table};
use serde_json::{Value, json};

const ANCHOR: &str = "Body anchor.";
const PREFIX: &str = "Body prefix.";
const TAIL: &str = "Body tail.";
const CELL: &str = "Cell anchor.";

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
fn block(style: &str, text: &str) -> Value {
    json!({"type":"paragraph","style":style,"runs":[{"type":"text","text":text}]})
}
fn insert(address: Value, position: &str, policy: &str, styles: Value, blocks: Value) -> Value {
    json!({"op":"insert_blocks","address":address,"position":position,"missing_style":policy,"styles":styles,"blocks":blocks})
}
fn read(path: &Path) -> Document {
    if path.extension().unwrap() == "hwp" {
        hwp5::read_document(path).unwrap().document
    } else {
        hwpx::read_document(path).unwrap().document
    }
}
fn index(doc: &Document, text: &str) -> usize {
    doc.sections[0]
        .paragraphs
        .iter()
        .position(|p| p.plain_text() == text)
        .unwrap()
}
fn at(doc: &Document, text: &str) -> Value {
    json!({"at":{"section":0,"paragraph":index(doc,text)}})
}
fn paragraph<'a>(doc: &'a Document, text: &str) -> &'a Paragraph {
    &doc.sections[0].paragraphs[index(doc, text)]
}
fn table(doc: &Document) -> &Table {
    doc.sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .find_map(|control| {
            if let Control::Table(table) = control {
                Some(table)
            } else {
                None
            }
        })
        .unwrap()
}
fn resolved(doc: &Document, para: &Paragraph) -> Value {
    json!({"text":para.plain_text(),"style":doc.header.styles[para.style.0 as usize],
        "para":doc.header.para_shapes[para.para_shape.0 as usize],
        "runs":para.char_shape_runs.iter().map(|(position,id)|json!([position,doc.header.char_shapes[id.0 as usize]])).collect::<Vec<_>>()})
}

fn assert_policy_report(report: &Value, expected: &str) {
    for operation in report["ops"].as_array().unwrap() {
        assert!(
            operation["reason"].is_null(),
            "applied reason must remain null: {operation}"
        );
        assert_eq!(
            operation["missing_style"], expected,
            "selected policy needs its own report field"
        );
    }
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(report));
    let mut broken = report.clone();
    broken["ops"][0]
        .as_object_mut()
        .unwrap()
        .remove("missing_style");
    assert!(
        !validator.is_valid(&broken),
        "applied insert_blocks needs its policy"
    );
    let mut broken = report.clone();
    broken["ops"][0]["missing_style"] = json!("fallback");
    assert!(!validator.is_valid(&broken), "unknown policies are refused");
    let mut broken = report.clone();
    broken["ops"][0]["op"] = json!("create_style");
    assert!(
        !validator.is_valid(&broken),
        "legacy outcomes cannot carry this policy field"
    );
    let mut broken = report.clone();
    broken["ops"][0]["reason"] = json!("applied policy cannot overload the failure reason");
    assert!(
        !validator.is_valid(&broken),
        "reason is reserved for failures"
    );
}

struct Fixture {
    dir: PathBuf,
    base: PathBuf,
    extension: &'static str,
}
impl Fixture {
    fn new(label: &str, extension: &'static str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hwp-insert-blocks-{}-{label}-{extension}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = json!({"version":"1.0","styles":{
            "body":{"font_family":"Fixture Serif","font_size_pt":10,"align":"left"},
            "note":{"font_family":"Fixture Serif","font_size_pt":9,"align":"right"}
        },"sections":[{"blocks":[block("body",PREFIX),block("body",ANCHOR),block("note",TAIL),
            {"type":"table","columns":[{"width_mm":40},{"width_mm":40}],"rows":[{"cells":[
                {"blocks":[block("body",CELL)]},{"blocks":[block("note","Other cell.")]}
            ]}]}
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
            "existing compose fixture setup",
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
                "existing HWP fixture conversion",
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
    fn edit(&self, source: &Path, stem: &str, ops: Value, partial: bool) -> (PathBuf, Output) {
        let ops_path = self.dir.join(format!("{stem}-ops.json"));
        std::fs::write(&ops_path, serde_json::to_vec(&ops).unwrap()).unwrap();
        let output = self.path(stem);
        let mut command = hwp();
        command
            .arg("edit")
            .arg(source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(ops_path)
            .arg("--report")
            .arg(self.dir.join(format!("{stem}-report.json")));
        if partial {
            command.arg("--allow-partial");
        }
        (output, command.output().unwrap())
    }
    fn refuse(&self, source: &Path, stem: &str, operations: Value, partial: bool) -> String {
        let source_bytes = std::fs::read(source).unwrap();
        let destination = self.path(stem);
        std::fs::copy(source, &destination).unwrap();
        let destination_bytes = std::fs::read(&destination).unwrap();
        let (_, run) = self.edit(source, stem, operations, partial);
        assert!(
            !run.status.success(),
            "invalid component must refuse ({stem}, allow_partial={partial})"
        );
        assert!(
            !run.stderr.is_empty(),
            "component refusal must explain its cause"
        );
        assert!(
            std::fs::read(source).unwrap() == source_bytes,
            "refusal changed source bytes: {stem}"
        );
        assert!(
            std::fs::read(destination).unwrap() == destination_bytes,
            "refusal published partial output: {stem}"
        );
        String::from_utf8_lossy(&run.stderr).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn para_id(path: &Path, text: &str) -> String {
    let run = hwp()
        .arg("cat")
        .arg(path)
        .args(["--format", "json", "--with-segments", "--segments", "v2"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "existing cat-v2 address read failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let envelope: Value = serde_json::from_slice(&run.stdout).unwrap();
    let mut doc = read(path);
    let matches: Vec<_> = envelope["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|segment| {
            if segment["kind"] != "para" {
                return false;
            }
            let path = hwp_convert::SegmentPath {
                section: segment["path"]["section"].as_u64().unwrap() as usize,
                indices: segment["path"]["indices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as usize)
                    .collect(),
            };
            hwp_convert::address::paragraph_at_mut(&mut doc, &path)
                .is_some_and(|p| p.plain_text() == text)
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "authored marker must have one paragraph ID: {text}"
    );
    matches[0]["id"].as_str().unwrap().to_owned()
}

#[test]
fn paragraphs_and_page_column_breaks_insert_before_after_original_addresses_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("order", extension);
        let before = read(&fixture.base);
        let tail_id = para_id(&fixture.base, TAIL);
        let (output,run) = fixture.edit(&fixture.base,"ordered",json!([
            insert(at(&before,ANCHOR),"before","refuse",json!({}),json!([
                block("body","Before inserted."),{"type":"break","kind":"page"},block("body","After page break.")
            ])),
            insert(json!({"id":tail_id}),"after","refuse",json!({}),json!([
                {"type":"break","kind":"column"},block("body","After column break.")
            ]))
        ]),false);
        success(
            run,
            "insert_blocks paragraphs/breaks with original-address offset tracking",
        );
        let report: Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.join("ordered-report.json")).unwrap(),
        )
        .unwrap();
        assert_policy_report(&report, "refuse");
        let after = read(&output);
        let paragraphs = &after.sections[0].paragraphs;
        let prefix = index(&after, PREFIX);
        assert_eq!(paragraphs[prefix + 1].plain_text(), "Before inserted.");
        assert_eq!(paragraphs[prefix + 2].plain_text(), "");
        assert_ne!(
            paragraphs[prefix + 2].header.break_type & 4,
            0,
            "page break is a separate authored paragraph"
        );
        assert_eq!(paragraphs[prefix + 3].plain_text(), "After page break.");
        assert_eq!(paragraphs[prefix + 4].plain_text(), ANCHOR);
        let tail = index(&after, TAIL);
        assert_eq!(paragraphs[tail + 1].plain_text(), "");
        assert_ne!(
            paragraphs[tail + 1].header.break_type & 8,
            0,
            "column break remains explicit"
        );
        assert_eq!(paragraphs[tail + 2].plain_text(), "After column break.");
        for text in [PREFIX, ANCHOR, TAIL] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
        let cat = hwp().arg("cat").arg(&output).output().unwrap();
        assert!(cat.status.success());
        let text = String::from_utf8(cat.stdout).unwrap();
        let positions: Vec<_> = [
            PREFIX,
            "Before inserted.",
            "After page break.",
            ANCHOR,
            TAIL,
            "After column break.",
        ]
        .iter()
        .map(|marker| text.find(marker).unwrap())
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "cat must show inserted content in document order"
        );
    }
}

#[test]
fn existing_target_style_wins_over_supplied_definition_and_lists_new_user() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("target-style", extension);
        let (source, setup) = fixture.edit(
            &fixture.base,
            "target-created",
            json!([{"op":"create_style","name":"target-created","kind":"paragraph","based_on":"body","size":"11pt"}]),
            false,
        );
        success(setup, "existing create_style target setup");
        let legacy: Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.join("target-created-report.json")).unwrap(),
        )
        .unwrap();
        assert!(
            legacy["ops"]
                .as_array()
                .unwrap()
                .iter()
                .all(|operation| operation.get("missing_style").is_none()
                    && operation["reason"].is_null()),
            "legacy outcomes keep their original report shape"
        );
        let before = read(&source);
        let (body_id, body) = before
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == "target-created")
            .unwrap();
        let inserted = "Literal {{name}} => & value.";
        let (output, run) = fixture.edit(
            &source,
            "target-wins",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({"target-created":{"font_size_pt":27,"bold":true,"align":"right"}}),
                json!([block("target-created", inserted)])
            )]),
            false,
        );
        success(
            run,
            "insert_blocks resolving target style before conflicting component definition",
        );
        let after = read(&output);
        let p = paragraph(&after, inserted);
        assert_eq!(p.style.0 as usize, body_id);
        assert_eq!(p.para_shape, body.para_shape);
        assert!(
            p.char_shape_runs
                .iter()
                .all(|(_, id)| *id == body.char_shape)
        );
        assert_eq!(&after.header.styles[body_id], body);
        assert_eq!(
            after.header.styles.len(),
            before.header.styles.len(),
            "conflicting supplied definition must not add a duplicate target name"
        );
        let run = hwp()
            .arg("styles")
            .arg(&output)
            .args(["--format", "json"])
            .output()
            .unwrap();
        assert!(run.status.success());
        let listing: Value = serde_json::from_slice(&run.stdout).unwrap();
        let row = listing["styles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "target-created")
            .unwrap();
        assert_eq!(row["id"], body_id);
        assert_eq!(row["attrs"]["char"]["size_pt"], 11.0);
        assert_eq!(row["attrs"]["para"]["alignment"], "left");
        let inserted_id = para_id(&output, inserted);
        assert!(
            row["used_by"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id.as_str() == Some(inserted_id.as_str()))
        );
        assert_eq!(
            paragraph(&after, inserted).plain_text(),
            inserted,
            "component values are literal, not variable substitution"
        );
    }
}

#[test]
fn missing_style_create_uses_component_definition_without_changing_existing_users() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("create-style", extension);
        let before = read(&fixture.base);
        let (output, run) = fixture.edit(
            &fixture.base,
            "created-style",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({"component":{"based_on":"body","font_size_pt":13,"align":"right"}}),
                json!([block("component", "Missing style created.")])
            )]),
            false,
        );
        success(
            run,
            "insert_blocks creating a missing component style from DocumentSpec definition",
        );
        let after = read(&output);
        let (id, style) = after
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == "component")
            .unwrap();
        assert_eq!(
            after
                .header
                .styles
                .iter()
                .filter(|s| s.name == "component")
                .count(),
            1
        );
        assert_eq!(
            paragraph(&after, "Missing style created.").style.0 as usize,
            id
        );
        assert_eq!(
            after.header.char_shapes[style.char_shape.0 as usize].base_size,
            1300
        );
        assert_eq!(
            after.header.para_shapes[style.para_shape.0 as usize].alignment(),
            2
        );
        for text in [PREFIX, ANCHOR, TAIL] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
    }
}

#[test]
fn missing_style_refusal_is_atomic_even_with_allow_partial() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("missing-refuse", extension);
        let source = fixture.base.clone();
        let before = read(&source);
        for partial in [false, true] {
            for (label, policy, styles) in [
                (
                    "refuse-defined",
                    "refuse",
                    json!({"missing":{"font_size_pt":13}}),
                ),
                ("create-undefined", "create", json!({})),
            ] {
                let diagnostic = fixture.refuse(
                    &source,
                    &format!("{label}-{partial}"),
                    json!([
                        insert(
                            at(&before, PREFIX),
                            "after",
                            "refuse",
                            json!({}),
                            json!([block("body", "Never publish partial content.")])
                        ),
                        insert(
                            at(&before, ANCHOR),
                            "after",
                            policy,
                            styles,
                            json!([block("missing", "Missing style refuses.")])
                        )
                    ]),
                    partial,
                );
                assert!(
                    diagnostic.contains("스타일 'missing'")
                        && diagnostic.contains("styles 정의")
                        && diagnostic.contains("missing_style=create")
                        && diagnostic.contains("hwp styles"),
                    "missing-style refusal must explain the named cause and remedy, not an unknown-op schema error: {diagnostic}"
                );
            }
        }
    }
}

#[test]
fn stale_component_address_refuses_before_publication() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("stale", extension);
        let source = fixture.base.clone();
        let id = para_id(&source, ANCHOR);
        let changed = fixture.path("replaced");
        success(
            hwp()
                .arg("edit")
                .arg(&source)
                .arg("-o")
                .arg(&changed)
                .arg("--replace")
                .arg(format!("{ANCHOR}=>Changed anchor."))
                .output()
                .unwrap(),
            "existing replace makes the captured ID stale",
        );
        assert_ne!(id, para_id(&changed, "Changed anchor."));
        let diagnostic = fixture.refuse(
            &changed,
            "stale-refused",
            json!([insert(
                json!({"id":id}),
                "after",
                "refuse",
                json!({}),
                json!([block("body", "Must not appear.")])
            )]),
            false,
        );
        assert!(
            diagnostic.contains("체크섬이 일치하지 않습니다")
                && diagnostic.contains("기대")
                && diagnostic.contains("실제"),
            "stale address must fail at the existing drift check, not unknown-op schema rejection: {diagnostic}"
        );
    }
}

#[test]
fn cell_component_insertion_preserves_other_cell_and_body_paragraphs() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("cell", extension);
        let before = read(&fixture.base);
        let cell_id = para_id(&fixture.base, CELL);
        let (output, run) = fixture.edit(
            &fixture.base,
            "cell-inserted",
            json!([insert(
                json!({"id":cell_id}),
                "after",
                "refuse",
                json!({}),
                json!([
                    block("body", "Cell inserted."),
                    block("body", "Cell second.")
                ])
            )]),
            false,
        );
        success(run, "insert_blocks at an actual nested cell paragraph ID");
        let after = read(&output);
        assert_eq!(
            table(&after).cells[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            [CELL, "Cell inserted.", "Cell second."]
        );
        assert_eq!(
            resolved(&before, &table(&before).cells[1].paragraphs[0]),
            resolved(&after, &table(&after).cells[1].paragraphs[0])
        );
        for text in [PREFIX, ANCHOR, TAIL] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
        assert_eq!(
            before.sections[0]
                .paragraphs
                .iter()
                .map(|paragraph| paragraph.chars.clone())
                .collect::<Vec<_>>(),
            after.sections[0]
                .paragraphs
                .iter()
                .map(|paragraph| paragraph.chars.clone())
                .collect::<Vec<_>>(),
            "cell insertion must not splice into the body list"
        );
    }
}

#[test]
fn section_break_stays_unsupported_and_refuses_atomically() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("section-break", extension);
        let source = fixture.base.clone();
        let before = read(&source);
        let diagnostic = fixture.refuse(
            &source,
            "section-break",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                json!([block("body", "Not partially published."), {"type":"break","kind":"section"}])
            )]),
            false,
        );
        assert!(
            diagnostic.contains("section"),
            "section break must name itself in the refusal: {diagnostic}"
        );
    }
}

#[test]
fn omitted_missing_style_policy_defaults_to_create_from_definition() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("default-policy", extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({"default-created":{"based_on":"body","font_size_pt":12}}),
            json!([block("default-created", "Default policy creates.")]),
        );
        operation.as_object_mut().unwrap().remove("missing_style");
        let (output, run) =
            fixture.edit(&fixture.base, "default-created", json!([operation]), false);
        success(
            run,
            "omitted missing_style must use the owner-selected create policy",
        );
        let report: Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.join("default-created-report.json")).unwrap(),
        )
        .unwrap();
        assert_policy_report(&report, "create");
        let after = read(&output);
        let (id, style) = after
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, style)| style.name == "default-created")
            .unwrap();
        let inserted = paragraph(&after, "Default policy creates.");
        assert_eq!(inserted.style.0 as usize, id);
        assert_eq!(
            after.header.char_shapes[style.char_shape.0 as usize].base_size,
            1200
        );
        assert_eq!(
            index(&after, "Default policy creates."),
            index(&after, ANCHOR) + 1
        );
    }
}

#[test]
fn bullet_and_ordered_components_use_local_lists_and_target_named_styles() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("lists", extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([
                {"type":"paragraph","list":{"name":"component-bullets","level":0},"runs":[{"type":"text","text":"Bullet component."}]},
                {"type":"paragraph","style":"body","list":{"name":"component-ordered","level":0},"runs":[{"type":"text","text":"Ordered component."}]}
            ]),
        );
        operation["lists"] = json!({
            "component-bullets":{"kind":"bullet","levels":[{"marker":"•","style":"note"}]},
            "component-ordered":{"kind":"ordered","levels":[{"marker":"{n}.","style":"note"}]}
        });
        let (output, run) =
            fixture.edit(&fixture.base, "lists-inserted", json!([operation]), false);
        success(
            run,
            "insert_blocks bullet/ordered components with explicit list definitions",
        );
        let after = read(&output);
        for (text, target, head_type) in [
            ("Bullet component.", "note", 3),
            ("Ordered component.", "body", 2),
        ] {
            let (id, style) = before
                .header
                .styles
                .iter()
                .enumerate()
                .find(|(_, style)| style.name == target)
                .unwrap();
            let inserted = paragraph(&after, text);
            assert_eq!(
                inserted.style.0 as usize, id,
                "explicit paragraph style wins; otherwise list-level style supplies the named target"
            );
            assert!(
                inserted
                    .char_shape_runs
                    .iter()
                    .all(|(_, id)| *id == style.char_shape)
            );
            let para_shape = &after.header.para_shapes[inserted.para_shape.0 as usize];
            assert_eq!(para_shape.head_type(), head_type);
            assert_eq!(para_shape.head_level(), 1);
            assert_eq!(
                para_shape.alignment(),
                before.header.para_shapes[style.para_shape.0 as usize].alignment()
            );
            if head_type == 3 {
                assert!(usize::from(para_shape.numbering_id) < after.header.bullet_chars.len());
                assert_eq!(
                    after.header.bullet_chars[usize::from(para_shape.numbering_id)],
                    '•'
                );
            } else {
                assert!(usize::from(para_shape.numbering_id) < after.header.numbering_levels.len());
            }
        }
        assert_eq!(
            index(&after, "Bullet component."),
            index(&after, ANCHOR) + 1
        );
        assert_eq!(
            index(&after, "Ordered component."),
            index(&after, ANCHOR) + 2
        );
        for text in [PREFIX, ANCHOR, TAIL] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
    }
}

#[test]
fn missing_component_list_definition_refuses_with_specific_remedy() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("missing-list", extension);
        let before = read(&fixture.base);
        let diagnostic = fixture.refuse(&fixture.base, "missing-list-refused", json!([
            insert(at(&before, ANCHOR), "after", "refuse", json!({}), json!([
                {"type":"paragraph","style":"body","list":{"name":"component-list","level":0},"runs":[{"type":"text","text":"List must refuse."}]}
            ]))
        ]), true);
        assert!(
            diagnostic.contains("목록 'component-list'")
                && diagnostic.contains("lists 정의")
                && diagnostic.contains("추가")
                && diagnostic.contains("다시"),
            "missing-list error must identify the cause and definition/retry remedy: {diagnostic}"
        );
    }
}

fn char_shape_at<'a>(doc: &'a Document, para: &Paragraph, wchar: u32) -> &'a hwp_model::CharShape {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(position, _)| *position <= wchar)
        .unwrap()
        .1;
    &doc.header.char_shapes[usize::from(id.0)]
}

#[test]
fn line_break_and_direct_run_properties_preserve_target_named_run_style() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("inline-format", extension);
        let before = read(&fixture.base);
        let (body_id, body) = before
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, style)| style.name == "body")
            .unwrap();
        let (_, note) = before
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, style)| style.name == "note")
            .unwrap();
        let (output, run) = fixture.edit(&fixture.base, "inline-inserted", json!([
            insert(at(&before, ANCHOR), "after", "refuse", json!({"note":{"font_size_pt":27}}), json!([
                {"type":"paragraph","style":"body","runs":[
                    {"type":"text","style":"note","text":"Styled","font_size_pt":12,"bold":true,"italic":true,"color":"#123456"},
                    {"type":"line_break"},
                    {"type":"text","style":"note","text":"Second line."}
                ]}
            ]))
        ]), false);
        success(
            run,
            "insert_blocks line break and direct run properties over a target named run style",
        );
        let after = read(&output);
        let inserted = paragraph(&after, "Styled\nSecond line.");
        assert_eq!(inserted.style.0 as usize, body_id);
        assert_eq!(inserted.para_shape, body.para_shape);
        assert!(inserted.chars.contains(&hwp_model::HwpChar::CharCtrl(
            hwp_model::ctrl_char::LINE_BREAK
        )));
        let direct = char_shape_at(&after, inserted, 0);
        assert_eq!(direct.base_size, 1200);
        assert!(direct.is_bold() && direct.is_italic());
        assert_eq!(direct.text_color, 0x0056_3412);
        assert_eq!(
            direct.face_ids,
            before.header.char_shapes[usize::from(note.char_shape.0)].face_ids
        );
        assert_eq!(
            char_shape_at(&after, inserted, 7),
            &after.header.char_shapes[usize::from(note.char_shape.0)],
            "the following run must use target note=9pt rather than conflicting supplied note=27pt"
        );
        for text in [PREFIX, ANCHOR, TAIL] {
            assert_eq!(
                resolved(&before, paragraph(&before, text)),
                resolved(&after, paragraph(&after, text))
            );
        }
    }
}

fn nested_fixture(extension: &'static str) -> Fixture {
    let mut fixture = Fixture::new("deep-cell", extension);
    let mut spec: Value =
        serde_json::from_slice(&std::fs::read(fixture.dir.join("spec.json")).unwrap()).unwrap();
    spec["sections"][0]["blocks"][3]["rows"][0]["cells"][0]["blocks"] = json!([
        block("body", CELL),
        {"type":"table","columns":[{"width_mm":20}],"rows":[{"cells":[{"blocks":[block("body","Deep cell anchor.")]}]}]}
    ]);
    let spec_path = fixture.dir.join("nested-spec.json");
    std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
    let hwpx = fixture.dir.join("nested-base.hwpx");
    success(
        hwp()
            .arg("compose")
            .arg(spec_path)
            .arg("-o")
            .arg(&hwpx)
            .output()
            .unwrap(),
        "existing compose creates nested table-cell fixture",
    );
    fixture.base = if extension == "hwp" {
        let native = fixture.dir.join("nested-base.hwp");
        success(
            hwp()
                .arg("convert")
                .arg(hwpx)
                .arg("-o")
                .arg(&native)
                .output()
                .unwrap(),
            "existing conversion of nested-cell fixture",
        );
        native
    } else {
        hwpx
    };
    fixture
}

fn deep_table(doc: &Document) -> &Table {
    table(doc).cells[0]
        .paragraphs
        .iter()
        .flat_map(|para| &para.controls)
        .find_map(|control| {
            if let Control::Table(table) = control {
                Some(table)
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn insertion_traverses_two_table_cell_depths_without_splicing_ancestor_lists() {
    for extension in ["hwp", "hwpx"] {
        let fixture = nested_fixture(extension);
        let before = read(&fixture.base);
        let id = para_id(&fixture.base, "Deep cell anchor.");
        let (output, run) = fixture.edit(
            &fixture.base,
            "deep-inserted",
            json!([insert(
                json!({"id":id}),
                "after",
                "refuse",
                json!({}),
                json!([block("body", "Deep inserted.")])
            )]),
            false,
        );
        success(
            run,
            "insert_blocks traverses every table-cell address depth",
        );
        let after = read(&output);
        assert_eq!(
            deep_table(&after).cells[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            ["Deep cell anchor.", "Deep inserted."]
        );
        assert_eq!(
            before.sections[0]
                .paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>(),
            after.sections[0]
                .paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>(),
            "body-owned characters/control indices and paragraph order must remain unchanged"
        );
        assert_eq!(
            table(&before).cells[0]
                .paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>(),
            table(&after).cells[0]
                .paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>(),
            "outer cell list must not receive the deep insertion"
        );
        assert_eq!(
            resolved(&before, &table(&before).cells[1].paragraphs[0]),
            resolved(&after, &table(&after).cells[1].paragraphs[0])
        );
        assert_eq!(
            resolved(&before, &table(&before).cells[0].paragraphs[0]),
            resolved(&after, &table(&after).cells[0].paragraphs[0])
        );
    }
}

#[test]
fn cyclic_component_style_inheritance_refuses_with_repair_step() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("style-cycle", extension);
        let before = read(&fixture.base);
        let diagnostic = fixture.refuse(
            &fixture.base,
            "cycle-refused",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({
                    "cycle-a":{"based_on":"cycle-b"}, "cycle-b":{"based_on":"cycle-a"}
                }),
                json!([block("cycle-a", "Cycle must not publish.")])
            )]),
            true,
        );
        assert!(
            diagnostic.contains("based_on")
                && diagnostic.contains("순환")
                && diagnostic.contains("제거")
                && diagnostic.contains("다시"),
            "style inheritance cycle must fail with its repair step rather than unknown-op rejection: {diagnostic}"
        );
    }
}

#[test]
fn generic_paragraph_target_refuses_early_with_body_cell_remedy() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("generic-target", extension);
        let mut doc = read(&fixture.base);
        let anchor = index(&doc, ANCHOR);
        let leaf = paragraph(&doc, PREFIX).clone();
        let control_index = doc.sections[0].paragraphs[anchor].controls.len();
        let object: hwp_model::GenericControl = serde_json::from_value(json!({
            "ctrl_id":[104,101,97,100], "data":"0000000000000000", "extras":[],
            "paragraph_lists":[{"header_data":"","paragraphs":[leaf]}]
        }))
        .unwrap();
        doc.sections[0].paragraphs[anchor]
            .controls
            .push(Control::Generic(object));
        let own = &mut doc.sections[0].paragraphs[anchor].chars;
        let end = if own.last()
            == Some(&hwp_model::HwpChar::CharCtrl(
                hwp_model::ctrl_char::PARA_BREAK,
            )) {
            own.len() - 1
        } else {
            own.len()
        };
        own.insert(
            end,
            hwp_model::HwpChar::ExtCtrl {
                code: 16,
                ctrl_id: *b"head",
                payload: vec![0; 12],
                ctrl_index: Some(u32::try_from(control_index).unwrap()),
            },
        );
        let source = fixture.dir.join("generic-input.hwpx");
        hwpx::write_document(&doc, &source).unwrap();
        let reread = read(&source);
        let anchor = index(&reread, ANCHOR);
        let control_index = reread.sections[0].paragraphs[anchor]
            .controls
            .iter()
            .position(
                |control| matches!(control, Control::Generic(object) if object.ctrl_id == *b"head"),
            )
            .unwrap();
        let path = hwp_convert::SegmentPath {
            section: 0,
            indices: vec![anchor, control_index, 0],
        };
        let Control::Generic(object) =
            &reread.sections[0].paragraphs[anchor].controls[control_index]
        else {
            panic!("fixture Generic control");
        };
        let id = hwp_convert::paragraph_id(&path, &object.paragraph_lists[0].paragraphs[0]);
        let diagnostic = fixture.refuse(
            &source,
            "generic-refused",
            json!([insert(
                json!({"id":id}),
                "after",
                "refuse",
                json!({}),
                json!([block("body", "Not inserted in Generic.")])
            )]),
            true,
        );
        assert!(
            diagnostic.contains("본문 또는 표 셀 문단이 아닙니다")
                && diagnostic.contains("다시 선택"),
            "Generic insertion must fail at the explicit target-kind gate before writing: {diagnostic}"
        );
    }
}

#[test]
fn high_named_style_refuses_hwp_without_publication_but_inserts_into_hwpx() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("high-style-bound", extension);
        let mut doc = read(&fixture.base);
        let template = doc
            .header
            .styles
            .iter()
            .find(|style| style.name == "body")
            .unwrap()
            .clone();
        assert!(
            doc.header.styles.len() < 257,
            "authored fixture must leave room for the controlled style-ID boundary"
        );
        while doc.header.styles.len() < 257 {
            let index = doc.header.styles.len();
            let mut style = template.clone();
            style.name = format!("padding-style-{index}");
            style.english_name = style.name.clone();
            doc.header.styles.push(style);
        }
        let high = &mut doc.header.styles[256];
        high.name = "high-style".into();
        high.english_name = "high-style".into();
        high.attr &= !7;
        high.kind_unknown = false;
        let source = fixture.dir.join("high-style-source.hwpx");
        hwpx::write_document(&doc, &source).unwrap();
        let before = read(&source);
        assert_eq!(before.header.styles[256].name, "high-style");
        assert!(!before.header.styles[256].kind_unknown);
        let operations = json!([insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([block("high-style", "High named style inserted.")])
        )]);
        let original_bytes = std::fs::read(&source).unwrap();
        if extension == "hwp" {
            let destination = fixture.path("high-style-result");
            std::fs::copy(&fixture.base, &destination).unwrap();
            let destination_bytes = std::fs::read(&destination).unwrap();
            let (_, run) = fixture.edit(&source, "high-style-result", operations, false);
            assert!(
                !run.status.success(),
                "HWP insertion cannot narrow style ID 256 to one byte"
            );
            let diagnostic = String::from_utf8_lossy(&run.stderr);
            assert!(
                diagnostic.contains("high-style")
                    && diagnostic.contains("256")
                    && diagnostic.contains("HWPX"),
                "style-bound refusal must identify the name, ID and HWPX remedy: {diagnostic}"
            );
            assert!(
                std::fs::read(destination).unwrap() == destination_bytes,
                "style-bound refusal must preserve the existing destination"
            );
        } else {
            let (output, run) = fixture.edit(&source, "high-style-result", operations, false);
            success(run, "style ID 256 remains valid for HWPX insertion");
            let after = read(&output);
            let inserted = paragraph(&after, "High named style inserted.");
            assert_eq!(inserted.style.0, 256);
            assert_eq!(after.header.styles[256].name, "high-style");
            assert_eq!(
                index(&after, "High named style inserted."),
                index(&after, ANCHOR) + 1
            );
        }
        assert!(
            std::fs::read(source).unwrap() == original_bytes,
            "style-bound check must never change the source HWPX"
        );
    }
}

fn section_owner_case(component: bool) {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new(
            if component {
                "section-def-component"
            } else {
                "section-def-paragraph"
            },
            extension,
        );
        let original = read(&fixture.base);
        assert!(
            original.sections[0].paragraphs[0]
                .controls
                .iter()
                .any(|control| matches!(control, Control::SectionDef(_))),
            "fixture must exercise the real section definition owner"
        );
        let operation = if component {
            insert(
                json!({"at":{"section":0,"paragraph":0}}),
                "before",
                "refuse",
                json!({}),
                json!([block("body", "Before section owner.")]),
            )
        } else {
            json!({"op":"insert_para","address":{"at":{"section":0,"paragraph":0}},"before":true,"text":"Before section owner."})
        };
        let diagnostic = fixture.refuse(
            &fixture.base,
            &format!("section-before-{component}"),
            json!([operation]),
            false,
        );
        assert!(
            diagnostic.contains("구역")
                && diagnostic.contains("앞")
                && diagnostic.contains("after"),
            "section-owner refusal must explain the boundary and after remedy: {diagnostic}"
        );
        let operation = if component {
            insert(
                json!({"at":{"section":0,"paragraph":0}}),
                "after",
                "refuse",
                json!({}),
                json!([block("body", "After section owner.")]),
            )
        } else {
            json!({"op":"insert_para","address":{"at":{"section":0,"paragraph":0}},"before":false,"text":"After section owner."})
        };
        let (output, run) = fixture.edit(
            &fixture.base,
            &format!("section-after-{component}"),
            json!([operation]),
            false,
        );
        success(run, "insertion after section definition owner");
        let after = read(&output);
        assert_eq!(index(&after, "After section owner."), 1);
        assert!(
            after.sections[0].paragraphs[0]
                .controls
                .iter()
                .any(|control| matches!(control, Control::SectionDef(_)))
        );
    }
}

#[test]
fn insert_blocks_before_section_definition_refuses_and_after_succeeds() {
    section_owner_case(true);
}

#[test]
fn insert_para_before_section_definition_refuses_and_after_succeeds() {
    section_owner_case(false);
}

fn ordered_custom_case(label: &str, marker: &str, start: u32, template: &str) {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new(&format!("ordered-{label}"), extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([
                {"type":"paragraph","style":"body","list":{"name":"custom-ordered","level":0},"runs":[{"type":"text","text":"Custom ordered item."}]}
            ]),
        );
        operation["lists"] =
            json!({"custom-ordered":{"kind":"ordered","levels":[{"marker":marker,"start":start}]}});
        if extension == "hwp" {
            let diagnostic =
                fixture.refuse(&fixture.base, "ordered-refused", json!([operation]), false);
            assert!(
                diagnostic.contains("custom-ordered")
                    && diagnostic.contains("0")
                    && diagnostic.contains("HWPX")
                    && diagnostic.contains("{n}.")
                    && diagnostic.contains('1'),
                "lossy HWP list refusal must identify list/level and HWPX/default marker/start remedies: {diagnostic}"
            );
        } else {
            let (output, run) =
                fixture.edit(&fixture.base, "ordered-retained", json!([operation]), false);
            success(run, "HWPX retains custom ordered definition");
            let after = read(&output);
            let item = paragraph(&after, "Custom ordered item.");
            let shape = &after.header.para_shapes[item.para_shape.0 as usize];
            assert_eq!(shape.head_type(), 2);
            let level = &after.header.numbering_levels[usize::from(shape.numbering_id)][0];
            assert_eq!(level.template, template);
            assert_eq!(level.start, start);
        }
    }
}

#[test]
fn ordered_component_custom_marker_requires_hwpx() {
    ordered_custom_case("marker", "({n})", 1, "(^1)");
}

#[test]
fn ordered_component_nondefault_start_requires_hwpx() {
    ordered_custom_case("start", "{n}.", 3, "^1.");
}

#[test]
fn ordered_component_default_marker_and_start_is_accepted_by_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("ordered-default", extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([
                {"type":"paragraph","style":"body","list":{"name":"default-ordered","level":0},"runs":[{"type":"text","text":"Default ordered item."}]}
            ]),
        );
        operation["lists"] =
            json!({"default-ordered":{"kind":"ordered","levels":[{"marker":"{n}.","start":1}]}});
        let (output, run) = fixture.edit(
            &fixture.base,
            "ordered-default-inserted",
            json!([operation]),
            false,
        );
        success(run, "default ordered component remains supported");
        let after = read(&output);
        let item = paragraph(&after, "Default ordered item.");
        let shape = &after.header.para_shapes[item.para_shape.0 as usize];
        assert_eq!(shape.head_type(), 2);
        assert_eq!(shape.head_level(), 1);
        let level = &after.header.numbering_levels[usize::from(shape.numbering_id)][0];
        assert_eq!(level.template, "^1.");
        assert_eq!(level.start, 1);
    }
}

fn ordered_used_level_case(level: usize) {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new(&format!("ordered-used-level-{level}"), extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([
                {"type":"paragraph","style":"body","list":{"name":"ladder-ordered","level":level},"runs":[{"type":"text","text":"Ordered ladder item."}]}
            ]),
        );
        let definitions = if level == 2 {
            json!([{"marker":"{n}.","start":1},{"marker":"{n}.","start":1},{"marker":"{n})","start":1}])
        } else {
            json!([{"marker":"{n}.","start":1},{"marker":"{n}.","start":1}])
        };
        operation["lists"] = json!({"ladder-ordered":{"kind":"ordered","levels":definitions}});
        if level == 1 && extension == "hwp" {
            let diagnostic = fixture.refuse(
                &fixture.base,
                "ordered-format-refused",
                json!([operation]),
                false,
            );
            assert!(
                diagnostic.contains("ladder-ordered")
                    && diagnostic.contains('1')
                    && diagnostic.contains("HWPX")
                    && diagnostic.contains("번호 형식"),
                "used level format mismatch must identify list/level and format cause with HWPX remedy: {diagnostic}"
            );
        } else {
            let (output, run) = fixture.edit(
                &fixture.base,
                "ordered-ladder-inserted",
                json!([operation]),
                false,
            );
            success(
                run,
                "representable used list level or HWPX authored numbering",
            );
            let after = read(&output);
            let item = paragraph(&after, "Ordered ladder item.");
            let shape = &after.header.para_shapes[item.para_shape.0 as usize];
            assert_eq!(shape.head_type(), 2);
            if extension == "hwp" {
                // HWP5 stores the level zero-based (#472, 07-hangul-compat-rules L1); the
                // HWP5 reader exposes those raw bits, so check them, not `head_level()`.
                assert_eq!(((shape.attr1 >> 25) & 0x7) as usize, level);
            } else {
                assert_eq!(usize::from(shape.head_level()), level + 1);
            }
            let actual = &after.header.numbering_levels[usize::from(shape.numbering_id)][level];
            assert_eq!(actual.template, if level == 2 { "^3)" } else { "^2." });
            assert_eq!(actual.start, 1);
            assert_eq!(actual.fmt, hwp_model::NumFmt::Digit);
        }
    }
}

#[test]
fn ordered_component_used_level_two_matching_default_ladder_succeeds() {
    ordered_used_level_case(2);
}

#[test]
fn ordered_component_used_level_one_digit_format_requires_hwpx() {
    ordered_used_level_case(1);
}

/// #472: HWP5 stores the paragraph-head level zero-based in attr1 bits 25..=27 and repeats it
/// in the PARA_SHAPE tail's final u32 (07-hangul-compat-rules, L1). Read the written HWP back
/// and check the RAW bits, which select the NUMBERING slot Hancom draws. The semantic
/// `head_level()` hides the defect because `list_level` wins over the bits.
#[test]
fn ordered_component_hwp_level_bits_are_zero_based() {
    for level in [0usize, 2] {
        let fixture = Fixture::new(&format!("ordered-raw-level-{level}"), "hwp");
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([
                {"type":"paragraph","style":"body","list":{"name":"raw-ordered","level":level},"runs":[{"type":"text","text":"Raw level item."}]}
            ]),
        );
        let definitions = json!([{"marker":"{n}.","start":1},{"marker":"{n}.","start":1},{"marker":"{n})","start":1}]);
        operation["lists"] = json!({"raw-ordered":{"kind":"ordered","levels":definitions}});
        let (output, run) = fixture.edit(
            &fixture.base,
            "ordered-raw-level",
            json!([operation]),
            false,
        );
        success(run, "ordered list into HWP");
        let after = read(&output);
        let item = paragraph(&after, "Raw level item.");
        let shape = &after.header.para_shapes[item.para_shape.0 as usize];
        assert_eq!(shape.head_type(), 2);
        let raw = ((shape.attr1 >> 25) & 0x7) as usize;
        assert_eq!(
            raw, level,
            "level {level} must be stored zero-based in attr1 bits 25..=27"
        );
        if shape.tail.len() >= 16 {
            assert_eq!(
                u32::from_le_bytes(shape.tail[12..16].try_into().unwrap()) as usize,
                level,
                "the PARA_SHAPE tail repeats the zero-based level"
            );
        }
        let drawn = &after.header.numbering_levels[usize::from(shape.numbering_id)][raw];
        assert_eq!(
            drawn.template,
            if level == 2 { "^3)" } else { "^1." },
            "the slot the raw bits select is the requested level's template"
        );
    }
}

fn official_source(fixture: &Fixture) -> (PathBuf, Document) {
    let input = fixture.dir.join("official.md");
    std::fs::write(
        &input,
        "Official opening.\n\n1. Official numbered item.\n\nOfficial ending.\n",
    )
    .unwrap();
    let source = fixture.dir.join("official-source.hwpx");
    success(
        hwp()
            .args(["new", "--from"])
            .arg(&input)
            .args(["--preset", "official", "--output"])
            .arg(&source)
            .output()
            .unwrap(),
        "existing official-profile HWPX creation",
    );
    let doc = read(&source);
    assert_ne!(doc.meta.source_format, "hwp5");
    assert!(
        doc.header.numberings.is_empty(),
        "real HWPX source must not supply raw HWP numbering records"
    );
    assert!(
        hwp5::numbering::is_official_eight_level_contract(&doc.header.numbering_levels),
        "official HWPX must carry the eight-level semantic contract"
    );
    (source, doc)
}

fn official_insert(doc: &Document, text: &str, list_kind: Option<&str>) -> Value {
    let style = &doc.header.styles[doc.sections[0].paragraphs[0].style.0 as usize].name;
    let mut paragraph = block(style, text);
    if list_kind.is_some() {
        paragraph["list"] = json!({"name":"component-official-list","level":0});
    }
    let mut operation = insert(
        json!({"at":{"section":0,"paragraph":0}}),
        "after",
        "refuse",
        json!({}),
        json!([paragraph]),
    );
    if let Some(kind) = list_kind {
        operation["lists"] = json!({"component-official-list":{"kind":kind,"levels":[{"marker":if kind == "ordered" { "{n}." } else { "•" },"start":1}]}});
    }
    operation
}

fn assert_official_hwp_numbering(doc: &Document, expected_count: usize) {
    assert!(hwp5::numbering::is_official_eight_level_contract(
        &doc.header.numbering_levels
    ));
    assert_eq!(doc.header.numberings.len(), expected_count);
    let expected = hwp5::numbering::official_eight_level_data();
    for record in &doc.header.numberings {
        assert_eq!(
            record.data, expected,
            "generated HWP must retain the bounded official numbering record bytes"
        );
        assert!(record.children.is_empty());
    }
}

fn official_native_control(fixture: &Fixture, source: &Path) -> Document {
    let output = fixture.dir.join("official-control.hwp");
    success(
        hwp()
            .arg("convert")
            .arg(source)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap(),
        "existing official HWPX to HWP conversion without insertion",
    );
    let control = read(&output);
    assert!(
        !control.header.numberings.is_empty(),
        "native control must materialize official numbering records"
    );
    assert_official_hwp_numbering(&control, control.header.numberings.len());
    control
}

#[test]
fn official_hwpx_ordered_insertion_to_hwp_refuses_before_changing_existing_numbering() {
    let fixture = Fixture::new("official-ordered-hwp", "hwp");
    let (source, before) = official_source(&fixture);
    let _native_control = official_native_control(&fixture, &source);
    let diagnostic = fixture.refuse(
        &source,
        "official-ordered-refused",
        json!([official_insert(
            &before,
            "Official ordered insertion.",
            Some("ordered")
        )]),
        false,
    );
    assert!(
        diagnostic.contains("8")
            && diagnostic.contains("기존")
            && diagnostic.contains("번호")
            && diagnostic.contains("변경")
            && diagnostic.contains("HWPX"),
        "ordered insertion must explain that HWP would change existing official eight-level numbering and offer HWPX: {diagnostic}"
    );
}

#[test]
fn official_hwpx_text_insertion_to_hwp_retains_official_record_bytes() {
    let fixture = Fixture::new("official-text-hwp", "hwp");
    let (source, before) = official_source(&fixture);
    let native_control = official_native_control(&fixture, &source);
    let source_bytes = std::fs::read(&source).unwrap();
    let (output, run) = fixture.edit(
        &source,
        "official-text-inserted",
        json!([official_insert(&before, "Official text insertion.", None)]),
        false,
    );
    success(
        run,
        "text-only insertion preserves official numbering HWP path",
    );
    let after = read(&output);
    assert_eq!(index(&after, "Official text insertion."), 1);
    assert_official_hwp_numbering(&after, native_control.header.numberings.len());
    assert_eq!(after.header.numberings, native_control.header.numberings);
    assert_eq!(
        after.header.numbering_levels,
        native_control.header.numbering_levels
    );
    assert!(std::fs::read(&source).unwrap() == source_bytes);
}

#[test]
fn official_hwpx_ordered_insertion_to_hwpx_preserves_existing_definitions() {
    let fixture = Fixture::new("official-ordered-hwpx", "hwpx");
    let (source, before) = official_source(&fixture);
    let source_bytes = std::fs::read(&source).unwrap();
    let (output, run) = fixture.edit(
        &source,
        "official-ordered-inserted",
        json!([official_insert(
            &before,
            "Official ordered insertion.",
            Some("ordered")
        )]),
        false,
    );
    success(
        run,
        "HWPX ordered insertion retains existing official definitions",
    );
    let after = read(&output);
    let count = before.header.numbering_levels.len();
    assert_eq!(after.header.numbering_levels.len(), count + 1);
    assert_eq!(
        &after.header.numbering_levels[..count],
        &before.header.numbering_levels
    );
    let item = paragraph(&after, "Official ordered insertion.");
    let shape = &after.header.para_shapes[item.para_shape.0 as usize];
    assert_eq!(shape.head_type(), 2);
    assert_eq!(usize::from(shape.numbering_id), count);
    assert_eq!(after.header.numbering_levels[count][0].template, "^1.");
    assert_eq!(after.header.numbering_levels[count][0].start, 1);
    assert!(std::fs::read(&source).unwrap() == source_bytes);
}

#[test]
fn official_hwpx_bullet_insertion_to_hwp_grows_only_bullets_and_retains_official_record() {
    let fixture = Fixture::new("official-bullet-hwp", "hwp");
    let (source, before) = official_source(&fixture);
    let native_control = official_native_control(&fixture, &source);
    let source_bytes = std::fs::read(&source).unwrap();
    let (output, run) = fixture.edit(
        &source,
        "official-bullet-inserted",
        json!([official_insert(
            &before,
            "Official bullet insertion.",
            Some("bullet")
        )]),
        false,
    );
    success(
        run,
        "bullet insertion does not replace official ordered definitions",
    );
    let after = read(&output);
    assert_official_hwp_numbering(&after, native_control.header.numberings.len());
    assert_eq!(after.header.numberings, native_control.header.numberings);
    assert_eq!(
        after.header.numbering_levels,
        native_control.header.numbering_levels
    );
    assert_eq!(
        after.header.bullet_chars.len(),
        before.header.bullet_chars.len() + 1
    );
    let item = paragraph(&after, "Official bullet insertion.");
    let shape = &after.header.para_shapes[item.para_shape.0 as usize];
    assert_eq!(shape.head_type(), 3);
    assert_eq!(
        after.header.bullet_chars[usize::from(shape.numbering_id)],
        '•'
    );
    assert!(std::fs::read(&source).unwrap() == source_bytes);
}

// ---- #428 T3: tables, images, fields and equations through the id remapper ----

const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn write_png(dir: &Path, name: &str) {
    use base64::Engine as _;
    std::fs::write(
        dir.join(name),
        base64::engine::general_purpose::STANDARD
            .decode(PNG_B64)
            .unwrap(),
    )
    .unwrap();
}

fn all_paragraphs<'a>(paragraphs: &'a [Paragraph], out: &mut Vec<&'a Paragraph>) {
    for paragraph in paragraphs {
        out.push(paragraph);
        for control in &paragraph.controls {
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        all_paragraphs(&cell.paragraphs, out);
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        all_paragraphs(&list.paragraphs, out);
                    }
                }
                _ => {}
            }
        }
    }
}

fn doc_paragraphs(doc: &Document) -> Vec<&Paragraph> {
    let mut out = Vec::new();
    for section in &doc.sections {
        all_paragraphs(&section.paragraphs, &mut out);
    }
    out
}

fn tables_of(doc: &Document) -> Vec<&Table> {
    doc_paragraphs(doc)
        .into_iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| {
            if let Control::Table(t) = c {
                Some(t)
            } else {
                None
            }
        })
        .collect()
}

fn pictures_of(doc: &Document) -> Vec<&hwp_model::Picture> {
    doc_paragraphs(doc)
        .into_iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| {
            if let Control::Picture(p) = c {
                Some(p)
            } else {
                None
            }
        })
        .collect()
}

fn common_id(common: &[u8]) -> u32 {
    u32::from_le_bytes(common[32..36].try_into().expect("gso common id"))
}

/// Field instance id: attr(4) etc(1) len(2) WCHAR[len] id(4) (spec table 152).
fn field_ids(doc: &Document) -> Vec<u32> {
    doc_paragraphs(doc)
        .into_iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| match c {
            Control::Generic(g) if hwp_convert::field::is_field_ctrl_id(&g.ctrl_id) => {
                let len = u16::from_le_bytes([g.data[5], g.data[6]]) as usize;
                let at = 7 + 2 * len;
                Some(u32::from_le_bytes(g.data[at..at + 4].try_into().unwrap()))
            }
            _ => None,
        })
        .collect()
}

fn assert_distinct(ids: &[u32], what: &str) {
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "{what} ids must be distinct: {ids:x?}"
    );
}

fn validates(path: &Path) {
    success(
        hwp().arg("validate").arg(path).output().unwrap(),
        "validate of the edited output",
    );
}

fn component_blocks() -> Value {
    json!([
        block("body", "Component lead."),
        {"type":"table","columns":[{"width_mm":30},{"width_mm":30}],"rows":[
            {"cells":[{"blocks":[block("body","Approval cell A.")]},{"blocks":[block("note","Approval cell B.")]}]},
            {"cells":[{"blocks":[{"type":"image","path":"logo.png","width_mm":10}]},
                      {"blocks":[{"type":"field","name":"cell-field","value":"cell value"}]}]}
        ]},
        {"type":"image","path":"logo.png","width_mm":12},
        {"type":"field","name":"body-field","value":"body value"},
        {"type":"paragraph","style":"body","runs":[
            {"type":"text","text":"Inline run "},
            {"type":"field","name":"run-field","value":"run value"}]},
        block("body", "Component tail.")
    ])
}

#[test]
fn table_image_field_components_insert_in_order_with_distinct_ids_in_both_formats() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("objects", extension);
        write_png(&fixture.dir, "logo.png");
        let before = read(&fixture.base);
        let existing_tables = tables_of(&before).len();
        let (output, run) = fixture.edit(
            &fixture.base,
            "objects",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                component_blocks()
            )]),
            false,
        );
        success(run, "insert_blocks with table, image and field blocks");
        validates(&output);
        let after = read(&output);
        let body: Vec<_> = after.sections[0]
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        let lead = body.iter().position(|t| t == "Component lead.").unwrap();
        assert_eq!(
            body[lead - 1],
            ANCHOR,
            "{extension}: inserted after the anchor"
        );
        let tail = body.iter().position(|t| t == "Component tail.").unwrap();
        assert_eq!(
            body[tail + 1],
            "Body tail.",
            "{extension}: original order resumes"
        );
        assert!(
            after.sections[0].paragraphs[lead + 1]
                .controls
                .iter()
                .any(|c| matches!(c, Control::Table(_))),
            "{extension}: the table follows the lead paragraph"
        );
        let tables = tables_of(&after);
        assert_eq!(tables.len(), existing_tables + 1, "{extension}");
        let inserted = tables
            .iter()
            .find(|t| {
                t.cells
                    .iter()
                    .flat_map(|c| &c.paragraphs)
                    .any(|p| p.plain_text() == "Approval cell A.")
            })
            .unwrap();
        let fill = after.header.border_fills[usize::from(inserted.border_fill.0) - 1].clone();
        assert!(
            fill.sides.iter().all(|side| side.line_type == 1),
            "{extension}: the inserted table must draw its borders, got {fill:?}"
        );
        let pictures = pictures_of(&after);
        assert_eq!(pictures.len(), 2, "{extension}: both image blocks survive");
        {
            use base64::Engine as _;
            let png = base64::engine::general_purpose::STANDARD
                .decode(PNG_B64)
                .unwrap();
            for picture in &pictures {
                assert_eq!(
                    after.resolve_bin(&picture.bin_ref),
                    Some(png.as_slice()),
                    "{extension}: an inserted picture must resolve to its own bytes through the \
                     target's manifest-first resolver"
                );
            }
        }
        assert_eq!(
            after.bin_streams.len(),
            before.bin_streams.len() + 1,
            "{extension}: one shared asset stream for the repeated image"
        );
        if extension == "hwp" {
            let table_ids: Vec<u32> = tables.iter().map(|t| common_id(&t.common_data)).collect();
            assert_distinct(&table_ids, "table");
            let picture_ids: Vec<u32> =
                pictures.iter().map(|p| common_id(&p.common_data)).collect();
            assert_distinct(&picture_ids, "picture");
            let fields = field_ids(&after);
            assert_eq!(fields.len(), 3, "three field blocks/runs");
            assert_distinct(&fields, "field");
            assert!(
                fields.iter().all(|id| *id != 0),
                "field ids are non-zero: {fields:x?}"
            );
        }
    }
}

#[test]
fn successive_hwp_image_insertions_do_not_reuse_a_picture_instance_id() {
    let fixture = Fixture::new("two-images", "hwp");
    write_png(&fixture.dir, "logo.png");
    let image = json!([{"type":"image","path":"logo.png","width_mm":10}]);
    let before = read(&fixture.base);
    let (first, run) = fixture.edit(
        &fixture.base,
        "first-image",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            image.clone()
        )]),
        false,
    );
    success(run, "first image insertion");
    let middle = read(&first);
    let (second, run) = fixture.edit(
        &first,
        "second-image",
        json!([insert(
            at(&middle, ANCHOR),
            "after",
            "create",
            json!({}),
            image
        )]),
        false,
    );
    success(
        run,
        "second image insertion into an output that already holds a picture",
    );
    validates(&second);
    let after = read(&second);
    let ids: Vec<u32> = pictures_of(&after)
        .iter()
        .map(|p| common_id(&p.common_data))
        .collect();
    assert_eq!(ids.len(), 2);
    assert_distinct(&ids, "picture");
}

#[test]
fn equation_blocks_insert_into_hwpx_and_refuse_for_hwp_output() {
    let blocks = json!([{"type":"equation","script":"x^2+y^2"},
        {"type":"paragraph","style":"body","runs":[{"type":"equation","script":"a over b"}]}]);
    let fixture = Fixture::new("equation", "hwpx");
    let before = read(&fixture.base);
    let (output, run) = fixture.edit(
        &fixture.base,
        "equation",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            blocks.clone()
        )]),
        false,
    );
    success(run, "equation blocks into HWPX");
    validates(&output);
    let scripts: Vec<String> = doc_paragraphs(&read(&output))
        .into_iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| match c {
            Control::Generic(g) => g.equation.as_ref().map(|e| e.script.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(scripts, ["x^2+y^2", "a over b"]);

    let fixture = Fixture::new("equation", "hwp");
    let before = read(&fixture.base);
    let diagnostic = fixture.refuse(
        &fixture.base.clone(),
        "equation-hwp",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            blocks
        )]),
        false,
    );
    assert!(
        diagnostic.contains("수식") && diagnostic.contains("HWPX"),
        "the HWP refusal must name the equation and the HWPX remedy: {diagnostic}"
    );
}

#[test]
fn image_assets_stay_inside_the_ops_directory_and_leave_the_file_untouched_on_refusal() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("asset-refusals", extension);
        let source = fixture.base.clone();
        let before = read(&source);
        for (label, path) in [
            ("parent", "../logo.png"),
            ("absolute", "/etc/hosts"),
            ("missing", "absent.png"),
        ] {
            let diagnostic = fixture.refuse(
                &source,
                label,
                json!([insert(
                    at(&before, ANCHOR),
                    "after",
                    "create",
                    json!({}),
                    json!([block("body", "Not partially published."),
                           {"type":"image","path":path,"width_mm":10}])
                )]),
                false,
            );
            assert!(
                !diagnostic.contains("insert_blocks는 현재"),
                "{label}: images are supported now, only this asset is refused: {diagnostic}"
            );
        }
    }
}

#[test]
fn nested_table_inside_cell_target_and_oversize_table_follow_the_cell_width() {
    let fixture = Fixture::new("cell-width", "hwpx");
    let before = read(&fixture.base);
    let id = para_id(&fixture.base, CELL);
    let wide = json!([{"type":"table","columns":[{"width_mm":90}],
        "rows":[{"cells":[{"blocks":[block("body","Too wide for a 40 mm cell.")]}]}]}]);
    let diagnostic = fixture.refuse(
        &fixture.base.clone(),
        "wide",
        json!([insert(json!({"id":id}), "after", "create", json!({}), wide)]),
        false,
    );
    assert!(diagnostic.contains("mm"), "{diagnostic}");
    let narrow = json!([{"type":"table","columns":[{"width_mm":20}],
        "rows":[{"cells":[{"blocks":[block("body","Fits the cell.")]}]}]}]);
    let (output, run) = fixture.edit(
        &fixture.base,
        "narrow",
        json!([insert(
            json!({"id":id}),
            "after",
            "create",
            json!({}),
            narrow
        )]),
        false,
    );
    success(run, "narrow table inside the cell");
    validates(&output);
    assert_eq!(
        tables_of(&read(&output)).len(),
        tables_of(&before).len() + 1
    );
}

#[test]
fn inserted_table_borders_come_from_the_target_header_not_compose_fill_three() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("border-fill", extension);
        // Move the bordered fill away from compose's id 3 so a hardcoded id draws no borders,
        // and drop every other bordered fill so the target has none to reuse.
        let mut doc = read(&fixture.base);
        let bordered = |fill: &hwp_model::BorderFill| fill.sides.iter().all(|s| s.line_type == 1);
        doc.header.border_fills.retain(|fill| !bordered(fill));
        assert!(doc.header.border_fills.len() >= 3);
        for table in doc
            .sections
            .iter_mut()
            .flat_map(|s| &mut s.paragraphs)
            .flat_map(|p| &mut p.controls)
            .filter_map(|c| {
                if let Control::Table(t) = c {
                    Some(t)
                } else {
                    None
                }
            })
        {
            table.border_fill = hwp_model::BorderFillId(1);
            table
                .cells
                .iter_mut()
                .for_each(|c| c.border_fill = hwp_model::BorderFillId(1));
        }
        let source = fixture.path("no-borders");
        if extension == "hwp" {
            hwp5::write_document(&doc, &source, &hwp5::write::WriteOptions::default()).unwrap();
        } else {
            hwpx::write_document(&doc, &source).unwrap();
        }
        let before = read(&source);
        assert!(before.header.border_fills.iter().all(|f| !bordered(f)));
        let (output, run) = fixture.edit(
            &source,
            "bordered",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                json!([{"type":"table","columns":[{"width_mm":30}],"rows":[{"cells":[{"blocks":[block("body","Bordered.")]}]}]}])
            )]),
            false,
        );
        success(run, "table insertion into a target without a bordered fill");
        validates(&output);
        let after = read(&output);
        let inserted = tables_of(&after)
            .into_iter()
            .find(|t| {
                t.cells
                    .iter()
                    .flat_map(|c| &c.paragraphs)
                    .any(|p| p.plain_text() == "Bordered.")
            })
            .unwrap();
        let fill = &after.header.border_fills[usize::from(inserted.border_fill.0) - 1];
        assert!(bordered(fill), "{extension}: inserted table fill {fill:?}");
        assert_eq!(
            after.header.border_fills.len(),
            before.header.border_fills.len() + 1,
            "{extension}: exactly one bordered fill is appended, none rewritten"
        );
    }
}

// ---- #428 T3 review round 1 (T21-F1, T21-F3) ----

#[test]
fn content_inside_an_inserted_table_cell_is_measured_against_that_cell() {
    // The cell is 30 mm wide minus 510+510 HWPUNIT of margins (about 3.6 mm): 26.4 mm usable.
    let cases = [
        (
            "image",
            json!({"type":"image","path":"logo.png","width_mm":100}),
        ),
        (
            "nested-table",
            json!({"type":"table","columns":[{"width_mm":100}],
                "rows":[{"cells":[{"blocks":[block("body","Too wide.")]}]}]}),
        ),
    ];
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("cell-content", extension);
        write_png(&fixture.dir, "logo.png");
        let source = fixture.base.clone();
        let before = read(&source);
        for (label, content) in &cases {
            let blocks = json!([{"type":"table","columns":[{"width_mm":30}],
                "rows":[{"cells":[{"blocks":[content]}]}]}]);
            let diagnostic = fixture.refuse(
                &source,
                &format!("wide-{label}"),
                json!([insert(
                    at(&before, ANCHOR),
                    "after",
                    "create",
                    json!({}),
                    blocks
                )]),
                false,
            );
            assert!(
                diagnostic.contains("mm") && diagnostic.contains("layout_overflow"),
                "{extension} {label}: an oversized block in a 30 mm cell must name the overflow: {diagnostic}"
            );
        }
        // A block that fits the cell still inserts.
        let fits = json!([{"type":"table","columns":[{"width_mm":30}],
            "rows":[{"cells":[{"blocks":[{"type":"image","path":"logo.png","width_mm":20}]}]}]}]);
        let (output, run) = fixture.edit(
            &source,
            "fits",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                fits
            )]),
            false,
        );
        success(run, "an image narrower than its cell");
        validates(&output);
    }
}

/// A cell's content width is its width minus the left and right inner margin (510 HWPUNIT
/// each, about 3.6 mm together), not the cell width: 28 mm sits between 26.4 mm and 30 mm.
#[test]
fn a_cell_loses_its_side_margins_before_content_is_measured() {
    let fixture = Fixture::new("cell-margin", "hwpx");
    write_png(&fixture.dir, "logo.png");
    let before = read(&fixture.base);
    let table = |image_mm: u32| {
        json!([{"type":"table","columns":[{"width_mm":30}],
            "rows":[{"cells":[{"blocks":[{"type":"image","path":"logo.png","width_mm":image_mm}]}]}]}])
    };
    let diagnostic = fixture.refuse(
        &fixture.base.clone(),
        "margin-over",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            table(28)
        )]),
        false,
    );
    assert!(diagnostic.contains("layout_overflow"), "{diagnostic}");
    let (output, run) = fixture.edit(
        &fixture.base,
        "margin-under",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            table(26)
        )]),
        false,
    );
    success(run, "an image inside the cell's usable width");
    validates(&output);
}

#[test]
fn spanned_cells_are_measured_by_their_column_span() {
    let fixture = Fixture::new("cell-span", "hwpx");
    write_png(&fixture.dir, "logo.png");
    let before = read(&fixture.base);
    // Two 30 mm columns: a spanning cell is 60 mm minus margins, so 50 mm fits and 70 mm does not.
    let table = |image_mm: u32| {
        json!([{"type":"table","columns":[{"width_mm":30},{"width_mm":30}],
            "rows":[{"cells":[{"col_span":2,"blocks":[{"type":"image","path":"logo.png","width_mm":image_mm}]}]}]}])
    };
    let diagnostic = fixture.refuse(
        &fixture.base.clone(),
        "span-wide",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            table(70)
        )]),
        false,
    );
    assert!(diagnostic.contains("layout_overflow"), "{diagnostic}");
    let (output, run) = fixture.edit(
        &fixture.base,
        "span-fits",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            table(50)
        )]),
        false,
    );
    success(run, "an image inside the span width");
    validates(&output);
}

#[test]
fn hwp_list_format_guard_sees_list_levels_used_only_inside_table_cells() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("list-in-cell", extension);
        let before = read(&fixture.base);
        let mut operation = insert(
            at(&before, ANCHOR),
            "after",
            "refuse",
            json!({}),
            json!([{"type":"table","columns":[{"width_mm":60}],"rows":[{"cells":[{"blocks":[
                {"type":"paragraph","style":"body","list":{"name":"ladder-ordered","level":1},
                 "runs":[{"type":"text","text":"Ordered cell item."}]}
            ]}]}]}]),
        );
        operation["lists"] = json!({"ladder-ordered":{"kind":"ordered","levels":[
            {"marker":"{n}.","start":1},{"marker":"{n}.","start":1}]}});
        if extension == "hwp" {
            let diagnostic = fixture.refuse(
                &fixture.base,
                "cell-ordered-refused",
                json!([operation]),
                false,
            );
            assert!(
                diagnostic.contains("ladder-ordered")
                    && diagnostic.contains("HWPX")
                    && diagnostic.contains("번호 형식"),
                "a level-1 decimal list used only in a cell must be refused for HWP like a body one: {diagnostic}"
            );
        } else {
            let (output, run) = fixture.edit(
                &fixture.base,
                "cell-ordered-inserted",
                json!([operation]),
                false,
            );
            success(run, "HWPX authors the numbering as written");
            validates(&output);
        }
    }
}

fn column_def_of(doc: &Document) -> Option<hwp_model::ColumnDef> {
    doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::Generic(g) => g.column_def.clone(),
            _ => None,
        })
}

/// The fixture with its section set to `count` equal columns separated by a 10 mm gap, written
/// back in its own format. The 170 mm text column becomes (170 - 10 * (count - 1)) / count.
fn columned(fixture: &Fixture, count: u16) -> PathBuf {
    let mut doc = read(&fixture.base);
    for control in doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.controls)
    {
        if let Control::Generic(g) = control
            && let Some(def) = &mut g.column_def
        {
            def.count = count;
            def.same_width = true;
            def.gap = 2835;
            def.widths.clear();
            // The HWP5 writer emits the raw payload, not the model: attr (count in bits 2..10,
            // same-width in bit 12) then the gap (spec table 138 layout, read back by
            // `parse_coldef`), padded to the 12 bytes the writer's own default uses.
            let attr: u16 = (count << 2) | (1 << 12);
            g.data = [
                attr.to_le_bytes().as_slice(),
                &2835i16.to_le_bytes(),
                &[0u8; 8],
            ]
            .concat();
        }
    }
    let path = fixture.path("columned");
    if fixture.extension == "hwp" {
        hwp5::write_document(&doc, &path, &hwp5::write::WriteOptions::default()).unwrap();
    } else {
        hwpx::write_document(&doc, &path).unwrap();
    }
    let def = column_def_of(&read(&path)).expect("the column definition survives a rewrite");
    assert_eq!(def.count, count, "{}: fixture columns", fixture.extension);
    path
}

#[test]
fn blocks_are_measured_against_the_target_column_of_a_multi_column_section() {
    let wide_table = json!([{"type":"table","columns":[{"width_mm":120}],
        "rows":[{"cells":[{"blocks":[block("body","Too wide for 80 mm.")]}]}]}]);
    let fits = json!([{"type":"table","columns":[{"width_mm":70}],
        "rows":[{"cells":[{"blocks":[block("body","Fits 80 mm.")]}]}]}]);
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("two-columns", extension);
        write_png(&fixture.dir, "logo.png");
        let source = columned(&fixture, 2);
        let before = read(&source);
        let wide_image = json!([{"type":"image","path":"logo.png","width_mm":120}]);
        for (label, blocks) in [("table", &wide_table), ("image", &wide_image)] {
            let diagnostic = fixture.refuse(
                &source,
                &format!("column-wide-{label}"),
                json!([insert(
                    at(&before, ANCHOR),
                    "after",
                    "create",
                    json!({}),
                    blocks.clone()
                )]),
                false,
            );
            assert!(
                diagnostic.contains("layout_overflow") && diagnostic.contains("80."),
                "{extension} {label}: a 120 mm block in an 80 mm column must name the column width: {diagnostic}"
            );
        }
        let (output, run) = fixture.edit(
            &source,
            "column-fits",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                fits.clone()
            )]),
            false,
        );
        success(run, "a table narrower than the column");
        validates(&output);
        // The same 120 mm table is fine in the one-column original.
        let single = read(&fixture.base);
        let (_, run) = fixture.edit(
            &fixture.base,
            "single-wide",
            json!([insert(
                at(&single, ANCHOR),
                "after",
                "create",
                json!({}),
                wide_table.clone()
            )]),
            false,
        );
        success(run, "a 120 mm table in a single 170 mm column");
    }
}

/// The fixture with its page rotated the way genuine landscape forms store it: portrait
/// dimensions and PAGE_DEF attr bit 0 set, which `hwp-render` reads as a 297 mm wide paper.
fn landscape(fixture: &Fixture) -> PathBuf {
    let mut doc = read(&fixture.base);
    for control in doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.controls)
    {
        if let Control::SectionDef(def) = control
            && let Some(page) = &mut def.page
        {
            let (w, h) = (
                page.width.0.min(page.height.0),
                page.width.0.max(page.height.0),
            );
            page.width = hwp_model::HwpUnit(w);
            page.height = hwp_model::HwpUnit(h);
            page.attr |= 1;
        }
    }
    let path = fixture.path("landscape");
    if fixture.extension == "hwp" {
        hwp5::write_document(&doc, &path, &hwp5::write::WriteOptions::default()).unwrap();
    } else {
        hwpx::write_document(&doc, &path).unwrap();
    }
    let page = read(&path).sections[0].section_def().unwrap().page.unwrap();
    assert_eq!(
        page.attr & 1,
        1,
        "{}: the landscape bit survives",
        fixture.extension
    );
    path
}

#[test]
fn landscape_pages_measure_against_the_rotated_paper() {
    // Body widths: portrait 170 mm, landscape 297 - 40 = 257 mm.
    let table = |mm: u32| {
        json!([{"type":"table","columns":[{"width_mm":mm}],
            "rows":[{"cells":[{"blocks":[block("body","Wide form table.")]}]}]}])
    };
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("landscape", extension);
        let source = landscape(&fixture);
        let before = read(&source);
        let (output, run) = fixture.edit(
            &source,
            "landscape-fits",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                table(240)
            )]),
            false,
        );
        success(run, "a 240 mm table on a landscape page");
        validates(&output);
        let diagnostic = fixture.refuse(
            &source,
            "landscape-wide",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                table(270)
            )]),
            false,
        );
        assert!(
            diagnostic.contains("layout_overflow") && diagnostic.contains("257"),
            "{extension}: a 270 mm table must name the 257 mm landscape body: {diagnostic}"
        );
        // The same 240 mm table is too wide for the portrait original.
        let portrait = read(&fixture.base);
        fixture.refuse(
            &fixture.base.clone(),
            "portrait-wide",
            json!([insert(
                at(&portrait, ANCHOR),
                "after",
                "create",
                json!({}),
                table(240)
            )]),
            false,
        );
    }
}

// ---- #428 T3 review round 4 (T21-F8) ----

/// The fixture with its table's inner margins and its first cell's margins rewritten, written
/// back in its own format. Returns the path and the cell's width in HWPUNIT.
fn with_cell_margins(
    fixture: &Fixture,
    stem: &str,
    cell: [u16; 4],
    table_inner: [u16; 4],
) -> (PathBuf, i32) {
    let mut doc = read(&fixture.base);
    let mut width = 0;
    for control in doc
        .sections
        .iter_mut()
        .flat_map(|s| &mut s.paragraphs)
        .flat_map(|p| &mut p.controls)
    {
        if let Control::Table(table) = control {
            table.inner_margins = table_inner;
            for (i, c) in table.cells.iter_mut().enumerate() {
                c.margins = cell;
                if i == 0 {
                    width = c.width.0;
                }
            }
        }
    }
    let path = fixture.path(stem);
    if fixture.extension == "hwp" {
        hwp5::write_document(&doc, &path, &hwp5::write::WriteOptions::default()).unwrap();
    } else {
        hwpx::write_document(&doc, &path).unwrap();
    }
    let reread = read(&path);
    let Control::Table(table) = reread
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .find(|c| matches!(c, Control::Table(_)))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        table.cells[0].margins, cell,
        "{}: the cell margins survive the rewrite as written",
        fixture.extension
    );
    (path, width)
}

fn mm_of(units: i32) -> f32 {
    (f64::from(units) * 25.4 / 7200.0) as f32
}

#[test]
fn a_cell_that_inherits_its_margins_accepts_text_and_measures_blocks_against_the_inherited_width() {
    let max = u16::MAX;
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("inherit-margins", extension);
        let (source, cell_width) =
            with_cell_margins(&fixture, "inherit", [max; 4], [700, 800, 900, 1000]);
        let usable = mm_of(cell_width - 700 - 800);
        let before = read(&source);
        let id = para_id(&source, CELL);
        // Plain text: refused on 714bd1b (width of about -422 mm), accepted here.
        let (output, run) = fixture.edit(
            &source,
            "inherit-text",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                json!([block("body", "Text in an inheriting cell.")])
            )]),
            false,
        );
        success(run, "text into a cell whose margins are inherited");
        validates(&output);
        // A table just under the inherited width fits, just over is refused naming the width.
        let table = |mm: f32| {
            json!([{"type":"table","columns":[{"width_mm":mm}],
                "rows":[{"cells":[{"blocks":[block("body","Nested.")]}]}]}])
        };
        let (output, run) = fixture.edit(
            &source,
            "inherit-fits",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                table(usable - 0.5)
            )]),
            false,
        );
        success(run, "a table narrower than the inherited-margin width");
        validates(&output);
        let diagnostic = fixture.refuse(
            &source,
            "inherit-over",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                table(usable + 0.5)
            )]),
            false,
        );
        assert!(
            diagnostic.contains("layout_overflow"),
            "{extension}: a table wider than the inherited width must be refused: {diagnostic}"
        );
        let _ = before;
    }
}

#[test]
fn a_cell_with_omitted_margins_uses_the_default_margins() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("omitted-margins", extension);
        let (source, cell_width) = with_cell_margins(&fixture, "omitted", [0; 4], [0; 4]);
        let usable = mm_of(cell_width - 510 - 510);
        let id = para_id(&source, CELL);
        let table = |mm: f32| {
            json!([{"type":"table","columns":[{"width_mm":mm}],
                "rows":[{"cells":[{"blocks":[block("body","Nested.")]}]}]}])
        };
        // Between the default-margin width and the bare cell width: only the margin-aware check
        // refuses it.
        let in_between = usable + 1.0;
        assert!(
            in_between < mm_of(cell_width),
            "the probe stays inside the cell itself"
        );
        let diagnostic = fixture.refuse(
            &source,
            "omitted-over",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                table(in_between)
            )]),
            false,
        );
        assert!(
            diagnostic.contains("layout_overflow"),
            "{extension}: {diagnostic}"
        );
        let (output, run) = fixture.edit(
            &source,
            "omitted-fits",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                table(usable - 0.5)
            )]),
            false,
        );
        success(run, "a table inside the default-margin width");
        validates(&output);
    }
}

// ---- #428 T4: inserted-id report (AC3) ----

fn report_of(fixture: &Fixture, stem: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixture.dir.join(format!("{stem}-report.json"))).unwrap())
        .unwrap()
}

/// Every segment `hwp cat --segments v2` publishes for the document: `(kind, id)` in order.
fn segments_of(path: &Path) -> Vec<(String, String)> {
    let run = hwp()
        .arg("cat")
        .arg(path)
        .args(["--format", "json", "--with-segments", "--segments", "v2"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "cat-v2 failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let envelope: Value = serde_json::from_slice(&run.stdout).unwrap();
    envelope["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_owned(),
                s["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The `after` ids of op `op`'s creation pairs, in report order. Every pair is a creation.
fn created_ids(report: &Value, op: usize) -> Vec<String> {
    report["ops"][op]["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            assert!(
                pair["before"].is_null(),
                "an insertion has no before id: {pair}"
            );
            pair["after"]
                .as_str()
                .expect("an inserted segment has an id")
                .to_owned()
        })
        .collect()
}

/// How many path steps a segment id carries after its hash: `<hash>.<section>.<i>...`.
fn depth(id: &str) -> usize {
    id.split('.').count().saturating_sub(2)
}

/// Split a report into `(paragraph id, [run ids])` groups using the kinds the output publishes.
/// The inserted paragraphs are the report's shallowest `para` ids (the anchor's own level: 1 in
/// the body, deeper in a cell); a group takes the runs one level below that follow it. The rest
/// of what the report names (a table, a cell, an image, paragraphs inside a cell) must be
/// published by the output too, and is checked to exist, but is not grouped.
fn grouped(report: &Value, op: usize, output: &Path) -> Vec<(String, Vec<String>)> {
    let kinds: std::collections::BTreeMap<String, String> = segments_of(output)
        .into_iter()
        .map(|(kind, id)| (id, kind))
        .collect();
    let ids = created_ids(report, op);
    for id in &ids {
        assert!(
            kinds.contains_key(id),
            "reported id {id} is not published by the output"
        );
    }
    let level = ids
        .iter()
        .filter(|id| kinds[*id] == "para")
        .map(|id| depth(id))
        .min()
        .expect("the report names at least one paragraph");
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for id in ids {
        match (kinds[&id].as_str(), depth(&id)) {
            ("para", d) if d == level => groups.push((id, Vec::new())),
            ("run", d) if d == level + 1 => groups
                .last_mut()
                .expect("a run id follows its paragraph id")
                .1
                .push(id),
            _ => {}
        }
    }
    groups
}

/// The plain text of the paragraph a top-level paragraph id (depth 1) names in `doc`, found by
/// matching the id `hwp cat` publishes for each body paragraph.
fn text_at_id(path: &Path, doc: &Document, id: &str) -> String {
    assert!(
        segments_of(path)
            .iter()
            .any(|(kind, candidate)| kind == "para" && candidate == id),
        "{id} is not a published paragraph"
    );
    // The id's own last step is the paragraph's index in the body: <hash>.<section>.<index>.
    let index: usize = id.rsplit('.').next().unwrap().parse().unwrap();
    doc.sections[0].paragraphs[index].plain_text()
}

fn is_bold_para(doc: &Document, text: &str) -> bool {
    paragraph(doc, text)
        .char_shape_runs
        .iter()
        .any(|(_, id)| doc.header.char_shapes[id.0 as usize].is_bold())
}

#[test]
fn insert_blocks_reports_paragraph_and_run_ids_and_set_format_can_use_one() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-ids", extension);
        let before = read(&fixture.base);
        let texts = ["Report one.", "Report two.", "Report three."];
        let (output, run) = fixture.edit(
            &fixture.base,
            "inserted",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                json!([
                    block("body", texts[0]),
                    block("body", texts[1]),
                    block("body", texts[2])
                ])
            )]),
            false,
        );
        success(run, "insert_blocks with three paragraphs");
        let report = report_of(&fixture, "inserted");
        assert_eq!(report["ops"][0]["pieces_touched"], 3);
        let groups = grouped(&report, 0, &output);
        assert_eq!(
            groups.len(),
            3,
            "{extension}: one paragraph id per inserted paragraph"
        );
        assert!(
            groups.iter().all(|(_, runs)| !runs.is_empty()),
            "{extension}: every text paragraph reports its run ids: {groups:?}"
        );
        let all = created_ids(&report, 0);
        assert_eq!(
            all.iter().collect::<std::collections::BTreeSet<_>>().len(),
            all.len(),
            "{extension}: the ids are distinct"
        );
        // The paragraph ids are consecutive in the output's own paragraph order.
        let published: Vec<String> = segments_of(&output)
            .into_iter()
            .filter(|(kind, _)| kind == "para")
            .map(|(_, id)| id)
            .collect();
        let at_of = |id: &str| published.iter().position(|p| p == id).unwrap();
        assert_eq!(at_of(&groups[1].0), at_of(&groups[0].0) + 1, "{extension}");
        assert_eq!(at_of(&groups[2].0), at_of(&groups[1].0) + 1, "{extension}");
        // A paragraph id is not a run address: set_format refuses it, which is why run ids are
        // reported. The second paragraph's first run id styles exactly that paragraph (AC3).
        let (styled, run) = fixture.edit(
            &output,
            "followup",
            json!([{"op":"set_format","address":{"id":groups[1].1[0]},"bold":"on"}]),
            false,
        );
        success(
            run,
            "set_format addressed by a run id from the insert_blocks report",
        );
        let after = read(&styled);
        assert!(
            is_bold_para(&after, texts[1]),
            "{extension}: the addressed paragraph is bold"
        );
        assert!(
            !is_bold_para(&after, texts[0]) && !is_bold_para(&after, texts[2]),
            "{extension}: and only that paragraph"
        );
    }
}

#[test]
fn insert_blocks_report_ids_hold_for_before_cell_and_object_blocks() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-ids-variants", extension);
        write_png(&fixture.dir, "logo.png");
        let source = fixture.base.clone();
        let before = read(&source);
        // `before` position: the paragraphs land ahead of the anchor, in order.
        let (output, run) = fixture.edit(
            &source,
            "before",
            json!([insert(
                at(&before, ANCHOR),
                "before",
                "create",
                json!({}),
                json!([block("body", "Ahead one."), block("body", "Ahead two.")])
            )]),
            false,
        );
        success(run, "insert_blocks before the anchor");
        let groups = grouped(&report_of(&fixture, "before"), 0, &output);
        assert_eq!(groups.len(), 2, "{extension}");
        let published: Vec<String> = segments_of(&output)
            .into_iter()
            .filter(|(kind, _)| kind == "para")
            .map(|(_, id)| id)
            .collect();
        let first = published.iter().position(|id| *id == groups[0].0).unwrap();
        assert_eq!(
            published[first + 1],
            groups[1].0,
            "{extension}: before: consecutive"
        );
        let after = read(&output);
        assert_eq!(
            text_at_id(&output, &after, &groups[0].0),
            "Ahead one.",
            "{extension}: the first reported id addresses the first inserted paragraph"
        );
        assert_eq!(
            text_at_id(&output, &after, &groups[1].0),
            "Ahead two.",
            "{extension}: and the second the second, directly ahead of the anchor"
        );
        assert_eq!(
            index(&after, "Ahead one."),
            index(&after, ANCHOR) - 2,
            "{extension}"
        );

        // Object blocks (table, image) are each one top-level paragraph and each reports it.
        let (output, run) = fixture.edit(
            &source,
            "objects",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                json!([
                    {"type":"table","columns":[{"width_mm":40}],"rows":[{"cells":[{"blocks":[block("body","In a table.")]}]}]},
                    {"type":"image","path":"logo.png","width_mm":10},
                    block("body", "After objects.")
                ])
            )]),
            false,
        );
        success(run, "insert_blocks with a table and an image");
        let groups = grouped(&report_of(&fixture, "objects"), 0, &output);
        assert_eq!(
            groups.len(),
            3,
            "{extension}: table, image and text paragraph"
        );

        // Inside a table cell the ids are the cell paragraphs' own.
        let id = para_id(&source, CELL);
        let (output, run) = fixture.edit(
            &source,
            "cell",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                json!([block("body", "Cell added.")])
            )]),
            false,
        );
        success(run, "insert_blocks inside a table cell");
        let groups = grouped(&report_of(&fixture, "cell"), 0, &output);
        assert_eq!(groups.len(), 1, "{extension}");
        assert!(
            !groups[0].1.is_empty(),
            "{extension}: the cell paragraph reports a run id"
        );
        let (styled, run) = fixture.edit(
            &output,
            "cell-followup",
            json!([{"op":"set_format","address":{"id":groups[0].1[0]},"italic":"on"}]),
            false,
        );
        success(
            run,
            "set_format on a run id of a paragraph inserted into a cell",
        );
        assert!(read(&styled).sections.len() == 1, "{extension}");
    }
}

/// A cell insertion's paragraphs sit under `[paragraph, control, cell, index]`. A body
/// paragraph with the same final index, and the cell's other paragraphs, are NOT inserted and
/// must not be reported.
#[test]
fn insert_blocks_in_a_cell_reports_only_the_cell_paragraphs_it_inserted() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-ids-cell-only", extension);
        let source = fixture.base.clone();
        let id = para_id(&source, CELL);
        let (output, run) = fixture.edit(
            &source,
            "cell-only",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                json!([block("body", "Cell one."), block("body", "Cell two.")])
            )]),
            false,
        );
        success(run, "two paragraphs into a cell");
        let report = report_of(&fixture, "cell-only");
        let reported = created_ids(&report, 0);
        let published = segments_of(&output);
        // The cell's anchor paragraph and the body around the table must stay out of the report.
        for (kind, other) in &published {
            let inserted_here = reported.contains(other);
            let is_anchor_or_body = *other == id
                || (kind == "para" && depth(other) == 1)
                || (kind == "run" && depth(other) == 2);
            if is_anchor_or_body {
                assert!(
                    !inserted_here,
                    "{extension}: {kind} {other} was not inserted but is reported"
                );
            }
        }
        let groups = grouped(&report, 0, &output);
        assert_eq!(
            groups.len(),
            2,
            "{extension}: the two cell paragraphs: {reported:?}"
        );
        assert!(
            groups.iter().all(|(para, _)| depth(para) == 4),
            "{extension}: cell depth"
        );
    }
}

/// Two tables in the same section: inserting into the first table's cell must not report the
/// second table's cell paragraphs, which sit at the same depth and the same final index range
/// but under a different parent.
#[test]
fn insert_blocks_in_one_table_does_not_report_a_sibling_tables_cell_paragraphs() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-ids-two-tables", extension);
        let mut spec: Value =
            serde_json::from_slice(&std::fs::read(fixture.dir.join("spec.json")).unwrap()).unwrap();
        let second = json!({"type":"table","columns":[{"width_mm":40}],"rows":[{"cells":[
            {"blocks":[block("body","Second cell A."), block("body","Second cell B.")]}]}]});
        spec["sections"][0]["blocks"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let spec_path = fixture.dir.join("two-tables.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let hwpx = fixture.dir.join("two-tables.hwpx");
        success(
            hwp()
                .arg("compose")
                .arg(&spec_path)
                .arg("-o")
                .arg(&hwpx)
                .output()
                .unwrap(),
            "compose two tables",
        );
        let source = if extension == "hwp" {
            let native = fixture.dir.join("two-tables.hwp");
            success(
                hwp()
                    .arg("convert")
                    .arg(&hwpx)
                    .arg("-o")
                    .arg(&native)
                    .output()
                    .unwrap(),
                "convert two tables",
            );
            native
        } else {
            hwpx
        };
        let id = para_id(&source, CELL);
        let (output, run) = fixture.edit(
            &source,
            "two-tables",
            json!([insert(
                json!({"id": id}),
                "after",
                "create",
                json!({}),
                json!([
                    block("body", "Into first."),
                    block("body", "Into first too.")
                ])
            )]),
            false,
        );
        success(run, "insert into the first table's cell");
        let reported = created_ids(&report_of(&fixture, "two-tables"), 0);
        let after = read(&output);
        let second_cell_ids: Vec<String> = segments_of(&output)
            .into_iter()
            .map(|(_, id)| id)
            .filter(|candidate| {
                // every segment whose path runs through the second table
                let steps: Vec<usize> = candidate
                    .split('.')
                    .skip(2)
                    .filter_map(|n| n.parse().ok())
                    .collect();
                steps.len() >= 4
                    && steps[0]
                        == after.sections[0]
                            .paragraphs
                            .iter()
                            .rposition(|p| {
                                p.controls.iter().any(|c| matches!(c, Control::Table(_)))
                            })
                            .unwrap()
            })
            .collect();
        assert!(
            !second_cell_ids.is_empty(),
            "{extension}: the second table publishes segments"
        );
        for other in second_cell_ids {
            assert!(
                !reported.contains(&other),
                "{extension}: {other} belongs to the second table and was not inserted"
            );
        }
    }
}

#[test]
fn insert_blocks_report_ids_follow_an_earlier_insertion_in_the_same_batch() {
    let fixture = Fixture::new("report-ids-batch", "hwpx");
    let before = read(&fixture.base);
    let anchor = at(&before, ANCHOR);
    let (output, run) = fixture.edit(
        &fixture.base,
        "batch",
        json!([
            insert(
                anchor.clone(),
                "after",
                "create",
                json!({}),
                json!([block("body", "First batch.")])
            ),
            insert(
                anchor,
                "after",
                "create",
                json!({}),
                json!([block("body", "Second batch.")])
            )
        ]),
        false,
    );
    success(run, "two insert_blocks ops at one original address");
    let report = report_of(&fixture, "batch");
    // Op 1 inserted at the anchor too, so it sits ahead of op 0's paragraph. The ids it reports
    // describe the state right after op 1, and must still address ITS paragraph in the output.
    let second = grouped(&report, 1, &output);
    let (styled, run) = fixture.edit(
        &output,
        "batch-followup",
        json!([{"op":"set_format","address":{"id":second[0].1[0]},"bold":"on"}]),
        false,
    );
    success(
        run,
        "set_format on a run id reported by the second insert_blocks",
    );
    let after = read(&styled);
    assert!(
        is_bold_para(&after, "Second batch."),
        "the id op 1 reports addresses op 1's paragraph"
    );
    assert!(!is_bold_para(&after, "First batch."));
}

// ---- #428 T4 review: the report must stay inside edit-report-v1's `changed` cap (T22-F1) ----

fn report_validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap()
}

fn many_paragraphs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|i| block("body", &format!("Bulk {i}.")))
            .collect(),
    )
}

/// The schema's own cap on `changed`, read from the published schema rather than copied.
fn changed_cap() -> usize {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    schema["$defs"]["opOutcome"]["properties"]["changed"]["maxItems"]
        .as_u64()
        .unwrap() as usize
}

#[test]
fn a_report_exactly_at_the_changed_cap_is_complete_and_unflagged() {
    assert_eq!(
        changed_cap(),
        10_000,
        "this test is written for the published cap"
    );
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-at-cap", extension);
        let before = read(&fixture.base);
        // One paragraph and one run per block: 5,000 blocks publish exactly 10,000 segments.
        let (output, run) = fixture.edit(
            &fixture.base,
            "at-cap",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                many_paragraphs(5_000)
            )]),
            false,
        );
        success(run, "5,000 one-run paragraphs");
        let report = report_of(&fixture, "at-cap");
        let op = &report["ops"][0];
        assert_eq!(
            op["changed"].as_array().unwrap().len(),
            10_000,
            "{extension}"
        );
        assert!(
            op.get("changed_total").is_none(),
            "{extension}: nothing was cut, so no truncation signal: {}",
            op["changed_total"]
        );
        assert!(
            report_validator().is_valid(&report),
            "{extension}: at the cap the report is valid"
        );
        assert_eq!(op["pieces_touched"], 5_000);
        let _ = output;
    }
}

#[test]
fn a_report_over_the_changed_cap_is_a_valid_prefix_with_an_explicit_total() {
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("report-over-cap", extension);
        let before = read(&fixture.base);
        // 5,001 blocks publish 10,002 segments: two over the cap.
        let (output, run) = fixture.edit(
            &fixture.base,
            "over-cap",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({}),
                many_paragraphs(5_001)
            )]),
            false,
        );
        success(
            run,
            "5,001 one-run paragraphs is a valid request and must succeed",
        );
        let report = report_of(&fixture, "over-cap");
        let op = &report["ops"][0];
        assert!(
            report_validator().is_valid(&report),
            "{extension}: a successful edit must produce a schema-valid report"
        );
        let changed = op["changed"].as_array().unwrap();
        assert_eq!(
            changed.len(),
            10_000,
            "{extension}: the prefix is cut at the cap"
        );
        assert_eq!(
            op["changed_total"], 10_002,
            "{extension}: the signal carries the real count"
        );
        assert_eq!(
            op["pieces_touched"], 5_001,
            "{extension}: pieces_touched still counts paragraphs"
        );
        // The prefix is deterministic document order and every id in it is published.
        let published: Vec<String> = segments_of(&output).into_iter().map(|(_, id)| id).collect();
        let first = published
            .iter()
            .position(|id| Some(id.as_str()) == changed[0]["after"].as_str())
            .expect("the first reported id is published");
        for (offset, pair) in changed.iter().enumerate() {
            assert!(pair["before"].is_null());
            assert_eq!(
                published[first + offset],
                pair["after"].as_str().unwrap(),
                "{extension}: reported id {offset} is the next published segment"
            );
        }
        // It is a PREFIX: the first reported id is the first inserted paragraph, directly after
        // the anchor, and the segment after the last reported one is still inside the insertion.
        let after = read(&output);
        // A body paragraph id ends in its index: <hash>.<section>.<index>.
        let first_inserted = format!(".0.{}", index(&after, ANCHOR) + 1);
        assert!(
            changed[0]["after"]
                .as_str()
                .unwrap()
                .ends_with(&first_inserted),
            "{extension}: the report starts at the first inserted paragraph ({first_inserted}): {}",
            changed[0]["after"]
        );
        assert_eq!(
            after.sections[0].paragraphs[index(&after, ANCHOR) + 1].plain_text(),
            "Bulk 0.",
            "{extension}: and that paragraph is the first one the request inserted"
        );
        let next = published[first + 10_000].as_str();
        assert!(
            next.starts_with(|c: char| c.is_ascii_hexdigit()) && depth(next) >= 1,
            "{extension}: the first segment left out is a real published segment: {next}"
        );
        // The cut ids are still reachable: the tail is in the output, just not in the report.
        assert!(
            published.len() > first + 10_000,
            "{extension}: the rest is published"
        );
    }
}

#[test]
fn a_large_inserted_table_keeps_the_report_valid() {
    let fixture = Fixture::new("report-big-table", "hwpx");
    let before = read(&fixture.base);
    // 100 x 50 cells, one paragraph and run each: far over the cap in one block.
    let rows: Vec<Value> = (0..100)
        .map(|_| {
            json!({"cells": (0..50).map(|_| json!({"blocks":[block("body", "c")]})).collect::<Vec<_>>()})
        })
        .collect();
    let table = json!([{"type":"table","columns":(0..50).map(|_| json!({"width_mm":3})).collect::<Vec<_>>(),"rows":rows}]);
    let (_, run) = fixture.edit(
        &fixture.base,
        "big-table",
        json!([insert(
            at(&before, ANCHOR),
            "after",
            "create",
            json!({}),
            table
        )]),
        false,
    );
    success(run, "a 100 x 50 table");
    let report = report_of(&fixture, "big-table");
    assert!(
        report_validator().is_valid(&report),
        "the report of a valid request is valid"
    );
    let op = &report["ops"][0];
    assert_eq!(op["changed"].as_array().unwrap().len(), 10_000);
    assert!(
        op["changed_total"].as_u64().unwrap() > 10_000,
        "{}",
        op["changed_total"]
    );
    assert_eq!(op["pieces_touched"], 1);
}
