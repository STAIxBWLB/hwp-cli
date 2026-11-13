//! HWP5-target verification must compare represented semantics without accepting real loss.
//! Public inputs are generated at test time; no private document or binary fixture is copied.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn workspace(label: &str) -> temp_path::TempPath {
    let dir = temp_path::TempPath::new("hwp444", label);
    std::fs::create_dir_all(&*dir).unwrap();
    dir
}

fn metadata_edit(input: &Path, output: &Path) -> Output {
    command()
        .arg("edit")
        .arg(input)
        .args(["--set-meta", "title=t", "--verify", "-o"])
        .arg(output)
        .output()
        .unwrap()
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn rewrite_public_entry(
    input: &Path,
    destination: &Path,
    entry_name: &str,
    mut rewrite: impl FnMut(String) -> String,
) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(input).unwrap()).unwrap();
    let mut output = zip::ZipWriter::new(std::fs::File::create(destination).unwrap());
    let mut found = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == entry_name {
            bytes = rewrite(String::from_utf8(bytes).unwrap()).into_bytes();
            found = true;
        }
        output.start_file(name, options).unwrap();
        output.write_all(&bytes).unwrap();
    }
    output.finish().unwrap();
    assert!(found, "public fixture must contain the rewritten entry");
}

fn set_public_hold_anchor(input: &Path) {
    let temporary = input.with_extension("repacked.hwpx");
    rewrite_public_entry(input, &temporary, "Contents/section0.xml", |xml| {
        assert!(xml.contains("holdAnchorAndSO=\"0\""));
        xml.replacen("holdAnchorAndSO=\"0\"", "holdAnchorAndSO=\"1\"", 1)
    });
    std::fs::remove_file(input).unwrap();
    std::fs::rename(temporary, input).unwrap();
}

