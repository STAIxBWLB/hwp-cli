//! 폰트 해석 체인.
//!
//! 해석 순서: 문서의 FACE_NAME 이름 → 대체 글꼴 이름 → 한국어 폴백
//! 목록 → 임의의 시스템 글꼴. **조용한 대체 금지** — 모든 해석 결과를
//! 리포트에 남긴다(픽셀 정확도가 폰트에 좌우되므로).

use std::collections::HashMap;
use std::sync::Arc;

use fontdb::{Database, Family, Query, Source, Weight};
use hwp_model::Document;
use sha2::{Digest as _, Sha256};

use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

pub const MAX_FONT_RESOLUTIONS: usize = 512;

/// 한국어 문서용 폴백 글꼴 (분류 불가 시, 우선순위순).
const FALLBACKS: &[&str] = &[
    "함초롬바탕",
    "함초롬돋움",
    "Apple SD Gothic Neo",
    "AppleGothic",
    "NanumGothic",
    "나눔고딕",
    "Noto Sans CJK KR",
    "Noto Sans KR",
];

/// 고딕(산세리프) 계열 폴백 — 요청 글꼴이 고딕/돋움/헤드라인일 때.
const GOTHIC_FALLBACKS: &[&str] = &[
    "함초롬돋움",
    "Apple SD Gothic Neo",
    "나눔고딕",
    "NanumGothic",
    "맑은 고딕",
    "Malgun Gothic",
    "AppleGothic",
    "Noto Sans CJK KR",
    "Noto Sans KR",
];

/// 명조(세리프) 계열 폴백 — 요청 글꼴이 명조/바탕/신명조일 때.
const SERIF_FALLBACKS: &[&str] = &[
    "함초롬바탕",
    "AppleMyungjo",
    "나눔명조",
    "NanumMyeongjo",
    "Batang",
    "바탕",
    "Noto Serif CJK KR",
    "Apple SD Gothic Neo",
];

#[derive(Clone, Copy, PartialEq)]
enum FontClass {
    Gothic,
    Serif,
}

/// 글꼴 이름으로 고딕/명조 계열을 추정한다(한국어 키워드 + 라틴 키워드).
/// 대체 폴백을 같은 계열로 골라 글리프 모양 차이를 줄인다(고딕→고딕, 명조→명조).
fn classify(name: &str) -> Option<FontClass> {
    let lower = name.to_ascii_lowercase();
    const GOTHIC: &[&str] = &["돋움", "돋음", "고딕", "헤드라인", "굴림"];
    const GOTHIC_L: &[&str] = &["gothic", "dotum", "gulim", "headline", "sans"];
    const SERIF: &[&str] = &["바탕", "명조", "신명조", "궁서"];
    const SERIF_L: &[&str] = &[
        "batang", "myungjo", "myeongjo", "gungsuh", "serif", "mincho",
    ];
    if GOTHIC.iter().any(|k| name.contains(k)) || GOTHIC_L.iter().any(|k| lower.contains(k)) {
        return Some(FontClass::Gothic);
    }
    if SERIF.iter().any(|k| name.contains(k)) || SERIF_L.iter().any(|k| lower.contains(k)) {
        return Some(FontClass::Serif);
    }
    None
}

pub struct LoadedFont {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    /// 해석된 패밀리 이름 (리포트용)
    pub family: String,
}

