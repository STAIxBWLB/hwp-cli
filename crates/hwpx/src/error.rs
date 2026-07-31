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

    #[error("XML 파싱 오류 ({entry}): {message}")]
    Xml { entry: String, message: String },
}

pub type Result<T> = std::result::Result<T, HwpxError>;
