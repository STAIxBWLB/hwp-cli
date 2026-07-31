//! 폰트 해석 체인.
//!
//! 해석 순서: 문서의 FACE_NAME 이름 → 대체 글꼴 이름 → 한국어 폴백
//! 목록 → 임의의 시스템 글꼴. **조용한 대체 금지** — 모든 해석 결과를
//! 리포트에 남긴다(픽셀 정확도가 폰트에 좌우되므로).

use std::collections::HashMap;
use std::sync::Arc;

use fontdb::{Database, Family, Query, Source};
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
    /// (요청 이름) → 해석 결과 캐시
    resolved: HashMap<String, Option<Arc<LoadedFont>>>,
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
    }

    /// 한 파일을 호출 순서대로 적재한다. 인증기는 검증된 manifest 정렬 순서를 그대로
    /// 사용해 디렉터리 열거 순서가 face 선택에 영향을 주지 않게 한다.
    pub fn load_file(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.db.load_font_file(path)
    }

    /// 문서의 (언어 슬롯, 글꼴 ID)를 실제 폰트로 해석한다.
    pub fn resolve(
        &mut self,
        doc: &Document,
        lang_slot: usize,
        face_id: u16,
    ) -> Option<Arc<LoadedFont>> {
        let face = doc.header.fonts.get(lang_slot)?.get(face_id as usize);
        let requested = face.map(|f| f.name.clone()).unwrap_or_default();
        let alt = face.and_then(|f| f.alt_name.clone());
        let cache_key = format!("{requested}\0{}", alt.as_deref().unwrap_or(""));

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
            if let Some(font) = self.try_family(name) {
                if *name != requested {
                    self.issues.push(
                        RenderIssueCode::FontSubstituted,
                        format!("{}\0{}", requested, name),
                    );
                    self.record_resolution(FontResolution {
                        requested: requested.clone(),
                        resolved: Some(font.family.clone()),
                        resolved_sha256: Some(sha256_hex(&font.data)),
                        resolved_face_index: Some(font.index),
                        outcome: FontResolutionOutcome::Substituted,
                    });
                } else {
                    self.issues
                        .push(RenderIssueCode::FontMatched, requested.as_bytes());
                    self.record_resolution(FontResolution {
                        requested: requested.clone(),
                        resolved: Some(font.family.clone()),
                        resolved_sha256: Some(sha256_hex(&font.data)),
                        resolved_face_index: Some(font.index),
                        outcome: FontResolutionOutcome::Matched,
                    });
                }
                result = Some(font);
                break;
            }
        }
        // 최후 수단: 시스템 기본 산세리프 (CI 등 한국어 폰트 부재 환경)
        if result.is_none()
            && let Some(id) = self.db.query(&Query {
                families: &[Family::SansSerif],
                ..Query::default()
            })
            && let Some(font) = self.load_by_id(id)
        {
            self.issues.push(
                RenderIssueCode::FontSubstituted,
                format!("{}\0{}", requested, font.family),
            );
            self.record_resolution(FontResolution {
                requested: requested.clone(),
                resolved: Some(font.family.clone()),
                resolved_sha256: Some(sha256_hex(&font.data)),
                resolved_face_index: Some(font.index),
                outcome: FontResolutionOutcome::Substituted,
            });
            result = Some(font);
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
        let key = format!("\u{1}cover:{}", c as u32);
        if let Some(cached) = self.resolved.get(&key) {
            return cached.clone();
        }
        let mut result = None;
        for name in COVERAGE_FALLBACKS {
            if let Some(font) = self.try_family(name)
                && font_has_char(&font, c)
            {
                self.record_resolution(FontResolution {
                    requested: "coverage_fallback".to_string(),
                    resolved: Some(font.family.clone()),
                    resolved_sha256: Some(sha256_hex(&font.data)),
                    resolved_face_index: Some(font.index),
                    outcome: FontResolutionOutcome::CoverageSubstituted,
                });
                result = Some(font);
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

    fn try_family(&mut self, name: &str) -> Option<Arc<LoadedFont>> {
        let id = self.db.query(&Query {
            families: &[Family::Name(name)],
            ..Query::default()
        })?;
        self.load_by_id(id)
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
}
