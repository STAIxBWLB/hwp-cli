//! 최상위 문서 모델.

use serde::{Deserialize, Serialize};

use crate::control::{BinRef, Control, SectionDef};
use crate::header::DocHeader;
use crate::paragraph::Paragraph;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// 출처 정보 (원본 포맷/버전 등)
    pub meta: DocMeta,
    /// 문서 속성 (제목/지은이/주제/키워드). hwp5 `\x05HwpSummaryInformation`,
    /// hwpx `Contents/content.hpf`(OPF dc:*)에 대응.
    #[serde(default)]
    pub metadata: Metadata,
    pub header: DocHeader,
    pub sections: Vec<Section>,
    /// 첨부 바이너리 (이미지 등). 키는 원본 컨테이너 항목 이름
    /// (hwp5: "BIN0001.png", hwpx: "BinData/image1.png").
    pub bin_streams: Vec<BinStream>,
    /// hwpx `settings.xml` 원문(앱 설정·캐럿 위치). IR 경유 되쓰기에서 원본을
    /// 그대로 통과시키기 위한 슬롯. hwp5 출신 문서는 `None`(쓰기 시 기본 상수).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwpx_settings_xml: Option<String>,
    /// hwpx `version.xml` 원문(버전 메타). IR 경유 되쓰기에서 원본을 그대로
    /// 통과시키기 위한 슬롯. hwp5 출신 문서는 `None`(쓰기 시 기본 상수).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwpx_version_xml: Option<String>,
    /// Raw HWPX preview image bytes. The payload is intentionally excluded from
    /// JSON, like [`BinStream::data`], and exists only for same-format pass-through.
    #[serde(skip)]
    pub hwpx_preview_image: Option<Vec<u8>>,
    /// hwp5 `/XMLTemplate` 스토리지의 스트림 원문(스트림 CFB 경로 + 압축 미해제
    /// 바이트). XML 스키마 바인딩 문서(전자정부 서식 등, §3.2.10)의 부속 파트를 IR
    /// 경유 되쓰기에서 그대로 통과시키는 슬롯. 내용은 해석하지 않는다. hwpx 출신·해당
    /// 스토리지가 없는 hwp5 문서는 빈 벡터.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwp5_xml_template: Vec<(String, Vec<u8>)>,
    /// hwp5 `/DocHistory` 스토리지의 스트림 원문(스트림 CFB 경로 + 압축 미해제
    /// 바이트). 문서 이력(§3.2.11·§4.4)을 되쓰기에서 그대로 통과시키는 슬롯. 내용은
    /// 해석하지 않는다(§4.4 파서는 범위 밖). hwpx 출신·이력이 없는 문서는 빈 벡터.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwp5_doc_history: Vec<(String, Vec<u8>)>,
    /// hwpx 원본 패키지 중 writer가 재생성하지 않는 엔트리(원본 순서·바이트 보존).
    /// 예: DocOptions, 스크립트, 추가 Preview, 원본 META-INF/*. hwpx 출신이 아니면 비어 있다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_extra_entries: Vec<(String, Vec<u8>)>,
    /// hwpx 원본 `Contents/content.hpf`의 OPF manifest 중 BinData 항목의
    /// (id, href) 매핑. 전체 재작성 경로에서 BinCollector id를 원본 manifest id로
    /// 시드해, 원문 캡처된 개체 안의 `binaryItemIDRef`가 원본 바이트를 계속
    /// 가리키게 한다. hwp5 출신·합성 문서는 빈 벡터(기존 image{N} 할당 유지).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_bin_manifest: Vec<(String, String)>,
}

/// 문서 수준 메타데이터 (요약 정보 / OPF 메타).
///
/// 모든 필드가 `Option`이며 `#[serde(default)]`이라 JSON 왕복 호환을 깨지 않는다.
/// 비어 있으면 쓰기 시 빈 문자열로 직렬화(표본 구조 유지).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// 설명(comments). hwp5 요약정보 PIDSI 6, hwpx OPF `meta[name=description]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 마지막 저장자. hwp5 요약정보 PIDSI 8, hwpx OPF `meta[name=lastsaveby]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_saved_by: Option<String>,
    /// 작성 일시. hwp5 요약정보 PIDSI 12(VT_FILETIME)의 **raw u64** 그대로 보존한다
    /// (1601-01-01 UTC 기준 100ns 단위). 무손실 우선 — 시간대 변환·문자열화 금지.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_time: Option<u64>,
    /// 수정(마지막 저장) 일시. hwp5 요약정보 PIDSI 13(VT_FILETIME)의 **raw u64** 그대로 보존.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modify_time: Option<u64>,
}

impl Metadata {
    /// 모든 필드가 비었는가.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.author.is_none()
            && self.subject.is_none()
            && self.keywords.is_none()
            && self.description.is_none()
            && self.last_saved_by.is_none()
            && self.create_time.is_none()
            && self.modify_time.is_none()
    }
}

/// 첨부 바이너리 하나. 바이트는 JSON 직렬화에서 제외한다 (L2 출력 비대 방지).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinStream {
    pub name: String,
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Document {
    /// Picture의 BinRef를 실제 바이트로 해석한다.
    pub fn resolve_bin(&self, bin_ref: &BinRef) -> Option<&[u8]> {
        match bin_ref {
            BinRef::Id(id) => {
                let item = self.header.bin_data.get((id.0 as usize).checked_sub(1)?)?;
                let storage_id = item.storage_id?;
                let ext = item.extension.as_deref().unwrap_or("");
                let name = format!("BIN{storage_id:04X}.{ext}");
                self.bin_streams
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(&name))
                    .map(|s| s.data.as_slice())
            }
            BinRef::ItemRef(item) => self
                .bin_streams
                .iter()
                .find(|s| {
                    s.name == *item
                        || s.name.ends_with(&format!("/{item}"))
                        || s.name
                            .rsplit('/')
                            .next()
                            .and_then(|f| f.split('.').next())
                            .is_some_and(|stem| stem == item)
                })
                .map(|s| s.data.as_slice()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocMeta {
    /// "hwp5" | "hwpx"
    pub source_format: String,
    /// 원본 파일 버전 (예: "5.1.0.1")
    pub source_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub paragraphs: Vec<Paragraph>,
    /// 문단이 아닌 최상위 레코드 (잘 형성된 파일에서는 비어 있음)
    pub extras: Vec<crate::opaque::OpaqueRecord>,
}

impl Section {
    /// 이 구역의 구역 정의 컨트롤 (보통 첫 문단의 첫 컨트롤).
    pub fn section_def(&self) -> Option<&SectionDef> {
        self.paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::SectionDef(sd) => Some(sd),
                _ => None,
            })
    }
}
