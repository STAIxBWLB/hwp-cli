//! 문서 헤더 — ID 참조 테이블 일체 (HWP DocInfo / HWPX header.xml에 대응).

use serde::{Deserialize, Serialize};

use crate::ids::{CharShapeId, ParaShapeId};
use crate::opaque::{OpaqueRecord, hex_bytes};

/// 언어 슬롯 수 (한글/영문/한자/일어/외국어/기호/사용자).
pub const LANG_COUNT: usize = 7;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DocHeader {
    pub properties: DocumentProperties,
    /// 언어 슬롯별 글꼴 목록
    pub fonts: [Vec<FaceName>; LANG_COUNT],
    pub bin_data: Vec<BinDataItem>,
    /// 테두리/배경. 참조는 1-기반(BorderFillId 1 = 첫 항목) 관례.
    pub border_fills: Vec<BorderFill>,
    pub char_shapes: Vec<CharShape>,
    pub tab_defs: Vec<RawEntry>,
    /// `tab_defs`와 병렬인 탭 정의 의미 파싱(위치·종류·채움). 없으면 비어 있음.
    /// hwp5 raw 왕복(identity)에는 관여하지 않고 hwpx 왕복·렌더 참고용이다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tab_stops: Vec<TabDef>,
    pub numberings: Vec<RawEntry>,
    pub bullets: Vec<RawEntry>,
    /// 렌더 전용: `bullets`와 병렬인 글머리 문자(없으면 비어 있음 — 기본 `•`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bullet_chars: Vec<char>,
    /// 렌더 전용: `numberings`와 병렬인 수준별 번호 형식(없으면 십진 기본).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numbering_levels: Vec<Vec<NumLevel>>,
    pub para_shapes: Vec<ParaShape>,
    pub styles: Vec<Style>,
    /// ID_MAPPINGS 원본 카운트 배열 (버전별 길이 보존 — 쓰기 시 유도값과 대조)
    #[serde(default)]
    pub id_mappings_counts: Vec<u32>,
    /// ID_MAPPINGS 자식 중 미해석 레코드 (메모 모양 등 — 위치: 테이블들 뒤)
    #[serde(default)]
    pub id_extras: Vec<OpaqueRecord>,
    /// DocInfo 최상위 수준의 미해석 레코드 (DOC_DATA, 호환 설정 등)
    pub extras: Vec<OpaqueRecord>,
}

/// DOCUMENT_PROPERTIES (26바이트).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentProperties {
    pub section_count: u16,
    /// 시작 번호: 쪽/각주/미주/그림/표/수식
    pub start_numbers: [u16; 6],
    /// 캐럿 위치: 리스트 ID / 문단 ID / 문단 내 위치
    pub caret: (u32, u32, u32),
}

/// FACE_NAME — 글꼴 하나.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceName {
    pub attr: u8,
    pub name: String,
    /// 대체 글꼴 (attr bit7)
    pub alt_kind: Option<u8>,
    pub alt_name: Option<String>,
    /// PANOSE 분류 10바이트 (attr bit6)
    pub panose: Option<[u8; 10]>,
    /// 기본 글꼴 이름 (attr bit5)
    pub default_name: Option<String>,
    /// OWPML typeInfo 요소 원문 (hwpx 왕복 보존용)
    #[serde(default)]
    pub type_info: Option<String>,
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

