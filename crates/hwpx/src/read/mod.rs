//! HWPX → IR 읽기.
//!
//! IR 의미를 hwp5와 일치시킨다: `hp:secPr`/`hp:ctrl(colPr)`/`hp:tbl`은
//! hwp5처럼 확장 컨트롤 문자(8 WCHAR) + `Control`로 표현해 두 포맷의
//! 위치 산수와 추출 로직이 같은 코드를 타게 한다.

pub mod header;
pub mod section;
mod xml;

use std::path::Path;

use hwp_model::{DocMeta, Document};

use crate::error::Result;
use crate::package::HwpxPackage;

pub struct ReadResult {
    pub document: Document,
    pub warnings: Vec<String>,
}

/// HWPX 파일을 IR로 읽는다.
pub fn read_document(path: &Path) -> Result<ReadResult> {
    read_document_impl(path, true)
}

/// HWPX의 구조와 XML을 읽되 `BinData/*` 본문은 압축 해제하지 않는다.
///
/// 모든 비디렉터리 엔트리를 EOF까지 `sink`로 스트리밍해 CRC/압축 스트림을 검증하되,
/// 실제 이미지는 IR에 적재하지 않아 큰 정상 문서도 첨부 크기에 비례한 메모리를 쓰지
/// 않는다.
pub fn read_structure(path: &Path) -> Result<ReadResult> {
    read_document_impl(path, false)
}

fn read_document_impl(path: &Path, load_binary_data: bool) -> Result<ReadResult> {
    let mut pkg = HwpxPackage::open(path)?;
    // 파서가 직접 사용하지 않는 Preview/BinData/확장 파트도 손상 여부를 놓치지
    // 않는다. 실제 바이트는 보관하지 않으며, 이후 필요한 파트만 다시 읽는다.
    pkg.verify_integrity()?;
    let mut warnings = Vec::new();

    let header_xml = pkg.read_entry_string("Contents/header.xml")?;
    let (mut doc_header, header_warnings) = header::parse_header(&header_xml)?;
    warnings.extend(
        header_warnings
            .into_iter()
            .map(|w| format!("[header.xml] {w}")),
    );

    let mut sections = Vec::new();
    for entry in pkg.section_entries()? {
        let xml = pkg.read_entry_string(&entry)?;
        let (section, sec_warnings) = section::parse_section(&xml)?;
        warnings.extend(sec_warnings.into_iter().map(|w| format!("[{entry}] {w}")));
        sections.push(section);
    }
    doc_header.properties.section_count = sections.len() as u16;

    // 첨부 바이너리 (이미지 등) — BinRef::ItemRef는 항목 이름 휴리스틱으로 해석
    let mut bin_streams = Vec::new();
    if load_binary_data {
        for entry in pkg.entries()? {
            if entry.name.starts_with("BinData/") {
                let data = pkg.read_entry(&entry.name)?;
                bin_streams.push(hwp_model::BinStream {
                    name: entry.name,
                    data,
                });
            }
        }
    }

    let version = pkg
        .version_info()?
        .into_iter()
        .filter(|(k, _)| ["major", "minor", "micro", "buildNumber"].contains(&k.as_str()))
        .map(|(_, v)| v)
        .collect::<Vec<_>>()
        .join(".");

    // 문서 메타데이터 (content.hpf OPF — 최선 노력: 없거나 손상돼도 진단 계속)
    let metadata = pkg
        .read_entry_string("Contents/content.hpf")
        .ok()
        .map(|xml| parse_content_meta(&xml))
        .unwrap_or_default();

    // 부속 파트 원문 pass-through 슬롯: settings.xml(앱 설정·캐럿)·version.xml(버전
    // 메타)을 통째로 보존한다. 없으면 None → 쓰기 시 기본 상수. "모르는 데이터는
    // 버리지 않는다".
    let hwpx_settings_xml = pkg.read_entry_string("settings.xml").ok();
    let hwpx_version_xml = pkg.read_entry_string("version.xml").ok();
    let has_preview_image = pkg
        .entries()?
        .iter()
        .any(|entry| entry.name == "Preview/PrvImage.png");
    let hwpx_preview_image = if load_binary_data && has_preview_image {
        Some(pkg.read_entry("Preview/PrvImage.png")?)
    } else {
        None
    };

    Ok(ReadResult {
        document: Document {
            meta: DocMeta {
                source_format: "hwpx".to_string(),
                source_version: version,
            },
            metadata,
            header: doc_header,
            sections,
            bin_streams,
            hwpx_settings_xml,
            hwpx_version_xml,
            hwpx_preview_image,
            // hwpx 출신은 hwp5 전용 스토리지가 없다(GE-β7/β8 경로 무관).
            hwp5_xml_template: Vec::new(),
            hwp5_doc_history: Vec::new(),
        },
        warnings,
    })
}

