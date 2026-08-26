//! HWPX → IR 읽기.
//!
//! IR 의미를 hwp5와 일치시킨다: `hp:secPr`/`hp:ctrl(colPr)`/`hp:tbl`은
//! hwp5처럼 확장 컨트롤 문자(8 WCHAR) + `Control`로 표현해 두 포맷의
//! 위치 산수와 추출 로직이 같은 코드를 타게 한다.

pub mod header;
pub(crate) mod password;
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

/// Per-call options for the HWPX reader. Password bytes are used exactly as
/// provided and do not survive the reader invocation.
#[derive(Clone, Copy, Default)]
pub struct ReadOptions<'a> {
    pub password: Option<&'a str>,
}

/// HWPX 파일을 IR로 읽는다.
pub fn read_document(path: &Path) -> Result<ReadResult> {
    read_document_with_options(path, &ReadOptions::default())
}

/// Reads an HWPX document with the explicit per-call password option.
pub fn read_document_with_options(path: &Path, options: &ReadOptions<'_>) -> Result<ReadResult> {
    read_document_impl(path, true, options)
}

/// HWPX의 구조와 XML을 읽되 `BinData/*` 본문은 압축 해제하지 않는다.
///
/// 모든 비디렉터리 엔트리를 EOF까지 `sink`로 스트리밍해 CRC/압축 스트림을 검증하되,
/// 실제 이미지는 IR에 적재하지 않아 큰 정상 문서도 첨부 크기에 비례한 메모리를 쓰지
/// 않는다.
pub fn read_structure(path: &Path) -> Result<ReadResult> {
    read_document_impl(path, false, &ReadOptions::default())
}

fn read_document_impl(
    path: &Path,
    load_binary_data: bool,
    options: &ReadOptions<'_>,
) -> Result<ReadResult> {
    let mut pkg = HwpxPackage::open(path)?;
    // GATE-02: refuse an encrypted package before the integrity sweep or any content
    // part is read. An encrypted package can fail integrity for reasons unrelated to
    // the user's actual problem; the typed encryption message is the one that helps.
    match options.password {
        Some(password) if pkg.has_encryption_marker()? => pkg.unlock_with_password(password)?,
        Some(_) => {}
        None => pkg.check_body_readable()?,
    }
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
    let mut hwpx_section_xmlns: Vec<String> = Vec::new();
    for entry in pkg.section_entries()? {
        let xml = pkg.read_entry_string(&entry)?;
        // 루트에만 선언된 확장 접두어를 전 섹션 합집합으로 보존한다(접두어 기준 dedup —
        // 같은 접두어의 다른 URI 선언이 중복 속성으로 방출되면 not well-formed).
        for decl in section::extract_section_root_xmlns(&xml) {
            let prefix = decl
                .strip_prefix("xmlns:")
                .and_then(|rest| rest.split('=').next())
                .unwrap_or("");
            let dominated = hwpx_section_xmlns.iter().any(|existing| {
                existing
                    .strip_prefix("xmlns:")
                    .and_then(|rest| rest.split('=').next())
                    .unwrap_or("")
                    == prefix
            });
            if !dominated {
                hwpx_section_xmlns.push(decl);
            }
        }
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

    // writer가 재생성·슬롯으로 보존하는 엔트리 외의 모든 패키지 엔트리를 원본
    // 순서·바이트 그대로 보존한다(원본 META-INF/*, DocOptions, 스크립트, 추가
    // Preview 등). "모르는 데이터는 버리지 않는다". BinData/*는 bin_streams가,
    // section/header/settings/version/preview는 기존 슬롯이 담당하므로 제외한다.
    let mut hwpx_extra_entries = Vec::new();
    let was_unlocked_with_password = pkg.was_unlocked_with_password();
    if load_binary_data {
        const REGENERATED: &[&str] = &[
            "mimetype",
            "version.xml",
            "Contents/content.hpf",
            "Contents/header.xml",
            "Preview/PrvText.txt",
            "Preview/PrvImage.png",
            "settings.xml",
        ];
        for entry in pkg.entries()? {
            let name = &entry.name;
            if name.ends_with('/')
                || REGENERATED.contains(&name.as_str())
                || (was_unlocked_with_password && name == "META-INF/manifest.xml")
                // section 목록은 패키지 실제 엔트리 기준(section_entries와 같은 판정).
                || (name.starts_with("Contents/section") && name.ends_with(".xml"))
                || name.starts_with("BinData/")
            {
                continue;
            }
            let data = pkg.read_entry(name)?;
            // writer가 기본 템플릿으로 바이트 동일하게 재생성하는 엔트리는 잉여가
            // 아니다(캡처하면 의미 검증이 writer 기본값 슬롯 유무를 차이로 오인한다).
            if is_writer_default_entry(name, &data) {
                continue;
            }
            hwpx_extra_entries.push((name.clone(), data));
        }
    }

    // 문서 메타데이터 (content.hpf OPF — 최선 노력: 없거나 손상돼도 진단 계속)
    let content_hpf = pkg.read_entry_string("Contents/content.hpf").ok();
    let metadata = content_hpf
        .as_deref()
        .map(parse_content_meta)
        .unwrap_or_default();
    // OPF 매니페스트의 BinData (id, href) 매핑 — 전체 재작성 writer가 BinCollector
    // id를 원본 id로 시드해 원문 캡처 개체의 binaryItemIDRef가 어긋나지 않게 한다.
    let hwpx_bin_manifest = content_hpf
        .as_deref()
        .map(parse_bin_manifest)
        .unwrap_or_default();
    // writer가 재생성하지 않는 확장 파트의 매니페스트 항목 — content.hpf 재생성 시
    // 다시 등재해 raw-copy된 엔트리가 고아 파트가 되지 않게 한다.
    let hwpx_opf_extra_items = content_hpf
        .as_deref()
        .map(parse_opf_extra_items)
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
            hwpx_extra_entries,
            hwpx_bin_manifest,
            hwpx_opf_extra_items,
            hwpx_section_xmlns,
        },
        warnings,
    })
}