/// A resolved face together with the shaping effect still required for it.
///
/// A bold request uses a real `Weight::BOLD` face when one exists. The renderer
/// only applies its synthetic stroke when the selected face is not an exact
/// bold face, preserving the previous faux-bold fallback behavior.
#[derive(Clone)]
pub(crate) struct FontSelection {
    pub font: Arc<LoadedFont>,
    pub faux_bold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FontCacheKey {
    Family {
        requested: String,
        alt: Option<String>,
        bold: bool,
    },
    Coverage {
        character: char,
        bold: bool,
    },
}

/// 렌더 중 실제로 관측한 글꼴 해석 결과.
///
/// 기존의 사람이 읽는 `report` 문자열과 별개인 안정된 기계 판독 표면이다. 인증기는
/// 이 값만 사용하며 보고 문자열을 파싱하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontResolution {
    pub requested: String,
    pub resolved: Option<String>,
    pub resolved_sha256: Option<String>,
    pub resolved_face_index: Option<u32>,
    pub outcome: FontResolutionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontResolutionOutcome {
    Matched,
    Substituted,
    Missing,
    /// 주 글꼴에 특정 글리프가 없어 문자 단위 폴백을 사용한 경우.
    CoverageSubstituted,
}

pub struct FontStore {
    db: Database,
    /// fontdb ID → 로드된 폰트
    loaded: HashMap<fontdb::ID, Arc<LoadedFont>>,
    /// (요청 이름, 대체 이름, weight) → 해석 결과 캐시.
    resolved: HashMap<FontCacheKey, Option<FontSelection>>,
    /// 문서 문자열을 보관하지 않는 source-bounded 해석 진단.
    pub issues: RenderIssueAccumulator,
    /// 기계 판독 가능한 해석 결과. 같은 요청은 캐시되므로 한 번만 기록된다.
    pub resolutions: Vec<FontResolution>,
    pub resolutions_complete: bool,
}

impl FontStore {
    pub fn new() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        Self::from_database(db)
    }

    /// 시스템 글꼴을 전혀 읽지 않는 격리 resolver. 인증 경로는 명시적으로 지정한
    /// 디렉터리만 `load_dir`로 추가해 환경별 ambient fallback을 차단한다.
    pub fn new_isolated() -> Self {
        Self::from_database(Database::new())
    }

    fn from_database(db: Database) -> Self {
        Self {
            db,
            loaded: HashMap::new(),
            resolved: HashMap::new(),
            issues: RenderIssueAccumulator::new(),
            resolutions: Vec::new(),
            resolutions_complete: true,
        }
    }

    /// 추가 폰트 디렉터리 로드 (`--font-dir`).
    pub fn load_dir(&mut self, dir: &std::path::Path) {
        self.db.load_fonts_dir(dir);
        self.resolved.clear();
    }

    /// 한 파일을 호출 순서대로 적재한다. 인증기는 검증된 manifest 정렬 순서를 그대로
    /// 사용해 디렉터리 열거 순서가 face 선택에 영향을 주지 않게 한다.
    pub fn load_file(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let result = self.db.load_font_file(path);
        self.resolved.clear();
        result
    }

    /// 문서의 (언어 슬롯, 글꼴 ID)를 실제 폰트로 해석한다.
    pub fn resolve(
        &mut self,
        doc: &Document,
        lang_slot: usize,
        face_id: u16,
    ) -> Option<Arc<LoadedFont>> {
        self.resolve_with_style(doc, lang_slot, face_id, false)
    }

    /// Resolve a face for a bold or regular shaping request.
    ///
    /// This is the public style-aware wrapper. Callers that also need to know
    /// whether synthetic bold is required should use the crate-internal
    /// `resolve_selection` helper.
    pub fn resolve_with_style(
        &mut self,
        doc: &Document,
        lang_slot: usize,
        face_id: u16,
        bold: bool,
    ) -> Option<Arc<LoadedFont>> {
        self.resolve_selection(doc, lang_slot, face_id, bold)
            .map(|selection| selection.font)
    }

