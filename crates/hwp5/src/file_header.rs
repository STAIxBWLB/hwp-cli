//! FileHeader 스트림 (256바이트 고정).
//!
//! 레이아웃 (한글문서파일형식 5.0 §4.1):
//! - 0..32   시그니처 `"HWP Document File"` + NUL 패딩
//! - 32..36  버전 DWORD (0xMMnnPPrr — 5.0.3.0 → 0x05000300)
//! - 36..40  속성 플래그 DWORD
//! - 40..44  라이선스(CCL/공공누리) 플래그 DWORD
//! - 44..48  EncryptVersion DWORD
//! - 48      공공누리 라이선스 지원 국가 BYTE
//! - 49..256 예약 (왕복 보존을 위해 그대로 유지)

use serde::Serialize;

use crate::codec::{ByteReader, ByteWriter};
use crate::error::{Hwp5Error, Result};

pub const FILE_HEADER_SIZE: usize = 256;
pub const SIGNATURE: &[u8; 17] = b"HWP Document File";

/// 파일 버전. 0xMMnnPPrr 인코딩의 각 바이트.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HwpVersion {
    pub major: u8,
    pub minor: u8,
    pub build: u8,
    pub revision: u8,
}

impl HwpVersion {
    pub fn from_u32(v: u32) -> Self {
        Self {
            major: (v >> 24) as u8,
            minor: (v >> 16) as u8,
            build: (v >> 8) as u8,
            revision: v as u8,
        }
    }

    pub fn to_u32(self) -> u32 {
        (u32::from(self.major) << 24)
            | (u32::from(self.minor) << 16)
            | (u32::from(self.build) << 8)
            | u32::from(self.revision)
    }
}

impl std::fmt::Display for HwpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

/// 속성 플래그 비트 (36..40 DWORD).
mod attr {
    pub const COMPRESSED: u32 = 1 << 0;
    pub const ENCRYPTED: u32 = 1 << 1;
    pub const DISTRIBUTION: u32 = 1 << 2;
    pub const HAS_SCRIPT: u32 = 1 << 3;
    pub const DRM: u32 = 1 << 4;
    pub const HAS_XML_TEMPLATE: u32 = 1 << 5;
    pub const HAS_HISTORY: u32 = 1 << 6;
    pub const HAS_SIGNATURE: u32 = 1 << 7;
    pub const CERT_ENCRYPTED: u32 = 1 << 8;
    pub const SIGNATURE_SPARE: u32 = 1 << 9;
    pub const CERT_DRM: u32 = 1 << 10;
    pub const CCL: u32 = 1 << 11;
    pub const MOBILE_OPTIMIZED: u32 = 1 << 12;
    pub const PRIVACY_SECURITY: u32 = 1 << 13;
    pub const TRACK_CHANGES: u32 = 1 << 14;
    pub const KOGL: u32 = 1 << 15;
    pub const HAS_VIDEO_CONTROL: u32 = 1 << 16;
    pub const HAS_TOC_FIELD: u32 = 1 << 17;
}

#[derive(Debug, Clone)]
pub struct FileHeader {
    pub version: HwpVersion,
    pub attributes: u32,
    pub license: u32,
    pub encrypt_version: u32,
    pub kogl_country: u8,
    /// 49..256 예약 영역 — 왕복 보존용.
    pub reserved: [u8; FILE_HEADER_SIZE - 49],
}

