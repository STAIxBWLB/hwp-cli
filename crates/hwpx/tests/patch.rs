//! 충실도 보존 fill (patch::fill_placeholders) 통합 테스트.
//!
//! 합성 HWPX(미리보기 썸네일 + `hp:switch` 호환 블록 + `{{name}}`)를 만든 뒤,
//! 채우기 후에도 비대상 엔트리가 바이트 보존되고 본문 자리표시자만 치환되는지 검증.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

const PRV_IMAGE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];

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

    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(PRV_IMAGE).unwrap();

    // 2016 호환 블록(hp:switch) — IR 경유 writer가 떨어뜨리는 부분.
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(
        b"<hh:head><hp:switch><hp:case>a</hp:case><hp:default>b</hp:default></hp:switch></hh:head>",
    )
    .unwrap();

    // 단일 런 자리표시자.
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        "<hs:sec><hp:p><hp:run><hp:t>{{기관명}} 운영 보고</hp:t></hp:run></hp:p></hs:sec>"
            .as_bytes(),
    )
    .unwrap();

    zip.finish().unwrap();
}

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

#[test]
fn fill_preserves_preview_and_compat() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_patch_src.hwpx");
    let out = dir.join("hwpx_patch_out.hwpx");
    build_fixture(&src);

    let mut values = BTreeMap::new();
    values.insert("기관명".to_string(), "제주한라대학교".to_string());
    let counts = hwpx::patch::fill_placeholders(&src, &out, &values).unwrap();
    assert_eq!(counts.get("기관명"), Some(&1), "{{기관명}} 1회 치환");

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();

    // mimetype 첫 엔트리 + STORED.
    {
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), CompressionMethod::Stored);
    }
    // 미리보기 썸네일 바이트 보존 (raw copy).
    assert_eq!(read_entry(&mut zip, "Preview/PrvImage.png"), PRV_IMAGE);
    // hp:switch 호환 블록 보존.
    let header = String::from_utf8(read_entry(&mut zip, "Contents/header.xml")).unwrap();
    assert!(header.contains("hp:switch"), "hp:switch 보존");
    // 본문: 자리표시자 → 값.
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(!section.contains("{{기관명}}"), "자리표시자 제거됨");
    assert!(section.contains("제주한라대학교"), "값 삽입됨");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn fill_reports_unfilled_as_zero() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_patch_src2.hwpx");
    let out = dir.join("hwpx_patch_out2.hwpx");
    build_fixture(&src);

    let mut values = BTreeMap::new();
    values.insert("없는키".to_string(), "x".to_string());
    let counts = hwpx::patch::fill_placeholders(&src, &out, &values).unwrap();
    assert_eq!(counts.get("없는키"), Some(&0), "미발견 키는 0");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn fill_동일_입출력_경로도_snapshot으로_안전하게_치환() {
    let dir = std::env::temp_dir();
    let f = dir.join("hwpx_patch_inplace.hwpx");
    build_fixture(&f);

    let mut values = BTreeMap::new();
    values.insert("기관명".to_string(), "x".to_string());
    let counts = hwpx::patch::fill_placeholders(&f, &f, &values).unwrap();
    assert_eq!(counts.get("기관명"), Some(&1));
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&f).unwrap()).unwrap();
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains(">x 운영 보고<"),
        "in-place 결과: {section}"
    );

    let _ = std::fs::remove_file(&f);
}

fn build_template_field_fixture(path: &std::path::Path, field_values: &[&str]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(PRV_IMAGE).unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(b"<hh:head><hp:switch><hp:case>a</hp:case></hp:switch></hh:head>")
        .unwrap();
    let fields = field_values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let id = index + 1;
            format!(
                r#"<hp:ctrl><hp:fieldBegin id="{id}" type="CLICK_HERE" name="수신"/></hp:ctrl>{value}<hp:ctrl><hp:fieldEnd beginIDRef="{id}"/></hp:ctrl>"#
            )
        })
        .collect::<String>();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<hs:sec xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p><hp:run><hp:t>{{{{기관명}}}}</hp:t>{fields}</hp:run></hp:p></hs:sec>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zip.start_file("Contents/section1.xml", deflated).unwrap();
    zip.write_all(
        br#"<hs:sec xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p><hp:run><hp:t>untouched second section</hp:t></hp:run></hp:p></hs:sec>"#,
    )
    .unwrap();
    zip.finish().unwrap();
    set_central_external_attributes(path, "mimetype", 0x0180_0000);
}

