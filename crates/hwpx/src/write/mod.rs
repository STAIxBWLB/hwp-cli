//! IR → HWPX 패키지 쓰기.
//!
//! 패키지 규칙: `mimetype`이 **첫 엔트리이며 무압축(stored)**, 나머지는
//! deflate. 항목 구성은 한글 저장 기준 표본을 따른다.

pub mod header;
pub mod section;
mod templates;

use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use hwp_model::Document;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::error::Result;
use crate::package::{PackageLimits, validate_output_entries};
use section::BinCollector;

/// writer가 문서에 별도 값이 없을 때 방출하는 표준 version.xml.
///
/// 저장 후 재읽기 semantic 검증이 writer가 materialize한 정확한 기본값만
/// 정규화할 수 있도록 공개한다.
pub const DEFAULT_VERSION_XML: &str = templates::VERSION_XML;

/// writer가 문서에 별도 값이 없을 때 방출하는 표준 settings.xml.
pub const DEFAULT_SETTINGS_XML: &str = templates::SETTINGS_XML;

#[derive(Default)]
pub struct HwpxWriteOptions {
    /// 줄 배치(linesegarray) 보존 여부. 한글은 줄 배치가 내용과 정합하지
    /// 않으면 "변조" 보안 경고를 띄우므로(한컴 공식: 수정 시 제거 권장)
    /// 기본 false. 무수정 왕복에만 true.
    pub preserve_linesegs: bool,
}

/// 문서를 HWPX 파일로 저장한다 (줄 배치 제거 — 한글이 재계산).
pub fn write_document(doc: &Document, path: &Path) -> Result<Vec<String>> {
    write_document_with(doc, path, &HwpxWriteOptions::default())
}

/// 옵션 지정 저장.
pub fn write_document_with(
    doc: &Document,
    path: &Path,
    opts: &HwpxWriteOptions,
) -> Result<Vec<String>> {
    write_document_with_limits(doc, path, opts, &PackageLimits::default())
}

/// 옵션과 패키지 자원 제한을 지정해 저장한다.
pub fn write_document_with_limits(
    doc: &Document,
    path: &Path,
    opts: &HwpxWriteOptions,
    limits: &PackageLimits,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();

    // 본문 먼저 직렬화 (BinData 수집 포함)
    let mut bins = BinCollector::default();
    let sections: Vec<String> = doc
        .sections
        .iter()
        .map(|s| section::write_section(doc, s, opts.preserve_linesegs, &mut bins, &mut warnings))
        .collect();
    let header_xml = header::write_header(&doc.header, doc.sections.len().max(1));

    // 미리보기 텍스트 (선두 1KB 근사)
    let mut preview = doc.plain_text();
    preview.truncate(
        preview
            .char_indices()
            .nth(1000)
            .map_or(preview.len(), |(i, _)| i),
    );

    let bin_meta: Vec<(String, String, String)> = bins
        .items
        .iter()
        .map(|(id, href, mime, _)| (id.clone(), href.clone(), mime.clone()))
        .collect();

    let content_hpf = templates::content_hpf(sections.len(), &bin_meta, &doc.metadata);
    let version_xml = doc
        .hwpx_version_xml
        .as_deref()
        .unwrap_or(templates::VERSION_XML);
    let settings_xml = doc
        .hwpx_settings_xml
        .as_deref()
        .unwrap_or(templates::SETTINGS_XML);

    // 실제 파일을 만들기 전에 생성 패키지도 읽기 경로와 같은 크기·중복 정책으로
    // 검증한다. 직렬화된 XML과 이미 수집된 바이너리만 계상하므로 출력은 남지 않는다.
    let mut output_entries: Vec<(String, u64)> = vec![
        ("mimetype".to_string(), templates::MIMETYPE.len() as u64),
        ("version.xml".to_string(), version_xml.len() as u64),
        (
            "META-INF/container.rdf".to_string(),
            templates::CONTAINER_RDF.len() as u64,
        ),
        (
            "META-INF/container.xml".to_string(),
            templates::CONTAINER_XML.len() as u64,
        ),
        (
            "META-INF/manifest.xml".to_string(),
            templates::MANIFEST_XML.len() as u64,
        ),
        ("Contents/content.hpf".to_string(), content_hpf.len() as u64),
        ("Contents/header.xml".to_string(), header_xml.len() as u64),
    ];
    output_entries.extend(
        sections
            .iter()
            .enumerate()
            .map(|(i, xml)| (format!("Contents/section{i}.xml"), xml.len() as u64)),
    );
    output_entries.extend(
        bins.items
            .iter()
            .map(|(_, href, _, bytes)| (href.clone(), bytes.len() as u64)),
    );
    output_entries.push(("Preview/PrvText.txt".to_string(), preview.len() as u64));
    output_entries.push(("settings.xml".to_string(), settings_xml.len() as u64));
    validate_output_entries(output_entries, limits)?;

    let file = File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    // Compose는 동일 입력에서 byte-identical 출력을 보장한다. `zip`의 기본 옵션은
    // 빌드 feature에 따라 현재 시각을 기록하므로, 모든 새 HWPX 엔트리를 DOS ZIP
    // epoch(1980-01-01 00:00:00)로 고정한다. 기존 패키지 엔트리를 raw-copy하는
    // patch 경로는 이 writer를 사용하지 않으므로 원본 timestamp 보존 계약과 독립적이다.
    let fixed_timestamp = zip::DateTime::default();
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(fixed_timestamp);
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(fixed_timestamp);

    // 1. mimetype — 반드시 첫 엔트리 + 무압축
    zip.start_file("mimetype", stored)?;
    zip.write_all(templates::MIMETYPE.as_bytes())?;

    let put = |zip: &mut zip::ZipWriter<File>, name: &str, data: &[u8]| -> Result<()> {
        zip.start_file(name, deflated)?;
        zip.write_all(data)?;
        Ok(())
    };

    // version.xml — 슬롯이 있으면 원문 pass-through, 없으면 기본 상수(바이트 동일).
    put(&mut zip, "version.xml", version_xml.as_bytes())?;
    put(
        &mut zip,
        "META-INF/container.rdf",
        templates::CONTAINER_RDF.as_bytes(),
    )?;
    put(
        &mut zip,
        "META-INF/container.xml",
        templates::CONTAINER_XML.as_bytes(),
    )?;
    put(
        &mut zip,
        "META-INF/manifest.xml",
        templates::MANIFEST_XML.as_bytes(),
    )?;
    put(&mut zip, "Contents/content.hpf", content_hpf.as_bytes())?;
    put(&mut zip, "Contents/header.xml", header_xml.as_bytes())?;
    for (i, xml) in sections.iter().enumerate() {
        put(
            &mut zip,
            &format!("Contents/section{i}.xml"),
            xml.as_bytes(),
        )?;
    }
    for (_, href, _, bytes) in &bins.items {
        put(&mut zip, href, bytes)?;
    }
    put(&mut zip, "Preview/PrvText.txt", preview.as_bytes())?;
    // settings.xml — 슬롯이 있으면 원문 pass-through, 없으면 기본 상수(바이트 동일).
    put(&mut zip, "settings.xml", settings_xml.as_bytes())?;

    zip.finish()?;
    Ok(warnings)
}
