//! #452 regression: an HWPX image fill (`hc:imgBrush`) on a border fill and on a semantic shape
//! is not modeled, so it must never be dropped silently — the HWPX writer ledgers
//! `image_fill_unrepresentable` and a same-format `hwp convert` refuses to publish the result.
//!
//! No genuine Hancom-saved HWPX sample carrying `imgBrush` exists locally, so the source is
//! synthetic: the element nesting follows `docs/design/11-hwpx-structure-map.md`, and the
//! assertion under test is that a read image fill is never dropped silently, not that this XML is
//! byte-faithful to a Hancom export.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-cli-image-fill-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Header carrying one border fill whose only fill is an image brush.
const HEADER_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hh:refList><hh:borderFills itemCnt="1"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:leftBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:rightBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:topBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:bottomBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/><hc:fillBrush><hc:imgBrush mode="TILE"><hc:img binaryItemIDRef="image1" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/></hc:imgBrush></hc:fillBrush></hh:borderFill></hh:borderFills></hh:refList></hh:head>"##;

/// Body carrying one rectangle whose only fill is an image brush. Its geometry (pos/sz) must
/// survive the conversion: dropping the fill may not cost the shape.
const SECTION_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="0" paraPrIDRef="0" styleIDRef="0"><hp:run charPrIDRef="0"><hp:rect id="1" ratio="0"><hp:pos treatAsChar="0" horzOffset="1000" vertOffset="2000"/><hp:sz width="3000" height="4000"/><hc:fillBrush><hc:imgBrush mode="TOTAL"><hc:img binaryItemIDRef="image1" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/></hc:imgBrush></hc:fillBrush></hp:rect></hp:run></hp:p></hs:sec>"##;

fn build_image_fill_hwpx(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(HEADER_XML.as_bytes()).unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(SECTION_XML.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn section_xml_of(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

/// The writer's own ledger: one border fill plus one shape, merged by
/// (code, resource, disposition) into a single event of count 2.
#[test]
fn hwpx_writer_ledgers_the_image_fill_it_cannot_emit() {
    let input = temp("writer-in.hwpx");
    build_image_fill_hwpx(&input);
    let document = hwpx::read_document(&input).unwrap().document;
    let output = temp("writer-out.hwpx");
    let report = hwpx::write::write_document_with_report(&document, &output).unwrap();

    let report_json = serde_json::to_value(&report.preservation).unwrap();
    let event = report_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["code"] == "image_fill_unrepresentable")
        .unwrap_or_else(|| panic!("no image_fill_unrepresentable event: {report_json}"));
    assert_eq!(event["count"], 2, "{report_json}");
    assert_eq!(event["resource"], "control");
    assert_eq!(event["disposition"], "unrepresentable");

    // The shape itself survives: only its fill is unrepresentable.
    let xml = section_xml_of(&output);
    assert!(xml.contains("<hp:rect"), "shape disappeared: {xml}");
    assert!(xml.contains(r#"width="3000""#), "shape size lost: {xml}");
    assert!(
        !xml.contains("imgBrush"),
        "an unmodeled fill must not be re-emitted: {xml}"
    );

    for path in [&input, &output] {
        std::fs::remove_file(path).ok();
    }
}

/// The CLI gate: a same-format conversion records the typed loss even when it refuses, and
/// `--strict` refuses the same input instead of publishing a silently lossy document.
#[test]
fn hwpx_image_fill_conversion_records_the_loss_and_refuses() {
    let input = temp("in.hwpx");
    build_image_fill_hwpx(&input);

    let output = temp("out.hwpx");
    let report_path = temp("loss.json");
    let attempt = hwp()
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--loss-report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        !attempt.status.success(),
        "same-format convert must not publish an image-fill loss: {}",
        String::from_utf8_lossy(&attempt.stderr)
    );
    let stderr = String::from_utf8_lossy(&attempt.stderr);
    assert!(
        stderr.contains("image_fill_unrepresentable"),
        "the refusal must name the typed loss: {stderr}"
    );
    assert!(
        !output.exists(),
        "no output may be published for a refused conversion"
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    let event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["code"] == "image_fill_unrepresentable")
        .unwrap_or_else(|| panic!("no image_fill_unrepresentable ledger event: {report}"));
    assert_eq!(event["count"], 2, "{report}");
    assert_eq!(event["resource"], "control");
    assert_eq!(event["disposition"], "unrepresentable");

    let strict_output = temp("strict.hwpx");
    let strict = hwp()
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&strict_output)
        .arg("--strict")
        .output()
        .unwrap();
    assert!(
        !strict.status.success(),
        "--strict must refuse a conversion whose image fill cannot be preserved"
    );
    assert!(!strict_output.exists());

    for path in [&input, &report_path] {
        std::fs::remove_file(path).ok();
    }
}
