//! Synthetic split side-storage policy and preservation reporting (#449).
//! Opaque sentinels exercise container passthrough, not history/schema record parsing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::{
    Document, LineSeg, PreservationCode, PreservationDisposition, PreservationReport,
    PreservationResourceKind,
};

#[path = "common/temp_path.rs"]
mod temp_path;

const HISTORY_A: &[u8] = b"PRIVATE_HISTORY_449_VERSION_A";
const HISTORY_B: &[u8] = b"PRIVATE_HISTORY_449_VERSION_B";
const SCHEMA: &[u8] = b"opaque owner-authored schema sentinel";

#[derive(Clone, Copy)]
enum Selection {
    Sections,
    Page,
    Range,
}

struct Workspace {
    dir: temp_path::TempPath,
    input: PathBuf,
    out: PathBuf,
    loss: PathBuf,
    range: PathBuf,
    source_bytes: Vec<u8>,
}

fn source_document(history: bool) -> Document {
    let mut doc = hwp_convert::from_markdown("First section\n\nSelected body\n\nLast body\n");
    assert_eq!(doc.sections[0].paragraphs.len(), 3);
    doc.sections.push(doc.sections[0].clone());
    doc.header.properties.section_count = 2;
    // Explicit synthetic page-start flags drive page selection; no font-derived page count.
    for section in &mut doc.sections {
        for (index, paragraph) in section.paragraphs.iter_mut().enumerate() {
            paragraph.line_segs = vec![LineSeg {
                text_start: 0,
                v_pos: if index == 2 { 1600 } else { 0 },
                line_height: 1000,
                text_height: 1000,
                baseline_gap: 850,
                line_spacing: 600,
                col_start: 0,
                seg_width: 10000,
                flags: u32::from(index < 2),
            }];
        }
    }
    doc.hwp5_xml_template = vec![("/XMLTemplate/Schema".into(), SCHEMA.to_vec())];
    if history {
        doc.hwp5_doc_history = vec![
            ("/DocHistory/VersionLog0".into(), HISTORY_A.to_vec()),
            ("/DocHistory/VersionLog1".into(), HISTORY_B.to_vec()),
        ];
    }
    doc
}

impl Workspace {
    fn new(label: &str, history: bool) -> Self {
        let dir = temp_path::TempPath::new("hwp449", label);
        std::fs::create_dir_all(&*dir).unwrap();
        let input = dir.join("source.hwp");
        hwp5::write_document(
            &source_document(history),
            &input,
            &hwp5::WriteOptions {
                preserve_linesegs: true,
                ..Default::default()
            },
        )
        .unwrap();
        let read = hwp5::read_document(&input).unwrap().document;
        assert_eq!(read.hwp5_doc_history.len(), if history { 2 } else { 0 });
        assert_eq!(
            hwp_convert::document_split::page_start_addresses(&read).len(),
            4,
            "the fixture must retain its four explicit saved-page markers"
        );
        let source_bytes = std::fs::read(&input).unwrap();
        Self {
            out: dir.join("out"),
            loss: dir.join("loss.json"),
            range: dir.join("range.json"),
            dir,
            input,
            source_bytes,
        }
    }

    fn split(&self, selection: Selection, strict: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ahwp"));
        command
            .arg("split")
            .arg(&self.input)
            .arg("--out-dir")
            .arg(&self.out)
            .arg("--loss-report")
            .arg(&self.loss);
        match selection {
            Selection::Sections => {}
            Selection::Page => {
                command.args(["--pages", "2"]);
            }
            Selection::Range => {
                command
                    .args(["--from", "at:0.1", "--to", "at:0.2", "--range-report"])
                    .arg(&self.range);
            }
        }
        if strict {
            command.arg("--strict");
        }
        command.output().unwrap()
    }

    fn fragments(&self) -> Vec<PathBuf> {
        let entries = match std::fs::read_dir(&self.out) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("cannot inspect fragment output: {error}"),
        };
        let mut paths: Vec<_> = entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "hwp"))
            .collect();
        paths.sort();
        paths
    }

    fn report(&self) -> PreservationReport {
        serde_json::from_slice(&std::fs::read(&self.loss).unwrap()).unwrap()
    }

    fn assert_source_unchanged(&self) {
        assert_eq!(std::fs::read(&self.input).unwrap(), self.source_bytes);
    }
}

fn removed_count(report: &PreservationReport, code: PreservationCode) -> usize {
    report
        .events
        .iter()
        .filter(|event| event.code == code && event.disposition == PreservationDisposition::Removed)
        .map(|event| event.count)
        .sum()
}

fn assert_no_history(path: &Path) {
    let mut container = hwp5::Hwp5Container::open(path).unwrap();
    assert!(
        !container
            .list_storages()
            .iter()
            .any(|name| name == "/DocHistory" || name.starts_with("/DocHistory/")),
        "a split fragment must not retain the source's whole DocHistory storage"
    );
    let bytes = std::fs::read(path).unwrap();
    for history in [HISTORY_A, HISTORY_B] {
        assert!(
            !bytes.windows(history.len()).any(|window| window == history),
            "excluded revision bytes must not remain in the fragment container"
        );
    }
    assert_eq!(
        container.read_stream_raw("/XMLTemplate/Schema").unwrap(),
        SCHEMA
    );
    assert!(
        hwp5::read_document(path)
            .unwrap()
            .document
            .hwp5_doc_history
            .is_empty()
    );
}