/// CHAR_SHAPE — 문자 모양. 알려진 prefix + tail 보존.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharShape {
    /// 언어 슬롯별 글꼴 ID
    pub face_ids: [u16; LANG_COUNT],
    /// 장평 % (50~200)
    pub ratios: [u8; LANG_COUNT],
    /// 자간 % (-50~50)
    pub spacings: [i8; LANG_COUNT],
    /// 상대 크기 % (10~250)
    pub rel_sizes: [u8; LANG_COUNT],
    /// 글자 위치(첨자 오프셋) % (-100~100)
    pub offsets: [i8; LANG_COUNT],
    /// 기준 크기 (HWPUNIT — 10pt = 1000)
    pub base_size: i32,
    /// 속성 비트 (기울임/굵게/밑줄 등). 취소선 비트(18~20)는 DIFFSPEC라 신뢰하지 않음 — `strike` 참조.
    pub attr: u32,
    /// 취소선 표시 여부 (의미 플래그). HWP5는 raw 비트가 불신뢰라 항상 false,
    /// HWPX는 visible `<hp:strikeout>`(NONE/3D 제외)일 때만 true. 바이너리에 쓰지 않음.
    #[serde(default)]
    pub strike: bool,
    pub shadow_gap: (i8, i8),
    /// COLORREF (0x00BBGGRR)
    pub text_color: u32,
    pub underline_color: u32,
    /// 밑줄 모양 코드(테두리선 종류계: 0=미지정→SOLID, 1=SOLID, 2=DASH …).
    /// hwpx `<hh:underline shape>` 왕복 보존용(hwp5는 미사용 → 0).
    #[serde(default)]
    pub underline_shape: u8,
    pub shade_color: u32,
    pub shadow_color: u32,
    /// 글자 테두리/배경 참조 (1-기반, 0 = 미지정)
    #[serde(default)]
    pub border_fill_id: u16,
    /// 버전별 추가 필드 (테두리채움 ID 5.0.2.1+, 취소선 색 5.0.3.0+)
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

impl CharShape {
    pub fn is_bold(&self) -> bool {
        self.attr & (1 << 1) != 0
    }

    pub fn is_italic(&self) -> bool {
        self.attr & 1 != 0
    }

    /// 밑줄 종류 (bits 2~3): 0 없음, 1 글자 아래, 3 글자 위.
    pub fn underline_kind(&self) -> u8 {
        ((self.attr >> 2) & 0x3) as u8
    }

    pub fn has_underline(&self) -> bool {
        self.underline_kind() == 1
    }

    /// Returns whether the underline is above the text (`kind == 3`).
    pub fn has_underline_above(&self) -> bool {
        self.underline_kind() == 3
    }

    /// Returns the zero-based underline shape from bits 4..=7.
    ///
    /// Values follow hwplib's `BorderType2` ordering (HWP 5.0 revision 1.2,
    /// table 35 family):
    /// 0 Solid, 1 Dash, 2 Dot, 3 DashDot, 4 DashDotDot, 5 LongDash, 6 CircleDot,
    /// 7 Double, 8 ThinThick, 9 ThickThin, 10 ThinThickThin, 11 Wave, 12 DoubleWave.
    /// Values 13..=15 are 3D variants and degrade to solid rendering.
    pub fn underline_shape_code(&self) -> u8 {
        ((self.attr >> 4) & 0xF) as u8
    }

    /// Returns the emphasis kind from bits 21..=24.
    ///
    /// hwplib `EmphasisSort` and hwpxlib `SymMarkSort` use this ordering
    /// (HWP 5.0 revision 1.2, table 35):
    /// 0 NONE, 1 DOT_ABOVE, 2 RING_ABOVE, 3 TILDE, 4 CARON, 5 SIDE(･), 6 COLON,
    /// 7 GRAVE_ACCENT, 8 ACUTE_ACCENT, 9 CIRCUMFLEX, 10 MACRON, 11 HOOK_ABOVE,
    /// 12 DOT_BELOW.
    /// The published table only lists values 1 through 6 and swaps 3 (caron)
    /// and 4 (tilde) relative to hwplib. This implementation follows the
    /// hwplib/hwpxlib order pending Hancom visual verification.
    pub fn emphasis_kind(&self) -> u8 {
        ((self.attr >> 21) & 0xF) as u8
    }

    /// Returns the zero-based strike shape from bits 26..=29.
    ///
    /// Values follow the same `BorderType2` ordering as
    /// [`CharShape::underline_shape_code`]. The bit position is based on the
    /// observed documents in compatibility rule B8 and awaits Hancom verification.
    pub fn strike_shape_code(&self) -> u8 {
        ((self.attr >> 26) & 0xF) as u8
    }