    /// Resolve a face and retain the faux-bold decision for shaping.
    pub(crate) fn resolve_selection(
        &mut self,
        doc: &Document,
        lang_slot: usize,
        face_id: u16,
        bold: bool,
    ) -> Option<FontSelection> {
        let face = doc.header.fonts.get(lang_slot)?.get(face_id as usize);
        let requested = face.map(|f| f.name.clone()).unwrap_or_default();
        let alt = face.and_then(|f| f.alt_name.clone());
        let cache_key = FontCacheKey::Family {
            requested: requested.clone(),
            alt: alt.clone(),
            bold,
        };

        if let Some(cached) = self.resolved.get(&cache_key) {
            return cached.clone();
        }

        let mut candidates: Vec<&str> = Vec::new();
        if !requested.is_empty() {
            candidates.push(&requested);
        }
        if let Some(alt) = &alt {
            candidates.push(alt);
        }
        // 요청 글꼴 계열(고딕/명조)을 같은 계열 폴백으로 — 모양 차이 최소화.
        let class = classify(&requested).or_else(|| alt.as_deref().and_then(classify));
        candidates.extend(match class {
            Some(FontClass::Gothic) => GOTHIC_FALLBACKS,
            Some(FontClass::Serif) => SERIF_FALLBACKS,
            None => FALLBACKS,
        });

        let mut result = None;
        for name in &candidates {
            if let Some(selection) = self.try_family_with_style(name, bold) {
                let font = &selection.font;
                if *name != requested {
                    self.issues.push(
                        RenderIssueCode::FontSubstituted,
                        resolution_detail(&requested, name, bold),
                    );
                    self.record_resolution(FontResolution {
                        requested: requested.clone(),
                        resolved: Some(font.family.clone()),
                        resolved_sha256: Some(sha256_hex(&font.data)),
                        resolved_face_index: Some(font.index),
                        outcome: FontResolutionOutcome::Substituted,
                    });
                } else {
                    self.issues.push(
                        RenderIssueCode::FontMatched,
                        resolution_detail(&requested, name, bold),
                    );
                    self.record_resolution(FontResolution {
                        requested: requested.clone(),
                        resolved: Some(font.family.clone()),
                        resolved_sha256: Some(sha256_hex(&font.data)),
                        resolved_face_index: Some(font.index),
                        outcome: FontResolutionOutcome::Matched,
                    });
                }
                result = Some(selection);
                break;
            }
        }
        // 최후 수단: 시스템 기본 산세리프 (CI 등 한국어 폰트 부재 환경)
        if result.is_none()
            && let Some(id) = self.db.query(&Query {
                families: &[Family::SansSerif],
                weight: requested_weight(bold),
                ..Query::default()
            })
            && let Some(selection) = self.load_selection_by_id(id, bold)
        {
            let font = &selection.font;
            self.issues.push(
                RenderIssueCode::FontSubstituted,
                resolution_detail(&requested, &font.family, bold),
            );
            self.record_resolution(FontResolution {
                requested: requested.clone(),
                resolved: Some(font.family.clone()),
                resolved_sha256: Some(sha256_hex(&font.data)),
                resolved_face_index: Some(font.index),
                outcome: FontResolutionOutcome::Substituted,
            });
            result = Some(selection);
        }
        if result.is_none() {
            self.issues
                .push(RenderIssueCode::FontMissing, requested.as_bytes());
            self.record_resolution(FontResolution {
                requested: requested.clone(),
                resolved: None,
                resolved_sha256: None,
                resolved_face_index: None,
                outcome: FontResolutionOutcome::Missing,
            });
        }
        self.resolved.insert(cache_key, result.clone());
        result
    }

    /// 주어진 문자에 (.notdef 아닌) 글리프가 있는 커버리지 폴백 글꼴을 찾는다
    /// (함초롬 우선 → CJK 폴백). 주 글꼴이 특정 글자를 못 가질 때 그 글자만 이
    /// 글꼴로 바꿔 두부(□) 글리프를 방지한다. 문자별 결과를 캐시한다.
    pub fn font_covering(&mut self, c: char) -> Option<Arc<LoadedFont>> {
        self.font_covering_with_style(c, false)
    }

    /// Resolve a coverage fallback for a regular or bold shaping request.
    pub fn font_covering_with_style(&mut self, c: char, bold: bool) -> Option<Arc<LoadedFont>> {
        self.font_covering_selection(c, bold)
            .map(|selection| selection.font)
    }

