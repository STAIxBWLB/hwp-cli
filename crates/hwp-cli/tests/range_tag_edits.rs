//! End-to-end controlled refusals for opaque HWP5 paragraph range tags (#470).
//! Sentinels intentionally encode no tag layout; no genuine tagged sample is available.

use std::path::PathBuf;
use std::process::Command;

use hwp_model::{Control, OpaqueRecord};

#[path = "common/temp_path.rs"]
mod temp_path;

struct Workspace {
    dir: temp_path::TempPath,
    source: PathBuf,
    output: PathBuf,
    bytes: Vec<u8>,
}

impl Workspace {
    fn new(label: &str, cell: bool, tagged: bool) -> Self {
        let dir = temp_path::TempPath::new("hwp470", label);
        std::fs::create_dir_all(&*dir).unwrap();
        let mut document = hwp_convert::from_markdown(if cell {
            "| name | value |\n| --- | --- |\n| field | anchor |\n"
        } else {
            "anchor first\n\nanchor marked\n"
        });
        let paragraph = if cell {
            let table = document.sections[0]
                .paragraphs
                .iter_mut()
                .flat_map(|paragraph| &mut paragraph.controls)
                .find_map(|control| match control {
                    Control::Table(table) => Some(table),
                    _ => None,
                })
                .unwrap();
            &mut table
                .cells
                .iter_mut()
                .find(|cell| cell.row == 1 && cell.col == 1)
                .unwrap()
                .paragraphs[0]
        } else {
            &mut document.sections[0].paragraphs[1]
        };
        if tagged {
            paragraph.extras.push(OpaqueRecord {
                tag: 0x46,
                data: b"opaque range sentinel".to_vec(),
                children: Vec::new(),
            });
        }
        let source = dir.join("source.hwp");
        hwp5::write_document(&document, &source, &hwp5::WriteOptions::default()).unwrap();
        // The public reader must retain the marker before this test can establish refusal.
        let read = hwp5::read_document(&source).unwrap().document;
        fn count(paragraphs: &[hwp_model::Paragraph]) -> usize {
            paragraphs
                .iter()
                .map(|paragraph| {
                    paragraph
                        .extras
                        .iter()
                        .filter(|extra| extra.tag == 0x46)
                        .count()
                        + paragraph
                            .controls
                            .iter()
                            .map(|control| match control {
                                Control::Table(table) => {
                                    table.cells.iter().map(|cell| count(&cell.paragraphs)).sum()
                                }
                                Control::Generic(generic) => generic
                                    .paragraph_lists
                                    .iter()
                                    .map(|list| count(&list.paragraphs))
                                    .sum(),
                                _ => 0,
                            })
                            .sum::<usize>()
                })
                .sum()
        }
        assert_eq!(
            read.sections
                .iter()
                .map(|section| count(&section.paragraphs))
                .sum::<usize>(),
            usize::from(tagged)
        );
        let bytes = std::fs::read(&source).unwrap();
        Self {
            output: dir.join("out.hwp"),
            dir,
            source,
            bytes,
        }
    }

    fn edit(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_hwp"))
            .arg("edit")
            .arg(&self.source)
            .arg("-o")
            .arg(&self.output)
            .args(args)
            .output()
            .unwrap()
    }

    fn assert_refusal(&self, result: std::process::Output) {
        assert!(
            !result.status.success(),
            "a shifting edit on an opaque range tag must refuse"
        );
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("PARA_RANGE_TAG"),
            "missing range-tag cause: {message}"
        );
        assert!(
            message.contains("한글") && message.contains("같"),
            "missing safe-edit remedy: {message}"
        );
        assert!(!self.output.exists(), "a refused edit must not publish");
        assert_eq!(std::fs::read(&self.source).unwrap(), self.bytes);
    }
}

#[test]
fn global_replace_refuses_before_publication_when_a_later_match_is_tagged() {
    let workspace = Workspace::new("global", false, true);
    workspace.assert_refusal(workspace.edit(&["--replace", "anchor=>expanded anchor"]));
}

