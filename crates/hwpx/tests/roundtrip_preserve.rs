//! hwpx→IR→hwpx 무수정 왕복 무손실 계약 테스트 (Phase 0, epic #90).
//!
//! 합성 HWPX를 코드로 만들어(read → write) 재작성한 뒤, writer가 예전에
//! silently drop하던 세 축이 보존되는지 검증한다:
//! - 미해석 run-level 개체(hp:container·hp:chartex)의 원문 XML
//! - 참조되지 않는/중복 바이트의 BinData/* 엔트리
//! - writer 고정 목록 밖 패키지 엔트리(원본 META-INF/*, DocOptions 등)

use std::io::{Read, Write};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

const PNG_REFERENCED: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 1, 1, 1];
const PNG_ORPHAN: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 2, 2, 2, 2];
const PNG_DUP: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 3, 3, 3, 3];
const PRV_IMAGE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 9, 9, 9, 9];
const CONTAINER_RDF: &[u8] =
    br#"<?xml version="1.0" encoding="UTF-8"?><rdf:RDF>DISTINCTIVE-RDF-MARKER</rdf:RDF>"#;
const CONTAINER_XML: &[u8] =
    br#"<?xml version="1.0" encoding="UTF-8"?><container>DISTINCTIVE-CONTAINER-MARKER</container>"#;
const MANIFEST_XML: &[u8] =
    br#"<?xml version="1.0" encoding="UTF-8"?><manifest>DISTINCTIVE-MANIFEST-MARKER</manifest>"#;
const LAYOUT_XML: &[u8] =
    br#"<?xml version="1.0" encoding="UTF-8"?><layout>DISTINCTIVE-DOCOPTIONS-MARKER</layout>"#;

const HEADER_XML: &str =
    r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##;

/// run-level 미해석 개체 2종(hp:container + 하위 subList 텍스트, hp:chartex)과
/// image1을 참조하는 hp:pic을 담은 본문.
const SECTION_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문</hp:t><hp:container id="77" zOrder="3"><hp:sz width="1000" height="500"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>컨테이너 텍스트</hp:t></hp:run></hp:p></hp:subList></hp:container><hp:chartex version="9"><hp:extData>차트확장</hp:extData></hp:chartex><hp:pic id="5" zOrder="0"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="image1" bright="0" contrast="0"/></hp:pic></hp:run></hp:p></hs:sec>"##;

fn build_fixture(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();

    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();

    // writer가 템플릿으로 대체하던 META-INF 3종 — 원본 바이트 보존 대상.
    zip.start_file("META-INF/container.rdf", deflated).unwrap();
    zip.write_all(CONTAINER_RDF).unwrap();
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(CONTAINER_XML).unwrap();
    zip.start_file("META-INF/manifest.xml", deflated).unwrap();
    zip.write_all(MANIFEST_XML).unwrap();

    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(HEADER_XML.as_bytes()).unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(SECTION_XML.as_bytes()).unwrap();

    // 참조됨 1 + 미참조 1 + 동일 바이트 2.
    zip.start_file("BinData/image1.png", deflated).unwrap();
    zip.write_all(PNG_REFERENCED).unwrap();
    zip.start_file("BinData/orphan.png", deflated).unwrap();
    zip.write_all(PNG_ORPHAN).unwrap();
    zip.start_file("BinData/dupA.png", deflated).unwrap();
    zip.write_all(PNG_DUP).unwrap();
    zip.start_file("BinData/dupB.png", deflated).unwrap();
    zip.write_all(PNG_DUP).unwrap();

    zip.start_file("Preview/PrvText.txt", deflated).unwrap();
    zip.write_all("원본 미리보기 텍스트".as_bytes()).unwrap();
    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(PRV_IMAGE).unwrap();

    // writer 고정 목록 밖 임의 엔트리.
    zip.start_file("DocOptions/Layout.xml", deflated).unwrap();
    zip.write_all(LAYOUT_XML).unwrap();

    zip.finish().unwrap();
}

fn read_entries(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        if entry.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        out.push((entry.name().to_string(), buf));
    }
    out
}

