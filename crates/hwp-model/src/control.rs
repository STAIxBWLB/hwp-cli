//! 확장 컨트롤(CTRL_HEADER) 모델.
//!
//! M1에서는 표(`tbl `)와 구역 정의(`secd`)를 의미 파싱하고, 나머지는
//! [`GenericControl`]로 보존한다 — 문단 리스트는 텍스트 추출을 위해
//! 종류와 무관하게 재귀 수집한다(셀/글상자/머리말/각주 모두 같은 구조).

use serde::{Deserialize, Serialize};

use crate::ids::BorderFillId;
use crate::opaque::{OpaqueRecord, hex_bytes};
use crate::paragraph::Paragraph;
use crate::units::HwpUnit;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Control {
    /// `secd` — 구역 정의 (PAGE_DEF 등을 자식으로 가짐)
    SectionDef(SectionDef),
    /// `tbl ` — 표
    Table(Table),
    /// `gso `(그림) / `hp:pic` — 이미지 개체
    Picture(Picture),
    /// 그 외 — ctrl_id와 원본을 보존, 문단 리스트는 수집
    Generic(GenericControl),
}

impl Control {
    /// 정방향 ctrl_id (예: b"secd", b"tbl ").
    pub fn ctrl_id(&self) -> [u8; 4] {
        match self {
            Control::SectionDef(_) => *b"secd",
            Control::Table(_) => *b"tbl ",
            Control::Picture(_) => *b"gso ",
            Control::Generic(g) => g.ctrl_id,
        }
    }
}

/// 이미지 개체.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    /// 개체 공통 속성 원본 (hwp5: CTRL_HEADER 페이로드, hwpx: 비어 있음)
    #[serde(with = "hex_bytes")]
    pub common_data: Vec<u8>,
    pub width: HwpUnit,
    pub height: HwpUnit,
    /// 글자처럼 취급 여부 (배치 힌트). false면 떠 있는(floating) 개체.
    pub treat_as_char: bool,
    /// z-순서(겹침). hwpx `<hp:pic zOrder>`.
    #[serde(default)]
    pub z_order: u32,
    /// 떠 있는 개체의 세로/가로 오프셋(HWPUNIT). hwpx `<hp:pos vertOffset/horzOffset>`.
    #[serde(default)]
    pub vert_offset: i32,
    #[serde(default)]
    pub horz_offset: i32,
    /// Accessible object description. HWP5 stores this in the GSO common
    /// header description BSTR; HWPX stores it as `hp:shapeComment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Crop rectangle (left, top, right, bottom) in source-image HWPUNIT
    /// coordinates under the 96 dpi convention. Stored at HWP5 picture byte
    /// 44 and in HWPX `hp:imgClip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f32; 4]>,
    /// Flip mode from HWPX `hp:flip`: 0=none, 1=horizontal, 2=vertical, 3=both.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub flip: u8,
    /// Clockwise-positive rotation in degrees in a y-down coordinate system.
    /// HWP5 derives it from the GSO matrix; HWPX uses `hp:rotationInfo@angle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f32>,
    /// Brightness (-100..100, 0=unchanged), from HWP5 byte 68 or HWPX `hc:img@bright`.
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub brightness: i8,
    /// Contrast (-100..100, 0=unchanged), from HWP5 byte 69 or HWPX `hc:img@contrast`.
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub contrast: i8,
    /// Raw HWP5 picture-effect flags (`UINT32` at byte 78). Zero means no
    /// effect. Effects such as shadow and glow (tables 108-116) are parsed
    /// and reported but are not rendered.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub effect_flags: u32,
    /// Raw picture-effect stream from byte 78 through the record end,
    /// including flags and payload. Preserved for reporting and future work.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "hex_bytes")]
    pub effects_raw: Vec<u8>,
    /// 캡션 (hwp5: 개체 공통 속성 뒤 LIST_HEADER, hwpx: `<hp:caption>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<Caption>,
    /// 바이너리 데이터 참조
    pub bin_ref: BinRef,
    pub extras: Vec<OpaqueRecord>,
}

/// 캡션 위치 (HWP §4.3.9 표 73 캡션 속성 bits 0-1 / HWPX `hp:caption@side`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptionSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// 캡션 텍스트 방향 (hwp5: LIST_HEADER listflags bits 0-2, 표 65 /
/// hwpx: 캡션 `hp:subList@textDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptionDirection {
    Horizontal,
    Vertical,
}

