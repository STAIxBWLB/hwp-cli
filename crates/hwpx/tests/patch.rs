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