impl FileHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() != FILE_HEADER_SIZE {
            return Err(Hwp5Error::BadFileHeaderSize(data.len()));
        }
        let mut r = ByteReader::new(data);
        let sig = r.read_bytes(32)?;
        if &sig[..SIGNATURE.len()] != SIGNATURE {
            return Err(Hwp5Error::BadSignature);
        }
        let version = HwpVersion::from_u32(r.read_u32()?);
        let attributes = r.read_u32()?;
        let license = r.read_u32()?;
        let encrypt_version = r.read_u32()?;
        let kogl_country = r.read_u8()?;
        let mut reserved = [0u8; FILE_HEADER_SIZE - 49];
        reserved.copy_from_slice(r.take_rest());
        Ok(Self {
            version,
            attributes,
            license,
            encrypt_version,
            kogl_country,
            reserved,
        })
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        let mut sig = [0u8; 32];
        sig[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        w.write_bytes(&sig);
        w.write_u32(self.version.to_u32());
        w.write_u32(self.attributes);
        w.write_u32(self.license);
        w.write_u32(self.encrypt_version);
        w.write_u8(self.kogl_country);
        w.write_bytes(&self.reserved);
        debug_assert_eq!(w.len(), FILE_HEADER_SIZE);
        w.into_bytes()
    }

    /// 지원 버전(HWP 5.x)인지 판별한다. major가 5여야만 5.0.x.x·5.1.x.x 등 전부 허용.
    pub fn is_supported_version(&self) -> bool {
        self.version.major == 5
    }

    /// 미지원 버전(major != 5)이면 [`Hwp5Error::UnsupportedVersion`]을 낸다.
    /// 시그니처만으로는 걸러지지 않는 (가상의) 6.x 등 상위 버전을 본문 접근 전에 거부한다.
    pub fn check_version(&self) -> Result<()> {
        if !self.is_supported_version() {
            return Err(Hwp5Error::UnsupportedVersion(self.version.to_string()));
        }
        Ok(())
    }

    pub fn is_compressed(&self) -> bool {
        self.attributes & attr::COMPRESSED != 0
    }

    pub fn is_encrypted(&self) -> bool {
        self.attributes & attr::ENCRYPTED != 0
    }

    pub fn is_distribution(&self) -> bool {
        self.attributes & attr::DISTRIBUTION != 0
    }

    pub fn is_drm(&self) -> bool {
        self.attributes & attr::DRM != 0
    }

    pub fn has_signature(&self) -> bool {
        self.attributes & attr::HAS_SIGNATURE != 0
    }

    pub fn is_cert_encrypted(&self) -> bool {
        self.attributes & attr::CERT_ENCRYPTED != 0
    }

    pub fn is_cert_drm(&self) -> bool {
        self.attributes & attr::CERT_DRM != 0
    }

    // Ordered refusal chain (GATE-02). The rule that decides the order: the
    // condition that blocks the body most completely comes first, and a
    // certificate-scoped condition is more specific than the general one it
    // implies — so password encryption, then certificate encryption, then
    // certificate DRM, then plain DRM, then a digital signature.
    //
    // The certificate, DRM and signature branches are unverified against a
    // genuine file: no certificate-secured or signed document was obtainable
    // here. What is established is only that `file_header.rs` parses these
    // bits and that `hwp info` displays them, not that they are set in the
    // situations their labels name (CONTEXT.md D-06/D-07).
    //
    // The SIGNATURE_SPARE bit (9) is deliberately not wired to a refusal: it
    // is not one of the three conditions GATE-02 names, its label describes
    // reserve storage rather than a protection state, and its meaning was
    // never checked against a real document.

    /// Refuses body access for an unsupported version or a protected document
    /// (password encryption, certificate encryption, certificate DRM, DRM or
    /// a digital signature), before any of the ordered branches above are
    /// consulted for a version this crate does not support. Distribution
    /// documents are body-readable — GATE-01 decrypts `/ViewText/` in
    /// `read_document` rather than refusing here.
    pub fn check_body_readable(&self) -> Result<()> {
        self.check_version()?;
        if self.is_encrypted() {
            return Err(Hwp5Error::Encrypted);
        }
        if self.is_cert_encrypted() {
            return Err(Hwp5Error::CertEncrypted);
        }
        if self.is_cert_drm() {
            return Err(Hwp5Error::CertDrm);
        }
        if self.is_drm() {
            return Err(Hwp5Error::Drm);
        }
        if self.has_signature() {
            return Err(Hwp5Error::Signed);
        }
        Ok(())
    }

    /// 사람이 읽을 수 있는 속성 플래그 이름 목록 (`hwp info`용).
    pub fn attribute_names(&self) -> Vec<&'static str> {
        const TABLE: &[(u32, &str)] = &[
            (attr::COMPRESSED, "압축"),
            (attr::ENCRYPTED, "암호화"),
            (attr::DISTRIBUTION, "배포용 문서"),
            (attr::HAS_SCRIPT, "스크립트 저장"),
            (attr::DRM, "DRM 보안"),
            (attr::HAS_XML_TEMPLATE, "XMLTemplate 스토리지"),
            (attr::HAS_HISTORY, "문서 이력 관리"),
            (attr::HAS_SIGNATURE, "전자 서명 정보"),
            (attr::CERT_ENCRYPTED, "공인 인증서 암호화"),
            (attr::SIGNATURE_SPARE, "전자 서명 예비 저장"),
            (attr::CERT_DRM, "공인 인증서 DRM 보안"),
            (attr::CCL, "CCL 문서"),
            (attr::MOBILE_OPTIMIZED, "모바일 최적화"),
            (attr::PRIVACY_SECURITY, "개인 정보 보안 문서"),
            (attr::TRACK_CHANGES, "변경 추적 문서"),
            (attr::KOGL, "공공누리(KOGL) 저작권 문서"),
            (attr::HAS_VIDEO_CONTROL, "비디오 컨트롤 포함"),
            (attr::HAS_TOC_FIELD, "차례 필드 컨트롤 포함"),
        ];
        TABLE
            .iter()
            .filter(|(bit, _)| self.attributes & bit != 0)
            .map(|(_, name)| *name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 표본_헤더() -> Vec<u8> {
        let mut data = vec![0u8; FILE_HEADER_SIZE];
        data[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        data[32..36].copy_from_slice(&0x05000300u32.to_le_bytes()); // 5.0.3.0
        data[36..40].copy_from_slice(&0b0000_0001u32.to_le_bytes()); // 압축
        data
    }

    /// `표본_헤더()`와 동일하지만 속성 DWORD를 인자로 받아 원하는 비트만
    /// 정확히 설정한다 — GATE-02 분기 테스트가 컨테이너·픽스처·코퍼스 없이
    /// 값 하나만 바꿔 각 케이스를 표현할 수 있도록.
    fn 표본_헤더_속성(attributes: u32) -> Vec<u8> {
        let mut data = 표본_헤더();
        data[36..40].copy_from_slice(&attributes.to_le_bytes());
        data
    }

    #[test]
    fn 파싱과_직렬화_왕복() {
        let data = 표본_헤더();
        let h = FileHeader::parse(&data).unwrap();
        assert_eq!(h.version.to_string(), "5.0.3.0");
        assert!(h.is_compressed());
        assert!(!h.is_distribution());
        assert_eq!(h.serialize(), data);
    }

    #[test]
    fn major_5는_지원_버전_통과() {
        // 5.0.3.0 (표본) — 통과.
        let h = FileHeader::parse(&표본_헤더()).unwrap();
        assert!(h.is_supported_version());
        assert!(h.check_version().is_ok());

        // 5.1.0.1 등 다른 5.x 마이너/빌드도 전부 허용.
        let mut data = 표본_헤더();
        data[32..36].copy_from_slice(&0x05010001u32.to_le_bytes()); // 5.1.0.1
        let h = FileHeader::parse(&data).unwrap();
        assert!(h.is_supported_version());
        assert!(h.check_version().is_ok());
    }

    #[test]
    fn major_6은_미지원_버전_에러() {
        // 가상의 6.0.0.0 — 시그니처는 유효하지만 버전 게이트에서 거부돼야 한다.
        let mut data = 표본_헤더();
        data[32..36].copy_from_slice(&0x06000000u32.to_le_bytes()); // 6.0.0.0
        let h = FileHeader::parse(&data).unwrap();
        assert!(!h.is_supported_version());
        assert!(matches!(
            h.check_version(),
            Err(Hwp5Error::UnsupportedVersion(v)) if v == "6.0.0.0"
        ));
    }

    #[test]
    fn 시그니처_불일치는_err() {
        let mut data = 표본_헤더();
        data[0] = b'X';
        assert!(matches!(
            FileHeader::parse(&data),
            Err(Hwp5Error::BadSignature)
        ));
    }

    // GATE-02: the ordered refusal chain on `check_body_readable`. These run
    // against a synthetic 256-byte header only — no CFB container, no
    // fixture, no corpus — so they run anywhere, including CI.

    #[test]
    fn certificate_encryption_bit_refuses() {
        let h = FileHeader::parse(&표본_헤더_속성(attr::CERT_ENCRYPTED)).unwrap();
        assert!(matches!(
            h.check_body_readable(),
            Err(Hwp5Error::CertEncrypted)
        ));
    }

    #[test]
    fn certificate_drm_bit_refuses() {
        let h = FileHeader::parse(&표본_헤더_속성(attr::CERT_DRM)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::CertDrm)));
    }

    #[test]
    fn drm_bit_refuses() {
        let h = FileHeader::parse(&표본_헤더_속성(attr::DRM)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::Drm)));
    }

    #[test]
    fn signature_bit_refuses() {
        let h = FileHeader::parse(&표본_헤더_속성(attr::HAS_SIGNATURE)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::Signed)));
    }

    #[test]
    fn signature_spare_bit_does_not_refuse() {
        // Deliberately not wired to a refusal — see the comment above
        // `check_body_readable`.
        let h = FileHeader::parse(&표본_헤더_속성(attr::SIGNATURE_SPARE)).unwrap();
        assert!(h.check_body_readable().is_ok());
    }

    #[test]
    fn an_ordinary_header_is_body_readable() {
        // Compression alone.
        let h = FileHeader::parse(&표본_헤더_속성(attr::COMPRESSED)).unwrap();
        assert!(h.check_body_readable().is_ok());

        // Distribution alone stays readable — GATE-01 made distribution
        // documents body-readable; `read_document` decrypts `/ViewText/`
        // separately rather than refusing here.
        let h = FileHeader::parse(&표본_헤더_속성(attr::DISTRIBUTION)).unwrap();
        assert!(h.check_body_readable().is_ok());

        // No attributes set at all.
        let h = FileHeader::parse(&표본_헤더_속성(0)).unwrap();
        assert!(h.check_body_readable().is_ok());
    }

    #[test]
    fn several_protection_bits_refuse_in_a_fixed_order() {
        // Password encryption alone still refuses, and its rendered message
        // still contains the sentence it printed before this phase — pinned
        // as a regression, not a rewrite, now with a remedy hint appended.
        let h = FileHeader::parse(&표본_헤더_속성(attr::ENCRYPTED)).unwrap();
        let err = h.check_body_readable().unwrap_err();
        assert!(matches!(err, Hwp5Error::Encrypted));
        assert!(
            err.to_string()
                .contains("암호화된 문서는 지원하지 않습니다")
        );

        // Password encryption + DRM: encryption is checked first.
        let h = FileHeader::parse(&표본_헤더_속성(attr::ENCRYPTED | attr::DRM)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::Encrypted)));

        // DRM + certificate DRM: the certificate-scoped condition is more
        // specific than the general one it implies.
        let h = FileHeader::parse(&표본_헤더_속성(attr::DRM | attr::CERT_DRM)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::CertDrm)));

        // DRM + signature: the signature check is last.
        let h = FileHeader::parse(&표본_헤더_속성(attr::DRM | attr::HAS_SIGNATURE)).unwrap();
        assert!(matches!(h.check_body_readable(), Err(Hwp5Error::Drm)));

        // An unsupported major version is refused before any protection bit
        // is consulted, even when protection bits are also set.
        let mut data = 표본_헤더_속성(attr::ENCRYPTED | attr::HAS_SIGNATURE);
        data[32..36].copy_from_slice(&0x06000000u32.to_le_bytes()); // 6.0.0.0
        let h = FileHeader::parse(&data).unwrap();
        assert!(matches!(
            h.check_body_readable(),
            Err(Hwp5Error::UnsupportedVersion(v)) if v == "6.0.0.0"
        ));
    }
}