/// 표/그림/도형의 캡션 (HWP §4.3.9 표 71-73 / HWPX `<hp:caption>`).
///
/// hwp5에서는 개체 공통 속성 뒤·개체 요소 레코드(TABLE 등) 앞에 오는
/// LIST_HEADER + 문단들로 저장된다(pyhwp `TableCaption`/`GShapeObjectCaption`
/// 실측 근거). hwpx에서는 `<hp:tbl>`/`<hp:pic>` 등의 `<hp:caption>` 자식.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub side: CaptionSide,
    pub direction: CaptionDirection,
    /// 캡션과 틀 사이 간격 (HWPUNIT, 바이너리는 HWPUNIT16).
    pub gap: i32,
    /// 캡션 폭 (HWPUNIT). None = full-size(바깥 여백까지 확대 — 바이너리 속성
    /// bit2 / hwpx `fullSz="1"`). 세로 방향(Left/Right)일 때만 의미가 있다(표 72).
    pub width: Option<u32>,
    /// 텍스트의 최대 길이(=개체의 폭, 표 72) — hwpx `hp:caption@lastWidth`와 대응.
    /// 0이면 미상(writer가 개체 폭으로 폴 백).
    #[serde(default)]
    pub last_width: u32,
    pub paragraphs: Vec<Paragraph>,
}

/// BinData 참조 방식.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinRef {
    /// hwp5: BIN_DATA 테이블 ID (1-기반)
    Id(crate::ids::BinDataId),
    /// hwpx: manifest 항목 ID (binaryItemIDRef)
    ItemRef(String),
}

/// hwpx `<hp:secPr>`의 미해석 자식이 저장될 [`SectionDef::secpr_raw_children`]에서
/// `<hp:pagePr>`(의미 파싱되어 페이지 정의로 재방출됨)가 자식 순서상 놓일 자리를
/// 표시하는 센티넬. 실제 XML엔 나타날 수 없는 제어문자 조합이라 원문과 충돌하지 않는다.
/// reader가 이 마커를 삽입하고, writer가 이 자리에서 페이지 정의로부터 생성한 pagePr을
/// 방출한다(원본 자식 순서 보존).
pub const SECPR_PAGEPR_SLOT: &str = "\u{0}pagePr\u{0}";

/// 구역 정의 컨트롤.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionDef {
    /// CTRL_HEADER 페이로드 (ctrl_id 이후) — 미해석 보존
    #[serde(with = "hex_bytes")]
    pub data: Vec<u8>,
    /// PAGE_DEF — 용지 크기/여백
    pub page: Option<PageDef>,
    /// FOOTNOTE_SHAPE, PAGE_BORDER_FILL 등 미해석 자식
    pub extras: Vec<OpaqueRecord>,
    /// hwpx `<hp:secPr>`의 미해석 자식(grid/startNum/visibility/lineNumberShape/
    /// footNotePr/endNotePr/pageBorderFill 등)의 원문 XML을 등장 순서대로 보존한다.
    /// pagePr 자리는 [`SECPR_PAGEPR_SLOT`] 마커로 표시된다(페이지 정의에서 재생성).
    /// 비어 있으면(hwp5 출신·구형 IR) writer가 기존 상수 템플릿을 방출한다.
    #[serde(default)]
    pub secpr_raw_children: Vec<String>,
    /// hwp5 출신 구역의 각주 모양(FOOTNOTE_SHAPE, 28B) 원본 바이트 — 교차 변환(→hwpx)
    /// 시 상수 대신 실측 footNotePr을 재구성하는 데 쓰는 **병행 사본**이다.
    /// 무손실 identity 게이트의 정본은 여전히 [`SectionDef::extras`]의 Opaque이며,
    /// 이 필드는 그 사본일 뿐 hwp5 재직렬화 경로엔 관여하지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footnote_shape_raw: Option<Vec<u8>>,
    /// hwp5 출신 구역의 미주 모양(FOOTNOTE_SHAPE 태그 공유, 28B) 원본 바이트.
    /// secd 자식 중 **둘째** FOOTNOTE_SHAPE가 미주다(정품 실측 순서: 각주→미주).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endnote_shape_raw: Option<Vec<u8>>,
    /// hwp5 출신 구역의 쪽 테두리/배경(PAGE_BORDER_FILL, 14B) 원본 바이트.
    /// 등장 순서가 곧 BOTH/EVEN/ODD다(구역당 최대 3개, 정품 실측).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_border_fills_raw: Vec<Vec<u8>>,
}