    /// 취소선 여부 (명시적 `strike` 플래그 기반).
    ///
    /// HWP5 속성의 "취소선" 비트(18~20)는 **스펙 이견(DIFFSPEC) 영역**이라 신뢰할 수 없다 —
    /// pyhwp(레퍼런스)는 취소선을 비트로 모델링하지 않고, bits 18~20을 취소선으로 읽으면 한글이
    /// **평문으로 렌더하는** 실문서(보도자료 등)에 가짜 취소선이 그어진다(한글 PrvImage 대조 확인).
    /// 따라서 HWP5 reader는 raw 비트로 strike를 켜지 않는다. HWPX reader만 명시적 `<hp:strikeout>`
    /// 의 visible shape(NONE/3D 제외)일 때 `strike`를 켠다. `attr`는 보존(바이트 동일 왕복 무영향).
    pub fn has_strike(&self) -> bool {
        self.strike
    }

    /// 글자 음영(배경 하이라이트)이 있는지. 0xFFFFFFFF=없음 관례.
    pub fn has_shade(&self) -> bool {
        self.shade_color != 0xFFFF_FFFF
    }

    /// 외곽선 종류 (bits 8~10, 0=없음). 스펙 표.
    pub fn has_outline(&self) -> bool {
        (self.attr >> 8) & 0x7 != 0
    }

    /// 그림자 종류 (bits 11~12).
    pub fn has_shadow(&self) -> bool {
        (self.attr >> 11) & 0x3 != 0
    }

    /// 양각 (bit 13).
    pub fn is_emboss(&self) -> bool {
        self.attr & (1 << 13) != 0
    }

    /// 음각 (bit 14).
    pub fn is_engrave(&self) -> bool {
        self.attr & (1 << 14) != 0
    }

    /// 위첨자 (bit 15).
    pub fn is_superscript(&self) -> bool {
        self.attr & (1 << 15) != 0
    }

    /// 아래첨자 (bit 16).
    pub fn is_subscript(&self) -> bool {
        self.attr & (1 << 16) != 0
    }

    /// 언어 슬롯의 수동 글자 위치(첨자 오프셋) % (-100~100). 위=양수.
    pub fn char_offset(&self, lang: usize) -> i8 {
        self.offsets.get(lang).copied().unwrap_or(0)
    }
}

/// PARA_SHAPE — 문단 모양. 알려진 prefix + tail 보존.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParaShape {
    /// 속성1 (정렬: bit2~4, 줄간격 종류(구버전): bit0~1 등)
    pub attr1: u32,
    /// 들여쓰기/내어쓰기
    pub indent: i32,
    pub margin_left: i32,
    pub margin_right: i32,
    pub spacing_top: i32,
    pub spacing_bottom: i32,
    /// 줄간격 (5.0.2.5 미만에서 사용)
    pub line_spacing_old: i32,
    pub tab_def_id: u16,
    /// 번호(head_type 2)·글머리(3) 정의 참조 — **IR 규약: 0-기반 인덱스**
    /// (`numbering_levels`/`bullet_chars`의 인덱스). 포맷 경계에서 ±1로 변환한다:
    /// hwp5는 on-disk가 1-기반이라 read에서 -1(`doc_info.rs`), write에서 +1(`write.rs`);
    /// hwpx는 idRef=numbering_id+1로 방출하고 read가 다시 0-기반으로 정규화한다.
    /// (head_type 0 없음·1 개요는 다른 참조 체계라 변환하지 않는다.)
    pub numbering_id: u16,
    pub border_fill_id: u16,
    /// 테두리 여백 (좌/우/위/아래)
    pub border_offsets: [i16; 4],
    /// 줄간격 종류 (0 비율%, 1 고정, 2 여백만, 3 최소) — OWPML lineSpacing
    #[serde(default)]
    pub line_spacing_type: u8,
    /// 줄간격 값 (0 = 미지정 → 쓰기 시 160%)
    #[serde(default)]
    pub line_spacing: i32,
    /// 속성2/속성3/줄간격(5.0.2.5+) 등
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

impl ParaShape {
    /// 정렬 (0:양쪽, 1:왼쪽, 2:오른쪽, 3:가운데, 4:배분, 5:나눔).
    pub fn alignment(&self) -> u8 {
        ((self.attr1 >> 2) & 0x7) as u8
    }

    /// 문단 머리 종류 (bit23~24): 0=없음, 1=개요, 2=번호, 3=글머리표(불릿).
    pub fn head_type(&self) -> u8 {
        ((self.attr1 >> 23) & 0x3) as u8
    }

    /// 문단 수준 (bit25~27): 1~7.
    pub fn head_level(&self) -> u8 {
        (((self.attr1 >> 25) & 0x7) as u8).clamp(1, 7)
    }
}

