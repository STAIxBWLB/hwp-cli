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

const HEADER_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##;

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
            container_box: None,
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

/// 자식 도형을 gso_shapes/container_box로 파싱하게 된 뒤에도, 원문 캡처가 있는
/// 컨테이너는 writer가 원문 pass-through로 방출해 왕복이 바이트 동일해야 한다.
#[test]
fn 컨테이너_자식도형_파싱후에도_원문_왕복_보존() {
    const CONTAINER_XML: &str = r##"<hp:container id="11" zOrder="1"><hp:sz width="6000" height="2400"/><hp:pos treatAsChar="1" vertOffset="1500" horzOffset="1000"/><hp:rect id="12" ratio="10"><hp:sz width="2000" height="1000"/><hp:pos treatAsChar="0" vertOffset="200" horzOffset="100"/><hp:lineShape color="#FF0000" width="40" style="SOLID"/><hc:fillBrush><hc:winBrush faceColor="#00FF00" hatchColor="#000000" alpha="0"/></hc:fillBrush></hp:rect><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>상자 텍스트</hp:t></hp:run></hp:p></hp:subList></hp:container>"##;
    let section_xml = format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문</hp:t>{CONTAINER_XML}</hp:run></hp:p></hs:sec>"##
    );

    let dir = std::env::temp_dir().join("hwpx-roundtrip-container-shapes");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    {
        let file = std::fs::File::create(&src).unwrap();
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
        zip.write_all(section_xml.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    let read = hwpx::read_document(&src).unwrap();
    // reader가 자식 도형과 컨테이너 상자를 채웠는지 먼저 확인한다(테스트 전제).
    let generic = read.document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Generic(g) if g.container_box.is_some() => Some(g),
            _ => None,
        })
        .expect("container_box가 채워진 컨테이너 컨트롤");
    assert!(!generic.gso_shapes.is_empty(), "자식 도형 파싱 전제");
    assert!(generic.hwpx_raw_xml.is_some(), "원문 캡처 전제");

    let report = hwpx::write_document_with_report(&read.document, &out).unwrap();
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );
    let out_entries = read_entries(&out);
    let out_section = String::from_utf8(
        out_entries
            .iter()
            .find(|(n, _)| n == "Contents/section0.xml")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap();
    assert!(
        out_section.contains(CONTAINER_XML),
        "container 원문이 바이트 동일하게 방출돼야 한다: {out_section}"
    );
}

// ── #90 PR3 후속: 전체 재작성 경로의 binaryItemIDRef 매니페스트 시드 ──────────

const PNG_ONE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 0, 0, 1];
const PNG_TWO: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 2, 0, 0, 2];
const PNG_THREE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 3, 0, 0, 3];
const PNG_FOUR: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 4, 0, 0, 4];