/// PAGE_DEF (40바이트) — 용지 정의.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageDef {
    pub width: HwpUnit,
    pub height: HwpUnit,
    pub margin_left: HwpUnit,
    pub margin_right: HwpUnit,
    pub margin_top: HwpUnit,
    pub margin_bottom: HwpUnit,
    pub margin_header: HwpUnit,
    pub margin_footer: HwpUnit,
    pub gutter: HwpUnit,
    /// bit0: 용지 방향(가로), bit1~2: 제책 방법
    pub attr: u32,
}

/// 개체 공통 속성(gso common)의 배치 정보 — hwpx `<hp:pos>`/`<hp:sz>`/`<hp:outMargin>`/
/// `zOrder`에서 읽어 hwp5 CTRL_HEADER 40바이트 공통 속성으로 합성한다. hwpx 출신 표가
/// 이 정보를 잃으면 writer가 떠 있는(floating) 상수로 덮어써 본문 흐름에서 빠진다.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GsoPlacement {
    /// 글자처럼 취급 (attr bit0). 정품 표는 대부분 true(인라인).
    pub treat_as_char: bool,
    /// 줄 간격에 영향 (attr bit2).
    pub affect_line_spacing: bool,
    /// 본문과 어울림(flowWithText) (attr bit13).
    pub flow_with_text: bool,
    /// 앵커/개체 고정(holdAnchorAndSO) — 공통 속성 @36.
    pub hold_anchor: bool,
    /// 세로 위치 기준: 0=PAPER, 1=PAGE, 2=PARA (attr bits3-4).
    pub vert_rel_to: u8,
    /// 가로 위치 기준: 0=PAPER, 1=PAGE, 2=COLUMN, 3=PARA (attr bits8-9).
    pub horz_rel_to: u8,
    /// 세로 정렬 (attr bits5-7).
    pub vert_align: u8,
    /// 가로 정렬 (attr bits10-12).
    pub horz_align: u8,
    pub vert_offset: i32,
    pub horz_offset: i32,
    pub z_order: i32,
    /// 개체 바깥 경계 너비/높이(HWPUNIT) — hwpx `<hp:sz>`. 병합 셀 합산보다 정확.
    pub width: i32,
    pub height: i32,
    /// 바깥 여백 (왼/오른/위/아래).
    pub out_margins: [u16; 4],
}

impl GsoPlacement {
    /// hwp5 개체 공통 속성 attr(u32)로 합성. 상위 16비트는 관측 상수(0x082a:
    /// widthRelTo/heightRelTo=ABSOLUTE, textWrap 등)로 둔다.
    pub fn synth_attr(&self) -> u32 {
        let mut low: u32 = 0;
        low |= u32::from(self.treat_as_char); // bit0
        low |= u32::from(self.affect_line_spacing) << 2; // bit2
        low |= (u32::from(self.vert_rel_to) & 0x3) << 3; // bits3-4
        low |= (u32::from(self.vert_align) & 0x7) << 5; // bits5-7
        low |= (u32::from(self.horz_rel_to) & 0x3) << 8; // bits8-9
        low |= (u32::from(self.horz_align) & 0x7) << 10; // bits10-12
        low |= u32::from(self.flow_with_text) << 13; // bit13
        0x082a_0000 | low
    }
}

/// 표 컨트롤.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    /// CTRL_HEADER 페이로드 (개체 공통 속성) — 미해석 보존
    #[serde(with = "hex_bytes")]
    pub common_data: Vec<u8>,
    /// hwpx 출신 표의 배치 정보 (hwp5 출신은 common_data가 채워져 None).
    #[serde(default)]
    pub placement: Option<GsoPlacement>,
    pub attr: u32,
    pub rows: u16,
    pub cols: u16,
    pub cell_spacing: u16,
    /// 안쪽 여백 (왼/오른/위/아래)
    pub inner_margins: [u16; 4],
    /// 행별 셀 개수 (실측: 스펙의 "Row Size"는 행 높이가 아니라 셀 수)
    pub row_cell_counts: Vec<u16>,
    pub border_fill: BorderFillId,
    /// TABLE 레코드의 나머지 (영역 속성 등)
    #[serde(with = "hex_bytes")]
    pub table_tail: Vec<u8>,
    /// 셀 목록 (LIST_HEADER 등장 순서 — 행 우선)
    pub cells: Vec<Cell>,
    /// 캡션 (hwp5: TABLE 레코드 앞의 LIST_HEADER, hwpx: `<hp:caption>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<Caption>,
    pub extras: Vec<OpaqueRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// LIST_HEADER 속성 비트 (hwp5 왕복 보존)
    #[serde(default)]
    pub list_attr: u32,
    pub col: u16,
    pub row: u16,
    pub col_span: u16,
    pub row_span: u16,
    pub width: HwpUnit,
    pub height: HwpUnit,
    /// 셀 안쪽 여백 (왼/오른/위/아래, HWPUNIT)
    pub margins: [u16; 4],
    pub border_fill: BorderFillId,
    /// LIST_HEADER 페이로드 중 미해석 부분
    #[serde(with = "hex_bytes")]
    pub header_tail: Vec<u8>,
    pub paragraphs: Vec<Paragraph>,
}

