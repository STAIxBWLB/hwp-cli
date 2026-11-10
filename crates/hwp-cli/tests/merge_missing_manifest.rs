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
fn replace_manifest_href(path: &Path, old_href: &str, new_href: &str) {
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
        if name == old_href {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "Contents/content.hpf" {
            let xml = String::from_utf8(data).unwrap();
            assert!(
                xml.contains(old_href),
                "authored source must expose the manifest target being replaced"
            );
            data = xml.replace(old_href, new_href).into_bytes();
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
    replace_manifest_href(&dir.join("b.hwpx"), "BinData/p2.png", "BinData/missing.png");
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
        "missing image must report BinaryAssetRemoved rather than reuse another input: {report:?}"
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

// Writer-survival positive controls; the parsed HWPX href differs from the bare core-IR case.
fn native_canonical_remint_case(extension: &str) {
    let dir = temp_path::TempPath::new("hwp417-merge-missing", &format!("canonical-{extension}"));
    std::fs::create_dir_all(&*dir).unwrap();
    hwpx::write_document(&input("BinData/a-original.png", false), &dir.join("a.hwpx")).unwrap();
    // The HWPX reader retains only BinData/ manifest hrefs, so the real source uses this
    // package spelling rather than the bare BIN0001.png accepted by the core IR controls.
    replace_manifest_href(
        &dir.join("a.hwpx"),
        "BinData/a-original.png",
        "BinData/BIN0001.png",
    );
    let a = hwpx::read_document(&dir.join("a.hwpx")).unwrap().document;
    assert_eq!(
        a.hwpx_bin_manifest,
        vec![("logo".into(), "BinData/BIN0001.png".into())]
    );
    assert!(
        a.bin_streams.is_empty(),
        "primary source must have no orphan bytes to mask loss evidence"
    );
    assert!(a.resolve_bin(&picture(&a).bin_ref).is_none());
    hwpx::write_document(
        &input("BinData/b-original.png", false),
        &dir.join("b-source.hwpx"),
    )
    .unwrap();
    let convert = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .current_dir(&*dir)
        .args(["convert", "b-source.hwpx", "-o", "b.hwp"])
        .output()
        .unwrap();
    assert!(
        convert.status.success(),
        "native numeric-source conversion must succeed: {}",
        String::from_utf8_lossy(&convert.stderr)
    );
    let b = hwp5::read_document(&dir.join("b.hwp")).unwrap().document;
    assert_eq!(b.meta.source_format, "hwp5");
    assert!(
        matches!(picture(&b).bin_ref, BinRef::Id(hwp_model::BinDataId(1))),
        "actual converted fixture must have numeric picture ID1"
    );
    assert_eq!(b.header.bin_data[0].storage_id, Some(1));
    assert_eq!(b.header.bin_data[0].extension.as_deref(), Some("png"));
    assert_eq!(
        b.resolve_bin(&picture(&b).bin_ref),
        Some(authored_png().as_slice())
    );
    let a_bytes = std::fs::read(dir.join("a.hwpx")).unwrap();
    let b_bytes = std::fs::read(dir.join("b.hwp")).unwrap();
    let output = format!("canonical-merged.{extension}");
    let run = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .current_dir(&*dir)
        .args([
            "merge",
            "a.hwpx",
            "b.hwp",
            "-o",
            &output,
            "--loss-report",
            "canonical-loss.json",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "native canonical merge must publish with missing-primary loss evidence: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_binary_loss(&dir.join("canonical-loss.json"));
    let merged = if extension == "hwp" {
        hwp5::read_document(&dir.join(&output)).unwrap().document
    } else {
        hwpx::read_document(&dir.join(&output)).unwrap().document
    };
    assert_eq!(merged.sections.len(), 2);
    for picture in merged.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .filter_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
    {
        assert!(
            merged.resolve_bin(&picture.bin_ref).is_none(),
            "retained primary missing picture must never resolve to B; writer-reported removal is also allowed"
        );
    }
    let b_picture = merged.sections[1]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("B's valid native picture must survive publication");
    assert_eq!(
        merged.resolve_bin(&b_picture.bin_ref),
        Some(authored_png().as_slice()),
        "B's published picture must retain exact source PNG bytes"
    );
    assert!(std::fs::read(dir.join("a.hwpx")).unwrap() == a_bytes);
    assert!(std::fs::read(dir.join("b.hwp")).unwrap() == b_bytes);
}
#[test]
fn hwp_native_canonical_remint_keeps_missing_primary_and_valid_numeric_payload_distinct() {
    native_canonical_remint_case("hwp");
}
#[test]
fn hwpx_native_canonical_remint_keeps_missing_primary_and_valid_numeric_payload_distinct() {
    native_canonical_remint_case("hwpx");
}

// #463: the primary names a picture id the package does not list, so nothing in the primary
// answers it. A later input carries `BinData/logo.png`, and the resolver's filename-stem rule
// would hand its bytes to the primary's picture unless the graft renames that stream.
fn drop_manifest_item(path: &Path, id: &str, href: &str) {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let mut dropped_payload = false;
    let mut dropped_item = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let method = entry.compression();
        // The payload goes too: an orphan stream would keep the id resolvable by name.
        if name == href {
            dropped_payload = true;
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "Contents/content.hpf" {
            let xml = String::from_utf8(data).unwrap();
            let listed = format!("id=\"{id}\"");
            let kept: Vec<&str> = xml
                .lines()
                .filter(|line| {
                    let item = line.contains("<opf:item ") && line.contains(&listed);
                    if item {
                        dropped_item = true;
                    }
                    !item
                })
                .collect();
            data = format!("{}\n", kept.join("\n")).into_bytes();
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
    assert!(
        dropped_item && dropped_payload,
        "authored source must list the manifest item and payload being removed"
    );
}

/// Writes A as a package whose picture names the unlisted id `logo` with no manifest entry and
/// no payload, and B as a package carrying `BinData/logo.png`. The graft tier is covered by
/// `document_merge`'s unit test; what this case pins is the published output of both formats.
fn unlisted_primary_case(extension: &str) {
    let dir = temp_path::TempPath::new("hwp463-merge-unlisted", &format!("unlisted-{extension}"));
    std::fs::create_dir_all(&*dir).unwrap();
    hwpx::write_document(&input("BinData/p1.png", false), &dir.join("a.hwpx")).unwrap();
    drop_manifest_item(&dir.join("a.hwpx"), "logo", "BinData/p1.png");
    hwpx::write_document(&input("BinData/logo.png", false), &dir.join("b.hwpx")).unwrap();

    let a = hwpx::read_document(&dir.join("a.hwpx")).unwrap().document;
    assert!(
        a.hwpx_bin_manifest.is_empty() && a.bin_streams.is_empty(),
        "the primary must carry nothing that answers its own picture id"
    );
    assert!(
        a.resolve_bin(&picture(&a).bin_ref).is_none(),
        "the primary's unlisted id must be unresolved before the merge"
    );
    let b = hwpx::read_document(&dir.join("b.hwpx")).unwrap().document;
    assert_eq!(
        b.resolve_bin(&picture(&b).bin_ref),
        Some(authored_png().as_slice()),
        "the later input must resolve the id against its own payload"
    );

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
            "unlisted-loss.json",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "permissive merge must publish with loss evidence: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_binary_loss(&dir.join("unlisted-loss.json"));

    let merged = if extension == "hwp" {
        hwp5::read_document(&dir.join(&output)).unwrap().document
    } else {
        hwpx::read_document(&dir.join(&output)).unwrap().document
    };
    assert_eq!(merged.sections.len(), 2);
    for picture in merged.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .filter_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
    {
        assert!(
            merged.resolve_bin(&picture.bin_ref).is_none(),
            "the primary's unresolved picture must never resolve to the later input's bytes"
        );
    }
    let b_picture = merged.sections[1]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("the later input's valid picture must survive publication");
    assert_eq!(
        merged.resolve_bin(&b_picture.bin_ref),
        Some(authored_png().as_slice()),
        "the later input's picture must keep its own bytes under its renamed stream"
    );
}

#[test]
fn hwp_merge_unlisted_primary_id_reports_binary_asset_removed() {
    unlisted_primary_case("hwp");
}

#[test]
fn hwpx_merge_unlisted_primary_id_reports_binary_asset_removed() {
    unlisted_primary_case("hwpx");
}
