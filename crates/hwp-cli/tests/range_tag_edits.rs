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
        let workspace = Workspace::new(
            if cell { "convert-cell" } else { "convert-body" },
            cell,
            true,
        );
        let output = workspace.dir.join("converted.hwpx");
        let report = workspace.dir.join("loss.json");
        for strict in [false, true] {
            if output.exists() {
                std::fs::remove_file(&output).unwrap();
            }
            if report.exists() {
                std::fs::remove_file(&report).unwrap();
            }
            let mut command = Command::new(env!("CARGO_BIN_EXE_hwp"));
            command
                .arg("convert")
                .arg(&workspace.source)
                .arg("-o")
                .arg(&output)
                .arg("--loss-report")
                .arg(&report);
            if strict {
                command.arg("--strict");
            }
            let result = command.output().unwrap();
            assert!(
                report.is_file(),
                "convert must write a fresh loss report even on strict refusal"
            );
            let ledger: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
            let event = ledger["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["code"] == "paragraph_range_tag_unrepresentable");
            assert!(
                event.is_some(),
                "PARA_RANGE_TAG loss must be reported: {ledger}"
            );
            let event = event.unwrap();
            assert_eq!(event["count"], 1);
            assert_eq!(event["resource"], "metadata");
            assert_eq!(event["disposition"], "unrepresentable");
            assert_eq!(
                result.status.success(),
                !strict,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(output.exists(), !strict, "strict loss must not publish");
            assert_eq!(std::fs::read(&workspace.source).unwrap(), workspace.bytes);
            if strict {
                let message = String::from_utf8_lossy(&result.stderr);
                assert!(
                    message.contains("ahwp convert")
                        && message.contains("hwp_convert")
                        && message.contains("strict: false"),
                    "convert recovery must name the CLI and MCP loss opt-ins: {message}"
                );
                assert!(message.contains("구간 표시"), "{message}");
            }
        }
    }
}

#[test]
fn hwpx_writer_aggregates_range_tag_records_in_nested_paragraphs() {
    use hwp_model::{
        Caption, CaptionDirection, CaptionSide, GenericControl, Paragraph, ParagraphList,
    };
    let tagged = || Paragraph {
        extras: vec![
            OpaqueRecord {
                tag: 0x46,
                data: b"opaque sentinel".to_vec(),
                children: Vec::new()
            };
            2
        ],
        ..Paragraph::default()
    };
    let caption = || Caption {
        side: CaptionSide::Bottom,
        direction: CaptionDirection::Horizontal,
        gap: 0,
        width: None,
        last_width: 0,
        paragraphs: vec![tagged()],
    };
    let mut document = hwp_convert::from_markdown("| value |\n| --- |\n| anchor |\n");
    let table = document.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.controls)
        .find_map(|c| match c {
            Control::Table(t) => Some(t),
            _ => None,
        })
        .unwrap();
    table.cells[0].paragraphs = vec![tagged()];
    table.caption = Some(caption());
    for ctrl_id in [*b"head", *b"gso "] {
        let control = Control::Generic(GenericControl {
            ctrl_id,
            data: vec![0; 40],
            paragraph_lists: vec![ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![tagged()],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        });
        let paragraph = &mut document.sections[0].paragraphs[0];
        paragraph.chars.push(hwp_model::HwpChar::ExtCtrl {
            code: 11,
            ctrl_id,
            payload: Vec::new(),
            ctrl_index: Some(paragraph.controls.len() as u32),
        });
        paragraph.controls.push(control);
    }
    let mut report = hwp_model::WriteReport::new();
    hwpx::write::section::write_section_with_report(
        &document,
        &document.sections[0],
        false,
        &mut hwpx::write::section::BinCollector::default(),
        &mut report,
    );
    let event = report
        .preservation
        .events
        .iter()
        .find(|e| e.code.as_str() == "paragraph_range_tag_unrepresentable")
        .expect("nested range-tag loss must be reported");
    assert_eq!(
        event.count, 8,
        "two records each in cell, caption, header and text box"
    );
}