/// Table page-break policy (`Table.attr` bits 0-1, HWPX `pageBreak`).
///
/// The mapping is preserved by the HWP/HWPX readers and writers. Renderer
/// behavior still requires comparison with a Hancom Office oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablePageBreak {
    /// Keep the table together when it fits on one page.
    None,
    /// Split at table row boundaries.
    Table,
    /// Allow cell splitting (currently rendered at row boundaries).
    Cell,
}

impl Table {
    /// Returns the page-break policy (`attr` bits 0-1).
    pub fn page_break_policy(&self) -> TablePageBreak {
        match self.attr & 0x3 {
            1 => TablePageBreak::Table,
            2 => TablePageBreak::Cell,
            _ => TablePageBreak::None,
        }
    }

    /// Returns whether leading header rows repeat (`attr` bit 2).
    pub fn repeat_header(&self) -> bool {
        self.attr & 0x4 != 0
    }
}

impl Cell {
    /// Returns whether this is a header cell (`list_attr` bit 18).
    pub fn is_header(&self) -> bool {
        self.list_attr & (1 << 18) != 0
    }

    /// Returns vertical alignment (`list_attr` bits 5-6).
    pub fn vert_align(&self) -> u8 {
        ((self.list_attr >> 5) & 0x3) as u8
    }
}

/// 의미 파싱하지 않는 컨트롤 (머리말/꼬리말/각주/글상자/필드 등).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericControl {
    /// 정방향 ctrl_id (예: b"gso ", b"head")
    pub ctrl_id: [u8; 4],
    /// CTRL_HEADER 페이로드 (ctrl_id 이후)
    #[serde(with = "hex_bytes")]
    pub data: Vec<u8>,
    /// LIST_HEADER 단위 문단 리스트 — 텍스트 추출용 재귀 수집
    pub paragraph_lists: Vec<ParagraphList>,
    pub extras: Vec<OpaqueRecord>,
    /// hwp5 원본 CTRL_HEADER 자식 서브트리(중첩 포함) — 무손실 재직렬화용.
    /// 존재하면 emit 시 이 트리를 그대로 방출하고 paragraph_lists/extras는
    /// 텍스트 추출 전용으로만 쓴다(gso 등 중첩 구조 평탄화 방지).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_children: Vec<OpaqueRecord>,
    /// hwpx 그리기 개체(도형) 기하/스타일 — 렌더 전용(hwpx reader가 채움).
    /// hwp5 도형은 raw_children에서 렌더 시점 파싱하므로 비어 있다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gso_shapes: Vec<ShapeGeom>,
    /// 수식(hp:equation) — 렌더 전용(box+스크립트 근사). 없으면 None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equation: Option<Equation>,
    /// 다단 정의(`cold`/hp:colPr) — 렌더러 단 배치·구분선용. 없으면 None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_def: Option<ColumnDef>,
    /// 캡션 (gso 개체의 직계 LIST_HEADER / hwpx 도형의 `<hp:caption>`).
    /// GenericControl은 raw_children이 재직렬화 정본이라 이 필드는 의미 파싱
    /// (렌더·텍스트) 전용이다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<Caption>,
}

/// 다단(multi-column) 정의 — COLDEF(`cold`)/hp:colPr. 렌더러가 단 배치·구분선에 사용.
/// 근거: 한글문서파일형식 5.0 표138/139 + hwplib ControlColumnDefine(bit단위 대조).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// 단 수(1-255).
    pub count: u16,
    /// 종류: 0 일반, 1 배분, 2 평행.
    pub kind: u8,
    /// 방향: 0 왼쪽부터, 1 오른쪽부터, 2 맞쪽.
    pub direction: u8,
    /// 단 너비 동일 여부.
    pub same_width: bool,
    /// 단 간격(HWPUNIT).
    pub gap: i32,
    /// 단별 폭(HWPUNIT). same_width면 비어 있고 렌더러가 균등 분할.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub widths: Vec<i32>,
    /// 단 사이 구분선(없으면 None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divider: Option<crate::header::BorderLine>,
}

