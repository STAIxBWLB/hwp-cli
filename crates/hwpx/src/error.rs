//! hwpx 크레이트 오류 타입.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HwpxError {
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),

    #[error("HWPX 파일이 아닙니다 (ZIP 열기 실패): {0}")]
    NotZip(#[from] zip::result::ZipError),

    #[error("HWPX 파일이 아닙니다 (mimetype이 `application/hwp+zip`이 아님: {0:?})")]
    BadMimetype(String),

    #[error("엔트리가 없습니다: {0}")]
    EntryNotFound(String),

    #[error("HWPX 패키지 제한 위반: {0}")]
    PackageLimit(String),

    #[error("HWPX 패키지 무결성 오류: {0}")]
    PackageIntegrity(String),

    #[error("HWPX 보호 프로필은 지원하지 않습니다.")]
    UnsupportedEncryptionProfile,

    #[error("XML 파싱 오류 ({entry}): {message}")]
    Xml { entry: String, message: String },

    // Same opening sentence as `hwp5::Hwp5Error::Encrypted` — it states the same
    // user-facing fact about a sibling format. Independent variant on purpose:
    // the hub-and-spoke invariant forbids hwpx from depending on hwp5, so this
    // is a sibling implementation, not a shared type (D-08: no typed error code).
    #[error("암호화된 문서는 지원하지 않습니다. 한글에서 암호를 해제한 뒤 다시 저장하세요.")]
    Encrypted,
}

pub type Result<T> = std::result::Result<T, HwpxError>;