fn set_central_external_attributes(path: &std::path::Path, name: &str, value: u32) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let start = archive.by_name(name).unwrap().central_header_start();
    drop(archive);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(start + 38)).unwrap();
    file.write_all(&value.to_le_bytes()).unwrap();
    file.sync_all().unwrap();
}

fn central_record_without_offset(path: &std::path::Path, name: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let start = archive.by_name(name).unwrap().central_header_start();
    drop(archive);
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::Start(start)).unwrap();
    let mut fixed = [0u8; 46];
    file.read_exact(&mut fixed).unwrap();
    assert_eq!(&fixed[..4], b"PK\x01\x02");
    let variable = usize::from(u16::from_le_bytes([fixed[28], fixed[29]]))
        + usize::from(u16::from_le_bytes([fixed[30], fixed[31]]))
        + usize::from(u16::from_le_bytes([fixed[32], fixed[33]]));
    let mut record = fixed.to_vec();
    record.resize(46 + variable, 0);
    file.read_exact(&mut record[46..]).unwrap();
    record[42..46].fill(0);
    record
}

fn build_placeholder_metadata_fixture(path: &std::path::Path) {
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
    zip.write_all(b"<hh:head/>").unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        r#"<hs:sec xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p><hp:run><hp:ctrl name="{{기관명}}"/><hp:t>visible</hp:t></hp:run></hp:p></hs:sec>"#
            .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap();
}

fn build_foreign_text_fixture(path: &std::path::Path) {
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
    zip.write_all(b"<hh:head/>").unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        r#"<hs:sec xmlns:x="urn:not-hwpx"><x:t>{{기관명}}</x:t><hp:t xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">visible</hp:t></hs:sec>"#
            .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap();
}

#[derive(Debug, PartialEq, Eq)]
struct RawEntry {
    compressed: Vec<u8>,
    method: CompressionMethod,
    crc32: u32,
    compressed_size: u64,
    modified: String,
    unix_mode: Option<u32>,
    comment: String,
    extra: Vec<u8>,
}

fn raw_entry(path: &std::path::Path, name: &str) -> RawEntry {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let index = (0..archive.len())
        .find(|index| archive.by_index(*index).unwrap().name() == name)
        .unwrap();
    let entry = archive.by_index(index).unwrap();
    let metadata = (
        entry.compression(),
        entry.crc32(),
        entry.compressed_size(),
        format!("{:?}", entry.last_modified()),
        entry.unix_mode(),
        entry.comment().to_string(),
        entry.extra_data().unwrap_or_default().to_vec(),
    );
    drop(entry);
    let mut compressed = Vec::new();
    archive
        .by_index_raw(index)
        .unwrap()
        .read_to_end(&mut compressed)
        .unwrap();
    RawEntry {
        compressed,
        method: metadata.0,
        crc32: metadata.1,
        compressed_size: metadata.2,
        modified: metadata.3,
        unix_mode: metadata.4,
        comment: metadata.5,
        extra: metadata.6,
    }
}