#[test]
fn edit_hwpx_refuses_range_tag_loss() {
    for cell in [false, true] {
        let mut workspace = Workspace::new(
            if cell {
                "edit-hwpx-cell"
            } else {
                "edit-hwpx-body"
            },
            cell,
            true,
        );
        workspace.output = workspace.dir.join("edited.hwpx");
        let result = workspace.edit(&["--set-meta", "title=reviewed"]);
        assert!(
            !result.status.success(),
            "cross-format edit must refuse range-tag loss"
        );
        assert!(!workspace.output.exists(), "refused edit must not publish");
        assert_eq!(std::fs::read(&workspace.source).unwrap(), workspace.bytes);
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("paragraph_range_tag_unrepresentable: 1 item(s)"),
            "{message}"
        );
        assert!(
            message.contains(".hwp 출력 경로"),
            "missing safe output remedy: {message}"
        );
        assert!(
            message.contains("ahwp convert") && message.contains("--strict 없이"),
            "missing opt-in loss remedy: {message}"
        );
        assert!(
            message.contains("hwp_convert") && message.contains("strict: false"),
            "edit recovery must name the MCP conversion loss opt-in: {message}"
        );
        assert!(message.contains("구간 표시"), "{message}");
    }
}

#[test]
fn merge_hwpx_reports_range_loss_and_names_its_own_loss_opt_in() {
    let tagged = Workspace::new("merge-tagged", false, true);
    let ordinary = Workspace::new("merge-ordinary", false, false);
    let output = tagged.dir.join("merged.hwpx");
    let report = tagged.dir.join("merge-loss.json");
    for strict in [false, true] {
        for artifact in [&output, &report] {
            if artifact.exists() {
                std::fs::remove_file(artifact).unwrap();
            }
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_ahwp"));
        command
            .arg("merge")
            .arg(&tagged.source)
            .arg(&ordinary.source)
            .arg("-o")
            .arg(&output)
            .arg("--loss-report")
            .arg(&report);
        if strict {
            command.arg("--strict");
        }
        let result = command.output().unwrap();
        let message = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.success(), !strict, "{message}");
        assert_eq!(output.exists(), !strict);
        assert!(report.is_file(), "merge must leave fresh loss evidence");
        let ledger: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        let event = ledger["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["code"] == "paragraph_range_tag_unrepresentable")
            .expect("merge must report its dropped range-tag record");
        assert_eq!(event["count"], 1);
        if strict {
            assert!(
                message.contains("ahwp merge")
                    && message.contains("hwp_merge")
                    && message.contains("strict: false"),
                "merge recovery must name merge-specific loss opt-ins: {message}"
            );
            assert!(!message.contains("ahwp convert"), "{message}");
            assert!(message.contains("구간 표시"), "{message}");
        } else {
            assert!(
                message.contains("paragraph_range_tag_unrepresentable"),
                "non-strict merge must warn about its range-tag loss: {message}"
            );
        }
        assert_eq!(std::fs::read(&tagged.source).unwrap(), tagged.bytes);
        assert_eq!(std::fs::read(&ordinary.source).unwrap(), ordinary.bytes);
    }
}

#[test]
fn fill_hwpx_zero_match_refuses_range_tag_loss() {
    for cell in [false, true] {
        let mut workspace = Workspace::new(
            if cell {
                "fill-zero-cell"
            } else {
                "fill-zero-body"
            },
            true,
            cell,
        );
        if !cell {
            let mut document = hwp5::read_document(&workspace.source).unwrap().document;
            document.sections[0].paragraphs[0]
                .extras
                .push(OpaqueRecord {
                    tag: 0x46,
                    data: b"opaque range sentinel".to_vec(),
                    children: Vec::new(),
                });
            hwp5::write_document(&document, &workspace.source, &hwp5::WriteOptions::default())
                .unwrap();
            workspace.bytes = std::fs::read(&workspace.source).unwrap();
            let reread = hwp5::read_document(&workspace.source).unwrap().document;
            assert_eq!(
                reread.sections[0].paragraphs[0]
                    .extras
                    .iter()
                    .filter(|record| record.tag == 0x46)
                    .count(),
                1
            );
        }
        let data = workspace.dir.join("tables.json");
        std::fs::write(
            &data,
            r#"{"missing":"value","tables":[{"table":0,"rows":[]}]}"#,
        )
        .unwrap();
        let output = workspace.dir.join("filled.hwpx");
        let result = Command::new(env!("CARGO_BIN_EXE_ahwp"))
            .arg("fill")
            .arg(&workspace.source)
            .arg("-o")
            .arg(&output)
            .arg("--data")
            .arg(&data)
            .arg("--allow-partial")
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "zero-match cross-format fill must refuse range-tag loss"
        );
        assert!(!output.exists(), "refused zero-match fill must not publish");
        assert_eq!(std::fs::read(&workspace.source).unwrap(), workspace.bytes);
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("paragraph_range_tag_unrepresentable"),
            "{message}"
        );
        assert!(
            message.contains("fill: 보존 불가"),
            "fill refusal must name its own operation: {message}"
        );
        assert!(
            message.contains("먼저 .hwpx로 변환한 뒤")
                && message.contains("변환된 파일로 원래 작업을 다시 실행"),
            "zero-match fill refusal must require convert-first and retry: {message}"
        );
    }
}