/// writer가 기본 템플릿으로 바이트 동일하게 재생성하는 META-INF 엔트리인가.
/// 이런 엔트리는 `hwpx_extra_entries`에 담지 않는다(재생성이 곧 보존).
fn is_writer_default_entry(name: &str, bytes: &[u8]) -> bool {
    let default = match name {
        "META-INF/container.rdf" => crate::write::templates::CONTAINER_RDF,
        "META-INF/container.xml" => crate::write::templates::CONTAINER_XML,
        "META-INF/manifest.xml" => crate::write::templates::MANIFEST_XML,
        _ => return false,
    };
    bytes == default.as_bytes()
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

/// content.hpf OPF 매니페스트의 모든 `<opf:item>`을 (id, href, media-type)으로 읽는다
/// (최선 노력). id/href가 없는 항목은 건너뛰고, media-type이 없으면 None이다.
/// 속성값은 원문(이스케이프 상태 그대로)을 유지해 재방출 시 이중 이스케이프를 피한다.
fn parse_opf_items(xml: &str) -> Vec<(String, String, Option<String>)> {
    use quick_xml::events::Event;

    let mut items = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.name().local_name().as_ref() != b"item" {
                    continue;
                }
                let mut id = None;
                let mut href = None;
                let mut mime = None;
                for attr in event.attributes().flatten() {
                    let value = String::from_utf8_lossy(&attr.value).into_owned();
                    match attr.key.local_name().as_ref() {
                        b"id" => id = Some(value),
                        b"href" => href = Some(value),
                        b"media-type" => mime = Some(value),
                        _ => {}
                    }
                }
                if let (Some(id), Some(href)) = (id, href) {
                    items.push((id, href, mime));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    items
}

/// content.hpf OPF 매니페스트에서 BinData 항목의 (id, href) 매핑을 읽는다(최선 노력).
///
/// 전체 재작성 writer가 BinCollector id를 원본 매니페스트 id로 시드하는 데 쓴다 —
/// 원문 캡처된 개체(hp:container 등) 안의 `binaryItemIDRef`가 재직렬화 후에도
/// 원본 바이트를 가리키게 한다. 파싱 실패·BinData 항목 없음이면 빈 벡터(호출자는
/// 기존 image{N} 할당 경로를 그대로 탄다).
pub fn parse_bin_manifest(xml: &str) -> Vec<(String, String)> {
    parse_opf_items(xml)
        .into_iter()
        .filter(|(_, href, _)| href.starts_with("BinData/"))
        .map(|(id, href, _)| (id, href))
        .collect()
}

/// OPF 매니페스트에서 writer가 재생성하지 않는 확장 파트 항목의 (id, href, media-type)을
/// 읽는다(최선 노력) — BinData·header·section·settings 외 항목(예: DocOptions/Layout.xml).
/// content.hpf 재생성 시 이 항목들을 다시 등재해 raw-copy된 패키지 엔트리가 매니페스트에서
/// 사라지는(고아 파트) 일을 막는다.
pub fn parse_opf_extra_items(xml: &str) -> Vec<(String, String, String)> {
    parse_opf_items(xml)
        .into_iter()
        .filter(|(_, href, _)| {
            !href.starts_with("BinData/")
                && href != "Contents/header.xml"
                && href != "settings.xml"
                && !(href.starts_with("Contents/section") && href.ends_with(".xml"))
        })
        .map(|(id, href, mime)| {
            (
                id,
                href,
                mime.unwrap_or_else(|| "application/octet-stream".to_string()),
            )
        })
        .collect()
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