/// 수식 개체 — 렌더러가 상자+스크립트 텍스트로 근사한다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equation {
    /// HWP 수식 스크립트 원문.
    pub script: String,
    /// 크기(HWPUNIT). 0이면 렌더러가 추정.
    pub width: i32,
    pub height: i32,
    /// 글자처럼 취급(인라인)이면 true.
    pub inline: bool,
    /// 떠 있는 경우 페이지 절대 오프셋(HWPUNIT).
    pub x: i32,
    pub y: i32,
    /// hwpx `<hp:equation>`의 시작 태그 속성 원문(`id` 제외 — writer가 재부여).
    /// 글자색·기준선·baseUnit·수식 글꼴·zOrder·본문배치 등 IR이 의미 모델링하지 않는
    /// 속성을 왕복에서 보존하기 위한 pass-through다([`SectionDef::secpr_raw_children`]
    /// 와 같은 규약). 비어 있으면(hwp5 출신·합성) writer가 표준값을 방출한다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_attrs: Option<String>,
    /// 개체 공통 자식(`hp:sz`·`hp:pos`·`hp:outMargin`·caption 등)의 원문 XML을 등장
    /// 순서대로 보존한다(`hp:script`는 제외 — script 필드가 정본). 비어 있으면 writer가
    /// width/height/inline/x/y와 hwp5 gso 공통 헤더로 재구성한다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_props: Vec<String>,
}

/// 도형 종류 (hwpx 그리기 개체).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Line,
    Polygon,
    Curve,
    Arc,
}

/// hwpx 그리기 개체 기하/스타일 (HWPUNIT, 페이지 기준). 렌더러가 Item::Path로 변환.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeGeom {
    pub kind: ShapeKind,
    /// 경계 상자(HWPUNIT): pos 오프셋 + sz.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// 선/다각형/곡선의 점(HWPUNIT, 경계 상자 원점 기준).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<(i32, i32)>,
    /// 채움색 COLORREF(없음=0xFFFFFFFF). fill_gradient가 Some면 무시.
    pub fill: u32,
    /// 그러데이션 채움(있으면 fill보다 우선).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_gradient: Option<GradientSpec>,
    /// 테두리 색 COLORREF(없음=0xFFFFFFFF).
    pub border_color: u32,
    /// 테두리 굵기 HWPUNIT(0이면 선 없음).
    pub border_width: i32,
    /// 둥근 사각형 모서리 곡률(%, 0=직각). Rect에만 의미.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub round_ratio: u8,
    /// 테두리 선 종류: 0=실선, 1=파선, 2=점선, 3=일점쇄선, 4=이점쇄선, 5=긴파선.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub border_style: u8,
    /// 선 시작 화살촉: 0=없음, 그 외=화살촉. Line에만 의미.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub arrow_start: u8,
    /// 선 끝 화살촉: 0=없음, 그 외=화살촉. Line에만 의미.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub arrow_end: u8,
    /// 글자처럼 취급(hp:pos treatAsChar) — 참이면 x/y 대신 텍스트 흐름 위치에 배치.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anchored: bool,
    /// Accessible object description (`hp:shapeComment` in HWPX).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_i8(v: &i8) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 그러데이션 채움 명세(렌더러 display::Gradient로 변환).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientSpec {
    pub radial: bool,
    pub angle_deg: f32,
    /// (위치 0..1, COLORREF). 위치 오름차순.
    pub stops: Vec<(f32, u32)>,
}