#[test]
fn template_fill_changes_placeholder_and_simple_field_but_raw_copies_untouched_entries() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_template_fill_src.hwpx");
    let out = dir.join("hwpx_template_fill_out.hwpx");
    build_template_field_fixture(&src, &["<hp:t>old</hp:t>"]);
    let before_preview = raw_entry(&src, "Preview/PrvImage.png");
    let before_header = raw_entry(&src, "Contents/header.xml");
    let before_untouched_section = raw_entry(&src, "Contents/section1.xml");
    let placeholders = BTreeMap::from([("기관명".to_string(), "A&B".to_string())]);
    let fields = BTreeMap::from([("수신".to_string(), "홍길동\n제주".to_string())]);

    let counts = hwpx::patch::fill_template_values(&src, &out, &placeholders, &fields).unwrap();
    assert_eq!(counts.placeholders["기관명"], 1);
    assert_eq!(counts.fields["수신"], 1);
    assert_eq!(raw_entry(&out, "Preview/PrvImage.png"), before_preview);
    assert_eq!(raw_entry(&out, "Contents/header.xml"), before_header);
    assert_eq!(
        raw_entry(&out, "Contents/section1.xml"),
        before_untouched_section,
        "unchanged section must preserve compressed bytes and ZIP metadata"
    );
    for name in [
        "mimetype",
        "version.xml",
        "Preview/PrvImage.png",
        "Contents/header.xml",
        "Contents/section1.xml",
    ] {
        assert_eq!(
            central_record_without_offset(&out, name),
            central_record_without_offset(&src, name),
            "untouched central-directory metadata differs for {name}"
        );
    }
    let mimetype = central_record_without_offset(&out, "mimetype");
    assert_eq!(
        u32::from_le_bytes(mimetype[38..42].try_into().unwrap()),
        0x0180_0000,
        "raw preservation must not normalize external attributes"
    );
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let section = String::from_utf8(read_entry(&mut archive, "Contents/section0.xml")).unwrap();
    assert!(section.contains("A&amp;B"));
    assert!(section.contains("홍길동<hp:lineBreak/>제주"));
    assert!(!section.contains(">old<"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn template_fill_rejects_placeholder_in_metadata_and_preserves_destination() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_template_metadata_placeholder.hwpx");
    let out = dir.join("hwpx_template_metadata_placeholder_out.hwpx");
    build_placeholder_metadata_fixture(&src);
    std::fs::write(&out, b"KEEP").unwrap();
    let placeholders = BTreeMap::from([("기관명".to_string(), "secret".to_string())]);

    let error = hwpx::patch::fill_template_values(&src, &out, &placeholders, &BTreeMap::new())
        .expect_err("metadata placeholder must fail closed");
    assert!(error.to_string().contains("outside a text node"));
    assert_eq!(std::fs::read(&out).unwrap(), b"KEEP");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn template_fill_rejects_foreign_namespace_text_and_preserves_destination() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_template_foreign_text.hwpx");
    let out = dir.join("hwpx_template_foreign_text_out.hwpx");
    build_foreign_text_fixture(&src);
    std::fs::write(&out, b"KEEP").unwrap();
    let placeholders = BTreeMap::from([("기관명".to_string(), "secret".to_string())]);

    let error = hwpx::patch::fill_template_values(&src, &out, &placeholders, &BTreeMap::new())
        .expect_err("foreign namespace text must fail closed");
    assert!(error.to_string().contains("outside a text node"));
    assert_eq!(std::fs::read(&out).unwrap(), b"KEEP");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn template_fill_rejects_ambiguous_or_non_text_field_and_preserves_destination() {
    let dir = std::env::temp_dir();
    let ambiguous = dir.join("hwpx_template_ambiguous.hwpx");
    let non_text = dir.join("hwpx_template_non_text.hwpx");
    let out = dir.join("hwpx_template_reject_out.hwpx");
    let fields = BTreeMap::from([("수신".to_string(), "secret".to_string())]);
    build_template_field_fixture(&ambiguous, &["<hp:t>one</hp:t>", "<hp:t>two</hp:t>"]);
    build_template_field_fixture(&non_text, &["<hp:tbl/>"]);

    for input in [&ambiguous, &non_text] {
        std::fs::write(&out, b"KEEP").unwrap();
        let error = hwpx::patch::fill_template_values(input, &out, &BTreeMap::new(), &fields)
            .expect_err("strict field gate");
        assert!(error.to_string().contains("field fill rejected"));
        assert_eq!(std::fs::read(&out).unwrap(), b"KEEP");
    }

    let _ = std::fs::remove_file(ambiguous);
    let _ = std::fs::remove_file(non_text);
    let _ = std::fs::remove_file(out);
}

// ---- patch::replace_texts ----

/// replace_texts용 픽스처: 섹션에 대학명 텍스트 + PrvText.txt(UTF-8) 포함.
fn build_replace_fixture(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();

    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();

    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(PRV_IMAGE).unwrap();

    zip.start_file("Preview/PrvText.txt", deflated).unwrap();
    zip.write_all("한빛대학교 보고서".as_bytes()).unwrap();

    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(b"<hh:head/>").unwrap();

    // 대학명 + XML 특수문자 포함 본문.
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        "<hs:sec><hp:p><hp:run><hp:t>한빛대학교 &amp; 한빛대 협약</hp:t></hp:run></hp:p></hs:sec>"
            .as_bytes(),
    )
    .unwrap();

    zip.finish().unwrap();
}

#[test]
fn replace_texts_바이트보존_순차치환() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_repl_src.hwpx");
    let out = dir.join("hwpx_repl_out.hwpx");
    build_replace_fixture(&src);

    // 긴 이름 먼저(순차 치환 — 짧은 이름이 먼저면 긴 이름 안을 오염).
    let pairs = vec![
        ("한빛대학교".to_string(), "누리대학교".to_string()),
        ("한빛대".to_string(), "누리대".to_string()),
    ];
    let counts = hwpx::patch::replace_texts(&src, &out, &pairs).unwrap();
    assert_eq!(counts.get("Contents/section0.xml"), Some(&2), "본문 2건");
    assert_eq!(counts.get("Preview/PrvText.txt"), Some(&1), "미리보기 1건");

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    // 비대상 엔트리는 바이트 보존.
    assert_eq!(read_entry(&mut zip, "Preview/PrvImage.png"), PRV_IMAGE);
    {
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), CompressionMethod::Stored);
    }
    // 재오염 없음: "누리대학교" 안에 "누리대"가 다시 치환되지 않았다.
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains("누리대학교 &amp; 누리대 협약"),
        "순차 치환 결과: {section}"
    );
    // PrvText도 치환.
    let prv = String::from_utf8(read_entry(&mut zip, "Preview/PrvText.txt")).unwrap();
    assert_eq!(prv, "누리대학교 보고서");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn replace_texts_xml_이스케이프() {
    let dir = std::env::temp_dir();
    let src = dir.join("hwpx_repl_esc_src.hwpx");
    let out = dir.join("hwpx_repl_esc_out.hwpx");
    build_replace_fixture(&src);

    // from/to의 특수문자는 XML 이스케이프 후 치환돼야 한다.
    let pairs = vec![("한빛대 협약".to_string(), "A&B 제휴".to_string())];
    hwpx::patch::replace_texts(&src, &out, &pairs).unwrap();

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains("A&amp;B 제휴"),
        "이스케이프 치환: {section}"
    );
    assert!(
        !section.contains("A&B"),
        "날 ampersand 방출 금지: {section}"
    );

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
}