#[test]
fn 무수정_왕복은_개체_바이너리_패키지엔트리를_보존한다() {
    let dir = std::env::temp_dir().join("hwpx-roundtrip-preserve");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_fixture(&src);
    let source_entries = read_entries(&src);

    let read = hwpx::read_document(&src).unwrap();
    let report = hwpx::write_document_with_report(&read.document, &out).unwrap();

    // 1. 무손실 계약: typed preservation 이벤트가 없어야 한다.
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    let out_entries = read_entries(&out);
    let out_map: std::collections::BTreeMap<&str, &[u8]> = out_entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();

    // 2. 엔트리 집합 상위(superset): 원본의 모든 엔트리 이름이 남아 있어야 한다.
    for (name, _) in &source_entries {
        assert!(out_map.contains_key(name.as_str()), "엔트리 유실: {name}");
    }

    // 3. 바이트 보존 대상: BinData 전부 + 원본 META-INF + DocOptions + Preview 이미지.
    for name in [
        "BinData/image1.png",
        "BinData/orphan.png",
        "BinData/dupA.png",
        "BinData/dupB.png",
        "META-INF/container.rdf",
        "META-INF/container.xml",
        "META-INF/manifest.xml",
        "DocOptions/Layout.xml",
        "Preview/PrvImage.png",
    ] {
        let source_bytes = source_entries
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("원본에 {name} 없음"))
            .1
            .as_slice();
        assert_eq!(
            out_map.get(name).copied(),
            Some(source_bytes),
            "{name} 바이트가 원본과 다르다"
        );
    }

    // 4. 미해석 개체 원문 보존: container + subList 텍스트 + chartex.
    let section = String::from_utf8(
        out_entries
            .iter()
            .find(|(n, _)| n == "Contents/section0.xml")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap();
    assert!(
        section.contains(r#"<hp:container id="77" zOrder="3">"#),
        "container 원문 유실: {section}"
    );
    assert!(
        section.contains("컨테이너 텍스트"),
        "container subList 텍스트 유실: {section}"
    );
    assert!(
        section.contains(r#"<hp:chartex version="9">"#),
        "chartex 원문 유실: {section}"
    );
}

/// 원문 캡처된 개체는 write_section_with_report에서 그대로 방출되고,
/// 원문이 없는 불투명 개체는 기존처럼 OpaqueControlUnrepresentable 손실 이벤트를 낸다.
#[test]
fn 원문_보유_개체는_방출하고_없으면_손실이벤트를_낸다() {
    fn section_with(generic: hwp_model::GenericControl) -> hwp_model::Section {
        let mut para = hwp_model::Paragraph::default();
        para.chars.push(hwp_model::HwpChar::Text('가'));
        para.chars.push(hwp_model::HwpChar::ExtCtrl {
            code: 11,
            ctrl_id: generic.ctrl_id,
            payload: Vec::new(),
            ctrl_index: Some(0),
        });
        para.controls.push(hwp_model::Control::Generic(generic));
        hwp_model::Section {
            paragraphs: vec![para],
            extras: Vec::new(),
        }
    }

    fn blank_generic(ctrl_id: [u8; 4]) -> hwp_model::GenericControl {
        hwp_model::GenericControl {
            ctrl_id,
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
        }
    }

    let doc = hwp_model::Document::default();

    // (a) 원문 보유 개체 → 원문 그대로 방출, 손실 없음.
    let raw = r#"<hp:container id="9"><hp:subList><hp:p><hp:run><hp:t>상자</hp:t></hp:run></hp:p></hp:subList></hp:container>"#;
    let mut generic = blank_generic(*b"cont");
    generic.hwpx_raw_xml = Some(raw.to_string());
    let mut report = hwp_model::WriteReport::new();
    let mut bins = hwpx::write::section::BinCollector::default();
    let xml = hwpx::write::section::write_section_with_report(
        &doc,
        &section_with(generic),
        false,
        &mut bins,
        &mut report,
    );
    assert!(xml.contains(raw), "원문 미방출: {xml}");
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    // (b) 원문 없음 + 표현 수단 없음 → 기존 OpaqueControlUnrepresentable 유지.
    let generic = blank_generic(*b"zzzz");
    let mut report = hwp_model::WriteReport::new();
    let mut bins = hwpx::write::section::BinCollector::default();
    let _ = hwpx::write::section::write_section_with_report(
        &doc,
        &section_with(generic),
        false,
        &mut bins,
        &mut report,
    );
    assert_eq!(report.preservation.events.len(), 1);
    assert_eq!(
        report.preservation.events[0].code,
        hwp_model::PreservationCode::OpaqueControlUnrepresentable
    );
}
