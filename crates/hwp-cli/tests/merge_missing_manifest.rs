//! Native merge publication must report a missing manifest target, never reuse another input.
use hwp_model::{BinRef, BinStream, Control, Document, HwpChar, HwpUnit, Picture};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;
#[path = "common/temp_path.rs"]
mod temp_path;

fn authored_png() -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([31, 97, 173, 255]),
    ))
    .write_to(&mut output, image::ImageFormat::Png)
    .unwrap();
    output.into_inner()
}
fn input(name: &str, general: bool) -> Document {
    let mut doc = hwp_convert::from_markdown("Owner-authored image paragraph.\n");
    let paragraph = &mut doc.sections[0].paragraphs[0];
    let index = paragraph.controls.len() as u32;
    paragraph.controls.push(Control::Picture(Picture {
        common_data: vec![],
        width: HwpUnit(100),
        height: HwpUnit(100),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: None,
        crop: None,
        flip: 0,
        rotation: None,
        brightness: 0,
        contrast: 0,
        effect_flags: 0,
        effects_raw: vec![],
        caption: None,
        bin_ref: BinRef::ItemRef("logo".into()),
        extras: vec![],
    }));
    paragraph.chars.insert(
        paragraph.chars.len().saturating_sub(1),
        HwpChar::ExtCtrl {
            code: 11,
            ctrl_id: *b"gso ",
            payload: vec![],
            ctrl_index: Some(index),
        },
    );
    doc.bin_streams.push(BinStream {
        name: name.into(),
        data: authored_png(),
    });
    doc.hwpx_bin_manifest = vec![("logo".into(), name.into())];
    if general {
        doc.header.styles.push(hwp_model::Style {
            name: "ExtraStyle".into(),
            english_name: "ExtraStyle".into(),
            ..Default::default()
        });
        doc.header.border_fills.push(Default::default());
    }
    doc
}
fn picture(doc: &Document) -> &Picture {
    doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .unwrap()
}
fn replace_manifest_href(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let mut changed = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let method = entry.compression();
        // Remove the old payload too: an unrelated orphan loss must not satisfy the
        // BinaryAssetRemoved assertion intended for B's missing picture.
        if name == "BinData/p2.png" {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "Contents/content.hpf" {
            let xml = String::from_utf8(data).unwrap();
            assert!(
                xml.contains("BinData/p2.png"),
                "authored source must expose the manifest target being replaced"
            );
            data = xml
                .replace("BinData/p2.png", "BinData/missing.png")
                .into_bytes();
            changed = true;
        }
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();
    assert!(changed);
}
fn sources(dir: &Path, general: bool) {
    hwpx::write_document(&input("BinData/p1.png", false), &dir.join("a.hwpx")).unwrap();
    hwpx::write_document(&input("BinData/p2.png", general), &dir.join("b.hwpx")).unwrap();
    replace_manifest_href(&dir.join("b.hwpx"));
    let a = hwpx::read_document(&dir.join("a.hwpx")).unwrap().document;
    let b = hwpx::read_document(&dir.join("b.hwpx")).unwrap().document;
    assert_eq!(
        a.resolve_bin(&picture(&a).bin_ref),
        Some(authored_png().as_slice())
    );
    assert!(
        b.resolve_bin(&picture(&b).bin_ref).is_none(),
        "source B's manifest target must already be missing"
    );
    assert!(
        b.bin_streams.is_empty(),
        "B must have no orphan payload that could mask the picture loss"
    );
    assert_eq!(
        hwp_convert::document_merge::merge_documents(&[a, b])
            .unwrap()
            .general_path_used,
        general,
        "native source fixtures must exercise the requested tier"
    );
}
fn assert_binary_loss(path: &Path) {
    let report: hwp_model::PreservationReport =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(
        report.events.iter().any(|event| event.code
            == hwp_model::PreservationCode::BinaryAssetRemoved
            && event.count > 0),
        "missing B image must report BinaryAssetRemoved rather than reuse A: {report:?}"
    );
}
fn native_case(extension: &str, general: bool) {
    let dir = temp_path::TempPath::new(
        "hwp417-merge-missing",
        &format!("native-{extension}-{general}"),
    );
    std::fs::create_dir_all(&*dir).unwrap();
    sources(&dir, general);
    let output = format!("merged.{extension}");
    let run = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .current_dir(&*dir)
        .args([
            "merge",
            "a.hwpx",
            "b.hwpx",
            "-o",
            &output,
            "--loss-report",
            "loss.json",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "permissive native merge must publish with loss evidence: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(dir.join(output).is_file());
    assert_binary_loss(&dir.join("loss.json"));
}
fn strict_case(extension: &str, general: bool) {
    let dir = temp_path::TempPath::new(
        "hwp417-merge-missing",
        &format!("strict-{extension}-{general}"),
    );
    std::fs::create_dir_all(&*dir).unwrap();
    sources(&dir, general);
    let a = std::fs::read(dir.join("a.hwpx")).unwrap();
    let b = std::fs::read(dir.join("b.hwpx")).unwrap();
    let output = format!("existing.{extension}");
    let sentinel = b"owner-authored existing destination";
    std::fs::write(dir.join(&output), sentinel).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .current_dir(&*dir)
        .args([
            "merge",
            "a.hwpx",
            "b.hwpx",
            "-o",
            &output,
            "--strict",
            "--loss-report",
            "strict-loss.json",
        ])
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "strict merge must refuse missing binary publication"
    );
    assert_binary_loss(&dir.join("strict-loss.json"));
    assert!(std::fs::read(dir.join(output)).unwrap() == sentinel);
    assert!(std::fs::read(dir.join("a.hwpx")).unwrap() == a);
    assert!(std::fs::read(dir.join("b.hwpx")).unwrap() == b);
}

#[test]
fn fast_hwp_merge_missing_manifest_reports_binary_asset_removed() {
    native_case("hwp", false);
}

#[test]
fn fast_hwp_merge_missing_manifest_strict_refuses_atomically_with_binary_loss() {
    strict_case("hwp", false);
}

#[test]
fn fast_hwpx_merge_missing_manifest_reports_binary_asset_removed() {
    native_case("hwpx", false);
}

#[test]
fn fast_hwpx_merge_missing_manifest_strict_refuses_atomically_with_binary_loss() {
    strict_case("hwpx", false);
}

#[test]
fn general_hwp_merge_missing_manifest_reports_binary_asset_removed() {
    native_case("hwp", true);
}

#[test]
fn general_hwp_merge_missing_manifest_strict_refuses_atomically_with_binary_loss() {
    strict_case("hwp", true);
}

#[test]
fn general_hwpx_merge_missing_manifest_reports_binary_asset_removed() {
    native_case("hwpx", true);
}

#[test]
fn general_hwpx_merge_missing_manifest_strict_refuses_atomically_with_binary_loss() {
    strict_case("hwpx", true);
}