    /// Resolve a coverage fallback and retain the faux-bold decision for shaping.
    pub(crate) fn font_covering_selection(&mut self, c: char, bold: bool) -> Option<FontSelection> {
        const COVERAGE_FALLBACKS: &[&str] = &[
            "함초롬바탕",
            "HCR Batang",
            "함초롬돋움",
            "HCR Dotum",
            "Noto Serif CJK KR",
            "Noto Sans CJK KR",
            "NanumMyeongjo",
            "NanumGothic",
            "Apple SD Gothic Neo",
            "AppleMyungjo",
        ];
        let key = FontCacheKey::Coverage { character: c, bold };
        if let Some(cached) = self.resolved.get(&key) {
            return cached.clone();
        }
        let mut result = None;
        for name in COVERAGE_FALLBACKS {
            if let Some(selection) = self.try_family_with_style(name, bold)
                && font_has_char(&selection.font, c)
            {
                let font = &selection.font;
                self.record_resolution(FontResolution {
                    requested: "coverage_fallback".to_string(),
                    resolved: Some(font.family.clone()),
                    resolved_sha256: Some(sha256_hex(&font.data)),
                    resolved_face_index: Some(font.index),
                    outcome: FontResolutionOutcome::CoverageSubstituted,
                });
                result = Some(selection);
                break;
            }
        }
        if result.is_none() {
            self.record_resolution(FontResolution {
                requested: "coverage_fallback".to_string(),
                resolved: None,
                resolved_sha256: None,
                resolved_face_index: None,
                outcome: FontResolutionOutcome::Missing,
            });
        }
        self.resolved.insert(key, result.clone());
        result
    }

    fn try_family_with_style(&mut self, name: &str, bold: bool) -> Option<FontSelection> {
        let id = self.db.query(&Query {
            families: &[Family::Name(name)],
            weight: requested_weight(bold),
            ..Query::default()
        })?;
        self.load_selection_by_id(id, bold)
    }

    fn record_resolution(&mut self, resolution: FontResolution) {
        if self
            .resolutions
            .iter()
            .any(|existing| existing == &resolution)
        {
            return;
        }
        if self.resolutions.len() >= MAX_FONT_RESOLUTIONS {
            self.issues.push_once(
                RenderIssueCode::FontResolutionBudgetExceeded,
                b"font_resolution_budget_exceeded",
            );
            self.resolutions_complete = false;
            return;
        }
        self.resolutions.push(resolution);
    }

    fn load_by_id(&mut self, id: fontdb::ID) -> Option<Arc<LoadedFont>> {
        if let Some(loaded) = self.loaded.get(&id) {
            return Some(loaded.clone());
        }
        let face = self.db.face(id)?;
        let index = face.index;
        let family = face
            .families
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        let data: Arc<Vec<u8>> = match &face.source {
            Source::File(path) => Arc::new(std::fs::read(path).ok()?),
            Source::Binary(bin) => Arc::new(bin.as_ref().as_ref().to_vec()),
            Source::SharedFile(_, bin) => Arc::new(bin.as_ref().as_ref().to_vec()),
        };
        let loaded = Arc::new(LoadedFont {
            data,
            index,
            family,
        });
        self.loaded.insert(id, loaded.clone());
        Some(loaded)
    }

    fn load_selection_by_id(&mut self, id: fontdb::ID, bold: bool) -> Option<FontSelection> {
        let actual_weight = self.db.face(id)?.weight;
        let font = self.load_by_id(id)?;
        Some(FontSelection {
            font,
            faux_bold: bold && actual_weight != Weight::BOLD,
        })
    }
}

fn requested_weight(bold: bool) -> Weight {
    if bold { Weight::BOLD } else { Weight::NORMAL }
}

