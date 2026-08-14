//! HWP/HWPX 공유 문서 모델(IR).
//!
//! 본 제품은 한글과컴퓨터의 한글 문서 파일(.hwp) 공개 문서(한글 문서 파일 형식 5.0,
//! <https://store.hancom.com/etc/hwpDownload.do>, © (주)한글과컴퓨터)를 참고하여 개발하였습니다.
//!
//! 설계 원칙 (계획 문서 참조):
//! - L0(포맷별 무손실 표현) — L1(이 크레이트의 의미 IR) — L2(파생 표현)의 3계층 중 L1.
//! - HWP 5.0과 HWPX(OWPML)는 의미론적으로 거의 동형이므로,
//!   IR은 "공통 조상"이 아니라 HWP 의미 모델 그 자체를 충실히 옮긴다.
//! - 모르는 데이터는 버리지 않는다: `OpaqueRecord`/`tail` 보존.
//! - 위치 단위는 WCHAR(UTF-16 코드 유닛, 확장 컨트롤 = 8) —
//!   [`paragraph::char_kind`]가 분류의 단일 진실 공급원.
//!
//! 이 크레이트는 의존성을 극도로 아낀다(serde만). 모든 크레이트가 여기에
//! 의존하므로 이 API의 안정성이 곧 전체 프로젝트의 안정성이다.

pub mod control;
pub mod document;
pub mod header;
pub mod ids;
pub mod list;
pub mod opaque;
pub mod paragraph;
pub mod preservation;
pub mod text;
pub mod units;

pub use control::{
    BinRef, Caption, CaptionDirection, CaptionSide, Cell, ColumnDef, Control, Equation,
    GenericControl, GradientSpec, GsoPlacement, PageDef, ParagraphList, Picture, SECPR_PAGEPR_SLOT,
    SectionDef, ShapeGeom, ShapeKind, Table, TablePageBreak,
};
pub use document::{BinStream, DocMeta, Document, Metadata, Section};
pub use header::{
    BinDataItem, BorderFill, BorderLine, CharShape, DocHeader, DocumentProperties, FaceName,
    LANG_COUNT, NumFmt, NumLevel, ParaShape, RawEntry, Style, TabDef, TabItem,
};
pub use ids::{BinDataId, BorderFillId, CharShapeId, FaceNameId, ParaShapeId, StyleId};
pub use opaque::OpaqueRecord;
pub use paragraph::{
    CharKind, Hwp5ParagraphChild, HwpChar, LineSeg, ParaHeaderInfo, Paragraph, char_kind, ctrl_char,
};
pub use preservation::{
    PRESERVATION_REPORT_CONTRACT, PreservationCode, PreservationDisposition, PreservationEvent,
    PreservationReport, PreservationResourceKind, WriteReport,
};
pub use text::TextOptions;
pub use units::{
    HwpUnit, filetime_to_iso8601_utc, filetime_to_korean_kst, iso8601_utc_to_filetime,
};
