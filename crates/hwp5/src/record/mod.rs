//! 레코드 계층: 헤더 코덱, 태그 상수, 평면 스트림 ↔ 트리 변환.
//!
//! 설계 원칙: **스캔과 해석의 분리**. 이 모듈은 태그의 의미를 전혀
//! 해석하지 않고 (tag, level, data)만 다룬다. 의미 파싱은 상위
//! 계층(doc_info/body_text)의 몫이다.

pub mod header;
pub mod scan;
pub mod tag;
pub mod tree;

pub use header::RecordHeader;
pub use scan::{
    RecordScanBudget, RecordScanLimits, ScanMode, ScanResult, scan_stream, scan_stream_bounded,
    walk_stream_strict,
};
pub use tree::RecordNode;