// ---- patch::rewrite_document_staged (IR 외과 수술 재작성) ----

const SURG_PNG_REFERENCED: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 1, 1, 1];
const SURG_PNG_NEW: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7, 7];
const SURG_CONTAINER_RDF: &[u8] = br#"<?xml version="1.0"?><rdf:RDF>SURGICAL-RDF-MARKER</rdf:RDF>"#;
const SURG_LAYOUT_XML: &[u8] =
    br#"<?xml version="1.0"?><layout>SURGICAL-DOCOPTIONS-MARKER</layout>"#;
const SURG_SETTINGS_XML: &[u8] =
    br#"<?xml version="1.0"?><setting>SURGICAL-SETTINGS-MARKER</setting>"#;

const SURG_HEADER_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##;

const SURG_CONTENT_HPF: &str = r##"<?xml version="1.0" encoding="UTF-8"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>원본 제목</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="section1" href="Contents/section1.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/><opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/><opf:itemref idref="section1" linear="yes"/></opf:spine></opf:package>"##;

/// 컨테이너(미해석 개체 원문) + image1을 참조하는 그림을 담은 첫째 구역.
const SURG_SECTION0_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>원본 본문</hp:t><hp:container id="77" zOrder="3"><hp:sz width="1000" height="500"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>컨테이너 텍스트</hp:t></hp:run></hp:p></hp:subList></hp:container><hp:pic id="5" zOrder="0"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="image1" bright="0" contrast="0"/></hp:pic></hp:run></hp:p></hs:sec>"##;

const SURG_SECTION1_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>둘째 구역</hp:t></hp:run></hp:p></hs:sec>"##;

fn build_surgical_fixture(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("META-INF/container.rdf", deflated).unwrap();
    zip.write_all(SURG_CONTAINER_RDF).unwrap();
    zip.start_file("Contents/content.hpf", deflated).unwrap();
    zip.write_all(SURG_CONTENT_HPF.as_bytes()).unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(SURG_HEADER_XML.as_bytes()).unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(SURG_SECTION0_XML.as_bytes()).unwrap();
    zip.start_file("Contents/section1.xml", deflated).unwrap();
    zip.write_all(SURG_SECTION1_XML.as_bytes()).unwrap();
    zip.start_file("BinData/image1.png", deflated).unwrap();
    zip.write_all(SURG_PNG_REFERENCED).unwrap();
    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(PRV_IMAGE).unwrap();
    zip.start_file("settings.xml", deflated).unwrap();
    zip.write_all(SURG_SETTINGS_XML).unwrap();
    zip.start_file("DocOptions/Layout.xml", deflated).unwrap();
    zip.write_all(SURG_LAYOUT_XML).unwrap();
    zip.finish().unwrap();
}

