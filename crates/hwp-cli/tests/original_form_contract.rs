//! Generated original-form evidence and deliberate-corruption self-checks (#416).
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Control, Document, HwpChar, Paragraph};
use serde_json::Value;

#[path = "common/original_form_contract.rs"]
mod contract;
#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;
use contract::{ExpectedCell, FormProjection};

struct Fixture {
    dir: temp_path::TempPath,
    doc: Document,
    expected: Value,
}
fn run(dir: &Path, args: &[&str]) -> Vec<u8> {
    let result = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn compose(dir: &Path) {
    let spec = dir.join("original-form.spec.json").canonicalize().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("compose")
        .arg(spec)
        .args(["-o", "source.hwpx"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture compose failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn generate(dir: &Path) -> bool {
    let python = std::env::var_os("PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/generate_original_form_fixture.py");
    let result = match Command::new(python).arg(generator).arg(dir).output() {
        Ok(result) => result,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fixture_skip::resource_unavailable(
                "Python for original-form fixture generator",
                "install Python 3 or set PYTHON to its executable",
            );
            return false;
        }
        Err(error) => panic!(
            "fixture generator could not start: {error}; check PYTHON and executable permissions"
        ),
    };
    assert!(
        result.status.success(),
        "fixture generator failed; check PYTHON and the generator diagnostics: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    true
}
impl Fixture {
    fn new(name: &str) -> Option<Self> {
        let dir = temp_path::TempPath::new("hwp416", name);
        std::fs::create_dir_all(&*dir).unwrap();
        if !generate(&dir) {
            return None;
        }
        compose(&dir);
        let expected: Value =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        assert!(
            contract::digest(&std::fs::read(dir.join("original-form.spec.json")).unwrap())
                == expected["spec"]["sha256"].as_str().unwrap(),
            "fixture specification identity differs from manifest"
        );
        let doc = hwpx::read_document(&dir.join("source.hwpx"))
            .unwrap()
            .document;
        Some(Self { dir, doc, expected })
    }
    fn forms(&self) -> Vec<(usize, usize)> {
        self.expected["form_starts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|form| {
                (
                    form["section"].as_u64().unwrap() as usize,
                    form["paragraph"].as_u64().unwrap() as usize,
                )
            })
            .collect()
    }
}
fn table_mut(doc: &mut Document, index: usize) -> &mut hwp_model::Table {
    doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|p| &mut p.controls)
        .filter_map(|c| {
            if let Control::Table(t) = c {
                Some(t)
            } else {
                None
            }
        })
        .nth(index)
        .unwrap()
}
fn cell_mut(doc: &mut Document, table: usize, row: u16, col: u16) -> &mut hwp_model::Cell {
    table_mut(doc, table)
        .cells
        .iter_mut()
        .find(|cell| cell.row == row && cell.col == col)
        .unwrap()
}
fn assert_manifest(doc: &Document, manifest: &Value) -> FormProjection {
    let projected = contract::project_form(doc);
    contract::compare_form_contract(&projected, &projected).unwrap();
    assert_eq!(
        projected.tables.len(),
        manifest["expected"]["table_count"].as_u64().unwrap() as usize
    );
    let mut expected_cells = Vec::new();
    for table in manifest["tables"].as_array().unwrap() {
        let index = table["table_index"].as_u64().unwrap() as usize;
        let actual = &projected.tables[index];
        assert!(
            actual.key.starts_with(&format!(
                "s{}/p{}/",
                table["section"].as_u64().unwrap(),
                table["paragraph"].as_u64().unwrap()
            )),
            "table occurrence differs from authored source address"
        );
        assert_eq!(u64::from(actual.rows), table["rows"].as_u64().unwrap());
        assert_eq!(u64::from(actual.cols), table["cols"].as_u64().unwrap());
        let cells = table["cells"].as_array().unwrap();
        assert_eq!(actual.cells.len(), cells.len());
        for cell in cells {
            let row = cell["row"].as_u64().unwrap() as u16;
            let col = cell["col"].as_u64().unwrap() as u16;
            let actual = actual
                .cells
                .iter()
                .find(|c| c.row == row && c.col == col)
                .unwrap();
            assert_eq!(
                u64::from(actual.row_span),
                cell["row_span"].as_u64().unwrap()
            );
            assert_eq!(
                u64::from(actual.col_span),
                cell["col_span"].as_u64().unwrap()
            );
            // Authored millimetres are quantized by the existing compiler to HWPUNIT.
            assert!(
                (hwp_model::HwpUnit(actual.width).to_mm() - cell["width_mm"].as_f64().unwrap())
                    .abs()
                    < 0.004,
                "authored column width differs beyond one HWPUNIT"
            );
            expected_cells.push(ExpectedCell {
                table: index,
                row,
                col,
                paragraphs: cell["paragraph_texts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().into())
                    .collect(),
            });
        }
    }
    contract::compare_expected_cells(&expected_cells, &projected).unwrap();
    for form in manifest["form_starts"].as_array().unwrap() {
        let paragraph = form["paragraph"].as_u64().unwrap() as usize;
        assert!(
            contract::direct_text(&doc.sections[0].paragraphs[paragraph])
                == form["marker"].as_str().unwrap(),
            "form-start marker address differs"
        );
        let before = form["break_paragraph"].as_u64().unwrap() as usize;
        assert_ne!(
            doc.sections[0].paragraphs[before].header.break_type & 4,
            0,
            "declared form break is missing"
        );
    }
    let mut resources: Vec<_> = manifest["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|image| image["sha256"].as_str().unwrap().to_string())
        .collect();
    resources.sort();
    assert!(
        projected.resource_hashes == resources,
        "resource inventory differs from authored image payloads"
    );
    assert_eq!(projected.images.len(), 2);
    for image in manifest["images"].as_array().unwrap() {
        let resolved: Vec<_> = projected
            .images
            .iter()
            .filter(|actual| actual.payload_sha256.as_deref() == image["sha256"].as_str())
            .collect();
        assert_eq!(
            resolved.len(),
            1,
            "each distinct authored image must resolve once"
        );
        if image["role"] == "retained" {
            assert!(
                resolved[0].key.contains("/r3c1/"),
                "in-form image left its cell"
            );
        } else {
            assert!(
                resolved[0].key.starts_with("s0/p2/"),
                "outside image left its source paragraph"
            );
        }
    }
    projected
}
fn observe(
    doc: &Document,
    selected: &[(usize, usize)],
) -> (contract::AnchorPositions, contract::FontIdentity) {
    let mut store = hwp_render::FontStore::new();
    if let Some(font_dir) = std::env::var_os("HWP_FONT_DIR") {
        store.load_dir(Path::new(&font_dir));
    }
    let mut issues = hwp_render::RenderIssueAccumulator::new();
    let (_, map) = hwp_render::layout::layout_document_with_segments(doc, &mut store, &mut issues);
    (
        contract::anchor_positions(doc, &map, selected).unwrap(),
        contract::FontIdentity::from_store(&store),
    )
}

#[test]
fn generated_forms_match_manifest_and_repeat_in_both_formats() {
    let Some(fixture) = Fixture::new("repeat") else {
        return;
    };
    let twin = fixture.dir.join("twin");
    assert!(generate(&twin));
    for file in [
        "original-form.spec.json",
        "inside.png",
        "outside.png",
        "expected.json",
    ] {
        assert!(
            std::fs::read(fixture.dir.join(file)).unwrap()
                == std::fs::read(twin.join(file)).unwrap(),
            "generator bytes are not deterministic"
        );
    }
    compose(&twin);
    let twin_doc = hwpx::read_document(&twin.join("source.hwpx"))
        .unwrap()
        .document;
    let source = assert_manifest(&fixture.doc, &fixture.expected);
    let other = assert_manifest(&twin_doc, &fixture.expected);
    contract::compare_form_contract(&source, &other).unwrap();
    let (before, fonts) = observe(&fixture.doc, &fixture.forms());
    let (after, other_fonts) = observe(&twin_doc, &fixture.forms());
    contract::compare_anchor_positions(&before, &after).unwrap();
    contract::compare_font_identity(&fonts, &other_fonts).unwrap();
    // Native and HWPX each have their own semantic baseline; this does not waive
    // format-materialization differences or assert universal cross-format styles.
    run(
        &fixture.dir,
        &[
            "convert",
            "source.hwpx",
            "-o",
            "source.hwp",
            "--strict",
            "--loss-report",
            "conversion.json",
        ],
    );
    let native = hwp5::read_document(&fixture.dir.join("source.hwp"))
        .unwrap()
        .document;
    let native_projection = assert_manifest(&native, &fixture.expected);
    run(
        &fixture.dir,
        &["convert", "source.hwp", "-o", "identity.hwp"],
    );
    assert!(
        std::fs::read(fixture.dir.join("source.hwp")).unwrap()
            == std::fs::read(fixture.dir.join("identity.hwp")).unwrap(),
        "unchanged native conversion must remain byte-identical"
    );
    let identity = hwp5::read_document(&fixture.dir.join("identity.hwp"))
        .unwrap()
        .document;
    contract::compare_form_contract(&native_projection, &contract::project_form(&identity))
        .unwrap();
    let (native_anchors, native_fonts) = observe(&native, &fixture.forms());
    let (identity_anchors, identity_fonts) = observe(&identity, &fixture.forms());
    contract::compare_anchor_positions(&native_anchors, &identity_anchors).unwrap();
    contract::compare_font_identity(&native_fonts, &identity_fonts).unwrap();
    let ledger: Value =
        serde_json::from_slice(&std::fs::read(fixture.dir.join("conversion.json")).unwrap())
            .unwrap();
    assert!(
        ledger["events"].as_array().unwrap().is_empty(),
        "synthetic conversion reports a preservation event"
    );
    std::fs::write(fixture.dir.join("font-observations.json"),serde_json::to_vec_pretty(&serde_json::json!({"hwpx":fonts,"hwp":native_fonts,"policy":"observed, not externally pinned","hancom":"unverified"})).unwrap()).unwrap();
}

#[test]
fn contract_rejects_changed_values_grid_styles_resources_and_unknown_references() {
    let Some(fixture) = Fixture::new("negative") else {
        return;
    };
    let baseline = assert_manifest(&fixture.doc, &fixture.expected);
    let mut broken = fixture.doc.clone();
    cell_mut(&mut broken, 0, 1, 1).paragraphs[0]
        .chars
        .insert(0, HwpChar::Text('X'));
    let error =
        contract::compare_form_contract(&baseline, &contract::project_form(&broken)).unwrap_err();
    assert_eq!(error.layer, "cell_values");
    assert!(error.id.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!error.to_string().contains("Synthetic Methods"));
    assert!(!error.to_string().contains("s0/p"));
    let mut controlled = fixture.doc.clone();
    cell_mut(&mut controlled, 0, 1, 1).paragraphs[0]
        .chars
        .insert(1, HwpChar::CharCtrl(hwp_model::ctrl_char::NB_SPACE));
    let controlled = contract::project_form(&controlled);
    let text = ExpectedCell {
        table: 0,
        row: 1,
        col: 1,
        paragraphs: vec!["Synthetic Methods".into()],
    };
    assert_eq!(
        contract::compare_expected_cells(std::slice::from_ref(&text), &controlled)
            .unwrap_err()
            .layer,
        "cell_values"
    );
    assert!(contract::expect_cell_values(&controlled, &[text]).is_err());
    let mut broken = fixture.doc.clone();
    cell_mut(&mut broken, 0, 0, 0).col_span -= 1;
    assert_eq!(
        contract::compare_form_contract(&baseline, &contract::project_form(&broken))
            .unwrap_err()
            .layer,
        "grid"
    );
    let mut broken = fixture.doc.clone();
    cell_mut(&mut broken, 0, 1, 1).width.0 += 1;
    assert_eq!(
        contract::compare_form_contract(&baseline, &contract::project_form(&broken))
            .unwrap_err()
            .layer,
        "grid"
    );
    let mut broken = fixture.doc.clone();
    let shape = cell_mut(&mut broken, 0, 1, 1).paragraphs[0].char_shape_runs[0].1;
    broken.header.char_shapes[shape.0 as usize].text_color ^= 0x0011_0000;
    assert_eq!(
        contract::compare_form_contract(&baseline, &contract::project_form(&broken))
            .unwrap_err()
            .layer,
        "style"
    );
    let mut diagonal_source = fixture.doc.clone();
    let mut fill = hwp_model::BorderFill::default();
    fill.diagonal.line_type = 1;
    fill.attr = 1 << 2; // The renderer's slash direction.
    diagonal_source.header.border_fills.push(fill);
    let border = hwp_model::BorderFillId(diagonal_source.header.border_fills.len() as u16);
    cell_mut(&mut diagonal_source, 0, 1, 1).border_fill = border;
    let diagonal_baseline = contract::project_form(&diagonal_source);
    diagonal_source.header.border_fills.last_mut().unwrap().attr = 1 << 5; // Backslash.
    assert_eq!(
        contract::compare_form_contract(
            &diagonal_baseline,
            &contract::project_form(&diagonal_source)
        )
        .unwrap_err()
        .layer,
        "style"
    );
    let mut broken = fixture.doc.clone();
    broken.bin_streams[0].data[0] ^= 1;
    assert_eq!(
        contract::compare_form_contract(&baseline, &contract::project_form(&broken))
            .unwrap_err()
            .layer,
        "resources"
    );
    let mut broken = fixture.doc.clone();
    cell_mut(&mut broken, 0, 1, 1).paragraphs[0].para_shape = hwp_model::ParaShapeId(u16::MAX);
    let broken = contract::project_form(&broken);
    assert_eq!(
        contract::compare_form_contract(&broken, &broken)
            .unwrap_err()
            .layer,
        "references"
    );
}

#[test]
fn explicit_value_budget_keeps_non_targets_and_palette_ids_are_not_styles() {
    let Some(fixture) = Fixture::new("budget") else {
        return;
    };
    let baseline = assert_manifest(&fixture.doc, &fixture.expected);
    let change = ExpectedCell {
        table: 0,
        row: 1,
        col: 1,
        paragraphs: vec!["Reviewed course value".into()],
    };
    let expected = contract::expect_cell_values(&baseline, std::slice::from_ref(&change)).unwrap();
    let mut actual = fixture.doc.clone();
    cell_mut(&mut actual, 0, 1, 1).paragraphs[0].chars =
        change.paragraphs[0].chars().map(HwpChar::Text).collect();
    contract::compare_form_contract(&expected, &contract::project_form(&actual)).unwrap();
    cell_mut(&mut actual, 0, 1, 0).paragraphs[0]
        .chars
        .push(HwpChar::Text('X'));
    assert_eq!(
        contract::compare_form_contract(&expected, &contract::project_form(&actual))
            .unwrap_err()
            .layer,
        "cell_values"
    );
    assert!(contract::expect_cell_values(&baseline, &[change.clone(), change]).is_err());
    let mut remapped = fixture.doc.clone();
    remapped
        .header
        .char_shapes
        .insert(0, remapped.header.char_shapes[0].clone());
    for style in &mut remapped.header.styles {
        style.char_shape.0 += 1;
    }
    for section in &mut remapped.sections {
        for para in &mut section.paragraphs {
            remap_runs(para);
        }
    }
    contract::compare_form_contract(&baseline, &contract::project_form(&remapped)).unwrap();
    let mut redundant = fixture.doc.clone();
    let para = &mut cell_mut(&mut redundant, 0, 1, 1).paragraphs[0];
    para.char_shape_runs.push((1, para.char_shape_runs[0].1));
    contract::compare_form_contract(&baseline, &contract::project_form(&redundant)).unwrap();
}
fn remap_runs(para: &mut Paragraph) {
    for (_, id) in &mut para.char_shape_runs {
        id.0 += 1;
    }
    for control in &mut para.controls {
        match control {
            Control::Table(table) => {
                for cell in &mut table.cells {
                    for para in &mut cell.paragraphs {
                        remap_runs(para);
                    }
                }
                if let Some(c) = &mut table.caption {
                    for p in &mut c.paragraphs {
                        remap_runs(p);
                    }
                }
            }
            Control::Picture(picture) => {
                if let Some(c) = &mut picture.caption {
                    for p in &mut c.paragraphs {
                        remap_runs(p);
                    }
                }
            }
            Control::Generic(generic) => {
                for list in &mut generic.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_runs(p);
                    }
                }
                if let Some(c) = &mut generic.caption {
                    for p in &mut c.paragraphs {
                        remap_runs(p);
                    }
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

#[test]
fn anchor_and_font_self_checks_reject_moved_starts_and_incomplete_evidence() {
    let Some(fixture) = Fixture::new("anchors") else {
        return;
    };
    let (before, fonts) = observe(&fixture.doc, &fixture.forms());
    assert!(
        !fonts.entries.is_empty(),
        "nonempty authored text must produce font-resolution observations"
    );
    let mut moved = fixture.doc.clone();
    // Add a page boundary before the existing outside image. No font size or
    // absolute page count is assumed; all subsequent form starts move together.
    moved.sections[0].paragraphs[2].header.break_type |= 4;
    let (after, _) = observe(&moved, &fixture.forms());
    assert_eq!(
        contract::compare_anchor_positions(&before, &after)
            .unwrap_err()
            .layer,
        "body_anchors"
    );
    let mut bad_fonts = fonts.clone();
    bad_fonts.entries[0].sha256 = Some("deliberately broken observation".into());
    assert_eq!(
        contract::compare_font_identity(&fonts, &bad_fonts)
            .unwrap_err()
            .layer,
        "fonts"
    );
    let mut store = hwp_render::FontStore::new_isolated();
    let mut issues = hwp_render::RenderIssueAccumulator::new();
    let (_, mut map) =
        hwp_render::layout::layout_document_with_segments(&fixture.doc, &mut store, &mut issues);
    contract::anchor_positions(&fixture.doc, &map, &contract::body_paths(&fixture.doc)).unwrap();
    map.truncated = true;
    assert!(contract::anchor_positions(&fixture.doc, &map, &fixture.forms()).is_err());
    map.truncated = false;
    map.rows.clear();
    assert!(contract::anchor_positions(&fixture.doc, &map, &fixture.forms()).is_err());
    assert!(contract::anchor_positions(&fixture.doc, &map, &[]).is_err());
}
