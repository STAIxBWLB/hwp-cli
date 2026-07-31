//! HWPX(OWPML, KS X 6101) 포맷 reader/writer.
//!
//! HWPX = ZIP 컨테이너 + XML:
//! - `mimetype`               — `application/hwp+zip` (첫 엔트리, 무압축)
//! - `version.xml`            — 포맷/작성 프로그램 버전
//! - `META-INF/container.xml`, `META-INF/manifest.xml`
//! - `Contents/header.xml`    — DocInfo에 해당 (ID 참조 테이블)
//! - `Contents/section0.xml…` — 본문
//! - `BinData/*`, `Preview/*`
//!
//! M0에서는 컨테이너 수준([`package`])만 구현한다. OWPML 파싱(M2)과
//! writer(M4)는 이후 마일스톤에서 추가한다.

pub mod error;
pub mod package;
pub mod patch;
pub mod read;
pub mod write;

pub use error::HwpxError;
pub use package::{
    HwpxPackage, NATIVE_PACKAGE_LIMITS, NATIVE_PACKAGE_LIMITS_PROFILE,
    NATIVE_PACKAGE_LIMITS_PROFILE_NAME, NATIVE_PACKAGE_LIMITS_RATIONALE, PackageLimits,
    PackageLimitsProfile,
};
pub use read::{ReadResult, parse_content_meta, read_document, read_structure};
pub use write::{DEFAULT_SETTINGS_XML, DEFAULT_VERSION_XML, write_document};