fn surgical_dir(tag: &str) -> std::path::PathBuf {
    // PID 포함 — 병렬 cargo test 세션끼리 산출물을 덮어쓰지 않게.
    let dir = std::env::temp_dir().join(format!("hwpx-surgical-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_surgical_doc(path: &std::path::Path) -> hwp_model::Document {
    hwpx::read_document(path).unwrap().document
}

/// 외과 수술 후에도 비대상 엔트리가 압축 바이트·ZIP 메타데이터까지 동일한지 검사.
fn assert_raw_preserved(src: &std::path::Path, out: &std::path::Path, names: &[&str]) {
    for name in names {
        assert_eq!(
            raw_entry(out, name),
            raw_entry(src, name),
            "{name} raw 보존"
        );
    }
}

/// 불투명 엔트리 전체(본문 콘텐츠 외 모든 것) — 어떤 dirty 조합에서도 보존돼야 한다.
const SURG_OPAQUE_ENTRIES: &[&str] = &[
    "mimetype",
    "version.xml",
    "META-INF/container.rdf",
    "BinData/image1.png",
    "Preview/PrvImage.png",
    "settings.xml",
    "DocOptions/Layout.xml",
];

#[test]
fn rewrite_staged_텍스트편집_비대상_엔트리_바이트보존() {
    let dir = surgical_dir("text");
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_surgical_fixture(&src);

    let mut doc = read_surgical_doc(&src);
    // section0 첫 문단 텍스트 한 글자 편집 ("원본 본문" → "개본 본문").
    for ch in &mut doc.sections[0].paragraphs[0].chars {
        if let hwp_model::HwpChar::Text('원') = ch {
            *ch = hwp_model::HwpChar::Text('개');
            break;
        }
    }
    let dirty = hwpx::patch::DirtyEntries {
        sections: Some(vec![0]),
        header: false,
        content_hpf: false,
    };
    let report = hwpx::patch::rewrite_document_staged(
        &src,
        &out,
        &doc,
        &dirty,
        &hwpx::PackageLimits::default(),
    )
    .unwrap();
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    // 비대상 콘텐츠 엔트리(section1·header·content.hpf)까지 raw 보존.
    let mut preserved = SURG_OPAQUE_ENTRIES.to_vec();
    preserved.extend([
        "Contents/section1.xml",
        "Contents/header.xml",
        "Contents/content.hpf",
    ]);
    assert_raw_preserved(&src, &out, &preserved);

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(section.contains("개본 본문"), "편집 반영: {section}");
    assert!(
        section.contains(r#"<hp:container id="77" zOrder="3">"#),
        "컨테이너 원문 유지: {section}"
    );
    assert!(
        section.contains(r#"binaryItemIDRef="image1""#),
        "기존 그림이 원본 id 유지: {section}"
    );
}

#[test]
fn rewrite_staged_이미지추가_새엔트리와_매니페스트_갱신() {
    let dir = surgical_dir("image");
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_surgical_fixture(&src);

    let mut doc = read_surgical_doc(&src);
    // 새 바이너리 + 그림 컨트롤 삽입 (insert-image op과 같은 IR 변화).
    doc.bin_streams.push(hwp_model::BinStream {
        name: "inserted1.png".to_string(),
        data: SURG_PNG_NEW.to_vec(),
    });
    let para = &mut doc.sections[0].paragraphs[0];
    let pic_pos = para
        .controls
        .iter()
        .position(|c| matches!(c, hwp_model::Control::Picture(_)))
        .unwrap();
    let mut new_pic = match &para.controls[pic_pos] {
        hwp_model::Control::Picture(pic) => pic.clone(),
        _ => unreachable!(),
    };
    new_pic.bin_ref = hwp_model::BinRef::ItemRef("inserted1.png".to_string());
    para.controls.push(hwp_model::Control::Picture(new_pic));
    let new_index = (para.controls.len() - 1) as u32;
    // 기존 그림 앵커 문자에서 확장 컨트롤 문자 형태(code/ctrl_id/payload)를 복사한다.
    let (code, ctrl_id, payload) = para
        .chars
        .iter()
        .find_map(|ch| match ch {
            hwp_model::HwpChar::ExtCtrl {
                code,
                ctrl_id,
                payload,
                ctrl_index,
            } if *ctrl_index == Some(pic_pos as u32) => Some((*code, *ctrl_id, payload.clone())),
            _ => None,
        })
        .expect("기존 그림 앵커 문자");
    para.chars.push(hwp_model::HwpChar::ExtCtrl {
        code,
        ctrl_id,
        payload,
        ctrl_index: Some(new_index),
    });

    // content.hpf는 dirty가 아니지만, 새 BinData가 생겨 패치가 강제로 재생성해야 한다.
    let dirty = hwpx::patch::DirtyEntries {
        sections: Some(vec![0]),
        header: false,
        content_hpf: false,
    };
    let report = hwpx::patch::rewrite_document_staged(
        &src,
        &out,
        &doc,
        &dirty,
        &hwpx::PackageLimits::default(),
    )
    .unwrap();
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    // 새 바이너리는 seed와 충돌하지 않는 새 엔트리로 추가됐다.
    assert_eq!(
        read_entry(&mut zip, "BinData/image2.png"),
        SURG_PNG_NEW,
        "새 BinData 엔트리"
    );
    // content.hpf 강제 재생성: 신규 항목 추가 + 기존 항목 유지.
    let hpf = String::from_utf8(read_entry(&mut zip, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains(
            r#"<opf:item id="image2" href="BinData/image2.png" media-type="image/png" isEmbeded="1"/>"#
        ),
        "신규 매니페스트 항목: {hpf}"
    );
    assert!(
        hpf.contains(r#"id="image1" href="BinData/image1.png""#),
        "기존 매니페스트 항목 유지: {hpf}"
    );
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains(r#"binaryItemIDRef="image2""#),
        "새 그림은 새 id: {section}"
    );
    assert!(
        section.contains(r#"binaryItemIDRef="image1""#),
        "기존 그림은 원본 id: {section}"
    );
    drop(zip);

    let mut preserved = SURG_OPAQUE_ENTRIES.to_vec();
    preserved.extend(["Contents/section1.xml", "Contents/header.xml"]);
    assert_raw_preserved(&src, &out, &preserved);
}

#[test]
fn rewrite_staged_메타데이터편집_content_hpf만_재생성() {
    let dir = surgical_dir("meta");
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_surgical_fixture(&src);

    let mut doc = read_surgical_doc(&src);
    doc.metadata.title = Some("새 제목".to_string());
    // 섹션 0개 dirty — 본문은 전부 raw 복사돼야 한다.
    let dirty = hwpx::patch::DirtyEntries {
        sections: Some(vec![]),
        header: false,
        content_hpf: true,
    };
    hwpx::patch::rewrite_document_staged(&src, &out, &doc, &dirty, &hwpx::PackageLimits::default())
        .unwrap();

    let mut preserved = SURG_OPAQUE_ENTRIES.to_vec();
    preserved.extend([
        "Contents/section0.xml",
        "Contents/section1.xml",
        "Contents/header.xml",
    ]);
    assert_raw_preserved(&src, &out, &preserved);

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let hpf = String::from_utf8(read_entry(&mut zip, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains("<opf:title>새 제목</opf:title>"),
        "메타데이터 반영: {hpf}"
    );
    assert!(
        hpf.contains(r#"id="image1" href="BinData/image1.png""#),
        "기존 매니페스트 항목 유지: {hpf}"
    );
}

#[test]
fn rewrite_staged_전체_dirty도_불투명_엔트리는_보존() {
    let dir = surgical_dir("all");
    let src = dir.join("src.hwpx");
    let out = dir.join("out.hwpx");
    build_surgical_fixture(&src);

    let doc = read_surgical_doc(&src);
    let report = hwpx::patch::rewrite_document_staged(
        &src,
        &out,
        &doc,
        &hwpx::patch::DirtyEntries::all(),
        &hwpx::PackageLimits::default(),
    )
    .unwrap();
    assert!(
        report.preservation.is_lossless(),
        "preservation events: {:?}",
        report.preservation.events
    );

    assert_raw_preserved(&src, &out, SURG_OPAQUE_ENTRIES);

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(section.contains("원본 본문"), "본문 유지: {section}");
    assert!(
        section.contains(r#"<hp:container id="77" zOrder="3">"#),
        "컨테이너 원문 유지: {section}"
    );
    assert!(
        section.contains(r#"binaryItemIDRef="image1""#),
        "기존 그림이 원본 id 유지: {section}"
    );
    // content.hpf가 재생성돼도 원본 매니페스트 id 체계를 유지한다.
    let hpf = String::from_utf8(read_entry(&mut zip, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains(r#"id="image1" href="BinData/image1.png""#),
        "매니페스트 id 유지: {hpf}"
    );
    assert!(
        hpf.contains("<opf:title>원본 제목</opf:title>"),
        "메타 유지: {hpf}"
    );
}