fn resolution_detail(requested: &str, resolved: &str, bold: bool) -> String {
    if bold {
        format!("{requested}\0{resolved}\0bold")
    } else if requested == resolved {
        requested.to_string()
    } else {
        format!("{requested}\0{resolved}")
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Default for FontStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 글꼴이 해당 문자에 (.notdef 아닌) 글리프를 갖는지.
fn font_has_char(font: &LoadedFont, c: char) -> bool {
    rustybuzz::ttf_parser::Face::parse(&font.data, font.index)
        .ok()
        .and_then(|f| f.glyph_index(c))
        .is_some_and(|g| g.0 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::{Document, FaceName};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A 400-byte outline font from ttf-parser's public test corpus. The test
    // builds two named faces from it, changing only the family name and OS/2
    // weight, so no host font directory is consulted.
    const DEMO_TTF_HEX: &str = "000100000007004000020030636d617000090076000001000000002c676c7966f1cb6698000001340000005c68656164f235ddf80000007c0000003668686561066100ca000000b400000024686d74780474006a000000f8000000086c6f6361002e00140000012c000000066d6178700005000b000000d8000000200001000000010000f59c29445f0f3cf5000203e800000000b492f40000000000dc2fa65c00060000025802bc000000030002000000000000000100000400fe70000002580006ffff0258000100000000000000000000000000000002000100000002000b00020000000000000000000000000000000000000000000002580064021c000600000001000000030000000c00040020000000040004000100000041ffff00000041ffffffc000010000000000000014002e0000000200640000025802bc00030007000033112111252111216401f4fe3401a4fe5c02bcfd4428026c000200060000021d02900002000a00001333030113331323272307adc463fef8da60dd593eef42010b0140fdb50290fd70c8c800";
    static TEMP_FONT_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn hex_bytes(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect()
    }

    fn be_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn be_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn align4(bytes: &mut Vec<u8>) {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }

    fn table_checksum(bytes: &[u8]) -> u32 {
        bytes
            .chunks(4)
            .map(|chunk| {
                let mut word = [0; 4];
                word[..chunk.len()].copy_from_slice(chunk);
                u32::from_be_bytes(word)
            })
            .fold(0u32, u32::wrapping_add)
    }

    fn weight_font(family: &str, weight: u16) -> Vec<u8> {
        let base = hex_bytes(DEMO_TTF_HEX);
        let table_count = usize::from(be_u16(&base, 4));
        let mut tables = Vec::<([u8; 4], Vec<u8>)>::with_capacity(table_count + 2);
        for index in 0..table_count {
            let record = 12 + index * 16;
            let mut tag = [0; 4];
            tag.copy_from_slice(&base[record..record + 4]);
            let offset = be_u32(&base, record + 8) as usize;
            let length = be_u32(&base, record + 12) as usize;
            tables.push((tag, base[offset..offset + length].to_vec()));
        }

        let family_utf16: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let post_script = family.replace(' ', "");
        let post_script_utf16: Vec<u8> = post_script
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        let mut name = Vec::new();
        push_u16(&mut name, 0); // format
        push_u16(&mut name, 2); // count
        push_u16(&mut name, 30); // string offset: 6 + 2 * 12
        for (name_id, bytes) in [(1u16, &family_utf16), (6u16, &post_script_utf16)] {
            push_u16(&mut name, 3); // Windows Unicode BMP
            push_u16(&mut name, 1);
            push_u16(&mut name, 0x0409);
            push_u16(&mut name, name_id);
            push_u16(&mut name, bytes.len() as u16);
            push_u16(
                &mut name,
                if name_id == 1 {
                    0
                } else {
                    family_utf16.len() as u16
                },
            );
        }
        name.extend_from_slice(&family_utf16);
        name.extend_from_slice(&post_script_utf16);
        tables.push((*b"name", name));

        let mut os2 = vec![0; 78];
        os2[0..2].copy_from_slice(&0u16.to_be_bytes()); // version
        os2[4..6].copy_from_slice(&weight.to_be_bytes()); // usWeightClass
        os2[6..8].copy_from_slice(&5u16.to_be_bytes()); // usWidthClass: normal
        tables.push((*b"OS/2", os2));
        tables.sort_by_key(|(tag, _)| *tag);

        let count = tables.len() as u16;
        let mut out =
            Vec::with_capacity(256 + tables.iter().map(|(_, data)| data.len()).sum::<usize>());
        push_u32(&mut out, 0x0001_0000);
        push_u16(&mut out, count);
        push_u16(&mut out, 128); // searchRange for 16 records
        push_u16(&mut out, 3); // entrySelector
        push_u16(&mut out, count * 16 - 128); // rangeShift
        out.resize(12 + usize::from(count) * 16, 0);

        for (index, (tag, data)) in tables.iter().enumerate() {
            align4(&mut out);
            let offset = out.len() as u32;
            out.extend_from_slice(data);
            let record = 12 + index * 16;
            out[record..record + 4].copy_from_slice(tag);
            out[record + 4..record + 8].copy_from_slice(&table_checksum(data).to_be_bytes());
            out[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
            out[record + 12..record + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
        }
        out
    }

    fn temp_font_path(label: &str) -> PathBuf {
        let serial = TEMP_FONT_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "hwp-render-font-weight-{}-{serial}-{label}.ttf",
            std::process::id()
        ))
    }

    fn test_document(family: &str) -> Document {
        let mut document = Document::default();
        document.header.fonts[0].push(FaceName {
            name: family.to_string(),
            ..FaceName::default()
        });
        document
    }

    #[test]
    fn more_than_512_distinct_font_requests_are_bounded_and_fatal() {
        let mut store = FontStore::new_isolated();
        for index in 0..=MAX_FONT_RESOLUTIONS {
            store.record_resolution(FontResolution {
                requested: format!("font-{index}"),
                resolved: None,
                resolved_sha256: None,
                resolved_face_index: None,
                outcome: FontResolutionOutcome::Missing,
            });
        }
        assert_eq!(store.resolutions.len(), MAX_FONT_RESOLUTIONS);
        assert!(!store.resolutions_complete);
        let report = store.issues.finish();
        assert_eq!(report.issue_count, 1);
        assert_eq!(
            report.issues[0].code,
            RenderIssueCode::FontResolutionBudgetExceeded
        );
        assert!(report.has_required_failure());
    }

    #[test]
    fn isolated_weight_resolution_selects_regular_and_bold_bytes_deterministically() {
        let family = "HWP Weight Fixture";
        let regular_bytes = weight_font(family, 400);
        let bold_bytes = weight_font(family, 700);
        let regular_hash = sha256_hex(&regular_bytes);
        let bold_hash = sha256_hex(&bold_bytes);
        assert_ne!(regular_hash, bold_hash);

        let regular_path = temp_font_path("regular");
        let bold_path = temp_font_path("bold");
        std::fs::write(&regular_path, &regular_bytes).unwrap();
        std::fs::write(&bold_path, &bold_bytes).unwrap();

        let document = test_document(family);
        let mut store = FontStore::new_isolated();
        store.load_file(&regular_path).unwrap();
        store.load_file(&bold_path).unwrap();
        let regular = store.resolve(&document, 0, 0).unwrap();
        let bold = store.resolve_with_style(&document, 0, 0, true).unwrap();
        assert_eq!(sha256_hex(&regular.data), regular_hash);
        assert_eq!(sha256_hex(&bold.data), bold_hash);
        assert_eq!(store.resolutions.len(), 2);
        assert!(store.resolutions.iter().any(|resolution| {
            resolution.resolved_sha256.as_deref() == Some(regular_hash.as_str())
                && resolution.resolved_face_index == Some(0)
        }));
        assert!(store.resolutions.iter().any(|resolution| {
            resolution.resolved_sha256.as_deref() == Some(bold_hash.as_str())
                && resolution.resolved_face_index == Some(0)
        }));

        // Reversing the explicitly loaded manifest order must not change the
        // CSS-style weight match because the faces have distinct weights.
        let mut reversed = FontStore::new_isolated();
        reversed.load_file(&bold_path).unwrap();
        reversed.load_file(&regular_path).unwrap();
        let reversed_regular = reversed.resolve(&document, 0, 0).unwrap();
        let reversed_bold = reversed.resolve_with_style(&document, 0, 0, true).unwrap();
        assert_eq!(sha256_hex(&reversed_regular.data), regular_hash);
        assert_eq!(sha256_hex(&reversed_bold.data), bold_hash);

        // With no exact bold face the selected regular bytes remain the
        // fallback, and shaping can still apply faux-bold.
        let mut regular_only = FontStore::new_isolated();
        regular_only.load_file(&regular_path).unwrap();
        let selection = regular_only
            .resolve_selection(&document, 0, 0, true)
            .unwrap();
        assert_eq!(sha256_hex(&selection.font.data), regular_hash);
        assert!(selection.faux_bold);

        let _ = std::fs::remove_file(regular_path);
        let _ = std::fs::remove_file(bold_path);
    }
}
