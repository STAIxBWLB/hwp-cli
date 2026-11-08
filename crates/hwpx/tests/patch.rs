//! Integration tests for fidelity-preserving fill (patch::fill_placeholders).
//!
//! Build a synthetic HWPX with a preview thumbnail, hp:switch compatibility block and {{name}},
//! then verify fill changes only body placeholders while retaining non-target entry bytes.

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

    // 2016 compatibility block (hp:switch), previously dropped by the IR writer.
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(
        b"<hh:head><hp:switch><hp:case>a</hp:case><hp:default>b</hp:default></hp:switch></hh:head>",
    )
    .unwrap();

    // A placeholder contained in one run.
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
    let dir = surgical_dir("fill-preview");
    let src = dir.join("hwpx_patch_src.hwpx");
    let out = dir.join("hwpx_patch_out.hwpx");
    build_fixture(&src);

    let mut values = BTreeMap::new();
    values.insert("기관명".to_string(), "제주한라대학교".to_string());
    let counts = hwpx::patch::fill_placeholders(&src, &out, &values).unwrap();
    assert_eq!(counts.get("기관명"), Some(&1), "{{기관명}} 1회 치환");

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();

    // mimetype is the first entry and STORED.
    {
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name().unwrap(), "mimetype");
        assert_eq!(first.compression(), CompressionMethod::Stored);
    }
    // Preserve preview-thumbnail bytes through raw copy.
    assert_eq!(read_entry(&mut zip, "Preview/PrvImage.png"), PRV_IMAGE);
    // Preserve the hp:switch compatibility block.
    let header = String::from_utf8(read_entry(&mut zip, "Contents/header.xml")).unwrap();
    assert!(header.contains("hp:switch"), "hp:switch 보존");
    // Body: replace the placeholder with its value.
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(!section.contains("{{기관명}}"), "자리표시자 제거됨");
    assert!(section.contains("제주한라대학교"), "값 삽입됨");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_reports_unfilled_as_zero() {
    let dir = surgical_dir("fill-unfilled");
    let src = dir.join("hwpx_patch_src2.hwpx");
    let out = dir.join("hwpx_patch_out2.hwpx");
    build_fixture(&src);

    let mut values = BTreeMap::new();
    values.insert("없는키".to_string(), "x".to_string());
    let counts = hwpx::patch::fill_placeholders(&src, &out, &values).unwrap();
    assert_eq!(counts.get("없는키"), Some(&0), "미발견 키는 0");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_동일_입출력_경로도_snapshot으로_안전하게_치환() {
    let dir = surgical_dir("fill-in-place");
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
    let _ = std::fs::remove_dir_all(&dir);
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
        .find(|index| archive.by_index(*index).unwrap().name().unwrap() == name)
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
    let dir = surgical_dir("template-fill");
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
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_fill_rejects_placeholder_in_metadata_and_preserves_destination() {
    let dir = surgical_dir("template-metadata");
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
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_fill_rejects_foreign_namespace_text_and_preserves_destination() {
    let dir = surgical_dir("template-foreign");
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
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_fill_rejects_ambiguous_or_non_text_field_and_preserves_destination() {
    let dir = surgical_dir("template-reject");
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
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- patch::replace_texts ----

/// replace_texts fixture: university-name text in the section and UTF-8 PrvText.txt.
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

    // Body containing a university name and XML special characters.
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
    let dir = surgical_dir("replace");
    let src = dir.join("hwpx_repl_src.hwpx");
    let out = dir.join("hwpx_repl_out.hwpx");
    build_replace_fixture(&src);

    // Longer names first; sequentially replacing a short name first would corrupt the longer name.
    let pairs = vec![
        ("한빛대학교".to_string(), "누리대학교".to_string()),
        ("한빛대".to_string(), "누리대".to_string()),
    ];
    let counts = hwpx::patch::replace_texts(&src, &out, &pairs).unwrap();
    assert_eq!(counts.get("Contents/section0.xml"), Some(&2), "본문 2건");
    assert_eq!(counts.get("Preview/PrvText.txt"), Some(&1), "미리보기 1건");

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    // Preserve non-target entry bytes.
    assert_eq!(read_entry(&mut zip, "Preview/PrvImage.png"), PRV_IMAGE);
    {
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name().unwrap(), "mimetype");
        assert_eq!(first.compression(), CompressionMethod::Stored);
    }
    // No repeated substitution of the shorter name inside the replacement university name.
    let section = String::from_utf8(read_entry(&mut zip, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains("누리대학교 &amp; 누리대 협약"),
        "순차 치환 결과: {section}"
    );
    // Replace PrvText as well.
    let prv = String::from_utf8(read_entry(&mut zip, "Preview/PrvText.txt")).unwrap();
    assert_eq!(prv, "누리대학교 보고서");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_texts_xml_이스케이프() {
    let dir = surgical_dir("replace-escape");
    let src = dir.join("hwpx_repl_esc_src.hwpx");
    let out = dir.join("hwpx_repl_esc_out.hwpx");
    build_replace_fixture(&src);

    // XML-escape special characters in from/to before substitution.
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
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- patch::rewrite_document_staged (surgical IR rewriting) ----

const SURG_PNG_REFERENCED: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 1, 1, 1];
const SURG_PNG_NEW: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7, 7];
const SURG_CONTAINER_RDF: &[u8] = br#"<?xml version="1.0"?><rdf:RDF>SURGICAL-RDF-MARKER</rdf:RDF>"#;
const SURG_LAYOUT_XML: &[u8] =
    br#"<?xml version="1.0"?><layout>SURGICAL-DOCOPTIONS-MARKER</layout>"#;
const SURG_SETTINGS_XML: &[u8] =
    br#"<?xml version="1.0"?><setting>SURGICAL-SETTINGS-MARKER</setting>"#;

const SURG_HEADER_XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##;

const SURG_CONTENT_HPF: &str = r##"<?xml version="1.0" encoding="UTF-8"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>원본 제목</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="section1" href="Contents/section1.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/><opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/><opf:itemref idref="section1" linear="yes"/></opf:spine></opf:package>"##;

/// First section containing captured container XML and a picture referencing image1.
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
    // Include the PID to prevent parallel cargo test sessions from overwriting artifacts.
    let dir = std::env::temp_dir().join(format!("hwpx-surgical-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_surgical_doc(path: &std::path::Path) -> hwp_model::Document {
    hwpx::read_document(path).unwrap().document
}

/// Check non-target entries retain compressed bytes and ZIP metadata after surgical rewriting.
fn assert_raw_preserved(src: &std::path::Path, out: &std::path::Path, names: &[&str]) {
    for name in names {
        assert_eq!(
            raw_entry(out, name),
            raw_entry(src, name),
            "{name} raw 보존"
        );
    }
}

/// All opaque entries (everything outside body content) must survive every dirty combination.
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
    // Edit one character in section0's first paragraph.
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

    // Raw-preserve non-target content entries too (section1/header/content.hpf).
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
    // Insert a new binary and picture control, matching the insert-image operation's IR change.
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
    // Copy the extended control character layout (code/ctrl_id/payload) from the existing picture anchor.
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

    // Although content.hpf is not dirty, new BinData requires the patch to regenerate it.
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
    // The new binary receives an entry name that does not conflict with the seed.
    assert_eq!(
        read_entry(&mut zip, "BinData/image2.png"),
        SURG_PNG_NEW,
        "새 BinData 엔트리"
    );
    // Force content.hpf regeneration: add the new item and retain existing items.
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
    // No dirty sections: raw-copy the entire body.
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
    // Retain the original manifest ID scheme even when regenerating content.hpf.
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