fn assert_history_omissions(selection: Selection, label: &str, expected_fragments: usize) {
    let workspace = Workspace::new(label, true);
    let result = workspace.split(selection, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let fragments = workspace.fragments();
    assert_eq!(fragments.len(), expected_fragments);
    for fragment in fragments {
        assert_no_history(&fragment);
    }
    let report = workspace.report();
    assert_eq!(
        removed_count(&report, PreservationCode::HwpContainerStorageRemoved),
        expected_fragments
    );
    assert_eq!(
        removed_count(&report, PreservationCode::HwpContainerStreamRemoved),
        expected_fragments * 2
    );
    assert!(report.events.iter().all(|event| matches!(
        event.resource,
        PreservationResourceKind::ContainerStorage | PreservationResourceKind::ContainerStream
    )));
    workspace.assert_source_unchanged();
}

#[test]
fn section_fragments_drop_history_and_report_omissions() {
    assert_history_omissions(Selection::Sections, "sections", 2);
}

#[test]
fn saved_page_fragments_drop_history_and_report_omissions() {
    assert_history_omissions(Selection::Page, "pages", 1);
}

#[test]
fn typed_range_fragments_drop_history_and_report_omissions() {
    assert_history_omissions(Selection::Range, "range", 1);
}

#[test]
fn history_omissions_are_reported_before_strict_refusal() {
    let workspace = Workspace::new("strict", true);
    let result = workspace.split(Selection::Sections, true);
    assert!(
        !result.status.success(),
        "strict split must refuse its intentional history loss"
    );
    assert_eq!(
        removed_count(
            &workspace.report(),
            PreservationCode::HwpContainerStorageRemoved
        ),
        2
    );
    assert_eq!(
        removed_count(
            &workspace.report(),
            PreservationCode::HwpContainerStreamRemoved
        ),
        4
    );
    assert!(workspace.fragments().is_empty());
    workspace.assert_source_unchanged();
}

#[test]
fn empty_history_keeps_schema_and_has_no_storage_omission() {
    let workspace = Workspace::new("empty", false);
    let result = workspace.split(Selection::Range, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        removed_count(
            &workspace.report(),
            PreservationCode::HwpContainerStorageRemoved
        ),
        0
    );
    assert_eq!(
        removed_count(
            &workspace.report(),
            PreservationCode::HwpContainerStreamRemoved
        ),
        0
    );
    for fragment in workspace.fragments() {
        assert_no_history(&fragment);
    }
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&workspace.range).unwrap()).unwrap();
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/range-extraction-v1.schema.json"
    ))
    .unwrap();
    assert!(
        jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&report),
        "storage accounting must not change the frozen range report"
    );
}

#[test]
fn ordinary_convert_edit_and_identity_keep_history() {
    let workspace = Workspace::new("ordinary", true);
    let original = hwp5::read_document(&workspace.input).unwrap().document;
    let identity = workspace.dir.join("identity.hwp");
    hwp5::rewrite_document_with_report(
        &workspace.input,
        &original,
        &original,
        &identity,
        &hwp5::WriteOptions::default(),
    )
    .unwrap();
    assert_eq!(std::fs::read(&identity).unwrap(), workspace.source_bytes);
    for (command, output) in [("convert", "converted.hwp"), ("edit", "edited.hwp")] {
        let output = workspace.dir.join(output);
        let mut process = Command::new(env!("CARGO_BIN_EXE_ahwp"));
        process
            .arg(command)
            .arg(&workspace.input)
            .arg("-o")
            .arg(&output);
        if command == "edit" {
            process.args(["--replace", "Selected body=>Edited body"]);
        }
        let result = process.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let document = hwp5::read_document(&output).unwrap().document;
        assert_eq!(document.hwp5_doc_history, original.hwp5_doc_history);
        assert_eq!(document.hwp5_xml_template, original.hwp5_xml_template);
    }
    workspace.assert_source_unchanged();
}

#[test]
fn hwpx_extra_parts_and_manifest_entries_remain_paired() {
    let dir = temp_path::TempPath::new("hwp449", "extra");
    std::fs::create_dir_all(&*dir).unwrap();
    let mut document = source_document(false);
    document.hwp5_xml_template.clear();
    let payload = b"<layout owner=\"synthetic\"/>";
    document.hwpx_extra_entries = vec![("DocOptions/Layout.xml".into(), payload.to_vec())];
    document.hwpx_opf_extra_items = vec![(
        "layout-part".into(),
        "DocOptions/Layout.xml".into(),
        "application/xml".into(),
    )];
    let source = dir.join("source.hwpx");
    hwpx::write_document(&document, &source).unwrap();
    let output = dir.join("out");
    let result = Command::new(env!("CARGO_BIN_EXE_ahwp"))
        .arg("split")
        .arg(&source)
        .arg("--out-dir")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let fragment = hwpx::read_document(&output.join("source-001.hwpx"))
        .unwrap()
        .document;
    assert!(
        fragment
            .hwpx_extra_entries
            .iter()
            .any(|(name, bytes)| name == "DocOptions/Layout.xml" && bytes == payload)
    );
    assert!(
        fragment
            .hwpx_opf_extra_items
            .iter()
            .any(|(_, href, _)| href == "DocOptions/Layout.xml")
    );
}
