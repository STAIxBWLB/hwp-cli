//! hwp5 크레이트 오류 타입.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Hwp5Error {
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),

    #[error("HWP 5.0 파일이 아닙니다 (CFB 컨테이너 열기 실패 또는 FileHeader 없음)")]
    NotHwp5,

    #[error("FileHeader 시그니처가 올바르지 않습니다")]
    BadSignature,

    #[error("FileHeader 크기가 올바르지 않습니다 (기대 256바이트, 실제 {0}바이트)")]
    BadFileHeaderSize(usize),

    #[error("스트림이 없습니다: {0}")]
    StreamNotFound(String),

    #[error("압축 해제 실패 ({stream}): {source}")]
    Decompress {
        stream: String,
        source: std::io::Error,
    },

    #[error(
        "스트림 끝을 지나 읽으려 했습니다 (오프셋 {offset}, 요청 {wanted}바이트, 남은 {remaining}바이트)"
    )]
    UnexpectedEof {
        offset: usize,
        wanted: usize,
        remaining: usize,
    },

    #[error("레코드 구조가 손상되었습니다: {0}")]
    MalformedRecord(String),

    #[error("리소스 제한 초과 ({resource}): 상한 {limit}바이트")]
    ResourceLimitExceeded { resource: String, limit: u64 },

    #[error("구조 제한 초과 ({resource}): 상한 {limit}")]
    StructureLimitExceeded { resource: String, limit: usize },

    #[error("지원하지 않는 HWP 버전입니다: {0} (HWP 5.x만 지원)")]
    UnsupportedVersion(String),

    // One message for both an absent and a wrong password: telling them apart
    // would turn this into a credential oracle.
    #[error("암호가 필요하거나 올바르지 않습니다. --password-stdin으로 암호를 전달하세요.")]
    Encrypted,

    #[error("지원하지 않는 HWP5 암호화 프로필입니다 (EncryptVersion {encrypt_version})")]
    UnsupportedPasswordProfile { encrypt_version: u32 },

    // GATE-02: certificate encryption, certificate DRM, DRM and digital
    // signature. The bits are parsed by `file_header.rs`; whether they are
    // set in the situations their labels name is unverified against a
    // genuine file — see the comment above `FileHeader::check_body_readable`.
    #[error(
        "공인 인증서로 암호화된 문서는 지원하지 않습니다. 한글에서 인증서 암호화를 해제한 뒤 다시 저장하세요."
    )]
    CertEncrypted,

    #[error(
        "공인 인증서 DRM으로 보호된 문서는 지원하지 않습니다. 한글에서 인증서 DRM 보안을 해제한 뒤 다시 저장하세요."
    )]
    CertDrm,

    #[error(
        "DRM으로 보호된 문서는 지원하지 않습니다. 한글에서 DRM 보안을 해제한 뒤 다시 저장하세요."
    )]
    Drm,

    #[error("서명된 문서는 지원하지 않습니다. 한글에서 서명을 제거한 뒤 다시 저장하세요.")]
    Signed,

    #[error(
        "배포용 문서(ViewText)의 원본 구조는 지원하지 않습니다. hwp cat/convert/render는 배포용 문서를 읽을 수 있습니다"
    )]
    DistributionDoc,

    #[error("HWP 원본 snapshot이 편집 기준 문서와 일치하지 않습니다")]
    SourceSnapshotMismatch,

    #[error("HWP 원본 보존 재작성 실패: {0}")]
    SourceRewrite(String),

    #[error("증거로 확인되지 않은 공식 번호 체계 범위: {0}")]
    UnsupportedOfficialNumberingRange(String),

    #[error("증거로 확인되지 않은 직접 HWP5 공식 번호 체계 구조: {0}")]
    UnsupportedOfficialNumberingTopology(String),
}

pub type Result<T> = std::result::Result<T, Hwp5Error>;