/// 번호 매기기 한 수준의 형식.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumLevel {
    /// 시작 번호.
    pub start: u32,
    pub fmt: NumFmt,
    /// 형식 템플릿(`^N`=N수준 번호 자리, 예 "^1." "(^5)" "제^1조"). 빈 문자열=없음(렌더 전용).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub template: String,
}

impl Default for NumLevel {
    fn default() -> Self {
        Self {
            start: 1,
            fmt: NumFmt::Digit,
            template: String::new(),
        }
    }
}

/// 번호 표기 형식.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumFmt {
    /// 1, 2, 3
    Digit,
    /// 가, 나, 다
    HangulSyllable,
    /// ㄱ, ㄴ, ㄷ
    HangulJamo,
    /// ①, ②, ③
    CircledDigit,
    /// A, B, C
    LatinUpper,
    /// a, b, c
    LatinLower,
    /// I, II, III
    RomanUpper,
    /// i, ii, iii
    RomanLower,
}

/// STYLE — 스타일 하나.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    pub english_name: String,
    pub attr: u8,
    pub next_style: u8,
    pub lang_id: i16,
    pub para_shape: ParaShapeId,
    pub char_shape: CharShapeId,
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

/// BIN_DATA (DocInfo) — 바이너리 데이터 참조.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinDataItem {
    pub attr: u16,
    /// 링크인 경우 절대/상대 경로
    pub link_abs: Option<String>,
    pub link_rel: Option<String>,
    /// 임베딩/스토리지인 경우 BinData 스토리지 내 ID
    pub storage_id: Option<u16>,
    /// 임베딩인 경우 확장자
    pub extension: Option<String>,
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

impl BinDataItem {
    /// 타입 (0: 링크, 1: 임베딩, 2: 스토리지).
    pub fn kind(&self) -> u16 {
        self.attr & 0xF
    }
}

/// TAB_DEF 한 정의(HWPTAG_TAB_DEF §4.2.7 / OWPML `hh:tabPr`).
///
/// hwp5 raw(`DocHeader::tab_defs`)와 병렬로 보존되는 의미 표현이다. hwp5 재직렬화는
/// 여전히 raw를 그대로 쓰므로(무손실 identity) 이 타입은 hwpx 왕복·렌더에만 쓰인다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabDef {
    /// 속성 비트(§4.2.7 표37): bit0 문단 왼쪽 끝 자동 탭, bit1 오른쪽 끝 자동 탭.
    #[serde(default)]
    pub attr: u32,
    /// 사용자 탭 항목(위치 오름차순 관례).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<TabItem>,
}

impl TabDef {
    /// 문단 왼쪽 끝 자동 탭(내어쓰기용) 유무.
    pub fn auto_tab_left(&self) -> bool {
        self.attr & 0x1 != 0
    }

    /// 문단 오른쪽 끝 자동 탭 유무.
    pub fn auto_tab_right(&self) -> bool {
        self.attr & 0x2 != 0
    }
}

/// 탭 항목 하나(§4.2.7 탭 정보 8바이트 / OWPML `hh:tabItem`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabItem {
    /// 탭 위치 (HWPUNIT).
    pub pos: i32,
    /// 탭 종류: 0 왼쪽, 1 오른쪽, 2 가운데, 3 소수점.
    pub kind: u8,
    /// 채움(리더) 선 종류 코드(테두리선 종류계: 0=없음, 1=실선, 2=DASH …).
    pub fill: u8,
}

/// 의미 파싱 전의 ID 테이블 항목 — 원시 페이로드 보존.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEntry {
    #[serde(with = "hex_bytes")]
    pub data: Vec<u8>,
    pub children: Vec<OpaqueRecord>,
}

/// 테두리선 하나. 굵기는 mm 테이블 인덱스 (0=0.1mm, 1=0.12mm, … 15=5.0mm).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BorderLine {
    /// 0 = 없음, 1 = 실선, 2+ = 점선/이중선 등
    pub line_type: u8,
    pub width: u8,
    /// COLORREF (0x00BBGGRR)
    pub color: u32,
}

