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
        assert!(
            report["ops"]
                .as_array()
                .unwrap()
                .iter()
                .all(|op| op["reason"] == "insert_blocks: missing_style=refuse")
        );
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
        let before = read(&fixture.base);
        let (body_id, body) = before
            .header
            .styles
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == "body")
            .unwrap();
        let inserted = "Literal {{name}} => & value.";
        let (output, run) = fixture.edit(
            &fixture.base,
            "target-wins",
            json!([insert(
                at(&before, ANCHOR),
                "after",
                "create",
                json!({"body":{"font_size_pt":27,"bold":true,"align":"right"}}),
                json!([block("body", inserted)])
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
            .find(|row| row["name"] == "body")
            .unwrap();
        assert_eq!(row["id"], body_id);
        assert_eq!(row["attrs"]["char"]["size_pt"], 10.0);
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
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            after.sections[0]
                .paragraphs
                .iter()
                .map(Paragraph::plain_text)
                .collect::<Vec<_>>(),
            "cell insertion must not splice into the body list"
        );
    }
}

#[test]
fn unsupported_resource_blocks_and_runs_refuse_atomically() {
    let unsupported = vec![
        (
            "table",
            json!({"type":"table","columns":[{"width_mm":20}],"rows":[{"cells":[{"blocks":[block("body","Nested table text.")]}]}]}),
        ),
        (
            "image",
            json!({"type":"image","path":"must-not-read.png","width_mm":20}),
        ),
        (
            "field",
            json!({"type":"field","name":"component-field","value":"value"}),
        ),
        ("equation", json!({"type":"equation","script":"x^2"})),
        (
            "run-image",
            json!({"type":"paragraph","style":"body","runs":[{"type":"image","path":"must-not-read.png","width_mm":20}]}),
        ),
        (
            "run-field",
            json!({"type":"paragraph","style":"body","runs":[{"type":"field","name":"component-field","value":"value"}]}),
        ),
        (
            "run-equation",
            json!({"type":"paragraph","style":"body","runs":[{"type":"equation","script":"x^2"}]}),
        ),
    ];
    for extension in ["hwp", "hwpx"] {
        let fixture = Fixture::new("unsupported", extension);
        let source = fixture.base.clone();
        let before = read(&source);
        for (label, unsupported) in &unsupported {
            let diagnostic = fixture.refuse(
                &source,
                label,
                json!([insert(
                    at(&before, ANCHOR),
                    "after",
                    "create",
                    json!({}),
                    json!([block("body", "Not partially published."), unsupported])
                )]),
                false,
            );
            let kind = label.strip_prefix("run-").unwrap_or(label);
            assert!(
                diagnostic.contains("insert_blocks는 현재")
                    && diagnostic.contains("지원")
                    && diagnostic.contains(kind)
                    && diagnostic.contains("#428 T3"),
                "unsupported {kind} must identify its supported-scope remedy, not merely fail schema validation: {diagnostic}"
            );
        }
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
        assert_eq!(
            report["ops"][0]["reason"],
            "insert_blocks: missing_style=create"
        );
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