fn without_public_font_type_info(xml: String) -> String {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut ranges = Vec::new();
    loop {
        let start = usize::try_from(reader.buffer_position()).unwrap();
        match reader.read_event().unwrap() {
            Event::Empty(element) if element.local_name().as_ref() == "typeInfo" => {
                ranges.push((start, usize::try_from(reader.buffer_position()).unwrap()));
            }
            Event::Start(element) if element.local_name().as_ref() == "typeInfo" => {
                panic!("public typeInfo isolation expects the fixture's self-closing elements");
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut output = String::new();
    let mut copied = 0;
    for (start, end) in ranges {
        output.push_str(&xml[copied..start]);
        copied = end;
    }
    output.push_str(&xml[copied..]);
    output
}

#[test]
fn synthetic_hwpx_metadata_edit_verifies_in_hwp() {
    for table in [false, true] {
        let dir = workspace(if table { "table" } else { "body" });
        let mut source = hwp_convert::from_markdown(if table {
            "Baseline input.\n\n| Field | Value |\n| --- | --- |\n| alpha | 1 |\n"
        } else {
            "Baseline input.\n"
        });
        // CHAR_SHAPE underline bits are already supported by the writer (§4.2.6).
        source.header.char_shapes[0].attr |= (1 << 2) | (2 << 4);
        source.header.char_shapes[0].underline_shape = 3;
        source.header.tab_stops = (0..3)
            .map(|attr| hwp_model::TabDef {
                attr,
                items: Vec::new(),
            })
            .collect();
        let mut levels = hwp5::write::synthesized_numbering_levels();
        for (level, format) in levels
            .iter_mut()
            .zip(hwp5::write::DEFAULT_NUMBERING_FORMATS)
        {
            level.fmt = format;
        }
        // Unused extension levels exercise emit_doc_info's explicit seven-slot fallback policy.
        for (fmt, template) in [
            (hwp_model::NumFmt::CircledHangulSyllable, "^8"),
            (hwp_model::NumFmt::HangulJamo, ""),
            (hwp_model::NumFmt::RomanLower, ""),
        ] {
            levels.push(hwp_model::NumLevel {
                start: 1,
                fmt,
                template: template.into(),
            });
        }
        source.header.numbering_levels = vec![levels];
        let input = dir.join("source.hwpx");
        hwpx::write_document(&source, &input).unwrap();
        let parsed = hwpx::read_document(&input).unwrap().document;
        assert_eq!(
            parsed.header.tab_stops.len(),
            3,
            "source retains all default tab slots"
        );
        assert_eq!(
            parsed.header.numbering_levels[0].len(),
            10,
            "source retains unused numbering extension slots"
        );
        assert_eq!(parsed.header.char_shapes[0].underline_shape_code(), 2);
        assert!(parsed.hwpx_settings_xml.is_some() && parsed.hwpx_version_xml.is_some());
        let bytes = std::fs::read(&input).unwrap();
        let output = dir.join("edited.hwp");
        let result = metadata_edit(&input, &output);
        assert!(
            result.status.success(),
            "faithful HWPX metadata edit must pass HWP5 verification (table={table}): {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let reread = hwp5::read_document(&output).unwrap().document;
        assert_eq!(reread.metadata.title.as_deref(), Some("t"));
        assert!(reread.plain_text().contains("Baseline input."));
        assert_eq!(reread.header.char_shapes[0].underline_shape_code(), 2);
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
    }
}

#[test]
fn composed_hwpx_text_edit_verifies_in_hwp() {
    let dir = workspace("compose");
    let spec = dir.join("source.json");
    std::fs::write(
        &spec,
        serde_json::to_vec(&serde_json::json!({
            "version":"1.0",
            "sections":[{"blocks":[{"type":"paragraph","runs":[
                {"type":"text","text":"Baseline input."}
            ]}]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let input = dir.join("composed.hwpx");
    let composed = command()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        composed.status.success(),
        "compose setup: {}",
        String::from_utf8_lossy(&composed.stderr)
    );
    let output = dir.join("edited.hwp");
    let result = command()
        .arg("edit")
        .arg(&input)
        .args([
            "--replace",
            "Baseline input.=>Baseline output.",
            "--verify",
            "-o",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "compose HWPX text edit must pass HWP5 verification: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        hwp5::read_document(&output)
            .unwrap()
            .document
            .plain_text()
            .contains("Baseline output.")
    );
}

#[test]
fn genuine_minimal_unmapped_font_metadata_refuses_with_a_named_cause() {
    let input = fixture("fixtures/hwpx/minimal.hwpx");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workspace("minimal");
    let original = std::fs::read(&input).unwrap();
    let source = hwpx::read_document(&input).unwrap().document;
    assert!(
        source
            .header
            .fonts
            .iter()
            .flatten()
            .any(|font| font.type_info.is_some()),
        "the private minimal fixture must retain the GE-11 premise"
    );
    let output = dir.join("edited.hwp");
    let result = metadata_edit(&input, &output);
    assert!(
        !result.status.success(),
        "unmapped font classification must refuse: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(
        message.contains("font_type_info_unrepresentable") && message.contains(".hwpx"),
        "font loss must name its cause and preservation target: {message}"
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[test]
fn decorative_shape_loss_still_refuses_publication() {
    let original_input = fixture("fixtures/samples/report-tables.hwpx");
    let dir = workspace("decorative-loss");
    let original_bytes = std::fs::read(&original_input).unwrap();
    let output = dir.join("refused.hwp");
    let original_result = metadata_edit(&original_input, &output);
    assert!(
        !original_result.status.success() && !output.exists(),
        "the unmodified public sample must still refuse"
    );
    let input = dir.join("shape-only.hwpx");
    // Isolate GE-1 from the independent GE-11 font loss. Only header typeInfo elements
    // are removed from this public copy; every section and shape payload stays unchanged.
    rewrite_public_entry(
        &original_input,
        &input,
        "Contents/header.xml",
        without_public_font_type_info,
    );
    let original = hwpx::read_document(&original_input).unwrap().document;
    let isolated = hwpx::read_document(&input).unwrap().document;
    assert!(
        isolated
            .header
            .fonts
            .iter()
            .flatten()
            .all(|font| font.type_info.is_none())
    );
    assert!(
        original.sections == isolated.sections,
        "shape-isolation setup must preserve every source section"
    );
    let result = metadata_edit(&input, &output);
    assert!(
        !result.status.success(),
        "GE-1 decorative shape loss must refuse"
    );
    assert_eq!(std::fs::read(&original_input).unwrap(), original_bytes);
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(
        message.contains("generic_controls=10->9")
            || message.contains("opaque_control_unrepresentable"),
        "GE-1 must refuse for lost control content: {message}"
    );
    assert!(
        !output.exists(),
        "lossy decorative-shape output must not publish"
    );
}

#[test]
fn genuinely_unmapped_hwpx_properties_still_refuse_publication() {
    for kind in [
        "tab",
        "number-template",
        "settings",
        "section",
        "hold-anchor",
        "font-info",
    ] {
        let dir = workspace(kind);
        let mut source = hwp_convert::from_markdown(
            "Baseline input.\n\n| Field | Value |\n| --- | --- |\n| alpha | 1 |\n",
        );
        let baseline = dir.join("baseline.hwpx");
        hwpx::write_document(&source, &baseline).unwrap();
        source = hwpx::read_document(&baseline).unwrap().document;
        match kind {
            "font-info" => {
                source.header.fonts[0][0].type_info = Some(" extension=\"retained\"".into());
            }
            "tab" => {
                source.header.tab_stops = vec![hwp_model::TabDef {
                    attr: 0,
                    items: vec![hwp_model::TabItem {
                        pos: 1234,
                        kind: 1,
                        fill: 2,
                    }],
                }]
            }
            "number-template" => {
                source.header.numbering_levels = vec![hwp5::write::synthesized_numbering_levels()];
                source.header.numbering_levels[0][0].template = "custom ^1".into();
            }
            "settings" => {
                source.hwpx_settings_xml =
                    Some(hwpx::DEFAULT_SETTINGS_XML.replace("pos=\"0\"", "pos=\"1\""))
            }
            "section" => {
                let def = source
                    .sections
                    .iter_mut()
                    .flat_map(|s| &mut s.paragraphs)
                    .flat_map(|p| &mut p.controls)
                    .find_map(|control| match control {
                        hwp_model::Control::SectionDef(def) => Some(def),
                        _ => None,
                    })
                    .unwrap();
                let grid = def
                    .secpr_raw_children
                    .iter_mut()
                    .find(|child| child.contains("<hp:grid"))
                    .unwrap();
                assert!(grid.contains("lineGrid=\"0\""));
                *grid = grid.replace("lineGrid=\"0\"", "lineGrid=\"123\"");
            }
            "hold-anchor" => {
                let table = source
                    .sections
                    .iter_mut()
                    .flat_map(|s| &mut s.paragraphs)
                    .flat_map(|p| &mut p.controls)
                    .find_map(|control| match control {
                        hwp_model::Control::Table(table) => Some(table),
                        _ => None,
                    })
                    .unwrap();
                table.placement = Some(hwp_model::GsoPlacement {
                    width: 1200,
                    height: 600,
                    hold_anchor: true,
                    ..Default::default()
                });
            }
            _ => unreachable!(),
        }
        let input = dir.join("source.hwpx");
        hwpx::write_document(&source, &input).unwrap();
        if kind == "hold-anchor" {
            // write_table currently hardcodes the flag to zero. Patch only this generated
            // public input and verify its read-back premise before exercising the HWP5 loss.
            set_public_hold_anchor(&input);
            let read = hwpx::read_document(&input).unwrap().document;
            assert!(read.sections.iter().flat_map(|section| &section.paragraphs)
                .flat_map(|paragraph| &paragraph.controls).any(|control| {
                    matches!(control, hwp_model::Control::Table(table) if table.placement.as_ref().is_some_and(|placement| placement.hold_anchor))
                }), "source must retain hold-anchor before the edit");
        }
        let original = std::fs::read(&input).unwrap();
        let output = dir.join("refused.hwp");
        let result = metadata_edit(&input, &output);
        assert!(!result.status.success(), "genuine {kind} loss must refuse");
        let message = String::from_utf8_lossy(&result.stderr);
        if kind == "font-info" {
            assert!(
                message.contains("font_type_info_unrepresentable") && message.contains(".hwpx"),
                "font loss needs its code and remedy: {message}"
            );
        } else {
            assert!(
                message.contains("의미 불변식"),
                "{kind} must reach semantic verification: {message}"
            );
        }
        assert!(!output.exists(), "{kind} loss must not publish");
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
}

#[test]
fn font_classification_loss_is_counted_without_exposing_payload() {
    let dir = workspace("font-ledger");
    let mut source = hwp_convert::from_markdown("Baseline input.\n");
    for slot in [0, 1] {
        source.header.fonts[slot][0].type_info =
            Some(" extension=\"TYPE_INFO_PAYLOAD_SENTINEL\"".into());
    }
    let input = dir.join("source.hwpx");
    hwpx::write_document(&source, &input).unwrap();
    let original = std::fs::read(&input).unwrap();
    let output = dir.join("converted.hwp");
    let report = dir.join("loss.json");
    for strict in [false, true] {
        if output.exists() {
            std::fs::remove_file(&output).unwrap();
        }
        if report.exists() {
            std::fs::remove_file(&report).unwrap();
        }
        let mut cmd = command();
        cmd.arg("convert")
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .arg("--loss-report")
            .arg(&report);
        if strict {
            cmd.arg("--strict");
        }
        let result = cmd.output().unwrap();
        assert!(
            report.is_file(),
            "font loss evidence must be fresh even on strict refusal"
        );
        let raw = std::fs::read_to_string(&report).unwrap();
        assert!(!raw.contains("TYPE_INFO_PAYLOAD_SENTINEL"));
        let ledger: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let event = ledger["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["code"] == "font_type_info_unrepresentable");
        assert!(
            event.is_some(),
            "the writer must name font classification loss: {ledger}"
        );
        let event = event.unwrap();
        assert_eq!(event["count"], 2);
        assert_eq!(event["resource"], "metadata");
        assert_eq!(event["disposition"], "unrepresentable");
        assert_eq!(
            result.status.success(),
            !strict,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(output.exists(), !strict);
        if strict {
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(
                message.contains("font_type_info_unrepresentable") && message.contains(".hwpx"),
                "{message}"
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
}

#[test]
fn opaque_equation_loss_still_refuses_publication() {
    let input = fixture("fixtures/hwpx/equation-ampersand-groundtruth.hwpx");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workspace("equation-loss");
    let output = dir.join("refused.hwp");
    let result = metadata_edit(&input, &output);
    assert!(!result.status.success(), "opaque equation loss must refuse");
    assert!(String::from_utf8_lossy(&result.stderr).contains("opaque_control_unrepresentable"));
    assert!(!output.exists(), "lossy equation output must not publish");
}

#[test]
fn synthetic_opaque_equation_loss_still_refuses_publication() {
    let dir = workspace("public-equation-loss");
    let spec = dir.join("equation.json");
    std::fs::write(
        &spec,
        serde_json::to_vec(&serde_json::json!({
            "version":"1.0",
            "sections":[{"blocks":[{"type":"paragraph","runs":[
                {"type":"equation","script":"\"&\"","width_mm":20,"height_mm":8}
            ]}]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let input = dir.join("source.hwpx");
    let composed = command()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        composed.status.success(),
        "equation setup: {}",
        String::from_utf8_lossy(&composed.stderr)
    );
    let document = hwpx::read_document(&input).unwrap().document;
    assert!(document.sections.iter().flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls).any(|control| {
            matches!(control, hwp_model::Control::Generic(generic) if generic.equation.as_ref().is_some_and(|equation| equation.script == "\"&\""))
        }), "public source must retain its opaque equation before the edit");
    let original = std::fs::read(&input).unwrap();
    let output = dir.join("refused.hwp");
    let result = metadata_edit(&input, &output);
    assert!(!result.status.success(), "opaque equation must refuse");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("opaque_control_unrepresentable"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
}