/// content.hpf OPF 메타데이터에서 요약정보를 추출한다(최선 노력).
///
/// 정품 표본 형식을 우선 읽는다:
/// - `<opf:title>`, `<opf:language>`(무시)
/// - `<opf:meta name="creator|subject|description|lastsaveby|keyword" content="text">값</opf:meta>`
///   (요소 텍스트가 값. `keyword`는 단수형)
/// - `<opf:meta name="CreatedDate|ModifiedDate" content="text">ISO8601</opf:meta>`
///   → [`iso8601_utc_to_filetime`]로 raw FILETIME u64 역산(초 정밀; 하위 100ns 소실).
/// - `<opf:meta name="date">`(한국어 KST 파생값)는 무시한다 — create_time에서 재파생.
///
/// 하위호환으로 구형 형식도 계속 읽는다: `<dc:creator>`/`<dc:subject>` 요소 텍스트,
/// `<opf:meta name="keywords" content="값"/>`(복수형 + content 속성).
pub fn parse_content_meta(xml: &str) -> hwp_model::Metadata {
    use quick_xml::events::Event;
    let mut meta = hwp_model::Metadata::default();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut capture: Option<(&'static str, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                // 구형: dc:title/dc:creator/dc:subject 요소 텍스트.
                b"title" => capture = Some(("title", String::new())),
                b"creator" => capture = Some(("author", String::new())),
                b"subject" => capture = Some(("subject", String::new())),
                b"meta" => {
                    keywords_from_meta(&e, &mut meta);
                    // 값을 요소 텍스트로 담는 meta는 다음 Text 이벤트에서 채운다.
                    capture = meta_capture(&e).map(|field| (field, String::new()));
                }
                _ => capture = None,
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"meta" {
                    // 빈 요소(값 없음)라도 구형 keywords content 속성은 읽는다.
                    keywords_from_meta(&e, &mut meta);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, value)) = &mut capture {
                    value.push_str(&t.xml10_content().unwrap_or_default());
                }
            }
            // quick-xml 0.40은 `&gt;` 같은 참조를 Text에 합치지 않고 별도
            // GeneralRef 이벤트로 방출한다. 첫 Text에서 캡처를 끝내면
            // `A=&gt;B`가 `A=`로 잘리므로 닫는 태그까지 모두 모아 해석한다.
            Ok(Event::GeneralRef(r)) => {
                if let Some((_, value)) = &mut capture
                    && let Some(c) = resolve_entity(&r)
                {
                    value.push(c);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some((_, value)) = &mut capture {
                    value.push_str(&t.xml10_content().unwrap_or_default());
                }
            }
            Ok(Event::End(_)) => {
                if let Some((field, value)) = capture.take() {
                    set_metadata_field(&mut meta, field, value.trim());
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    meta
}

fn resolve_entity(r: &quick_xml::events::BytesRef<'_>) -> Option<char> {
    r.resolve_char_ref()
        .ok()
        .flatten()
        .or_else(|| match &r[..] {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ => None,
        })
}

fn set_metadata_field(meta: &mut hwp_model::Metadata, field: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match field {
        "title" => meta.title = Some(value.to_string()),
        "author" => meta.author = Some(value.to_string()),
        "subject" => meta.subject = Some(value.to_string()),
        "keywords" => meta.keywords = Some(value.to_string()),
        "description" => meta.description = Some(value.to_string()),
        "last_saved_by" => meta.last_saved_by = Some(value.to_string()),
        "create_time" => meta.create_time = hwp_model::iso8601_utc_to_filetime(value),
        "modify_time" => meta.modify_time = hwp_model::iso8601_utc_to_filetime(value),
        _ => {}
    }
}

/// `<opf:meta name="...">`의 name 속성 → 캡처 대상 필드 태그(요소 텍스트를 값으로 담는 것).
fn meta_capture(e: &quick_xml::events::BytesStart<'_>) -> Option<&'static str> {
    match xml::attr(e, "name").as_deref() {
        Some("creator") => Some("author"),
        Some("subject") => Some("subject"),
        Some("keyword") => Some("keywords"), // 정품: 단수형 요소 텍스트
        Some("description") => Some("description"),
        Some("lastsaveby") => Some("last_saved_by"),
        Some("CreatedDate") => Some("create_time"),
        Some("ModifiedDate") => Some("modify_time"),
        _ => None,
    }
}

/// 구형 형식 하위호환: `<opf:meta name="keywords" content="값"/>`(복수형 + content 속성).
fn keywords_from_meta(e: &quick_xml::events::BytesStart<'_>, meta: &mut hwp_model::Metadata) {
    if xml::attr(e, "name").as_deref() == Some("keywords")
        && let Some(v) = xml::attr(e, "content").filter(|v| !v.is_empty())
    {
        meta.keywords = Some(v);
    }
}