#[test]
fn fill_parts_hwpx_refusal_names_convert_first_recovery() {
    for cell in [false, true] {
        let mut workspace = Workspace::new(
            if cell {
                "fill-parts-cell"
            } else {
                "fill-parts-body"
            },
            cell,
            true,
        );
        let mut document = hwp5::read_document(&workspace.source).unwrap().document;
        document.sections[0].paragraphs.push(hwp_model::Paragraph {
            chars: "{{body}}".chars().map(hwp_model::HwpChar::Text).collect(),
            ..hwp_model::Paragraph::default()
        });
        hwp5::write_document(&document, &workspace.source, &hwp5::WriteOptions::default()).unwrap();
        workspace.bytes = std::fs::read(&workspace.source).unwrap();
        let part = workspace.dir.join("part.md");
        std::fs::write(&part, "Public replacement part.\n").unwrap();
        let output = workspace.dir.join("filled.hwpx");
        let result = Command::new(env!("CARGO_BIN_EXE_ahwp"))
            .arg("fill")
            .arg(&workspace.source)
            .arg("-o")
            .arg(&output)
            .arg("--set")
            .arg(format!("body=@{}", part.display()))
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "parts fill must refuse range-tag loss"
        );
        assert!(!output.exists(), "refused parts fill must not publish");
        assert_eq!(std::fs::read(&workspace.source).unwrap(), workspace.bytes);
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("paragraph_range_tag_unrepresentable"),
            "{message}"
        );
        assert!(
            message.contains("먼저 .hwpx로 변환한 뒤")
                && message.contains("변환된 파일로 원래 작업을 다시 실행"),
            "fill-parts refusal must require convert-first and retry: {message}"
        );
        assert!(
            message.contains("fill: 보존 불가"),
            "fill-parts refusal must name its own operation: {message}"
        );
        assert!(
            message.contains("hwp_convert") && message.contains("strict: false"),
            "fill-parts recovery must include the MCP conversion opt-in: {message}"
        );
    }
}

/// #531: the refused run still writes edit-report-v1, and the refusing op carries
/// `range_tag_shift`.
#[test]
fn range_tag_refusal_is_reported_with_its_code() {
    let workspace = Workspace::new("report", false, true);
    let ops = workspace.dir.join("ops.json");
    let report = workspace.dir.join("report.json");
    std::fs::write(&ops, r#"[{"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"anchor","to":"expanded anchor"}]"#).unwrap();
    let result = workspace.edit(&[
        "--ops",
        ops.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    workspace.assert_refusal(result);
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("refused run writes the report"))
            .unwrap();
    assert_eq!(report["refusal"]["code"], "range_tag_shift", "{report}");
    assert_eq!(report["ops"][0]["status"], "failed");
    assert_eq!(report["ops"][0]["refusal"]["code"], "range_tag_shift");
}