impl BorderLine {
    /// 굵기 인덱스 → mm (한글문서파일형식 5.0 굵기 표).
    pub fn width_mm(&self) -> f32 {
        const TABLE: [f32; 16] = [
            0.1, 0.12, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.6, 0.7, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0,
        ];
        TABLE.get(self.width as usize).copied().unwrap_or(0.12)
    }

    pub fn is_visible(&self) -> bool {
        self.line_type != 0
    }
}

/// BORDER_FILL — 테두리/배경 (실측으로 확정한 레이아웃).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BorderFill {
    pub attr: u16,
    /// 왼/오른/위/아래
    pub sides: [BorderLine; 4],
    pub diagonal: BorderLine,
    /// Fill-type bits: bit 0=solid, bit 1=image, bit 2=gradient.
    pub fill_type: u32,
    /// 단색 배경 (COLORREF, 0xFFFFFFFF = 없음)
    pub bg_color: Option<u32>,
    /// Hatch fill as `(COLORREF, table 29 style 1-6)`. When fill-type bit 0
    /// is set, this is read from the pattern color and style in `tail`;
    /// style -1 means no hatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hatch: Option<(u32, i32)>,
    /// Gradient fill from fill-type bit 2. It takes precedence over hatch and solid fills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<crate::control::GradientSpec>,
    #[serde(with = "hex_bytes")]
    pub tail: Vec<u8>,
}

impl BorderFill {
    /// 그릴 배경색 (없음 표식 제외).
    pub fn visible_bg(&self) -> Option<u32> {
        self.bg_color.filter(|&c| c != 0xFFFF_FFFF)
    }

    /// Returns a visible hatch as `(pattern color, style, background color)`.
    /// `0xFFFFFFFF` means no background. A gradient suppresses the hatch.
    pub fn visible_hatch(&self) -> Option<(u32, i32, u32)> {
        if self.gradient.is_some() {
            return None;
        }
        let (color, style) = self.hatch?;
        (1..=6)
            .contains(&style)
            .then_some((color, style, self.bg_color.unwrap_or(0xFFFF_FFFF)))
    }

    /// Returns a gradient only when it has at least two stops.
    pub fn visible_gradient(&self) -> Option<&crate::control::GradientSpec> {
        self.gradient.as_ref().filter(|g| g.stops.len() >= 2)
    }
}

#[cfg(test)]
mod char_effect_tests {
    use super::*;

    #[test]
    fn 글자효과_접근자() {
        // 음영: 0xFFFFFFFF=없음, 그 외=있음.
        let none = CharShape {
            shade_color: 0xFFFF_FFFF,
            ..CharShape::default()
        };
        assert!(!none.has_shade());
        let shaded = CharShape {
            shade_color: 0x0000_FFFF,
            ..CharShape::default()
        };
        assert!(shaded.has_shade());

        // 위/아래 첨자(bit15/16), 그림자(bits11~12), 외곽선(8~10), 양각13/음각14.
        let sup = CharShape {
            attr: 1 << 15,
            ..CharShape::default()
        };
        assert!(sup.is_superscript() && !sup.is_subscript() && !sup.has_shadow());
        let sub = CharShape {
            attr: 1 << 16,
            ..CharShape::default()
        };
        assert!(sub.is_subscript() && !sub.is_superscript());
        let shadow = CharShape {
            attr: 1 << 11,
            ..CharShape::default()
        };
        assert!(shadow.has_shadow() && !shadow.is_emboss());
        let outline = CharShape {
            attr: 1 << 8,
            ..CharShape::default()
        };
        assert!(outline.has_outline());
        let emboss = CharShape {
            attr: 1 << 13,
            ..CharShape::default()
        };
        assert!(emboss.is_emboss() && !emboss.has_shadow() && !emboss.is_engrave());
        let engrave = CharShape {
            attr: 1 << 14,
            ..CharShape::default()
        };
        assert!(engrave.is_engrave() && !engrave.is_emboss());

        // 수동 글자위치(offsets%).
        let mut cs = CharShape::default();
        cs.offsets[1] = 30;
        assert_eq!(cs.char_offset(1), 30);
        assert_eq!(cs.char_offset(0), 0);
        // 기본값은 전부 효과 없음.
        let d = CharShape::default();
        assert!(!d.is_superscript() && !d.is_subscript() && !d.has_shadow());
    }
}