impl GradientSpec {
    /// Parses an HWP5 table 28 gradient block: type, angle, horizontal center,
    /// vertical center, spread, and color count as `i16`; positions as
    /// `INT32[num]` when `num > 2`; then `COLORREF[num]`. Returns the parsed
    /// specification and consumed byte count.
    pub fn parse_hwp5(d: &[u8], off: usize) -> Option<(Self, usize)> {
        let rd_u16 =
            |o: usize| -> Option<u16> { d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])) };
        let rd_i32 = |o: usize| -> Option<i32> {
            d.get(o..o + 4)
                .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let rd_u32 = |o: usize| -> Option<u32> {
            d.get(o..o + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let gtype = rd_u16(off)? as i16;
        let angle = rd_u16(off + 2)? as i16 as f32;
        let num = rd_u16(off + 10)? as usize;
        if !(1..=16).contains(&num) {
            return None;
        }
        let mut cur = off + 12;
        let positions: Vec<f32> = if num > 2 {
            let mut v = Vec::with_capacity(num);
            for i in 0..num {
                v.push(rd_i32(cur + i * 4)? as f32);
            }
            cur += num * 4;
            let max = v.iter().cloned().fold(1.0_f32, f32::max);
            v.iter().map(|p| (p / max).clamp(0.0, 1.0)).collect()
        } else {
            (0..num)
                .map(|i| i as f32 / (num.max(2) - 1) as f32)
                .collect()
        };
        let mut stops = Vec::with_capacity(num);
        for i in 0..num {
            let c = rd_u32(cur + i * 4)?;
            stops.push((positions.get(i).copied().unwrap_or(0.0), c));
        }
        cur += num * 4;
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Preserve the established shape renderer mapping where type 1 is radial.
        // Table 30 can be read as type 2 being radial; changing this requires
        // comparison against a genuine Hangul-authored file.
        Some((
            GradientSpec {
                radial: gtype == 1,
                angle_deg: angle,
                stops,
            },
            cur - off,
        ))
    }
}

/// LIST_HEADER 하나가 여는 문단 리스트.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphList {
    /// LIST_HEADER 페이로드 — 미해석 보존
    #[serde(with = "hex_bytes")]
    pub header_data: Vec<u8>,
    pub paragraphs: Vec<Paragraph>,
}

#[cfg(test)]
mod tests {
    use super::{Cell, GsoPlacement, Table, TablePageBreak};
    use crate::units::HwpUnit;

    /// Locks the model accessors to the serialized attribute mappings.
    #[test]
    fn 표_쪽나눔_제목셀_비트_접근자() {
        let table = |attr: u32| Table {
            common_data: Vec::new(),
            placement: None,
            attr,
            rows: 0,
            cols: 0,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: Vec::new(),
            border_fill: Default::default(),
            table_tail: Vec::new(),
            cells: Vec::new(),
            caption: None,
            extras: Vec::new(),
        };
        assert_eq!(table(0).page_break_policy(), TablePageBreak::None);
        assert_eq!(table(1).page_break_policy(), TablePageBreak::Table);
        assert_eq!(table(2).page_break_policy(), TablePageBreak::Cell);
        assert!(!table(0).repeat_header());
        assert!(table(0b100).repeat_header());
        assert!(table(0b101).repeat_header());

        let cell = |list_attr: u32| Cell {
            list_attr,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: Default::default(),
            header_tail: Vec::new(),
            paragraphs: Vec::new(),
        };
        assert!(!cell(0).is_header());
        assert!(cell(1 << 18).is_header());
        assert_eq!(cell(0).vert_align(), 0);
        assert_eq!(cell(0x20).vert_align(), 1);
        assert_eq!(cell(0x40).vert_align(), 2);
        // Header and vertical-alignment bits are independent.
        let both = cell((1 << 18) | 0x20);
        assert!(both.is_header());
        assert_eq!(both.vert_align(), 1);
    }

    /// 정품 한라대 .hwp 실측 표 공통 속성 attr를 비트 합성으로 재현하는지 확인한다.
    /// (글자처럼 취급 보존 — 인라인 표가 떠 있는 개체로 빠지던 버그의 회귀 방지.)
    #[test]
    fn 표_공통속성_attr_정품값_재현() {
        // 인라인 표(제목바·본문·꼬리말): treatAsChar=1, vertRelTo=PARA(2),
        // horzRelTo=PARA(3), flowWithText=1 → 0x082a2311
        let inline = GsoPlacement {
            treat_as_char: true,
            flow_with_text: true,
            vert_rel_to: 2,
            horz_rel_to: 3,
            ..Default::default()
        };
        assert_eq!(inline.synth_attr(), 0x082a_2311);

        // 표지/꼬리말 3x3: flowWithText=0 → 0x082a0311
        let cover = GsoPlacement {
            flow_with_text: false,
            ..inline.clone()
        };
        assert_eq!(cover.synth_attr(), 0x082a_0311);

        // 목차 박스: treatAsChar=0(떠 있음) → 0x082a2310
        let toc = GsoPlacement {
            treat_as_char: false,
            ..inline
        };
        assert_eq!(toc.synth_attr(), 0x082a_2310);
    }
}