/// OPF manifest id ≠ 파일명 stem·참조 등장 순서 ≠ 파일명 순서인 합성 HWPX.
/// - image2.png를 참조하는 typed pic이 image1.png 참조 pic보다 먼저 등장
/// - 원문 캡처 hp:container 안의 pic이 id "chartA"(stem 불일치)를 참조
/// - 미참조 image3.png (id "unused1", stem 불일치)
fn build_manifest_seed_fixture(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("Contents/content.hpf", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>시드 문서</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/><opf:item id="layout" href="DocOptions/Layout.xml" media-type="application/xml"/><opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/><opf:item id="image2" href="BinData/image2.png" media-type="image/png" isEmbeded="1"/><opf:item id="unused1" href="BinData/image3.png" media-type="image/png" isEmbeded="1"/><opf:item id="chartA" href="BinData/image4.png" media-type="image/png" isEmbeded="1"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("DocOptions/Layout.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><layout>DISTINCTIVE-DOCOPTIONS-MARKER</layout>"#)
        .unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(HEADER_XML.as_bytes()).unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문</hp:t><hp:pic id="5" zOrder="0"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="image2" bright="0" contrast="0"/></hp:pic><hp:pic id="6" zOrder="1"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="image1" bright="0" contrast="0"/></hp:pic><hp:container id="77" zOrder="3"><hp:sz width="1000" height="500"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:pic id="7" zOrder="0"><hp:sz width="50" height="50"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="chartA" bright="0" contrast="0"/></hp:pic></hp:run></hp:p></hp:subList></hp:container></hp:run></hp:p></hs:sec>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("BinData/image1.png", deflated).unwrap();
    zip.write_all(PNG_ONE).unwrap();
    zip.start_file("BinData/image2.png", deflated).unwrap();
    zip.write_all(PNG_TWO).unwrap();
    zip.start_file("BinData/image3.png", deflated).unwrap();
    zip.write_all(PNG_THREE).unwrap();
    zip.start_file("BinData/image4.png", deflated).unwrap();
    zip.write_all(PNG_FOUR).unwrap();
    zip.finish().unwrap();
}

/// content.hpf 매니페스트에서 BinData 항목의 (id → href) 매핑을 뽑는다.
fn manifest_bin_map(xml: &str) -> std::collections::BTreeMap<String, String> {
    hwpx::read::parse_bin_manifest(xml).into_iter().collect()
}

/// 전체 재작성(hwpx→IR→hwpx) 후에도 모든 binaryItemIDRef가 원본과 같은 바이트를
/// 가리켜야 한다 — 원문 캡처 컨테이너 안의 참조·id≠stem 매니페스트 항목까지.
#[test]
fn 전체_재작성은_원본_manifest_id로_binary참조를_보존한다() {
    let dir = std::env::temp_dir().join("hwpx-roundtrip-manifest-seed");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_manifest_seed_fixture(&src);
    let source_entries = read_entries(&src);

    let read = hwpx::read_document(&src).unwrap();
    let report = hwpx::write_document_with_report(&read.document, &out).unwrap();
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    let out_entries = read_entries(&out);
    let bytes_of = |entries: &[(String, Vec<u8>)], name: &str| -> Vec<u8> {
        entries
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("엔트리 없음: {name}"))
            .1
            .clone()
    };
    let src_manifest = manifest_bin_map(
        &String::from_utf8(bytes_of(&source_entries, "Contents/content.hpf")).unwrap(),
    );
    let out_manifest = manifest_bin_map(
        &String::from_utf8(bytes_of(&out_entries, "Contents/content.hpf")).unwrap(),
    );

    // 1. 매니페스트의 BinData 항목 수 보존(미참조 항목 포함).
    assert_eq!(out_manifest.len(), src_manifest.len(), "매니페스트 항목 수");

    // 2. 출력의 모든 binaryItemIDRef(원문 컨테이너 안 포함)가 출력 매니페스트에서
    //    원본과 같은 바이트로 해석돼야 한다.
    let section = String::from_utf8(bytes_of(&out_entries, "Contents/section0.xml")).unwrap();
    let refs: Vec<&str> = section
        .match_indices("binaryItemIDRef=\"")
        .map(|(i, _)| {
            let rest = &section[i + "binaryItemIDRef=\"".len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(refs.len(), 3, "pic 2 + 컨테이너 안 pic 1: {section}");
    for r in refs {
        let out_href = out_manifest
            .get(r)
            .unwrap_or_else(|| panic!("출력 매니페스트에 {r} 없음(댕글링 참조)"));
        let src_href = src_manifest
            .get(r)
            .unwrap_or_else(|| panic!("원본 매니페스트에 {r} 없음"));
        assert_eq!(
            bytes_of(&out_entries, out_href),
            bytes_of(&source_entries, src_href),
            "{r} 참조 바이트가 원본과 다르다"
        );
    }

    // 3. 원본 BinData 엔트리 집합·바이트 보존(미참조 image3.png 포함).
    for (name, bytes) in &source_entries {
        if name.starts_with("BinData/") {
            assert_eq!(&bytes_of(&out_entries, name), bytes, "{name} 바이트 보존");
        }
    }

    // 4. 확장 파트(DocOptions/Layout.xml): 엔트리 바이트 보존 + 재생성된 매니페스트에
    //    원본 항목(id/href/media-type)이 그대로 등재돼야 한다(고아 파트 방지).
    assert_eq!(
        &bytes_of(&out_entries, "DocOptions/Layout.xml"),
        &bytes_of(&source_entries, "DocOptions/Layout.xml"),
        "DocOptions 엔트리 바이트 보존"
    );
    let out_hpf = String::from_utf8(bytes_of(&out_entries, "Contents/content.hpf")).unwrap();
    assert!(
        out_hpf.contains(
            r#"<opf:item id="layout" href="DocOptions/Layout.xml" media-type="application/xml"/>"#
        ),
        "재생성 매니페스트에 확장 파트 항목 유지: {out_hpf}"
    );
}

/// content.hpf에 BinData 항목이 없어(매니페스트 슬롯 빔) 시드되지 않는 문서에서,
/// 이름이 다른 동일 바이트 BinData 엔트리는 바이트 dedup으로 붕괴하지 않고 모두
/// 원본 이름으로 보존돼야 한다. pic 참조는 출력 매니페스트에서 같은 바이트로 해석된다.
#[test]
fn 미시드_경로도_이름_다른_동일바이트_bin_data를_모두_보존한다() {
    let dir = std::env::temp_dir().join("hwpx-roundtrip-unseeded-dup");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");

    {
        let file = std::fs::File::create(&src).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/hwp+zip").unwrap();
        zip.start_file("version.xml", deflated).unwrap();
        zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
            .unwrap();
        // 매니페스트에 BinData 항목이 없다 → hwpx_bin_manifest 슬롯이 비어 미시드 경로.
        zip.start_file("Contents/content.hpf", deflated).unwrap();
        zip.write_all(
            r##"<?xml version="1.0"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>중복 바이너리</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
                .as_bytes(),
        )
        .unwrap();
        zip.start_file("Contents/header.xml", deflated).unwrap();
        zip.write_all(HEADER_XML.as_bytes()).unwrap();
        zip.start_file("Contents/section0.xml", deflated).unwrap();
        zip.write_all(
            r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문</hp:t><hp:pic id="5" zOrder="0"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="dupA" bright="0" contrast="0"/></hp:pic></hp:run></hp:p></hs:sec>"##
                .as_bytes(),
        )
        .unwrap();
        zip.start_file("BinData/dupA.png", deflated).unwrap();
        zip.write_all(PNG_DUP).unwrap();
        zip.start_file("BinData/dupB.png", deflated).unwrap();
        zip.write_all(PNG_DUP).unwrap();
        zip.finish().unwrap();
    }

    let read = hwpx::read_document(&src).unwrap();
    assert!(
        read.document.hwpx_bin_manifest.is_empty(),
        "미시드 경로여야 한다"
    );
    hwpx::write_document_with_report(&read.document, &out).unwrap();

    let out_entries = read_entries(&out);
    let bytes_of = |name: &str| -> Vec<u8> {
        out_entries
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("엔트리 없음: {name}"))
            .1
            .clone()
    };
    // 두 엔트리 모두 원본 이름·바이트로 보존.
    assert_eq!(bytes_of("BinData/dupA.png"), PNG_DUP, "dupA 보존");
    assert_eq!(bytes_of("BinData/dupB.png"), PNG_DUP, "dupB 보존");

    // pic 참조는 출력 매니페스트에서 같은 바이트로 해석된다.
    let section = String::from_utf8(bytes_of("Contents/section0.xml")).unwrap();
    let marker = "binaryItemIDRef=\"";
    let at = section.find(marker).unwrap();
    let rest = &section[at + marker.len()..];
    let pic_ref = &rest[..rest.find('"').unwrap()];
    let out_manifest =
        manifest_bin_map(&String::from_utf8(bytes_of("Contents/content.hpf")).unwrap());
    let href = out_manifest
        .get(pic_ref)
        .unwrap_or_else(|| panic!("출력 매니페스트에 {pic_ref} 없음"));
    assert_eq!(bytes_of(href), PNG_DUP, "pic 참조 바이트 보존");
}

/// 루트에만 선언된 벤더 접두어(xmlns:vnd)를 쓰는 원문 캡처 개체는, 재직렬화된
/// 섹션 루트가 그 선언을 실어야 namespace-well-formed하다. 빈 슬롯이면 루트는
/// 기존과 바이트 동일해야 한다(표준 hs/hp/hc만).
#[test]
fn 전체_재작성은_루트의_확장_xmlns_선언을_보존한다() {
    let dir = std::env::temp_dir().join("hwpx-roundtrip-xmlns");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");

    {
        let file = std::fs::File::create(&src).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/hwp+zip").unwrap();
        zip.start_file("version.xml", deflated).unwrap();
        zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
            .unwrap();
        zip.start_file("Contents/content.hpf", deflated).unwrap();
        zip.write_all(
            r##"<?xml version="1.0"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>벤더 접두어</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
                .as_bytes(),
        )
        .unwrap();
        zip.start_file("Contents/header.xml", deflated).unwrap();
        zip.write_all(HEADER_XML.as_bytes()).unwrap();
        zip.start_file("Contents/section0.xml", deflated).unwrap();
        // xmlns:vnd는 루트에만 선언 — 컨테이너 원문 안의 <vnd:mark>는 이 선언에 의존한다.
        zip.write_all(
            r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:vnd="urn:example"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>본문</hp:t><hp:container id="77" zOrder="3"><hp:sz width="1000" height="500"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>컨테이너 텍스트</hp:t></hp:run></hp:p></hp:subList><vnd:mark key="k1"/></hp:container></hp:run></hp:p></hs:sec>"##
                .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }

    let read = hwpx::read_document(&src).unwrap();
    assert_eq!(
        read.document.hwpx_section_xmlns,
        vec!["xmlns:vnd=\"urn:example\"".to_string()],
        "확장 xmlns 캡처"
    );
    hwpx::write_document_with_report(&read.document, &out).unwrap();

    let out_entries = read_entries(&out);
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
        section.contains(r#"xmlns:vnd="urn:example""#),
        "루트 확장 xmlns 유실: {section}"
    );
    assert!(
        section.contains(r#"<vnd:mark key="k1"/>"#),
        "원문 프래그먼트 유실: {section}"
    );

    // namespace-aware 검증: 모든 접두어가 해석돼야 하고(Unknown 없음), vnd:mark는
    // urn:example에 바인드돼야 한다.
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;
    let mut ns_reader = quick_xml::NsReader::from_str(&section);
    let mut mark_bound = false;
    loop {
        match ns_reader.read_resolved_event() {
            Ok((ns, Event::Start(e))) | Ok((ns, Event::Empty(e))) => {
                assert!(
                    !matches!(ns, ResolveResult::Unknown(_)),
                    "미바인딩 접두어: {:?}",
                    String::from_utf8_lossy(e.name().as_ref())
                );
                if e.local_name().as_ref() == b"mark" {
                    mark_bound =
                        matches!(ns, ResolveResult::Bound(ns) if ns.as_ref() == b"urn:example");
                }
            }
            Ok((_, Event::Eof)) => break,
            Err(e) => panic!("섹션 XML 파싱 실패: {e}"),
            _ => {}
        }
    }
    assert!(
        mark_bound,
        "vnd:mark가 urn:example에 바인드돼야 한다: {section}"
    );
}