#[test]
fn addressed_replace_refuses_before_publication_on_a_tagged_paragraph() {
    let workspace = Workspace::new("addressed", false, true);
    let ops = workspace.dir.join("ops.json");
    std::fs::write(&ops, r#"[{"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"anchor","to":"expanded anchor"}]"#).unwrap();
    workspace.assert_refusal(workspace.edit(&["--ops", ops.to_str().unwrap()]));
}

#[test]
fn cell_rebuild_refuses_to_discard_a_range_tag() {
    let workspace = Workspace::new("cell", true, true);
    workspace.assert_refusal(workspace.edit(&["--set-cell", "0:1:1=expanded anchor"]));
}

#[test]
fn replacement_without_tags_remains_available() {
    let workspace = Workspace::new("ordinary", false, false);
    let result = workspace.edit(&["--replace", "anchor=>expanded anchor"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn equal_wchar_width_replacement_preserves_opaque_tag_bytes() {
    let workspace = Workspace::new("same-width", false, true);
    let result = workspace.edit(&["--replace", "anchor=>ANCHOR"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let document = hwp5::read_document(&workspace.output).unwrap().document;
    assert_eq!(
        document.sections[0].paragraphs[1]
            .extras
            .iter()
            .find(|extra| extra.tag == 0x46)
            .unwrap()
            .data,
        b"opaque range sentinel"
    );
}

#[test]
fn named_field_tab_refuses_to_shift_a_tagged_range_before_publication() {
    let dir = temp_path::TempPath::new("hwp470", "field-tab-width");
    std::fs::create_dir_all(&*dir).unwrap();
    let mut document = hwp_convert::from_markdown("Field anchor.");
    assert!(
        hwp_convert::create_field(&mut document, "Field anchor.", "entry", "A").unwrap(),
        "setup must create a named field before attaching the opaque tag"
    );
    document.sections[0].paragraphs[0]
        .extras
        .push(OpaqueRecord {
            tag: 0x46,
            data: b"opaque range sentinel".to_vec(),
            children: Vec::new(),
        });
    let source = dir.join("source.hwp");
    hwp5::write_document(&document, &source, &hwp5::WriteOptions::default()).unwrap();
    let reread = hwp5::read_document(&source).unwrap().document;
    let fields = hwp_convert::list_fields(&reread);
    assert_eq!(fields.len(), 1, "setup must retain exactly one field");
    assert_eq!(fields[0].name.as_deref(), Some("entry"));
    assert_eq!(fields[0].value, "A", "setup must retain a one-WCHAR value");
    assert_eq!(
        reread.sections[0].paragraphs[0]
            .extras
            .iter()
            .find(|record| record.tag == 0x46)
            .expect("source read-back must retain the opaque range tag")
            .data,
        b"opaque range sentinel"
    );
    let workspace = Workspace {
        output: dir.join("out.hwp"),
        bytes: std::fs::read(&source).unwrap(),
        source,
        dir,
    };
    // A serialized HWP tab occupies eight WCHARs, so this changes the value width.
    workspace.assert_refusal(workspace.edit(&["--set-field", "entry=	"]));
}

#[test]
fn convert_hwpx_reports_range_tag_loss_and_strict_refuses() {
    for cell in [false, true] {
        let workspace = Workspace::new(if cell { "convert-cell" } else { "convert-body" }, cell, true);
        let output = workspace.dir.join("converted.hwpx");
        let report = workspace.dir.join("loss.json");
        for strict in [false, true] {
            if output.exists() {
                std::fs::remove_file(&output).unwrap();
            }
            let mut command = Command::new(env!("CARGO_BIN_EXE_hwp"));
            command.arg("convert").arg(&workspace.source).arg("-o").arg(&output)
                .arg("--loss-report").arg(&report);
            if strict {
                command.arg("--strict");
            }
            let result = command.output().unwrap();
            let ledger: serde_json::Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
            let event = ledger["events"].as_array().unwrap().iter()
                .find(|event| event["code"] == "paragraph_range_tag_unrepresentable");
            assert!(event.is_some(), "PARA_RANGE_TAG loss must be reported: {ledger}");
            let event = event.unwrap();
            assert_eq!(event["count"], 1);
            assert_eq!(event["resource"], "metadata");
            assert_eq!(event["disposition"], "unrepresentable");
            assert_eq!(result.status.success(), !strict, "{}", String::from_utf8_lossy(&result.stderr));
            assert_eq!(output.exists(), !strict, "strict loss must not publish");
            assert_eq!(std::fs::read(&workspace.source).unwrap(), workspace.bytes);
        }
    }
}
